//! The application window: open a drawing, pan and zoom it, step through pages.

use std::time::{Duration, Instant};

use eframe::egui;
use kurbo::{Point, Rect, Shape, Size, Vec2};

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

/// How near an existing anchor a placement has to fall to be pulled onto it,
/// in logical points on screen.
const SNAP: f64 = 10.0;

/// How far a constrained drag has to travel, in logical points on screen,
/// before the axis it is held to is settled for the rest of the drag.
const DECIDED: f64 = 6.0;

/// The ring drawn round the anchor a placement has caught.
const SNAPPED: egui::Color32 = egui::Color32::from_rgb(255, 255, 255);

/// The side of the magnifier, in logical points on screen.
const LOUPE: f32 = 150.0;

/// How far the magnifier sits from the corner of the canvas.
const LOUPE_MARGIN: f32 = 10.0;

/// How much the magnifier magnifies, over the current zoom.
const MAGNIFY: f64 = 4.0;

/// The magnifier re-renders once the cursor has been this still. Rendering is
/// priced by what is on the page, not by the size of the bitmap, so this
/// cannot be done per frame.
const LOUPE_SETTLE: Duration = Duration::from_millis(90);

/// The tools, one of which is always the one in hand. The names and keys
/// follow the drawing applications this is meant to sit beside.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Tool {
    /// `V` — a whole measurement at a time.
    #[default]
    Select,
    /// `A` — the anchors and handles within one.
    DirectSelect,
    /// `P` — placing anchors to trace an outline.
    Pen,
    /// `Shift+C` — flipping an anchor between a corner and a smooth point.
    AnchorPoint,
    /// `+` — adding an anchor to a segment.
    AddAnchor,
    /// `-` — taking an anchor away.
    DeleteAnchor,
    /// Setting the page scale, from the measurement panel.
    Calibrate,
}

impl Tool {
    /// Whether this tool works on the anchors of an outline, which is what
    /// decides if the editor's hit-testing and its handles are in play.
    fn edits_anchors(self) -> bool {
        matches!(
            self,
            Self::DirectSelect | Self::AnchorPoint | Self::AddAnchor | Self::DeleteAnchor
        )
    }
}

/// The precision aids, which are toggles rather than tools.
struct Assist {
    snap: bool,
    magnifier: bool,
}

impl Default for Assist {
    fn default() -> Self {
        Self {
            snap: true,
            magnifier: true,
        }
    }
}

/// Somewhere a drawing is being painted, and the transform that puts page
/// space onto it. The sheet and the magnifier are the same picture through
/// different viewports, so everything that draws takes one of these.
struct Scene<'a> {
    painter: &'a egui::Painter,
    view: Viewport,
    /// Captions belong on the sheet; inside the magnifier they would cover the
    /// very detail being looked at.
    labels: bool,
}

/// Whose anchors are worth showing.
enum Showing {
    Every,
    JustOne(usize),
}

impl Showing {
    fn includes(&self, measurement: usize) -> bool {
        match self {
            Self::Every => true,
            Self::JustOne(index) => *index == measurement,
        }
    }
}

impl Scene<'_> {
    fn at(&self, page: Point) -> egui::Pos2 {
        to_egui_pos(self.view.page_to_screen(page))
    }

    /// How far a painted outline may stray from the curve, in page units.
    fn flatness(&self) -> f64 {
        FLATNESS / self.view.zoom
    }
}

/// The magnified patch of drawing under the cursor.
struct Loupe {
    texture: egui::TextureHandle,
    /// Page space, what the texture covers.
    region: Rect,
    /// The page point it was rendered around.
    centred_on: Point,
}

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
    undo: bool,
    redo: bool,
    /// The tool a key asked for, if one did.
    tool: Option<Tool>,
    snap: bool,
    hole: bool,
    scroll: f64,
    cursor: Option<egui::Pos2>,
    /// Where the button went down, while one is held.
    pressed_at: Option<egui::Pos2>,
    alt: bool,
    shift: bool,
}

