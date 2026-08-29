//! The application window: open a drawing, pan and zoom it, step through pages.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use eframe::egui;
use kurbo::{Point, Rect, Size, Vec2};

use crate::doc::{Calibration, Unit};
use crate::pdf;
use crate::viewport::Viewport;

/// A pan, zoom or resize is re-rendered once the view has been still this long.
/// Until then the existing texture is scaled and shifted to stand in for it.
const SETTLE: Duration = Duration::from_millis(120);

/// Screen points of wheel travel that double the zoom.
const WHEEL_DOUBLING: f64 = 300.0;

/// The area around the sheet.
const BACKDROP: egui::Color32 = egui::Color32::from_rgb(56, 58, 62);

/// The calibrated span, and the rubber band while it is being picked.
const SPAN: egui::Color32 = egui::Color32::from_rgb(224, 132, 24);

/// Half the length of a span's end ticks, in logical points, so they stay the
/// same size on screen whatever the zoom.
const TICK: f64 = 6.0;

/// The shortest span that can be calibrated, in logical points on screen.
const MIN_SPAN: f64 = 3.0;

/// What the current texture holds, and where it sits on the page.
struct Rendered {
    texture: egui::TextureHandle,
    page: usize,
    region: Rect,
}

/// How far through picking the two ends of a known distance we are.
#[derive(Clone, Copy)]
enum Pick {
    First,
    Second { from: Point },
}

/// Both ends are picked; the real distance between them is being typed in.
struct Entry {
    from: Point,
    to: Point,
    distance: String,
    unit: Unit,
    /// The field takes focus on the frame the panel opens, not every frame.
    focused: bool,
    /// Why the last attempt was refused, if it was.
    problem: Option<&'static str>,
}

#[derive(Default)]
pub struct App {
    document: Option<pdf::Document>,
    page: usize,
    page_count: usize,
    page_size: Size,
    viewport: Viewport,
    rendered: Option<Rendered>,
    /// When the view has settled enough to be worth re-rendering.
    resettle_at: Option<Instant>,
    /// Fit the page to the window on the next frame, once its size is known.
    refit: bool,
    /// The canvas as it was last frame, to notice window resizes.
    canvas: Rect,
    /// The scale of each calibrated page.
    calibrations: HashMap<usize, Calibration>,
    pick: Option<Pick>,
    entry: Option<Entry>,
    error: Option<String>,
}

impl App {
    fn open_dialog(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("PDF drawing", &["pdf"])
            .pick_file()
        else {
            return;
        };

        self.rendered = None;
        self.resettle_at = None;
        self.calibrations.clear();
        self.cancel_pick();

        match pdf::Document::open(&path) {
            Ok(document) => {
                self.error = None;
                self.page_count = document.page_count();
                self.document = Some(document);
                self.show_page(0);
            }
            Err(message) => {
                self.document = None;
                self.page_count = 0;
                self.error = Some(message);
            }
        }
    }

    /// Moves to `index`, fitted to the window.
    fn show_page(&mut self, index: usize) {
        // A span belongs to the page it was picked on; abandon a half-finished
        // one rather than carrying its first end to another page.
        self.cancel_pick();

        let Some(document) = &self.document else {
            return;
        };

        match document.page_size(index) {
            Ok(size) => {
                self.page = index;
                self.page_size = size;
                self.refit = true;
                self.rendered = None;
                self.resettle_at = None;
            }
            Err(message) => {
                self.document = None;
                self.error = Some(message);
            }
        }
    }

    /// The page has moved under the window; schedule a sharp re-render for
    /// once it stops moving.
    fn invalidate(&mut self, now: Instant) {
        self.resettle_at = Some(now + SETTLE);
    }

    /// Abandons a span that is part-picked or waiting on a distance.
    fn cancel_pick(&mut self) {
        self.pick = None;
        self.entry = None;
    }

    /// Takes a click while calibrating: the first ends up as one end of the
    /// span, the second opens the distance panel.
    fn pick_point(&mut self, point: Point) {
        match self.pick {
            Some(Pick::First) => self.pick = Some(Pick::Second { from: point }),
            Some(Pick::Second { from }) => {
                // A span of no length yields no scale, so treat a second click
                // on top of the first as a misfire and keep waiting for a
                // usable one. The threshold is on screen, not on the page, so
                // zooming in lets the two ends be placed as close as you like.
                if (point - from).hypot() * self.viewport.zoom < MIN_SPAN {
                    return;
                }

                self.pick = None;
                self.entry = Some(Entry {
                    from,
                    to: point,
                    distance: String::new(),
                    unit: Unit::Millimetres,
                    focused: false,
                    problem: None,
                });
            }
            None => {}
        }
    }

    fn page_rect(&self) -> Rect {
        Rect::from_origin_size(Point::ZERO, self.page_size)
    }

