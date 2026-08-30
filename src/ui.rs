//! The application window: open a drawing, pan and zoom it, step through pages.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;
use kurbo::{Point, Rect, Shape, Size, Vec2};

use crate::doc::{self, Anchor, Calibration, Measurement, Outline, Project, SubPath, Unit};
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
const SNAPPED: egui::Color32 = egui::Color32::from_rgb(255, 0, 160);

/// How wide the panels start out.
const PANEL_WIDTH: f32 = 320.0;

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
    /// `u` — setting the page scale, once per drawing.
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
#[derive(Clone, Copy)]
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
    /// Every outline of one area: what an anchor tool can reach while it has
    /// that area in hand.
    OneArea(usize),
    /// One outline of one area: what the selection tool has hold of.
    JustOne(usize, Outline),
}

impl Showing {
    fn includes(&self, measurement: usize, outline: Outline) -> bool {
        match self {
            Self::Every => true,
            Self::OneArea(index) => *index == measurement,
            Self::JustOne(index, which) => *index == measurement && *which == outline,
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

/// What saving or exporting had to say, kept in the toolbar until the next one
/// has something to say instead.
struct Notice {
    text: String,
    /// Whether it is a complaint rather than a report.
    problem: bool,
}

/// What a confirmation is standing in the way of: putting the drawing down,
/// or shutting the window on it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Closing {
    Project,
    Window,
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
    save: bool,
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

            // `+` is a shifted key on most layouts and a key of its own on the
            // number pad, so shift is not part of the binding: however the
            // sign was typed, it means the same thing.
            let sign = |k| key(k) && !i.modifiers.command;

            Self {
                page_up: key(egui::Key::PageUp),
                page_down: key(egui::Key::PageDown),
                escape: i.key_pressed(egui::Key::Escape),
                delete: key(egui::Key::Delete) || key(egui::Key::Backspace),
                undo: command && !i.modifiers.shift && key(egui::Key::Z),
                redo: command && (key(egui::Key::Y) || (i.modifiers.shift && key(egui::Key::Z))),
                save: command && key(egui::Key::S),
                tool: if plain(egui::Key::U) {
                    Some(Tool::Calibrate)
                } else if shifted(egui::Key::C) {
                    Some(Tool::AnchorPoint)
                } else if plain(egui::Key::V) {
                    Some(Tool::Select)
                } else if plain(egui::Key::A) {
                    Some(Tool::DirectSelect)
                } else if plain(egui::Key::P) {
                    Some(Tool::Pen)
                } else if sign(egui::Key::Plus) || sign(egui::Key::Equals) {
                    Some(Tool::AddAnchor)
                } else if sign(egui::Key::Minus) {
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
    /// The drawing on screen, which is where its sidecar is kept.
    drawing: Option<PathBuf>,
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
    /// Whether the project has changed since it was last written out.
    unsaved: bool,
    /// What closing is waiting on an answer about, if anything is.
    closing: Option<Closing>,
    /// Whether the window has been given leave to shut on unsaved work, so
    /// that the question is not asked a second time.
    may_close: bool,
    /// What saving or exporting last reported.
    notice: Option<Notice>,
    /// States to go back to, and the ones undone out of.
    undo: Vec<Project>,
    redo: Vec<Project>,
    /// The tool in hand, and the state belonging to it.
    tool: Tool,
    /// The precision aids, on or off.
    assist: Assist,
    /// Whether the column beside the tool strip is folded away. Held the
    /// negative way round so that starting open needs no ceremony.
    groups_hidden: bool,
    /// How wide the measurements panel is, which the empty column matches.
    panel_width: f32,
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
    /// The area being worked on, by one of its outlines, whatever tool is in
    /// hand. Every tool reads this one: the anchor tools show and edit only
    /// this area, `h` punches its hole in it, and the list picks its row out.
    /// Clicking inside a hole takes the hole rather than the area it is cut
    /// from, which is what makes a hole removable.
    in_hand: Option<(usize, Outline)>,
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
        self.unsaved = false;
        self.notice = None;
        self.undo.clear();
        self.redo.clear();
        self.take_up(Tool::Select);

        match pdf::Document::open(&path) {
            Ok(document) => {
                self.error = None;
                self.page_count = document.page_count();
                self.document = Some(document);
                self.drawing = Some(path.clone());

                // Work already done on this drawing comes back with it. A
                // sidecar that cannot be read is worth saying so about, but
                // not worth withholding the drawing over.
                match doc::load(&path) {
                    Ok(Some(project)) => {
                        self.project = project;
                        self.report(format!(
                            "Reopened {}",
                            doc::file_name(&doc::sidecar_path(&path))
                        ));
                    }
                    Ok(None) => {}
                    Err(message) => self.complain(message),
                }

                self.show_page(0);
            }
            Err(message) => {
                self.document = None;
                self.drawing = None;
                self.page_count = 0;
                self.error = Some(message);
            }
        }
    }

    /// Writes the project to its sidecar, beside the drawing.
    fn save(&mut self) {
        let Some(drawing) = self.drawing.clone() else {
            return;
        };

        match doc::save(&self.project, &drawing) {
            Ok(path) => {
                self.unsaved = false;
                self.report(format!("Saved {}", doc::file_name(&path)));
            }
            Err(message) => self.complain(message),
        }
    }

    /// Asks where the CSV should go, and writes it there.
    fn export_dialog(&mut self) {
        let Some(drawing) = self.drawing.clone() else {
            return;
        };

        let mut dialog = rfd::FileDialog::new()
            .add_filter("Comma separated values", &["csv"])
            .set_file_name(doc::file_name(&drawing.with_extension("csv")));

        // The drawing's own folder is the likeliest place for the export.
        if let Some(folder) = drawing.parent() {
            dialog = dialog.set_directory(folder);
        }

        let Some(path) = dialog.save_file() else {
            return;
        };

        match doc::export_csv(&self.project, &path) {
            Ok(0) => self.complain("Nothing measured to export".to_owned()),
            Ok(rows) => self.report(format!(
                "Exported {rows} measurement{} to {}",
                if rows == 1 { "" } else { "s" },
                doc::file_name(&path)
            )),
            Err(message) => self.complain(message),
        }
    }

    /// Puts the drawing down, asking first if that would throw away work.
    fn close_project(&mut self) {
        if self.unsaved {
            self.closing = Some(Closing::Project);
        } else {
            self.close();
        }
    }

    /// The window has been asked to shut, by the title bar or otherwise.
    /// Unsaved measurements are worth the same question the File menu asks, so
    /// the shutting is called off until it has an answer.
    fn intercept_close(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }

        if self.unsaved && !self.may_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.closing = Some(Closing::Window);
        }
    }

    /// Back to the window as it started, with nothing open. The aids and the
    /// widths are how the user likes the window, not part of the project, so
    /// they stay as they were.
    fn close(&mut self) {
        *self = Self {
            assist: self.assist,
            groups_hidden: self.groups_hidden,
            panel_width: self.panel_width,
            ..Self::default()
        };
    }

    /// The question asked when closing would lose unsaved measurements.
    fn confirm_close(&mut self, ctx: &egui::Context) {
        let Some(closing) = self.closing else {
            return;
        };

        let (question, keep) = match closing {
            Closing::Project => ("Close the project?", "Save and close"),
            Closing::Window => ("Close the application?", "Save and quit"),
        };

        let modal = egui::Modal::new(egui::Id::new("confirm close")).show(ctx, |ui| {
            ui.heading(question);
            ui.label("The measurements on this drawing have not been saved.");
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                if ui.button(keep).clicked() {
                    self.save();

                    // A save that failed has already said so; stay where we
                    // are rather than close over the top of it.
                    if self.unsaved {
                        self.closing = None;
                    } else {
                        self.go_through_with(closing, ctx);
                    }
                }

                if ui.button("Discard").clicked() {
                    self.go_through_with(closing, ctx);
                }

                if ui.button("Cancel").clicked() {
                    self.closing = None;
                }
            });
        });