impl Input {
    fn read(ui: &egui::Ui) -> Self {
        // While a name or a distance is being typed, the keyboard belongs to
        // the field, not to the tools. Escape is the exception: it means "let
        // go of this" wherever it is pressed.
        let typing = ui.ctx().egui_wants_keyboard_input();

        ui.input(|i| {
            // Matched on the physical key as well as the logical one. Holding
            // Ctrl and Alt together is AltGr on Windows, and a layout may
            // then produce no letter at all for the key that was pressed.
            let key = |wanted: egui::Key| {
                !typing
                    && i.events.iter().any(|event| {
                        matches!(
                            event,
                            egui::Event::Key {
                                key,
                                physical_key,
                                pressed: true,
                                ..
                            } if *key == wanted || *physical_key == Some(wanted)
                        )
                    })
            };
            let command = !typing && i.modifiers.command;

            // Nothing here may hold Ctrl and a letter the clipboard claims:
            // the window layer turns any Ctrl+C, Ctrl+X or Ctrl+V into a
            // clipboard event and returns before a key event is made at all.

            // A tool's letter is the lower case one. Shift makes a different
            // binding, not the same one — `n` is Snap and `Shift+N` is not.
            let plain = |k| key(k) && !i.modifiers.shift && !i.modifiers.command;
            let shifted = |k| key(k) && i.modifiers.shift && !i.modifiers.command;

            Self {
                page_up: key(egui::Key::PageUp),
                page_down: key(egui::Key::PageDown),
                escape: i.key_pressed(egui::Key::Escape),
                delete: key(egui::Key::Delete) || key(egui::Key::Backspace),
                undo: command && !i.modifiers.shift && key(egui::Key::Z),
                redo: command && (key(egui::Key::Y) || (i.modifiers.shift && key(egui::Key::Z))),
                tool: if shifted(egui::Key::C) && i.modifiers.alt {
                    Some(Tool::Calibrate)
                } else if shifted(egui::Key::C) {
                    Some(Tool::AnchorPoint)
                } else if plain(egui::Key::V) {
                    Some(Tool::Select)
                } else if plain(egui::Key::A) {
                    Some(Tool::DirectSelect)
                } else if plain(egui::Key::P) {
                    Some(Tool::Pen)
                } else if plain(egui::Key::Plus) || plain(egui::Key::Equals) {
                    Some(Tool::AddAnchor)
                } else if plain(egui::Key::Minus) {
                    Some(Tool::DeleteAnchor)
                } else {
                    None
                },
                snap: plain(egui::Key::N),
                hole: plain(egui::Key::H),
                scroll: i.smooth_scroll_delta.y as f64,
                cursor: i.pointer.hover_pos(),
                pressed_at: i.pointer.press_origin(),
                alt: i.modifiers.alt,
                shift: i.modifiers.shift,
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
    /// The tool in hand, and the state belonging to it.
    tool: Tool,
    /// The precision aids, on or off.
    assist: Assist,
    pick: Option<Pick>,
    entry: Option<Entry>,
    /// Present while the area tool is armed.
    pen: Option<Pen>,
    /// The measurement a traced outline becomes a hole in, when the pen was
    /// armed from a row of the list rather than from the toolbar.
    pen_target: Option<usize>,
    /// The measurement whose name is being typed, so a rename is one thing to
    /// undo rather than one per keystroke.
    renaming: Option<usize>,
    /// Present while the edit tool is armed.
    editor: Option<Editor>,
    /// The measurement selected as a whole, by the selection tool.
    selected_measurement: Option<usize>,
    /// A measurement being dragged bodily: which one, how it was before the
    /// drag began, and the page point the drag started from.
    moving: Option<(usize, Box<Measurement>, Point)>,
    /// The axis a constrained drag has settled on, held until it ends.
    constraint: Option<geom::Axis>,
    /// The magnified patch under the cursor, and when to re-render it.
    loupe: Option<Loupe>,
    loupe_settle: Option<Instant>,
    /// The anchor a placement is currently caught on, for the frame it is
    /// drawn in.
    snapped: Option<Point>,
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
        self.take_up(Tool::Select);

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
                // is nothing to trace on a page with no scale. A selection,
                // and the measurement a hole was going to go into, index one
                // page's measurements and mean nothing on another.
                self.renaming = None;
                self.loupe = None;

                if self.tool == Tool::Pen && !self.project.calibrations.contains_key(&index) {
                    self.take_up(Tool::Select);
                } else {
                    self.take_up(self.tool);
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

    /// Takes up a tool, putting down whatever was in hand. Each tool's state
    /// starts fresh, so nothing half-done carries across.
    fn take_up(&mut self, tool: Tool) {
        self.cancel_pick();
        self.pen = None;
        self.pen_target = None;
        self.editor = None;
        self.tool = tool;

        match tool {
            Tool::Pen => self.pen = Some(Pen::default()),
            Tool::Calibrate => self.pick = Some(Pick::First),
            tool if tool.edits_anchors() => self.editor = Some(Editor::default()),
            _ => {}
        }
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

    /// Records a closed outline: as a hole in the measurement the pen was
    /// armed for, or as a new measurement on the page.
    fn add_outline(&mut self, outline: SubPath) {
        self.commit();

        let target = self.pen_target;
        let measurements = self.project.measurements.entry(self.page).or_default();

        if let Some(measurement) = target.and_then(|index| measurements.get_mut(index)) {
            // Wound against its outline, so it takes area away rather than
            // adding it, however it was traced.
            let hole = geom::as_hole(&measurement.outer, outline);
            measurement.holes.push(hole);

            // One press of `h`, one hole. The pen goes back to drawing new
            // areas rather than quietly filling the same one with holes.
            self.pen_target = None;
            return;
        }

        let name = format!("Area {}", measurements.len() + 1);

        measurements.push(Measurement {
            name,
            outer: outline,
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

            ui.separator();
            ui.label(self.hint());

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                match self.project.calibrations.get(&self.page) {
                    Some(calibration) => ui.label(scale_label(calibration)),
                    None => ui.label("Not calibrated"),
                };
            });
        });
    }

    /// What the tool in hand is waiting for.
    fn hint(&self) -> &'static str {
        match self.tool {
            Tool::Calibrate => match self.pick {
                Some(Pick::First) => "Click the first point of a known distance",
                Some(Pick::Second { .. }) => "Click the second point",
                None => "Enter the distance between the two points",
            },
            Tool::Pen if !self.project.calibrations.contains_key(&self.page) => {
                "Calibrate this page before measuring areas on it"
            }
            Tool::Pen if self.pen_target.is_some() => "Tracing a hole; right-click to close",
            Tool::Pen => "Click to place corners, drag to curve, right-click to close",
            Tool::Select => "Click an area to select it, drag to move it, h for a hole in it",
            Tool::DirectSelect => "Drag an anchor or a handle; Alt breaks a smooth pair",
            Tool::AnchorPoint => "Click an anchor to flip it between a corner and a curve",
            Tool::AddAnchor => "Click an edge to add an anchor to it",
            Tool::DeleteAnchor => "Click an anchor to take it away",
        }
    }

    /// The tools, in a strip down the left: the ones that change what the
    /// pointer does, and the aids that change how precisely it does it.
    fn tool_strip(&mut self, ui: &mut egui::Ui) {
        // The tools that work, in the order and with the keys they carry in
        // the drawing applications this sits beside.
        const TOOLS: [(Tool, &str, &str); 6] = [
            (Tool::Select, "v", "Selection (v)"),
            (Tool::DirectSelect, "a", "Direct Selection (a)"),
            (Tool::Pen, "p", "Pen (p)"),
            (Tool::AnchorPoint, "⇧C", "Anchor Point (Shift+c)"),
            (Tool::AddAnchor, "+", "Add Anchor Point (+)"),
            (Tool::DeleteAnchor, "−", "Delete Anchor Point (−)"),
        ];

        // Labels only, holding letters for a later project. Nothing lies
        // behind them; see CLAUDE.md §2.
        const PLACEHOLDERS: [(&str, &str); 7] = [
            ("\\", "Line — not in this project"),
            ("m", "Rectangle — not in this project"),
            ("l", "Ellipse — not in this project"),
            ("⇧P", "Polygon — not in this project"),
            ("⇧N", "Polyline — not in this project"),
            ("t", "Type — not in this project"),
            ("i", "Eyedropper — not in this project"),
        ];

        egui::Panel::left("tools")
            .exact_size(46.0)
            .resizable(false)
            .show(ui, |ui| {
                ui.add_space(6.0);

                for (tool, letter, name) in TOOLS {
                    let button = egui::Button::new(letter).selected(self.tool == tool);

                    if ui
                        .add_sized([32.0, 28.0], button)
                        .on_hover_text(name)
                        .clicked()
                    {
                        self.take_up(tool);
                    }
                    ui.add_space(2.0);
                }

                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);

                for (letter, name) in PLACEHOLDERS {
                    ui.add_enabled_ui(false, |ui| {
                        ui.add_sized([32.0, 28.0], egui::Button::new(letter))
                            .on_disabled_hover_text(name)
                    });
                    ui.add_space(2.0);
                }

                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);

                for (on, letter, name) in [
                    (&mut self.assist.snap, "n", "Snap to anchors (n)"),
                    (&mut self.assist.magnifier, "◎", "Magnifier"),
                ] {
                    let button = egui::Button::new(letter).selected(*on);

                    if ui
                        .add_sized([32.0, 28.0], button)
                        .on_hover_text(name)
                        .clicked()
                    {
                        *on = !*on;
                    }
                    ui.add_space(2.0);
                }
            });
    }

    /// The measurement tools, grouped and collapsible.
    fn tool_groups(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("tool_groups")
            .default_size(170.0)
            .show(ui, |ui| {
                ui.add_space(4.0);

                egui::CollapsingHeader::new("Measurement")
                    .default_open(true)
                    .show(ui, |ui| {
                        let width = ui.available_width();

                        let calibrate =
                            egui::Button::new("Calibrate").selected(self.tool == Tool::Calibrate);
                        if ui
                            .add_sized([width, 24.0], calibrate)
                            .on_hover_text("Shift+Alt+C")
                            .clicked()
                        {
                            self.take_up(Tool::Calibrate);
                        }

                        // There is no Area button: area is the only kind of
                        // measurement there is, so the button said nothing the
                        // Pen does not. When there is a second kind, this is
                        // where the choice between them will be made.
                        ui.add_enabled_ui(false, |ui| {
                            ui.add_sized([width, 24.0], egui::Button::new("Length"))
                                .on_disabled_hover_text("Not in this project")
                        });
                    });
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

        // Whatever a placement catches on is worked out afresh each frame.
        self.snapped = None;

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

        if let Some(tool) = input.tool {
            self.take_up(tool);
        }
        if input.snap {
            self.assist.snap = !self.assist.snap;
        }

        // A hole goes into whichever measurement is in hand, however it came
        // to be in hand.
        if input.hole
            && let Some(index) = self.current_measurement()
        {
            self.take_up(Tool::Pen);
            self.pen_target = Some(index);
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
            {
                let from = self.pen_from();
                let at = self.aim(self.page_point(pos), from, &input, None);

                if let Some(pen) = &mut self.pen {
                    pen.place(at);
                }
            }

            // The anchor belongs where the button went down, not where the
            // drag was recognised, or the outline would shift under the hand.
            if response.drag_started_by(egui::PointerButton::Primary)
                && let Some(origin) = pressed_at
            {
                let from = self.pen_from();
                let at = self.aim(self.page_point(origin), from, &input, None);

                if let Some(pen) = &mut self.pen {
                    pen.place(at);
                }
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
                self.add_outline(outer);
            }
        }

        if self.editor.is_some() {
            self.edit_anchors(ui, &response, &input);
        }

        if self.tool == Tool::Select {
            self.select_measurement(ui, &response, &input);
        }

        let settled = self.resettle_at.is_some_and(|at| now >= at);
        if self.rendered.is_none() || settled {
            self.render(ui.ctx(), canvas);
        } else if self.resettle_at.is_some() {
            ui.ctx().request_repaint_after(SETTLE);
        }

        // The preview follows where the placement would actually land, not
        // where the hand is, so what you see is what you get.
        let cursor = cursor.map(|pos| self.page_point(pos));
        let cursor = match cursor {
            Some(point) if self.pen.is_some() => {
                let from = self.pen_from();
                Some(self.aim(point, from, &input, None))
            }
            other => other,
        };
        self.refresh_loupe(ui.ctx(), cursor, now);
        self.paint(ui, canvas, cursor);
        self.distance_entry(ui);
    }

    /// The list of what has been measured on this page, and everything that
    /// can be done to a measurement as a whole.
    fn measurements_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::right("measurements")
            .default_size(320.0)
            .show(ui, |ui| {
                ui.add_space(4.0);
                ui.heading("Measurements");
                ui.add_space(4.0);

                let Some(calibration) = self.project.calibrations.get(&self.page).copied() else {
                    ui.label("Calibrate this page to measure areas on it.");
                    return;
                };

                if self
                    .project
                    .measurements
                    .get(&self.page)
                    .is_none_or(Vec::is_empty)
                {
                    ui.label("Nothing traced on this page yet.");
                    return;
                }

                // Widgets change the value in place, so the state to go back
                // to has to be kept before any of them run.
                let snapshot = self.project.clone();
                let mut changed = false;
                let mut renaming = self.renaming;
                let selected = (self.tool == Tool::Select)
                    .then_some(self.selected_measurement)
                    .flatten();
                let mut remove = None;
                let mut hole_in = None;

                let measurements = self
                    .project
                    .measurements
                    .get_mut(&self.page)
                    .expect("checked above");

                for (index, measurement) in measurements.iter_mut().enumerate() {
                    // The row of whatever the selection tool has hold of is
                    // picked out, so the canvas and the list agree.
                    if selected == Some(index) {
                        let row = ui.available_rect_before_wrap();
                        ui.painter().rect_filled(
                            egui::Rect::from_min_size(row.min, egui::vec2(row.width(), 48.0)),
                            2,
                            SELECTED.gamma_multiply(0.15),
                        );
                    }

                    ui.horizontal(|ui| {
                        changed |= ui.checkbox(&mut measurement.visible, "").changed();
                        changed |= ui
                            .color_edit_button_srgba(&mut measurement.colour)
                            .changed();

                        let name = ui.add(
                            egui::TextEdit::singleline(&mut measurement.name).desired_width(150.0),
                        );

                        // One undo step per rename, not one per keystroke.
                        if name.changed() && renaming != Some(index) {
                            changed = true;
                            renaming = Some(index);
                        }
                        if name.lost_focus() && renaming == Some(index) {
                            renaming = None;
                        }
                    });

                    ui.horizontal(|ui| {
                        ui.label(area_label(
                            calibration.square_millimetres(geom::measurement_area(measurement)),
                            calibration.unit,
                        ));

                        if !measurement.holes.is_empty() {
                            ui.weak(format!("less {} holes", measurement.holes.len()));
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Remove").clicked() {
                                remove = Some(index);
                            }
                            if ui.button("Hole").clicked() {
                                hole_in = Some(index);
                            }
                        });
                    });

                    ui.separator();
                }

                if let Some(index) = remove
                    && index < measurements.len()
                {
                    measurements.remove(index);
                    changed = true;
                }

                self.renaming = renaming;

                if changed {
                    self.undo.push(snapshot);
                    self.redo.clear();
                }

                if remove.is_some() {
                    // The indices everything else was holding have moved.
                    self.take_up(Tool::Select);
                }

                if let Some(index) = hole_in {
                    self.take_up(Tool::Pen);
                    self.pen_target = Some(index);
                }
            });
    }