    /// Rasterises whatever part of the page the window is showing.
    fn render(&mut self, ctx: &egui::Context, canvas: Rect) {
        self.resettle_at = None;

        let Some(document) = &self.document else {
            return;
        };

        let visible = self
            .viewport
            .visible_page_rect(canvas)
            .intersect(self.page_rect());
        if visible.is_zero_area() {
            // The sheet has been panned right off the window; the last texture
            // stands until some of the page comes back into view.
            return;
        }

        let scale = self.viewport.zoom * ctx.pixels_per_point() as f64;

        match document.render_region(self.page, visible, scale) {
            Ok(raster) => {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [raster.width, raster.height],
                    &raster.rgba,
                );

                self.rendered = Some(Rendered {
                    texture: ctx.load_texture("page", image, egui::TextureOptions::LINEAR),
                    page: self.page,
                    region: raster.region,
                });
            }
            Err(message) => {
                self.document = None;
                self.rendered = None;
                self.error = Some(message);
            }
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Open…").clicked() {
                self.open_dialog();
            }

            if self.document.is_none() {
                return;
            }

            ui.separator();

            let previous = egui::Button::new("◀");
            if ui.add_enabled(self.page > 0, previous).clicked() {
                self.show_page(self.page - 1);
            }

            ui.label(format!("Page {} of {}", self.page + 1, self.page_count));

            let next = egui::Button::new("▶");
            if ui
                .add_enabled(self.page + 1 < self.page_count, next)
                .clicked()
            {
                self.show_page(self.page + 1);
            }

            ui.separator();

            if ui.button("Fit").clicked() {
                self.refit = true;
            }

            ui.label(format!("{:.0}%", self.viewport.zoom * 100.0));

            ui.separator();

            if ui.button("Calibrate").clicked() {
                self.cancel_pick();
                self.pick = Some(Pick::First);
            }

