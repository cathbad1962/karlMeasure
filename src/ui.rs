//! The application window: open a drawing, pan and zoom it, step through pages.

use std::time::{Duration, Instant};

use eframe::egui;
use kurbo::{Point, Rect, Size, Vec2};

use crate::doc::{Anchor, Calibration, Measurement, Project, SubPath, Unit};
use crate::geom;
use crate::pdf;
use crate::tools::{self, Editor, Grab, Pen};
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

/// A measured area's outline.
const OUTLINE: egui::Color32 = egui::Color32::from_rgb(0, 150, 190);

/// The radius of an anchor dot, in logical points on screen.
const ANCHOR_DOT: f32 = 3.0;

/// The handle bar being pulled out of an anchor.
const HANDLE: egui::Color32 = egui::Color32::from_rgb(120, 195, 225);

/// The radius of a handle's end dot, in logical points on screen.
const HANDLE_DOT: f32 = 2.5;

/// How close a click has to land to take hold of something, in logical points
/// on screen. Divided by zoom to get the reach in page units.
const HIT: f64 = 8.0;

/// The anchor under the cursor's attention.
const SELECTED: egui::Color32 = egui::Color32::from_rgb(255, 210, 80);

/// How far a painted outline may stray from the true curve, in logical points
/// on screen. Divided by zoom to get the flattening tolerance in page units.
const FLATNESS: f64 = 0.25;

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

/// The keys, wheel and pointer as they stand this frame.
struct Input {
    page_up: bool,
    page_down: bool,
    escape: bool,
    delete: bool,
    toggle: bool,
    undo: bool,
    redo: bool,
    scroll: f64,
    cursor: Option<egui::Pos2>,
    /// Where the button went down, while one is held.
    pressed_at: Option<egui::Pos2>,
    alt: bool,
}

impl Input {
    fn read(ui: &egui::Ui) -> Self {
        ui.input(|i| {
            let command = i.modifiers.command;

            Self {
                page_up: i.key_pressed(egui::Key::PageUp),
                page_down: i.key_pressed(egui::Key::PageDown),
                escape: i.key_pressed(egui::Key::Escape),
                delete: i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
                toggle: i.key_pressed(egui::Key::S),
                undo: command && !i.modifiers.shift && i.key_pressed(egui::Key::Z),
                redo: command
                    && (i.key_pressed(egui::Key::Y)
                        || (i.modifiers.shift && i.key_pressed(egui::Key::Z))),
                scroll: i.smooth_scroll_delta.y as f64,
                cursor: i.pointer.hover_pos(),
                pressed_at: i.pointer.press_origin(),
                alt: i.modifiers.alt,
            }
        })
    }
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
    /// Everything the user has built on the drawing.
    project: Project,
    /// States to go back to, and the ones undone out of.
    undo: Vec<Project>,
    redo: Vec<Project>,
    pick: Option<Pick>,
    entry: Option<Entry>,
    /// Present while the area tool is armed.
    pen: Option<Pen>,
    /// Present while the edit tool is armed.
    editor: Option<Editor>,
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
        self.project = Project::default();
        self.undo.clear();
        self.redo.clear();
        self.disarm();

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

                // An outline belongs to the page it was traced on, and there
                // is nothing to trace on a page with no scale.
                if self.project.calibrations.contains_key(&index) {
                    if let Some(pen) = &mut self.pen {
                        pen.clear();
                    }
                } else {
                    self.pen = None;
                }

                // A selection indexes into one page's measurements, so it
                // means nothing on another.
                if self.project.measurements.contains_key(&index) {
                    self.forget_selection();
                } else {
                    self.editor = None;
                }
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

    /// Puts every tool away. Only one is ever armed at a time.
    fn disarm(&mut self) {
        self.cancel_pick();
        self.pen = None;
        self.editor = None;
    }

    /// Keeps the state as it stands, so the operation about to change it can
    /// be undone. Anything undone is discarded, since the history is a line
    /// rather than a tree.
    fn commit(&mut self) {
        self.undo.push(self.project.clone());
        self.redo.clear();
    }

    /// Runs a change against the current page's measurements, keeping a
    /// snapshot only if the change reports it did something.
    fn edit<T>(&mut self, change: impl FnOnce(&mut Vec<Measurement>) -> Option<T>) -> Option<T> {
        let snapshot = self.project.clone();
        let outcome = change(self.project.measurements.entry(self.page).or_default());

        if outcome.is_some() {
            self.undo.push(snapshot);
            self.redo.clear();
        }

        outcome
    }