    /// Where a screen position falls on the page.
    fn page_point(&self, pos: egui::Pos2) -> Point {
        self.viewport.screen_to_page(to_kurbo_point(pos))
    }

    /// Every anchor a placement could be pulled onto: the visible outlines and
    /// their holes, and whatever the pen has put down so far.
    fn snap_targets(&self, except: Option<tools::Selection>) -> Vec<Point> {
        let mut targets = Vec::new();

        for (index, measurement) in self
            .project
            .measurements
            .get(&self.page)
            .into_iter()
            .flatten()
            .enumerate()
        {
            if !measurement.visible {
                continue;
            }

            for (outline, subpath) in measurement.outlines() {
                for (at, anchor) in subpath.anchors.iter().enumerate() {
                    let this = tools::Selection {
                        measurement: index,
                        outline,
                        anchor: at,
                    };

                    if except != Some(this) {
                        targets.push(anchor.pos);
                    }
                }
            }
        }

        if let Some(pen) = &self.pen {
            targets.extend(pen.anchors().iter().map(|anchor| anchor.pos));
        }

        targets
    }

    /// Holds a movement to one axis while Shift is down.
    ///
    /// The axis is settled once the movement is decisively along one of them
    /// and then kept, rather than decided afresh every frame: re-deciding lets
    /// a wobbling hand flip the constraint, and a tie goes to the horizontal.
    fn constrain(&mut self, from: Point, point: Point, shift: bool) -> Point {
        if !shift {
            self.constraint = None;
            return point;
        }

        let decisive = (point - from).hypot() * self.viewport.zoom >= DECIDED;
        let axis = match self.constraint {
            Some(axis) => axis,
            None if decisive => *self.constraint.insert(geom::Axis::of(from, point)),
            None => geom::Axis::of(from, point),
        };

        axis.hold(from, point)
    }