        // Escape, or a click on the backdrop, means the same as Cancel.
        if modal.should_close() {
            self.closing = None;
        }
    }

    /// Sees a closing through, the unsaved work having been dealt with one way
    /// or the other.
    fn go_through_with(&mut self, closing: Closing, ctx: &egui::Context) {
        match closing {
            Closing::Project => self.close(),
            Closing::Window => {
                self.may_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn report(&mut self, text: String) {
        self.notice = Some(Notice {
            text,
            problem: false,
        });
    }

    fn complain(&mut self, text: String) {
        self.notice = Some(Notice {
            text,
            problem: true,
        });
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
                // is nothing to trace on a page with no scale. What was in
                // hand, and the measurement a hole was going to go into,
                // index one page's measurements and mean nothing on another.
                self.let_go();
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
        self.keep(self.project.clone());
    }

    /// Keeps `snapshot` as the state to go back to. Anything undone is
    /// discarded, since the history is a line rather than a tree, and the
    /// project has now moved on from whatever is written beside the drawing.
    fn keep(&mut self, snapshot: Project) {
        self.undo.push(snapshot);
        self.redo.clear();
        self.unsaved = true;
    }

    /// Runs a change against the current page's measurements, keeping a
    /// snapshot only if the change reports it did something.
    fn edit<T>(&mut self, change: impl FnOnce(&mut Vec<Measurement>) -> Option<T>) -> Option<T> {
        let snapshot = self.project.clone();
        let outcome = change(self.project.measurements.entry(self.page).or_default());

        if outcome.is_some() {
            self.keep(snapshot);
        }

        outcome
    }

    fn undo(&mut self) {
        if let Some(previous) = self.undo.pop() {
            let current = std::mem::replace(&mut self.project, previous);
            self.redo.push(current);
            self.unsaved = true;
            self.let_go();
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            let current = std::mem::replace(&mut self.project, next);
            self.undo.push(current);
            self.unsaved = true;
            self.let_go();
        }
    }

    /// Lets go of the anchor held within the area in hand, the area itself
    /// staying where it is.
    fn let_go_of_anchor(&mut self) {
        if let Some(editor) = &mut self.editor {
            editor.selected = None;
            editor.grabbed = None;
        }
    }

    /// Lets go of everything: the area in hand and any anchor within it. What
    /// was in hand is a pair of indices, which the state it was taken from no
    /// longer guarantees.
    fn let_go(&mut self) {
        self.in_hand = None;
        self.let_go_of_anchor();
    }

    /// Takes an area up: what the tools show and act on from here. Any anchor
    /// held in the last one goes with it.
    fn take_in_hand(&mut self, shape: (usize, Outline)) {
        self.in_hand = Some(shape);
        self.let_go_of_anchor();
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

        let traced = match target.and_then(|index| Some((index, measurements.get_mut(index)?))) {
            Some((index, measurement)) => {
                // Wound against its outline, so it takes area away rather than
                // adding it, however it was traced.
                let hole = geom::as_hole(&measurement.outer, outline);
                measurement.holes.push(hole);

                // One press of `h`, one hole. The pen goes back to drawing new
                // areas rather than quietly filling the same one with holes.
                self.pen_target = None;

                (index, Outline::Hole(measurement.holes.len() - 1))
            }
            None => {
                let name = format!("Area {}", measurements.len() + 1);

                measurements.push(Measurement {
                    name,
                    outer: outline,
                    holes: Vec::new(),
                    colour: OUTLINE,
                    visible: true,
                });

                (measurements.len() - 1, Outline::Outer)
            }
        };

        // What was just traced is what is in hand, so `h` punches a hole in it
        // without a selection step in between.
        self.take_in_hand(traced);
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

    /// Everything that acts on the project as a whole, in one place.
    fn file_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("File", |ui| {
            if ui.button("Open…").clicked() {
                self.open_dialog();
            }

            // Every one of these needs a drawing to act on.
            let open = self.drawing.is_some();

            if ui
                .add_enabled(open, egui::Button::new("Save").shortcut_text("Ctrl+S"))
                .clicked()
            {
                self.save();
            }

            if ui
                .add_enabled(open, egui::Button::new("Export CSV…"))
                .clicked()
            {
                self.export_dialog();
            }

            ui.separator();

            // A drawing that failed to open is also something to close: it is
            // what the window is showing.
            if ui
                .add_enabled(
                    open || self.error.is_some(),
                    egui::Button::new("Close project"),
                )
                .clicked()
            {
                self.close_project();
            }
        });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            self.file_menu(ui);

            if self.document.is_none() {
                return;
            }

            // Whether there is anything to save is worth a glance, not a
            // sentence.
            ui.weak(if self.unsaved { "•" } else { " " })
                .on_hover_text(if self.unsaved {
                    "Unsaved changes"
                } else {
                    "Saved"
                });

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

            // Snapping is silent when nothing is in reach, so it says whether
            // it is even looking.
            if matches!(self.tool, Tool::Pen | Tool::DirectSelect) {
                ui.weak(if self.assist.snap {
                    "· snapping"
                } else {
                    "· snapping off (n)"
                });
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                match self.project.calibrations.get(&self.page) {
                    Some(calibration) => ui.label(scale_label(calibration)),
                    None => ui.label("Not calibrated"),
                };

                if let Some(notice) = &self.notice {
                    ui.separator();
                    if notice.problem {
                        ui.colored_label(ui.visuals().error_fg_color, &notice.text);
                    } else {
                        ui.weak(&notice.text);
                    }
                }
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
        const TOOLS: [(Tool, &str, &str); 7] = [
            (Tool::Select, "v", "Selection (v)"),
            (Tool::DirectSelect, "a", "Direct Selection (a)"),
            (Tool::Pen, "p", "Pen (p)"),
            (Tool::AnchorPoint, "⇧C", "Anchor Point (Shift+c)"),
            (Tool::AddAnchor, "+", "Add Anchor Point (+)"),
            (Tool::DeleteAnchor, "−", "Delete Anchor Point (−)"),
            (Tool::Calibrate, "u", "Calibrate the page scale (u)"),
        ];

        // Labels only, holding letters for a later project. Nothing lies
        // behind them; see CLAUDE.md §2.
        const PLACEHOLDERS: [(&str, &str); 7] = [
            ("\\", "Line — not in this project"),
            ("m", "Rectangle — not in this project"),
            ("o", "Ellipse — not in this project"),
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

                // The panel beside this one folds away against the edge of
                // the window, and this is what brings it back.
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);

                let (arrow, name) = if self.groups_hidden {
                    ("»", "Open the panel")
                } else {
                    ("«", "Fold the panel away")
                };

                if ui
                    .add_sized([32.0, 28.0], egui::Button::new(arrow))
                    .on_hover_text(name)
                    .clicked()
                {
                    self.groups_hidden = !self.groups_hidden;
                }
            });
    }

    /// The measurement tools, grouped and collapsible.
    fn tool_groups(&mut self, ui: &mut egui::Ui) {
        if self.groups_hidden {
            return;
        }

        // Deliberately empty. The column is here to hold the space for the
        // groups of tools a later project will fill it with; nothing this
        // project has belongs in it. Everything that was here has moved: the
        // tools to the strip, and Length to a reserved shortcut in CLAUDE.md
        // rather than a button that does nothing.
        //
        // It is given the measurements panel's width outright rather than a
        // default of its own. A default is only consulted the first time a
        // panel is ever laid out, and the width is then remembered between
        // runs, so changing one has no effect on a window that has already
        // been opened once. Taking the width each frame also keeps the two
        // sides even when either is dragged.
        // Nothing has measured the other side yet on the very first frame.
        let width = if self.panel_width > 0.0 {
            self.panel_width
        } else {
            PANEL_WIDTH
        };

        egui::Panel::left("tool_groups")
            .exact_size(width)
            .resizable(false)
            .show(ui, |_ui| {});
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

            // Letting go puts every area back within reach of the tools.
            self.let_go();
        }

        if input.undo {
            self.undo();
        }
        if input.redo {
            self.redo();
        }
        if input.save {
            self.save();
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
            && let Some(index) = self.area_in_hand()
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
        let panel = egui::Panel::right("measurements")
            .default_size(PANEL_WIDTH)
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
                // Whatever is in hand is picked out whatever tool is holding
                // it, so the canvas and the list never disagree.
                let selected = self.in_hand;
                let mut remove = None;
                let mut remove_hole = None;
                let mut hole_in = None;

                let measurements = self
                    .project
                    .measurements
                    .get_mut(&self.page)
                    .expect("checked above");

                for (index, measurement) in measurements.iter_mut().enumerate() {
                    // The row of whatever the selection tool has hold of is
                    // picked out, so the canvas and the list agree.
                    if selected.map(|(index, _)| index) == Some(index) {
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

                        // The perimeter is the outline's own: a hole is a
                        // boundary of its own, reported on its own row.
                        ui.weak(format!(
                            "· {}",
                            length_label(
                                calibration.millimetres(geom::perimeter(&measurement.outer)),
                                calibration.unit,
                            )
                        ))
                        .on_hover_text("Perimeter");

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Remove").clicked() {
                                remove = Some(index);
                            }
                            if ui.button("Hole").clicked() {
                                hole_in = Some(index);
                            }
                        });
                    });

                    // Each hole, with what it takes off and a way to take it
                    // out again.
                    for (hole, subpath) in measurement.holes.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.add_space(16.0);
                            ui.weak(format!("Hole {}", hole + 1));
                            ui.weak(format!(
                                "− {}",
                                area_label(
                                    calibration.square_millimetres(geom::taken_by(
                                        &measurement.outer,
                                        subpath
                                    )),
                                    calibration.unit,
                                )
                            ));
                            ui.weak(format!(
                                "· {}",
                                length_label(
                                    calibration.millimetres(geom::perimeter(subpath)),
                                    calibration.unit,
                                )
                            ))
                            .on_hover_text("Perimeter");

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.small_button("Remove").clicked() {
                                        remove_hole = Some((index, hole));
                                    }
                                },
                            );
                        });
                    }

                    ui.separator();
                }

                if let Some(index) = remove
                    && index < measurements.len()
                {
                    measurements.remove(index);
                    changed = true;
                }

                if let Some((index, hole)) = remove_hole
                    && let Some(measurement) = measurements.get_mut(index)
                    && hole < measurement.holes.len()
                {
                    measurement.holes.remove(hole);
                    changed = true;
                }

                self.renaming = renaming;

                if changed {
                    self.keep(snapshot);
                }

                if remove.is_some() || remove_hole.is_some() {
                    // The indices everything else was holding have moved.
                    self.let_go();
                    self.take_up(Tool::Select);
                }

                if let Some(index) = hole_in {
                    // The area the hole is going into is the one in hand,
                    // whichever one was there before.
                    self.take_in_hand((index, Outline::Outer));
                    self.take_up(Tool::Pen);
                    self.pen_target = Some(index);
                }
            });

        // What the empty column on the other side matches itself to.
        self.panel_width = panel.response.rect.width();
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

        editor.hit(measurements, self.area_in_hand(), point, radius)
    }

    /// Which area is in hand, if one is: the only one an anchor tool reaches
    /// into, and the one a hole is punched in.
    fn area_in_hand(&self) -> Option<usize> {
        self.in_hand.map(|(index, _)| index)
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

            // Dragging an anchor takes its area up, the same as clicking one.
            self.in_hand = Some((found.0.measurement, found.0.outline));

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

            // A click on another area takes that one up rather than editing it
            // at arm's length: a tool only ever acts on the anchors it is
            // showing. The click after this one edits it.
            if let Some(shape) = self.elsewhere(point) {
                self.take_in_hand(shape);
                return;
            }

            let found = self.hit(point, radius).map(|(selection, _)| selection);
            let within = self.area_in_hand();

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
                    self.edit(|measurements| tools::insert(measurements, within, point, radius))
                }
                _ => found,
            };

            // Whatever was acted on is now the area in hand, which is how the
            // tools come to have one when they started with none.
            if let Some(selection) = selected.or(found) {
                self.in_hand = Some((selection.measurement, selection.outline));
            }

            if let Some(editor) = &mut self.editor {
                editor.selected = selected;
            }
        }

        let Some(selection) = self.editor.as_ref().and_then(|editor| editor.selected) else {
            return;
        };

        if input.delete && self.edit(|m| tools::delete(m, selection)).is_some() {
            self.let_go_of_anchor();
        }
    }

    /// What lies under a page point, taken whole: a hole if the point is in
    /// one, otherwise the area itself, from inside it or near its edge.
    ///
    /// Holes come first because a point inside one is inside the outline too,
    /// and the hole is the smaller, more particular thing to have meant.
    fn shape_at(&self, point: Point) -> Option<(usize, Outline)> {
        // Later measurements are drawn over earlier ones, so they are the
        // ones a click means.
        let measurements = self.project.measurements.get(&self.page)?;

        measurements
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, measurement)| measurement.visible)
            .find_map(|(index, measurement)| Some((index, self.shape_of(measurement, point)?)))
    }

    /// Which outline of one measurement a point falls on, by the same reading
    /// as `shape_at`: the hole it is in, or else the area itself.
    fn shape_of(&self, measurement: &Measurement, point: Point) -> Option<Outline> {
        let reach = HIT / self.viewport.zoom;

        let within = |subpath: &SubPath| {
            geom::bez_path(subpath).contains(point)
                || geom::nearest(subpath, point).is_some_and(|found| found.distance <= reach)
        };

        match measurement.holes.iter().position(&within) {
            Some(hole) => Some(Outline::Hole(hole)),
            None => within(&measurement.outer).then_some(Outline::Outer),
        }
    }

    /// The area a click has landed on when it is not the one in hand — what an
    /// anchor tool takes up instead of editing this one at arm's length.
    ///
    /// `None` when the click still belongs to the area in hand, when there is
    /// nothing under it, or when nothing is in hand: in that last case every
    /// area is within reach already.
    fn elsewhere(&self, point: Point) -> Option<(usize, Outline)> {
        let (in_hand, _) = self.in_hand?;
        let measurements = self.project.measurements.get(&self.page)?;

        // A click that still lands on the area in hand belongs to it, even
        // where another area overlaps it.
        if measurements
            .get(in_hand)
            .and_then(|measurement| self.shape_of(measurement, point))
            .is_some()
        {
            return None;
        }

        self.shape_at(point).filter(|(index, _)| *index != in_hand)
    }

    /// Selecting with the selection tool: click an area, or a hole in one.
    /// Dragging moves the whole area; Delete takes off whatever is selected —
    /// the area, or just the hole.
    fn select_measurement(&mut self, ui: &egui::Ui, response: &egui::Response, input: &Input) {
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
        }

        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            self.in_hand = self.shape_at(self.page_point(pos));
        }

        // A drag takes hold of whatever it started on, and moves it as one
        // thing: outline, holes and all.
        if response.drag_started_by(egui::PointerButton::Primary)
            && let Some(origin) = input.pressed_at
        {
            let from = self.page_point(origin);

            if let Some((index, outline)) = self.shape_at(from) {
                self.commit();
                self.in_hand = Some((index, outline));

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
            && let Some((index, outline)) = self.in_hand
        {
            let removed = self.edit(|measurements| match outline {
                Outline::Outer => (index < measurements.len()).then(|| measurements.remove(index)),
                Outline::Hole(hole) => {
                    let measurement = measurements.get_mut(index)?;

                    (hole < measurement.holes.len()).then(|| {
                        measurement.holes.remove(hole);
                        measurement.clone()
                    })
                }
            });

            if removed.is_some() {
                self.let_go();
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
            let selected = self
                .in_hand
                .filter(|(selected, _)| *selected == index)
                .map(|(_, outline)| outline);

            self.draw_measurement(scene, measurement, selected);
        }

        self.draw_pen(scene, cursor);
        self.draw_anchors(scene);

        // The ring was white, which on a white drawing is nothing at all.
        // A dark halo under a bright ring reads on paper and on the backdrop
        // either side of the sheet.
        if let Some(caught) = self.snapped {
            let at = scene.at(caught);
            let radius = ANCHOR_DOT + 4.0;

            scene.painter.circle_stroke(
                at,
                radius,
                egui::Stroke::new(3.0, egui::Color32::from_black_alpha(120)),
            );
            scene
                .painter
                .circle_stroke(at, radius, egui::Stroke::new(1.5, SNAPPED));
            scene.painter.circle_filled(at, 1.5, SNAPPED);
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
            // An anchor tool shows what it can reach: the area in hand, or
            // every area while it has none.
            tool if tool.edits_anchors() => match self.in_hand {
                Some((index, _)) => Showing::OneArea(index),
                None => Showing::Every,
            },
            Tool::Select => match self.in_hand {
                Some((index, outline)) => Showing::JustOne(index, outline),
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
            if !measurement.visible {
                continue;
            }

            for (outline, subpath) in measurement.outlines() {
                if !showing.includes(index, outline) {
                    continue;
                }

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
    fn draw_measurement(
        &self,
        scene: &Scene,
        measurement: &Measurement,
        selected: Option<Outline>,
    ) {
        if !measurement.visible {
            return;
        }

        for (which, subpath) in measurement.outlines() {
            let outline = geom::outline(subpath, scene.flatness());
            if outline.len() < 2 {
                continue;
            }

            // Whatever is selected is drawn heavier, which is all the
            // selection tool has to say about it. Selecting a hole picks out
            // the hole alone, since that is what Delete would take off.
            let weight = if selected == Some(which) { 3.0 } else { 1.5 };

            scene.painter.add(egui::Shape::closed_line(
                outline.iter().map(|&p| scene.at(p)).collect(),
                egui::Stroke::new(weight, measurement.colour),
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

/// A length in the unit a person would read it in, the same way round as
/// `area_label`: metres for a metric drawing, feet for an imperial one,
/// dropping to the small unit when the large one says nothing.
fn length_label(millimetres: f64, unit: Unit) -> String {
    if unit.is_metric() {
        let metres = millimetres / 1e3;

        if metres >= 0.1 {
            format!("{metres:.2} m")
        } else {
            format!("{millimetres:.0} mm")
        }
    } else {
        let inches = millimetres / 25.4;
        let feet = inches / 12.0;

        if feet >= 0.5 {
            format!("{feet:.2} ft")
        } else {
            format!("{inches:.1} in")
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

        let ctx = ui.ctx().clone();
        self.intercept_close(&ctx);
        self.confirm_close(&ctx);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// What `Input::read` makes of one key press, with the modifiers held.
    fn pressing(key: egui::Key, physical: egui::Key, modifiers: egui::Modifiers) -> Input {
        let context = egui::Context::default();
        let mut raw = egui::RawInput::default();

        // What the window layer sends: the modifiers change, then the key.
        raw.events.push(egui::Event::ModifiersChanged(modifiers));
        raw.events.push(egui::Event::Key {
            key,
            physical_key: Some(physical),
            pressed: true,
            repeat: false,
            modifiers,
        });

        let mut read = None;
        let output = context.run_ui(raw, |ui| read = Some(Input::read(ui)));
        output.drop_without_applying_deltas();

        read.expect("the frame ran")
    }

    /// On most layouts `+` is typed by holding shift over `=`, which is the
    /// same key press either way and has to reach the same tool.
    #[test]
    fn the_add_anchor_key_answers_however_the_sign_was_typed() {
        let shift = egui::Modifiers {
            shift: true,
            ..Default::default()
        };

        for input in [
            pressing(egui::Key::Plus, egui::Key::Equals, shift),
            pressing(egui::Key::Equals, egui::Key::Equals, Default::default()),
            pressing(egui::Key::Plus, egui::Key::Plus, Default::default()),
        ] {
            assert_eq!(input.tool, Some(Tool::AddAnchor));
        }
    }

    #[test]
    fn the_delete_anchor_key_answers_however_the_sign_was_typed() {
        for input in [
            pressing(egui::Key::Minus, egui::Key::Minus, Default::default()),
            pressing(
                egui::Key::Minus,
                egui::Key::Minus,
                egui::Modifiers {
                    shift: true,
                    ..Default::default()
                },
            ),
        ] {
            assert_eq!(input.tool, Some(Tool::DeleteAnchor));
        }
    }

    /// The letters stay case-sensitive: shift makes a different binding rather
    /// than the same one.
    #[test]
    fn a_shifted_letter_is_not_the_letter() {
        let shift = egui::Modifiers {
            shift: true,
            ..Default::default()
        };

        assert_eq!(
            pressing(egui::Key::C, egui::Key::C, shift).tool,
            Some(Tool::AnchorPoint)
        );
        assert_eq!(
            pressing(egui::Key::C, egui::Key::C, Default::default()).tool,
            None
        );
        assert_eq!(
            pressing(egui::Key::V, egui::Key::V, Default::default()).tool,
            Some(Tool::Select)
        );
        assert_eq!(pressing(egui::Key::V, egui::Key::V, shift).tool, None);
    }
}