    fn undo(&mut self) {
        if let Some(previous) = self.undo.pop() {
            let current = std::mem::replace(&mut self.project, previous);
            self.redo.push(current);
            self.forget_selection();
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            let current = std::mem::replace(&mut self.project, next);
            self.undo.push(current);
            self.forget_selection();
        }
    }

    /// A selection is a pair of indices into the measurements, which the state
    /// it was made against no longer guarantees.
    fn forget_selection(&mut self) {
        if let Some(editor) = &mut self.editor {
            editor.selected = None;
            editor.grabbed = None;
        }
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

    /// Records a closed outline as a measurement on the current page.
    fn add_measurement(&mut self, outer: SubPath) {
        self.commit();

        let measurements = self.project.measurements.entry(self.page).or_default();
        let name = format!("Area {}", measurements.len() + 1);

        measurements.push(Measurement {
            name,
            outer,
            holes: Vec::new(),
            colour: OUTLINE,
            visible: true,
        });
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

            let calibrating = self.pick.is_some() || self.entry.is_some();
            if ui
                .add(egui::Button::new("Calibrate").selected(calibrating))
                .clicked()
            {
                self.disarm();
                self.pick = Some(Pick::First);
            }

            let calibrated = self.project.calibrations.contains_key(&self.page);
            let tracing = self.pen.is_some();
            let area = egui::Button::new("Area").selected(tracing);

            if ui.add_enabled(calibrated, area).clicked() {
                self.disarm();
                self.pen = if tracing { None } else { Some(Pen::default()) };
            }

            let traced = self
                .project
                .measurements
                .get(&self.page)
                .is_some_and(|measurements| !measurements.is_empty());
            let editing = self.editor.is_some();
            let edit = egui::Button::new("Edit").selected(editing);

            if ui.add_enabled(traced, edit).clicked() {
                self.disarm();
                self.editor = if editing {
                    None
                } else {
                    Some(Editor::default())
                };
            }

            ui.separator();

            if ui
                .add_enabled(!self.undo.is_empty(), egui::Button::new("↶"))
                .clicked()
            {
                self.undo();
            }

            if ui
                .add_enabled(!self.redo.is_empty(), egui::Button::new("↷"))
                .clicked()
            {
                self.redo();
            }

            if !calibrated {
                ui.label("Calibrate this page first");
            } else if tracing {
                ui.label("Click to place corners, right-click to close");
            } else if editing {
                ui.label("Drag an anchor or handle; click an edge to add; Delete, S to toggle");
            }

            match (&self.pick, self.project.calibrations.get(&self.page)) {
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

        let input = Input::read(ui);
        let (scroll, cursor, pressed_at, alt) =
            (input.scroll, input.cursor, input.pressed_at, input.alt);

        if input.escape {
            self.cancel_pick();
            if let Some(pen) = &mut self.pen {
                pen.clear();
            }
        }

        if input.undo {
            self.undo();
        }
        if input.redo {
            self.redo();
        }

        if input.page_up && self.page > 0 {
            self.show_page(self.page - 1);
        }
        if input.page_down && self.page + 1 < self.page_count {
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

        // Tracing an outline. A left click places a corner and a left drag
        // pulls handles out of it; a right click closes the outline back to
        // the first anchor, however far away it is.
        if self.pen.is_some() {
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }

            if response.clicked()
                && let Some(pos) = response.interact_pointer_pos()
                && let Some(pen) = &mut self.pen
            {
                pen.place(self.viewport.screen_to_page(to_kurbo_point(pos)));
            }

            // The anchor belongs where the button went down, not where the
            // drag was recognised, or the outline would shift under the hand.
            if response.drag_started_by(egui::PointerButton::Primary)
                && let Some(origin) = pressed_at
                && let Some(pen) = &mut self.pen
            {
                pen.place(self.viewport.screen_to_page(to_kurbo_point(origin)));
            }

            if response.dragged_by(egui::PointerButton::Primary)
                && let Some(origin) = pressed_at
                && let Some(pos) = response.interact_pointer_pos()
            {
                let handle = self.viewport.screen_to_page(to_kurbo_point(pos))
                    - self.viewport.screen_to_page(to_kurbo_point(origin));

                if let Some(pen) = &mut self.pen {
                    pen.shape(handle, !alt);
                }
            }

            if response.secondary_clicked()
                && let Some(outer) = self.pen.as_mut().and_then(Pen::close)
            {
                self.add_measurement(outer);
            }
        }

        if self.editor.is_some() {
            self.edit_anchors(ui, &response, &input);
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

    /// Where a screen position falls on the page.
    fn page_point(&self, pos: egui::Pos2) -> Point {
        self.viewport.screen_to_page(to_kurbo_point(pos))
    }

    /// What lies under a page-space point: a handle of the selected anchor, or
    /// any anchor on the page.
    fn hit(&self, point: Point, radius: f64) -> Option<(tools::Selection, Grab)> {
        let editor = self.editor.as_ref()?;
        let measurements = self.project.measurements.get(&self.page)?;

        editor.hit(measurements, point, radius)
    }

    /// Reshaping an outline that is already traced: select an anchor, drag it
    /// or one of its handles, insert one on an edge, delete one, or flip it
    /// between a corner and a smooth point.
    fn edit_anchors(&mut self, ui: &egui::Ui, response: &egui::Response, input: &Input) {
        // Everything a hand aims at is sized on screen and divided by the
        // zoom, so it stays the same target however far the drawing is zoomed.
        let radius = HIT / self.viewport.zoom;

        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        // A drag takes hold of whatever was under the button when it went
        // down, and the whole drag is one thing to undo.
        let grabbed = response
            .drag_started_by(egui::PointerButton::Primary)
            .then_some(input.pressed_at)
            .flatten()
            .and_then(|origin| self.hit(self.page_point(origin), radius));

        if let Some(found) = grabbed {
            self.commit();

            if let Some(editor) = &mut self.editor {
                editor.selected = Some(found.0);
                editor.grabbed = Some(found);
            }
        }

        if response.dragged_by(egui::PointerButton::Primary)
            && let Some(pos) = response.interact_pointer_pos()
            && let Some((selection, grab)) = self.editor.as_ref().and_then(|e| e.grabbed)
        {
            let to = self.page_point(pos);
            let mirror = !input.alt;
            let measurements = self.project.measurements.entry(self.page).or_default();

            tools::move_to(measurements, selection, grab, to, mirror);
        }

        if response.drag_stopped_by(egui::PointerButton::Primary)
            && let Some(editor) = &mut self.editor
        {
            editor.grabbed = None;
        }

        // A click selects what it lands on, or adds an anchor to the edge it
        // lands on.
        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let point = self.page_point(pos);

            let selected = match self.hit(point, radius) {
                Some((selection, _)) => Some(selection),
                None => self.edit(|measurements| tools::insert(measurements, point, radius)),
            };

            if let Some(editor) = &mut self.editor {
                editor.selected = selected;
            }
        }

        let Some(selection) = self.editor.as_ref().and_then(|editor| editor.selected) else {
            return;
        };

        if input.delete && self.edit(|m| tools::delete(m, selection)).is_some() {
            self.forget_selection();
        }

        if input.toggle {
            self.edit(|m| tools::toggle(m, selection));
        }
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
                                    self.commit();
                                    self.project.calibrations.insert(self.page, calibration);
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
        if let Some(calibration) = self.project.calibrations.get(&self.page) {
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

        for measurement in self
            .project
            .measurements
            .get(&self.page)
            .into_iter()
            .flatten()
        {
            self.draw_measurement(&painter, measurement);
        }

        self.draw_pen(&painter, cursor);
        self.draw_editable(&painter);
    }

    /// While editing, every anchor on the page is a target, and the selected
    /// one shows the handles that shape the curve either side of it.
    fn draw_editable(&self, painter: &egui::Painter) {
        let Some(editor) = &self.editor else {
            return;
        };

        for (index, measurement) in self
            .project
            .measurements
            .get(&self.page)
            .into_iter()
            .flatten()
            .enumerate()
        {
            for (at, anchor) in measurement.outer.anchors.iter().enumerate() {
                let selected = editor.selected
                    == Some(tools::Selection {
                        measurement: index,
                        anchor: at,
                    });

                if selected {
                    self.draw_handles(painter, anchor);
                }

                painter.circle_filled(
                    self.screen(anchor.pos),
                    if selected {
                        ANCHOR_DOT + 1.5
                    } else {
                        ANCHOR_DOT
                    },
                    if selected { SELECTED } else { OUTLINE },
                );
            }
        }
    }

    /// A finished area: its outline, and what it encloses in real units.
    fn draw_measurement(&self, painter: &egui::Painter, measurement: &Measurement) {
        let outline = geom::outline(&measurement.outer, FLATNESS / self.viewport.zoom);
        if outline.is_empty() {
            return;
        }

        painter.add(egui::Shape::closed_line(
            outline.iter().map(|&p| self.screen(p)).collect(),
            egui::Stroke::new(1.5, measurement.colour),
        ));

        let Some(calibration) = self.project.calibrations.get(&self.page) else {
            return;
        };

        self.draw_label(
            painter,
            geom::centre(&measurement.outer),
            &area_label(
                calibration.square_millimetres(geom::area(&measurement.outer)),
                calibration.unit,
            ),
        );
    }

    /// The outline being traced: the anchors placed so far, the curve through
    /// them, a rubber band to the cursor, and a hint of the edge a right-click
    /// would close. Every band is the curve it will actually become, handles
    /// and all, rather than a straight stand-in.
    fn draw_pen(&self, painter: &egui::Painter, cursor: Option<Point>) {
        let Some(pen) = &self.pen else {
            return;
        };
        let anchors = pen.anchors();
        let (Some(first), Some(last)) = (anchors.first(), anchors.last()) else {
            return;
        };

        let stroke = egui::Stroke::new(1.5, OUTLINE);
        self.draw_curve(painter, anchors.to_vec(), stroke);

        if let Some(cursor) = cursor {
            self.draw_curve(painter, vec![*last, Anchor::corner(cursor)], stroke);

            if anchors.len() >= 2 {
                self.draw_curve(
                    painter,
                    vec![Anchor::corner(cursor), *first],
                    egui::Stroke::new(1.0, OUTLINE.gamma_multiply(0.4)),
                );
            }
        }

        for anchor in anchors {
            painter.circle_filled(self.screen(anchor.pos), ANCHOR_DOT, OUTLINE);
        }

        self.draw_handles(painter, last);
    }

    /// An open run of anchors, flattened to the accuracy the screen can show.
    fn draw_curve(&self, painter: &egui::Painter, anchors: Vec<Anchor>, stroke: egui::Stroke) {
        let path = SubPath {
            anchors,
            closed: false,
        };
        let outline = geom::outline(&path, FLATNESS / self.viewport.zoom);

        if outline.len() > 1 {
            painter.add(egui::Shape::line(
                outline.iter().map(|&p| self.screen(p)).collect(),
                stroke,
            ));
        }
    }

    /// The handle bar of the anchor being placed, which is what a drag is
    /// pulling out and what the modifier breaks apart.
    fn draw_handles(&self, painter: &egui::Painter, anchor: &Anchor) {
        if anchor.in_handle == Vec2::ZERO && anchor.out_handle == Vec2::ZERO {
            return;
        }

        let stroke = egui::Stroke::new(1.0, HANDLE);

        for handle in [anchor.in_handle, anchor.out_handle] {
            let end = self.screen(anchor.pos + handle);
            painter.line_segment([self.screen(anchor.pos), end], stroke);
            painter.circle_filled(end, HANDLE_DOT, HANDLE);
        }
    }

    /// A readable caption on the drawing, sized in logical points so it stays
    /// legible at every zoom.
    fn draw_label(&self, painter: &egui::Painter, at: Point, text: &str) {
        let galley = painter.layout_no_wrap(
            text.to_owned(),
            egui::FontId::proportional(12.0),
            egui::Color32::WHITE,
        );
        let rect = egui::Align2::CENTER_CENTER.anchor_size(self.screen(at), galley.size());

        painter.rect_filled(rect.expand(4.0), 3, egui::Color32::from_black_alpha(190));
        painter.galley(rect.min, galley, egui::Color32::WHITE);
    }

    fn screen(&self, page: Point) -> egui::Pos2 {
        to_egui_pos(self.viewport.page_to_screen(page))
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

/// An area in the unit a person would actually read it in: square metres for a
/// metric drawing, square feet for an imperial one, dropping to the small unit
/// when the area is too small for the large one to say anything.
fn area_label(square_millimetres: f64, unit: Unit) -> String {
    if unit.is_metric() {
        let square_metres = square_millimetres / 1e6;

        if square_metres >= 0.01 {
            format!("{square_metres:.2} m²")
        } else {
            format!("{square_millimetres:.0} mm²")
        }
    } else {
        let square_inches = square_millimetres / (25.4 * 25.4);
        let square_feet = square_inches / 144.0;

        if square_feet >= 0.1 {
            format!("{square_feet:.2} ft²")
        } else {
            format!("{square_inches:.1} in²")
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