    /// Where a placement actually lands. Shift holds it to one axis from
    /// `from`; otherwise it is pulled onto a nearby anchor, if snapping is on
    /// and one is in reach. The two would fight each other, so Shift wins.
    fn aim(
        &mut self,
        point: Point,
        from: Option<Point>,
        input: &Input,
        except: Option<tools::Selection>,
    ) -> Point {
        if input.shift
            && let Some(from) = from
        {
            return self.constrain(from, point, true);
        }
        self.constraint = None;

        if self.assist.snap
            && let Some(caught) = tools::snap(
                point,
                SNAP / self.viewport.zoom,
                self.snap_targets(except).into_iter(),
            )
        {
            self.snapped = Some(caught);
            return caught;
        }

        point
    }

    /// The anchor the pen would run a new segment from.
    fn pen_from(&self) -> Option<Point> {
        self.pen.as_ref()?.anchors().last().map(|anchor| anchor.pos)
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

            let origin = self
                .project
                .measurements
                .get(&self.page)
                .and_then(|measurements| {
                    measurements
                        .get(found.0.measurement)?
                        .outline(found.0.outline)?
                        .anchors
                        .get(found.0.anchor)
                })
                .map(|anchor| anchor.pos);

            if let Some(editor) = &mut self.editor {
                editor.selected = Some(found.0);
                editor.grabbed = Some(found);
                editor.origin = origin;
            }
        }