            match (&self.pick, self.calibrations.get(&self.page)) {
                (Some(Pick::First), _) => {
                    ui.label("Click the first point");
                }
                (Some(Pick::Second { .. }), _) => {
                    ui.label("Click the second point");
                }
                (None, Some(calibration)) => {
                    ui.label(scale_label(calibration));
                }
                (None, None) => {
                    ui.label("Not calibrated");
                }
            }
        });
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let canvas = ui.available_rect_before_wrap();
        let response = ui.interact(
            canvas,
            ui.id().with("canvas"),
            egui::Sense::click_and_drag(),
        );
        let canvas = to_kurbo_rect(canvas);
        let now = Instant::now();

        let (page_up, page_down, escape, scroll, cursor) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::PageUp),
                i.key_pressed(egui::Key::PageDown),
                i.key_pressed(egui::Key::Escape),
                i.smooth_scroll_delta.y as f64,
                i.pointer.hover_pos(),
            )
        });

        if escape {
            self.cancel_pick();
        }

        if page_up && self.page > 0 {
            self.show_page(self.page - 1);
        }
        if page_down && self.page + 1 < self.page_count {
            self.show_page(self.page + 1);
        }

        if self.refit {
            self.viewport = Viewport::fit(self.page_size, canvas);
            self.refit = false;
            self.invalidate(now);
        }

        if self.canvas != canvas {
            self.canvas = canvas;
            self.invalidate(now);
        }

        // Hold the wheel button down and drag to pan.
        if response.dragged_by(egui::PointerButton::Middle) {
            let delta = response.drag_delta();
            if delta != egui::Vec2::ZERO {
                self.viewport
                    .pan_by(Vec2::new(delta.x as f64, delta.y as f64));
                self.invalidate(now);
            }
        }

        // The wheel zooms about the cursor.
        if response.hovered()
            && scroll != 0.0
            && let Some(cursor) = cursor
        {
            self.viewport
                .zoom_about(to_kurbo_point(cursor), (scroll / WHEEL_DOUBLING).exp2());
            self.invalidate(now);
        }

        // Picking a span. Points are taken in page space, so panning or
        // zooming between the two clicks does not move the first one.
        if self.pick.is_some() {
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }

            if response.clicked()
                && let Some(pos) = response.interact_pointer_pos()
            {
                self.pick_point(self.viewport.screen_to_page(to_kurbo_point(pos)));
            }
        }

        let settled = self.resettle_at.is_some_and(|at| now >= at);
        if self.rendered.is_none() || settled {
            self.render(ui.ctx(), canvas);
        } else if self.resettle_at.is_some() {
            ui.ctx().request_repaint_after(SETTLE);
        }

        let cursor = cursor.map(|pos| self.viewport.screen_to_page(to_kurbo_point(pos)));
        self.paint(ui, canvas, cursor);
        self.distance_entry(ui);
    }

    /// The panel that asks what the picked span measures in the real world.
    fn distance_entry(&mut self, ui: &egui::Ui) {
        let Some(mut entry) = self.entry.take() else {
            return;
        };
        let mut keep = true;

        egui::Window::new("Known distance")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                ui.label("How far apart are the two points?");

                let mut accept = false;

                ui.horizontal(|ui| {
                    let field =
                        ui.add(egui::TextEdit::singleline(&mut entry.distance).desired_width(90.0));

                    if !entry.focused {
                        field.request_focus();
                        entry.focused = true;
                    }

                    accept = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

                    egui::ComboBox::from_id_salt("unit")
                        .selected_text(entry.unit.label())
                        .width(60.0)
                        .show_ui(ui, |ui| {
                            for unit in Unit::ALL {
                                ui.selectable_value(&mut entry.unit, unit, unit.label());
                            }
                        });
                });

                if let Some(problem) = entry.problem {
                    ui.colored_label(ui.visuals().error_fg_color, problem);
                }

                ui.horizontal(|ui| {
                    accept |= ui.button("Set scale").clicked();

                    if ui.button("Cancel").clicked() {
                        keep = false;
                    }
                });

                if accept {
                    // Say which of the two things was wrong: what was typed,
                    // or what it would mean.
                    entry.problem = match entry.distance.trim().parse::<f64>() {
                        Err(_) => {
                            Some("That is not a number. Use a full stop for the decimal point.")
                        }
                        Ok(distance) => {
                            match Calibration::new(entry.from, entry.to, distance, entry.unit) {
                                Some(calibration) => {
                                    self.calibrations.insert(self.page, calibration);
                                    keep = false;
                                    None
                                }
                                None => Some("Enter a distance greater than zero."),
                            }
                        }
                    };

                    if entry.problem.is_some() {
                        // Put the cursor back in the field to correct it.
                        entry.focused = false;
                    }
                }
            });

        if keep {
            self.entry = Some(entry);
        }
    }

    fn paint(&self, ui: &egui::Ui, canvas: Rect, cursor: Option<Point>) {
        let painter = ui.painter_at(to_egui_rect(canvas));
        painter.rect_filled(to_egui_rect(canvas), 0, BACKDROP);

        if let Some(rendered) = &self.rendered
            && rendered.page == self.page
        {
            // Between renders this is the stale texture, drawn through the
            // live viewport: it scales and shifts with the gesture.
            let rect = to_egui_rect(self.viewport.page_rect_to_screen(rendered.region));
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            painter.image(rendered.texture.id(), rect, uv, egui::Color32::WHITE);
        }

        painter.rect_stroke(
            to_egui_rect(self.viewport.page_rect_to_screen(self.page_rect())),
            0,
            egui::Stroke::new(1.0, egui::Color32::from_gray(90)),
            egui::StrokeKind::Outside,
        );

        // The calibrated span stays on the sheet, in page space, as the
        // evidence of what the scale was taken from.
        if let Some(calibration) = self.calibrations.get(&self.page) {
            self.draw_span(&painter, calibration.from, calibration.to);
        }

        match (&self.pick, &self.entry) {
            (Some(Pick::Second { from }), _) => {
                if let Some(cursor) = cursor {
                    self.draw_span(&painter, *from, cursor);
                }
            }
            (_, Some(entry)) => self.draw_span(&painter, entry.from, entry.to),
            _ => {}
        }
    }

    fn draw_span(&self, painter: &egui::Painter, from: Point, to: Point) {
        let a = self.viewport.page_to_screen(from);
        let b = self.viewport.page_to_screen(to);
        let stroke = egui::Stroke::new(1.5, SPAN);

        painter.line_segment([to_egui_pos(a), to_egui_pos(b)], stroke);

        // End ticks are a fixed length on screen, so they stay readable
        // however far the drawing is zoomed.
        let along = b - a;
        let length = along.hypot();
        if length > f64::EPSILON {
            let across = Vec2::new(-along.y, along.x) / length * TICK;
            for end in [a, b] {
                painter.line_segment(
                    [to_egui_pos(end - across), to_egui_pos(end + across)],
                    stroke,
                );
            }
        }
    }
}

/// e.g. `Scale 1:100 · 35.2778 mm/pt`.
fn scale_label(calibration: &Calibration) -> String {
    let ratio = calibration.ratio();
    let ratio_text = if ratio >= 1.0 {
        format!("1:{ratio:.0}")
    } else {
        // A detail drawn larger than life reads the other way round.
        format!("{:.0}:1", 1.0 / ratio)
    };

    format!(
        "Scale {ratio_text} · {:.4} {}/pt",
        calibration.units_per_point(),
        calibration.unit.label()
    )
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(ui));

        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(message) = self.error.clone() {
                let colour = ui.visuals().error_fg_color;
                ui.colored_label(colour, message);
                return;
            }

            if self.document.is_none() {
                ui.centered_and_justified(|ui| ui.label("Open a PDF to begin."));
                return;
            }

            self.canvas(ui);
        });
    }
}

fn to_kurbo_rect(rect: egui::Rect) -> Rect {
    Rect::new(
        rect.min.x as f64,
        rect.min.y as f64,
        rect.max.x as f64,
        rect.max.y as f64,
    )
}

fn to_egui_rect(rect: Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(rect.x0 as f32, rect.y0 as f32),
        egui::pos2(rect.x1 as f32, rect.y1 as f32),
    )
}

fn to_kurbo_point(pos: egui::Pos2) -> Point {
    Point::new(pos.x as f64, pos.y as f64)
}

fn to_egui_pos(point: Point) -> egui::Pos2 {
    egui::pos2(point.x as f32, point.y as f32)
}