        if response.dragged_by(egui::PointerButton::Primary)
            && let Some(pos) = response.interact_pointer_pos()
            && let Some((selection, grab)) = self.editor.as_ref().and_then(|e| e.grabbed)
        {
            let point = self.page_point(pos);

            // An anchor is aimed like a placement; a handle is not, since
            // neither snapping nor squareness means anything for one.
            let to = match grab {
                Grab::Anchor => {
                    let origin = self.editor.as_ref().and_then(|editor| editor.origin);
                    self.aim(point, origin, input, Some(selection))
                }
                Grab::In | Grab::Out => point,
            };

            let mirror = !input.alt;
            let measurements = self.project.measurements.entry(self.page).or_default();

            tools::move_to(measurements, selection, grab, to, mirror);
        }

        if response.drag_stopped_by(egui::PointerButton::Primary)
            && let Some(editor) = &mut self.editor
        {
            editor.grabbed = None;
            editor.origin = None;
        }

        // What a click does is the tool's business: Direct Selection only
        // selects, and each of the anchor tools does its own one thing.
        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let point = self.page_point(pos);
            let found = self.hit(point, radius).map(|(selection, _)| selection);

            let selected = match self.tool {
                Tool::DirectSelect => found,
                Tool::AnchorPoint => {
                    if let Some(selection) = found {
                        self.edit(|m| tools::toggle(m, selection));
                    }
                    found
                }
                Tool::DeleteAnchor => {
                    if let Some(selection) = found {
                        self.edit(|m| tools::delete(m, selection));
                    }
                    None
                }
                Tool::AddAnchor => {
                    self.edit(|measurements| tools::insert(measurements, point, radius))
                }
                _ => found,
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
    }

    /// The measurement being worked on, whether it was taken hold of whole or
    /// by one of its anchors.
    fn current_measurement(&self) -> Option<usize> {
        self.selected_measurement.or_else(|| {
            self.editor
                .as_ref()?
                .selected
                .map(|selection| selection.measurement)
        })
    }

    /// What lies under a page point, taking the whole measurement rather than
    /// any part of it: inside its outline, or close enough to the edge.
    fn measurement_at(&self, point: Point) -> Option<usize> {
        let reach = HIT / self.viewport.zoom;

        self.project
            .measurements
            .get(&self.page)
            .into_iter()
            .flatten()
            .enumerate()
            .filter(|(_, measurement)| measurement.visible)
            .find(|(_, measurement)| {
                geom::bez_path(&measurement.outer).contains(point)
                    || geom::nearest(&measurement.outer, point)
                        .is_some_and(|found| found.distance <= reach)
            })
            .map(|(index, _)| index)
    }

    /// Selecting a whole measurement: click its outline, or anywhere inside
    /// it. Dragging moves it bodily; Delete takes it off the page.
    fn select_measurement(&mut self, ui: &egui::Ui, response: &egui::Response, input: &Input) {
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
        }

        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            self.selected_measurement = self.measurement_at(self.page_point(pos));
        }

        // A drag takes hold of whatever it started on, and moves it as one
        // thing: outline, holes and all.
        if response.drag_started_by(egui::PointerButton::Primary)
            && let Some(origin) = input.pressed_at
        {
            let from = self.page_point(origin);

            if let Some(index) = self.measurement_at(from) {
                self.commit();
                self.selected_measurement = Some(index);

                let held = self
                    .project
                    .measurements
                    .get(&self.page)
                    .and_then(|measurements| measurements.get(index))
                    .cloned();

                self.moving = held.map(|measurement| (index, Box::new(measurement), from));
            }
        }

        if response.dragged_by(egui::PointerButton::Primary)
            && let Some(pos) = response.interact_pointer_pos()
            && let Some((index, held, from)) = &self.moving
        {
            let (index, from) = (*index, *from);
            let held = held.as_ref().clone();
            let to = self.page_point(pos);
            let to = self.constrain(from, to, input.shift);

            // Moved from where it was rather than by this frame's delta, so a
            // constrained drag cannot creep off the line it is held to.
            let mut moved = held;
            moved.translate(to - from);

            if let Some(measurements) = self.project.measurements.get_mut(&self.page)
                && let Some(target) = measurements.get_mut(index)
            {
                *target = moved;
            }
        }

        if response.drag_stopped_by(egui::PointerButton::Primary) {
            self.moving = None;
            self.constraint = None;
        }

        if input.delete
            && let Some(index) = self.selected_measurement
        {
            let removed = self.edit(|measurements| {
                (index < measurements.len()).then(|| measurements.remove(index))
            });

            if removed.is_some() {
                self.selected_measurement = None;
            }
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

        self.draw_overlay(
            &Scene {
                painter: &painter,
                view: self.viewport,
                labels: true,
            },
            cursor,
        );

        self.draw_loupe(&painter, canvas, cursor);
    }

    /// Everything the user has put on the drawing, painted through whichever
    /// viewport the scene carries. The sheet and the magnifier are the same
    /// picture at two magnifications.
    fn draw_overlay(&self, scene: &Scene, cursor: Option<Point>) {
        // The calibrated span stays on the sheet, in page space, as the
        // evidence of what the scale was taken from.
        if let Some(calibration) = self.project.calibrations.get(&self.page) {
            self.draw_span(scene, calibration.from, calibration.to);
        }

        match (&self.pick, &self.entry) {
            (Some(Pick::Second { from }), _) => {
                if let Some(cursor) = cursor {
                    self.draw_span(scene, *from, cursor);
                }
            }
            (_, Some(entry)) => self.draw_span(scene, entry.from, entry.to),
            _ => {}
        }

        for (index, measurement) in self
            .project
            .measurements
            .get(&self.page)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let selected = self.tool == Tool::Select && self.selected_measurement == Some(index);
            self.draw_measurement(scene, measurement, selected);
        }

        self.draw_pen(scene, cursor);
        self.draw_anchors(scene);

        if let Some(caught) = self.snapped {
            scene.painter.circle_stroke(
                scene.at(caught),
                ANCHOR_DOT + 3.0,
                egui::Stroke::new(1.5, SNAPPED),
            );
        }
    }

    /// Whether the magnifier is worth showing: while something is being
    /// placed or moved, not while the drawing is merely being looked at.
    fn magnifying(&self) -> bool {
        if !self.assist.magnifier {
            return false;
        }

        match self.tool {
            Tool::Pen | Tool::Calibrate => true,
            Tool::AnchorPoint | Tool::AddAnchor | Tool::DeleteAnchor => true,
            Tool::DirectSelect => self
                .editor
                .as_ref()
                .is_some_and(|editor| editor.grabbed.is_some()),
            Tool::Select => self.moving.is_some(),
        }
    }

    /// The magnified patch of drawing under the cursor, offset up and to the
    /// right so it never covers what is being aimed at.
    fn draw_loupe(&self, painter: &egui::Painter, canvas: Rect, cursor: Option<Point>) {
        let (Some(loupe), Some(cursor)) = (&self.loupe, cursor) else {
            return;
        };
        if !self.magnifying() {
            return;
        }

        // Pinned to the top right of the canvas. It followed the cursor to
        // begin with, which made it jump about exactly when a steady hand was
        // wanted.
        let bounds = to_egui_rect(canvas);
        let rect = egui::Rect::from_min_size(
            egui::pos2(
                bounds.max.x - LOUPE - LOUPE_MARGIN,
                bounds.min.y + LOUPE_MARGIN,
            ),
            egui::vec2(LOUPE, LOUPE),
        );

        // A viewport of its own, magnifying about the point under the cursor.
        let zoom = self.viewport.zoom * MAGNIFY;
        let centre = rect.center();
        let inside = Viewport {
            zoom,
            pan: Vec2::new(centre.x as f64, centre.y as f64) - loupe.centred_on.to_vec2() * zoom,
        };

        let painter = painter.with_clip_rect(rect);
        painter.rect_filled(rect, 2, egui::Color32::WHITE);
        painter.image(
            loupe.texture.id(),
            to_egui_rect(inside.page_rect_to_screen(loupe.region)),
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );

        // The same overlay as the sheet, at four times the size: the point of
        // the magnifier is to place an anchor against the drawing, so the
        // anchors have to be in it.
        self.draw_overlay(
            &Scene {
                painter: &painter,
                view: inside,
                labels: false,
            },
            Some(cursor),
        );

        // The crosshair marks the exact point, which is the whole purpose.
        let arm = 8.0;
        let stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(220, 40, 40));
        painter.line_segment(
            [
                egui::pos2(centre.x - arm, centre.y),
                egui::pos2(centre.x + arm, centre.y),
            ],
            stroke,
        );
        painter.line_segment(
            [
                egui::pos2(centre.x, centre.y - arm),
                egui::pos2(centre.x, centre.y + arm),
            ],
            stroke,
        );

        painter.rect_stroke(
            rect,
            2,
            egui::Stroke::new(1.0, egui::Color32::from_gray(120)),
            egui::StrokeKind::Inside,
        );
    }

    /// Rasterises the patch under the cursor, once it has stopped moving.
    fn refresh_loupe(&mut self, ctx: &egui::Context, cursor: Option<Point>, now: Instant) {
        let Some(cursor) = cursor.filter(|_| self.magnifying()) else {
            self.loupe_settle = None;
            return;
        };

        let moved = self
            .loupe
            .as_ref()
            .is_none_or(|loupe| (loupe.centred_on - cursor).hypot() > 0.5 / self.viewport.zoom);

        if moved && self.loupe_settle.is_none() {
            self.loupe_settle = Some(now + LOUPE_SETTLE);
        }

        match self.loupe_settle {
            Some(at) if now >= at => self.loupe_settle = None,
            Some(_) => {
                ctx.request_repaint_after(LOUPE_SETTLE);
                return;
            }
            None => return,
        }

        let Some(document) = &self.document else {
            return;
        };

        let zoom = self.viewport.zoom * MAGNIFY;
        let half = f64::from(LOUPE) / 2.0 / zoom;
        let region = Rect::new(
            cursor.x - half,
            cursor.y - half,
            cursor.x + half,
            cursor.y + half,
        );

        if let Ok(raster) =
            document.render_region(self.page, region, zoom * f64::from(ctx.pixels_per_point()))
        {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [raster.width, raster.height],
                &raster.rgba,
            );

            self.loupe = Some(Loupe {
                texture: ctx.load_texture("loupe", image, egui::TextureOptions::LINEAR),
                region: raster.region,
                centred_on: cursor,
            });
        }
    }

    /// The anchors a tool can reach: every one on the page while an anchor
    /// tool is in hand, or just the selected shape's while the selection tool
    /// is. The selected anchor also shows the handles either side of it.
    fn draw_anchors(&self, scene: &Scene) {
        let showing = match self.tool {
            tool if tool.edits_anchors() => Showing::Every,
            Tool::Select => match self.selected_measurement {
                Some(index) => Showing::JustOne(index),
                None => return,
            },
            _ => return,
        };

        let selected = self.editor.as_ref().and_then(|editor| editor.selected);

        for (index, measurement) in self
            .project
            .measurements
            .get(&self.page)
            .into_iter()
            .flatten()
            .enumerate()
        {
            if !measurement.visible || !showing.includes(index) {
                continue;
            }

            for (outline, subpath) in measurement.outlines() {
                for (at, anchor) in subpath.anchors.iter().enumerate() {
                    let is_selected = selected
                        == Some(tools::Selection {
                            measurement: index,
                            outline,
                            anchor: at,
                        });

                    if is_selected {
                        self.draw_handles(scene, anchor);
                    }

                    scene.painter.circle_filled(
                        scene.at(anchor.pos),
                        if is_selected {
                            ANCHOR_DOT + 1.5
                        } else {
                            ANCHOR_DOT
                        },
                        if is_selected { SELECTED } else { OUTLINE },
                    );
                }
            }
        }
    }

    /// A finished area: its outline, the holes in it, and what it covers in
    /// real units once they are taken off.
    fn draw_measurement(&self, scene: &Scene, measurement: &Measurement, selected: bool) {
        if !measurement.visible {
            return;
        }

        // The selected measurement is drawn heavier, which is all the
        // selection tool has to say about it.
        let stroke = egui::Stroke::new(if selected { 3.0 } else { 1.5 }, measurement.colour);

        for (_, subpath) in measurement.outlines() {
            let outline = geom::outline(subpath, scene.flatness());
            if outline.len() < 2 {
                continue;
            }

            scene.painter.add(egui::Shape::closed_line(
                outline.iter().map(|&p| scene.at(p)).collect(),
                stroke,
            ));
        }

        let Some(calibration) = self.project.calibrations.get(&self.page) else {
            return;
        };

        let area = area_label(
            calibration.square_millimetres(geom::measurement_area(measurement)),
            calibration.unit,
        );

        self.draw_label(
            scene,
            geom::centre(&measurement.outer),
            &format!("{} · {area}", measurement.name),
        );
    }

    /// The outline being traced: the anchors placed so far, the curve through
    /// them, a rubber band to the cursor, and a hint of the edge a right-click
    /// would close. Every band is the curve it will actually become, handles
    /// and all, rather than a straight stand-in.
    fn draw_pen(&self, scene: &Scene, cursor: Option<Point>) {
        let Some(pen) = &self.pen else {
            return;
        };
        let anchors = pen.anchors();
        let (Some(first), Some(last)) = (anchors.first(), anchors.last()) else {
            return;
        };

        let stroke = egui::Stroke::new(1.5, OUTLINE);
        self.draw_curve(scene, anchors.to_vec(), stroke);

        if let Some(cursor) = cursor {
            self.draw_curve(scene, vec![*last, Anchor::corner(cursor)], stroke);

            if anchors.len() >= 2 {
                self.draw_curve(
                    scene,
                    vec![Anchor::corner(cursor), *first],
                    egui::Stroke::new(1.0, OUTLINE.gamma_multiply(0.4)),
                );
            }
        }

        for anchor in anchors {
            scene
                .painter
                .circle_filled(scene.at(anchor.pos), ANCHOR_DOT, OUTLINE);
        }

        self.draw_handles(scene, last);
    }

    /// An open run of anchors, flattened to the accuracy the screen can show.
    fn draw_curve(&self, scene: &Scene, anchors: Vec<Anchor>, stroke: egui::Stroke) {
        let path = SubPath {
            anchors,
            closed: false,
        };
        let outline = geom::outline(&path, scene.flatness());

        if outline.len() > 1 {
            scene.painter.add(egui::Shape::line(
                outline.iter().map(|&p| scene.at(p)).collect(),
                stroke,
            ));
        }
    }

    /// The handle bar of an anchor, which is what a drag pulls out and what
    /// the modifier breaks apart.
    fn draw_handles(&self, scene: &Scene, anchor: &Anchor) {
        if anchor.in_handle == Vec2::ZERO && anchor.out_handle == Vec2::ZERO {
            return;
        }

        let stroke = egui::Stroke::new(1.0, HANDLE);

        for handle in [anchor.in_handle, anchor.out_handle] {
            let end = scene.at(anchor.pos + handle);
            scene
                .painter
                .line_segment([scene.at(anchor.pos), end], stroke);
            scene.painter.circle_filled(end, HANDLE_DOT, HANDLE);
        }
    }

    /// A readable caption on the drawing, sized in logical points so it stays
    /// legible at every zoom.
    fn draw_label(&self, scene: &Scene, at: Point, text: &str) {
        if !scene.labels {
            return;
        }

        let galley = scene.painter.layout_no_wrap(
            text.to_owned(),
            egui::FontId::proportional(12.0),
            egui::Color32::WHITE,
        );
        let rect = egui::Align2::CENTER_CENTER.anchor_size(scene.at(at), galley.size());

        scene
            .painter
            .rect_filled(rect.expand(4.0), 3, egui::Color32::from_black_alpha(190));
        scene.painter.galley(rect.min, galley, egui::Color32::WHITE);
    }

    fn draw_span(&self, scene: &Scene, from: Point, to: Point) {
        let a = scene.view.page_to_screen(from);
        let b = scene.view.page_to_screen(to);
        let stroke = egui::Stroke::new(1.5, SPAN);

        scene
            .painter
            .line_segment([to_egui_pos(a), to_egui_pos(b)], stroke);

        // End ticks are a fixed length on screen, so they stay readable
        // however far the drawing is zoomed.
        let along = b - a;
        let length = along.hypot();
        if length > f64::EPSILON {
            let across = Vec2::new(-along.y, along.x) / length * TICK;
            for end in [a, b] {
                scene.painter.line_segment(
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

        if self.document.is_some() && self.error.is_none() {
            self.tool_strip(ui);
            self.tool_groups(ui);
            self.measurements_panel(ui);
        }

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
