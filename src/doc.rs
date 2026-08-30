//! What the user builds on top of a drawing: the scale of a page, and the
//! areas measured on it.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use eframe::egui::Color32;
use kurbo::{Point, Vec2};
use serde::{Deserialize, Serialize};

use crate::geom;

/// Millimetres in one PDF point: 72 points to the inch, 25.4 mm to the inch.
const MM_PER_POINT: f64 = 25.4 / 72.0;

/// The unit a real-world distance was given in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unit {
    Millimetres,
    Centimetres,
    Metres,
    Inches,
    Feet,
}

impl Unit {
    /// Every unit that can be picked, in the order they are offered.
    pub const ALL: [Self; 5] = [
        Self::Millimetres,
        Self::Centimetres,
        Self::Metres,
        Self::Inches,
        Self::Feet,
    ];

    /// Metric units read naturally in square metres, imperial ones in square
    /// feet, which is the only thing this distinction is for.
    pub fn is_metric(self) -> bool {
        matches!(self, Self::Millimetres | Self::Centimetres | Self::Metres)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Millimetres => "mm",
            Self::Centimetres => "cm",
            Self::Metres => "m",
            Self::Inches => "in",
            Self::Feet => "ft",
        }
    }

    fn millimetres(self) -> f64 {
        match self {
            Self::Millimetres => 1.0,
            Self::Centimetres => 10.0,
            Self::Metres => 1000.0,
            Self::Inches => 25.4,
            Self::Feet => 304.8,
        }
    }
}

/// A page's scale, fixed by naming the real distance between two points on it.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Calibration {
    /// Page space, in points.
    pub from: Point,
    pub to: Point,
    /// The real distance from `from` to `to`.
    pub distance: f64,
    pub unit: Unit,
}

impl Calibration {
    /// `None` when the two points coincide or the distance is not a positive
    /// number: neither yields a scale.
    pub fn new(from: Point, to: Point, distance: f64, unit: Unit) -> Option<Self> {
        if !distance.is_finite() || distance <= 0.0 || (to - from).hypot() <= f64::EPSILON {
            return None;
        }

        Some(Self {
            from,
            to,
            distance,
            unit,
        })
    }

    /// Real-world units per PDF point: the number every measurement on this
    /// page is scaled by. It is a property of the page, so zoom cannot alter it.
    pub fn units_per_point(&self) -> f64 {
        self.distance / (self.to - self.from).hypot()
    }

    /// The drawing ratio: real millimetres covered by one millimetre of paper.
    /// A 1:100 drawing gives 100.
    pub fn ratio(&self) -> f64 {
        self.units_per_point() * self.unit.millimetres() / MM_PER_POINT
    }

    /// The real area, in square millimetres, of `area` square page points.
    /// Square millimetres because every unit converts to them exactly; how the
    /// number is then read out is a question for whoever displays it.
    pub fn square_millimetres(&self, area: f64) -> f64 {
        area * (self.units_per_point() * self.unit.millimetres()).powi(2)
    }

    /// The real length, in millimetres, of `length` page points.
    pub fn millimetres(&self, length: f64) -> f64 {
        length * self.units_per_point() * self.unit.millimetres()
    }
}

/// A colour as the four bytes it already is, rather than whatever shape the
/// window toolkit's own serialisation would give it.
mod colour {
    use eframe::egui::Color32;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(colour: &Color32, serializer: S) -> Result<S::Ok, S::Error> {
        colour.to_array().serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Color32, D::Error> {
        let [r, g, b, a] = <[u8; 4]>::deserialize(deserializer)?;

        Ok(Color32::from_rgba_premultiplied(r, g, b, a))
    }
}

/// How an anchor's two handles relate to each other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnchorKind {
    Corner,
    Smooth,
    Asymmetric,
}

/// A point on a path, with the handles that shape the curve either side of it.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Anchor {
    /// Page space.
    pub pos: Point,
    /// Relative to `pos`.
    pub in_handle: Vec2,
    /// Relative to `pos`.
    pub out_handle: Vec2,
    pub kind: AnchorKind,
}

impl Anchor {
    /// A corner: both handles collapsed onto the point, which leaves the
    /// edges either side straight.
    pub fn corner(pos: Point) -> Self {
        Self {
            pos,
            in_handle: Vec2::ZERO,
            out_handle: Vec2::ZERO,
            kind: AnchorKind::Corner,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SubPath {
    pub anchors: Vec<Anchor>,
    pub closed: bool,
}

/// Which outline of a measurement: the one round the outside, or one of the
/// holes punched in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outline {
    Outer,
    Hole(usize),
}

/// A named area on a page: an outline, and the holes punched in it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Measurement {
    pub name: String,
    pub outer: SubPath,
    pub holes: Vec<SubPath>,
    #[serde(with = "colour")]
    pub colour: Color32,
    pub visible: bool,
}

impl Measurement {
    pub fn outline(&self, which: Outline) -> Option<&SubPath> {
        match which {
            Outline::Outer => Some(&self.outer),
            Outline::Hole(index) => self.holes.get(index),
        }
    }

    pub fn outline_mut(&mut self, which: Outline) -> Option<&mut SubPath> {
        match which {
            Outline::Outer => Some(&mut self.outer),
            Outline::Hole(index) => self.holes.get_mut(index),
        }
    }

    /// Moves the whole measurement, outline and holes together. Handles are
    /// held relative to their anchor, so only the anchors move.
    pub fn translate(&mut self, by: Vec2) {
        for subpath in std::iter::once(&mut self.outer).chain(&mut self.holes) {
            for anchor in &mut subpath.anchors {
                anchor.pos += by;
            }
        }
    }

    /// Every outline in turn, outer first, each with the way to address it.
    pub fn outlines(&self) -> impl Iterator<Item = (Outline, &SubPath)> {
        std::iter::once((Outline::Outer, &self.outer)).chain(
            self.holes
                .iter()
                .enumerate()
                .map(|(index, hole)| (Outline::Hole(index), hole)),
        )
    }
}

/// Which sheet: a page of one of the project's drawings.
///
/// Everything the user builds is filed under one of these. A drawing is
/// referred to by its place in the project's list, so moving the file it came
/// from changes nothing but where the file is looked for.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct Sheet {
    pub drawing: usize,
    pub page: usize,
}

impl Sheet {
    pub fn new(drawing: usize, page: usize) -> Self {
        Self { drawing, page }
    }
}

/// A drawing the project measures on.
#[derive(Clone, Debug)]
pub struct Drawing {
    /// Where the file is, as the project last knew it. Written to the project
    /// file relative to the project file itself, so a folder can be moved or
    /// copied whole and still open.
    pub path: PathBuf,
}

impl Drawing {
    /// What the drawing is called in the list: its file name without the
    /// extension, which is what a sheet is usually named after.
    pub fn name(&self) -> String {
        self.path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| file_name(&self.path))
    }
}

/// Everything the user has built, and the drawings it was built on.
///
/// This is the unit of undo: it is cloned whole before each committed
/// operation. The document is small, and a stack of clones is far easier to
/// keep correct than a set of reversible commands.
#[derive(Clone, Default)]
pub struct Project {
    pub drawings: Vec<Drawing>,
    pub calibrations: HashMap<Sheet, Calibration>,
    pub measurements: HashMap<Sheet, Vec<Measurement>>,
}

impl Project {
    /// A project holding one drawing, with nothing measured on it yet.
    pub fn of(drawing: &Path) -> Self {
        Self {
            drawings: vec![Drawing {
                path: drawing.to_path_buf(),
            }],
            ..Self::default()
        }
    }

    /// Every sheet with anything on it, in order, so that two saves of the
    /// same work give the same file and an export reads down the sheets.
    pub fn worked(&self) -> Vec<Sheet> {
        let mut sheets: Vec<Sheet> = self
            .calibrations
            .keys()
            .chain(self.measurements.keys())
            .copied()
            .collect();

        sheets.sort_unstable();
        sheets.dedup();
        sheets
    }
}

/// The project file's format. Raised when a change would stop this reader
/// making sense of a file, so a file from the future is refused rather than
/// half-understood. Version 1 was a sidecar beside a single drawing, and is
/// still read.
const VERSION: u32 = 2;

/// The project as it is written to disk.
#[derive(Serialize, Deserialize)]
struct Stored {
    version: u32,
    drawings: Vec<StoredDrawing>,
    sheets: Vec<StoredSheet>,
}

#[derive(Serialize, Deserialize)]
struct StoredDrawing {
    /// Relative to the project file wherever that is possible.
    path: String,
}

/// One sheet's work. A sheet with nothing on it is not written at all.
#[derive(Serialize, Deserialize)]
struct StoredSheet {
    drawing: usize,
    page: usize,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    calibration: Option<Calibration>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    measurements: Vec<Measurement>,
}

/// Phase one's file: one drawing, and its pages keyed by number.
#[derive(Deserialize)]
struct StoredV1 {
    drawing: String,
    calibrations: BTreeMap<usize, Calibration>,
    measurements: BTreeMap<usize, Vec<Measurement>>,
}

/// What a project file is called for a drawing that has none yet: the same
/// name, beside it.
pub fn project_path(drawing: &Path) -> PathBuf {
    drawing.with_extension("measure.json")
}

/// Phase one's sidecar for a drawing, which is read when there is no project
/// file to read instead.
pub fn sidecar_path(drawing: &Path) -> PathBuf {
    drawing.with_extension("measurements.json")
}

/// Writes the project to `path`.
pub fn save(project: &Project, path: &Path) -> Result<(), String> {
    let folder = path.parent().unwrap_or(Path::new("."));

    let stored = Stored {
        version: VERSION,
        drawings: project
            .drawings
            .iter()
            .map(|drawing| StoredDrawing {
                path: written_relative(&drawing.path, folder),
            })
            .collect(),
        sheets: project
            .worked()
            .into_iter()
            .filter_map(|sheet| {
                let calibration = project.calibrations.get(&sheet).copied();
                let measurements = project
                    .measurements
                    .get(&sheet)
                    .cloned()
                    .unwrap_or_default();

                (calibration.is_some() || !measurements.is_empty()).then_some(StoredSheet {
                    drawing: sheet.drawing,
                    page: sheet.page,
                    calibration,
                    measurements,
                })
            })
            .collect(),
    };

    let json = serde_json::to_string_pretty(&stored)
        .map_err(|error| format!("Could not write the project: {error}"))?;

    std::fs::write(path, json)
        .map_err(|error| format!("Could not write {}: {error}", file_name(path)))?;

    Ok(())
}

/// Reads a project file, or phase one's sidecar, whichever `path` names.
/// Drawings come back with their paths resolved against the file's own folder.
pub fn load(path: &Path) -> Result<Project, String> {
    let json = std::fs::read_to_string(path)
        .map_err(|error| format!("Could not read {}: {error}", file_name(path)))?;

    let folder = path.parent().unwrap_or(Path::new(".")).to_path_buf();

    // The version has to be read before the shape is known: a phase-one file
    // and this one agree on nothing else.
    let value: serde_json::Value = serde_json::from_str(&json)
        .map_err(|error| format!("{} is not readable: {error}", file_name(path)))?;

    let version = value.get("version").and_then(serde_json::Value::as_u64);

    match version {
        Some(1) => {
            let stored: StoredV1 = serde_json::from_value(value)
                .map_err(|error| format!("{} is not readable: {error}", file_name(path)))?;

            Ok(from_v1(stored, &folder))
        }
        Some(2) => {
            let stored: Stored = serde_json::from_value(value)
                .map_err(|error| format!("{} is not readable: {error}", file_name(path)))?;

            Ok(from_stored(stored, &folder))
        }
        _ => Err(format!(
            "{} was written by a later version of this application",
            file_name(path)
        )),
    }
}

fn from_stored(stored: Stored, folder: &Path) -> Project {
    let mut project = Project {
        drawings: stored
            .drawings
            .iter()
            .map(|drawing| Drawing {
                path: resolved(&drawing.path, folder),
            })
            .collect(),
        ..Project::default()
    };

    for sheet in stored.sheets {
        let at = Sheet::new(sheet.drawing, sheet.page);

        if let Some(calibration) = sheet.calibration {
            project.calibrations.insert(at, calibration);
        }
        if !sheet.measurements.is_empty() {
            project.measurements.insert(at, sheet.measurements);
        }
    }

    project
}

/// Phase one's work, as a project of one drawing: its pages become that
/// drawing's sheets, and nothing else changes.
fn from_v1(stored: StoredV1, folder: &Path) -> Project {
    Project {
        drawings: vec![Drawing {
            path: resolved(&stored.drawing, folder),
        }],
        calibrations: stored
            .calibrations
            .into_iter()
            .map(|(page, calibration)| (Sheet::new(0, page), calibration))
            .collect(),
        measurements: stored
            .measurements
            .into_iter()
            .map(|(page, measurements)| (Sheet::new(0, page), measurements))
            .collect(),
    }
}

/// A drawing's path as it should be written: relative to the project file when
/// it can be, so that a folder moved or copied whole still opens, and absolute
/// when the two are too far apart for that to mean anything.
fn written_relative(path: &Path, folder: &Path) -> String {
    let relative = path
        .strip_prefix(folder)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| path.to_path_buf());

    // Written with forward slashes whatever the platform, so a project file
    // opens on either.
    relative.to_string_lossy().replace('\\', "/")
}

/// The other direction: a written path against the folder it was written in.
fn resolved(written: &str, folder: &Path) -> PathBuf {
    let path = PathBuf::from(written.replace('/', std::path::MAIN_SEPARATOR_STR));

    if path.is_absolute() {
        path
    } else {
        folder.join(path)
    }
}

/// Writes every measurement to `path` as one row apiece, and reports how many
/// rows that came to.
///
/// Areas and lengths are given in the large unit of the sheet's own system —
/// square metres and metres, or square feet and feet — so that a column can be
/// summed. A sheet with no scale has no measurements to report.
pub fn export_csv(project: &Project, path: &Path) -> Result<usize, String> {
    let mut writer = csv::Writer::from_path(path)
        .map_err(|error| format!("Could not write {}: {error}", file_name(path)))?;

    let write = |writer: &mut csv::Writer<std::fs::File>, row: [&str; 8]| {
        writer
            .write_record(row)
            .map_err(|error| format!("Could not write {}: {error}", file_name(path)))
    };

    write(
        &mut writer,
        [
            "drawing",
            "page",
            "name",
            "area",
            "area_unit",
            "perimeter",
            "perimeter_unit",
            "holes",
        ],
    )?;

    let mut rows = 0;

    for sheet in project.worked() {
        let (Some(calibration), Some(measurements)) = (
            project.calibrations.get(&sheet),
            project.measurements.get(&sheet),
        ) else {
            continue;
        };

        let drawing = project
            .drawings
            .get(sheet.drawing)
            .map(Drawing::name)
            .unwrap_or_default();

        for measurement in measurements {
            let square_millimetres =
                calibration.square_millimetres(geom::measurement_area(measurement));
            let millimetres = calibration.millimetres(geom::perimeter(&measurement.outer));

            let (area, area_unit, perimeter, perimeter_unit) = if calibration.unit.is_metric() {
                (square_millimetres / 1e6, "m2", millimetres / 1e3, "m")
            } else {
                (
                    square_millimetres / (304.8 * 304.8),
                    "ft2",
                    millimetres / 304.8,
                    "ft",
                )
            };

            write(
                &mut writer,
                [
                    &drawing,
                    &(sheet.page + 1).to_string(),
                    &measurement.name,
                    &format!("{area:.6}"),
                    area_unit,
                    &format!("{perimeter:.6}"),
                    perimeter_unit,
                    &measurement.holes.len().to_string(),
                ],
            )?;

            rows += 1;
        }
    }

    writer
        .flush()
        .map_err(|error| format!("Could not write {}: {error}", file_name(path)))?;

    Ok(rows)
}

/// A path's own name, which is what a message about it should carry: the rest
/// of it is the user's business, not the message's.
pub fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One hundred points of page standing for five metres.
    #[test]
    fn units_per_point_divides_the_distance_by_the_span() {
        let calibration = Calibration::new(
            Point::new(20.0, 60.0),
            Point::new(120.0, 60.0),
            5.0,
            Unit::Metres,
        )
        .expect("a positive distance over a real span calibrates");

        assert!((calibration.units_per_point() - 0.05).abs() < 1e-12);
    }

    /// A 100 mm line on a 1:100 drawing occupies one millimetre of paper.
    #[test]
    fn ratio_reads_the_drawing_scale_in_millimetres() {
        let paper_millimetre = 72.0 / 25.4;
        let calibration = Calibration::new(
            Point::ZERO,
            Point::new(paper_millimetre, 0.0),
            100.0,
            Unit::Millimetres,
        )
        .expect("a positive distance over a real span calibrates");

        assert!((calibration.ratio() - 100.0).abs() < 1e-9);
    }

    /// The same drawing scale, established in inches, reads the same.
    #[test]
    fn ratio_reads_the_drawing_scale_in_inches() {
        let calibration = Calibration::new(Point::ZERO, Point::new(0.0, 72.0), 100.0, Unit::Inches)
            .expect("a positive distance over a real span calibrates");

        assert!((calibration.ratio() - 100.0).abs() < 1e-9);
    }

    #[test]
    fn a_scale_needs_two_distinct_points_and_a_positive_distance() {
        let point = Point::new(10.0, 10.0);

        assert!(Calibration::new(point, point, 5.0, Unit::Metres).is_none());
        assert!(Calibration::new(point, Point::new(50.0, 10.0), 0.0, Unit::Metres).is_none());
        assert!(Calibration::new(point, Point::new(50.0, 10.0), -3.0, Unit::Metres).is_none());
        assert!(Calibration::new(point, Point::new(50.0, 10.0), f64::NAN, Unit::Metres).is_none());
    }

    /// A square of `side` page points with its corner at the origin.
    fn square(side: f64) -> SubPath {
        SubPath {
            anchors: [(0.0, 0.0), (side, 0.0), (side, side), (0.0, side)]
                .into_iter()
                .map(|(x, y)| Anchor::corner(Point::new(x, y)))
                .collect(),
            closed: true,
        }
    }

    /// A sheet calibrated at 100 points to 5 metres, with one measurement on
    /// it: a 100 point square, less a 20 point square hole.
    fn worked_page() -> Project {
        let calibration = Calibration::new(Point::ZERO, Point::new(100.0, 0.0), 5.0, Unit::Metres)
            .expect("a positive distance over a real span calibrates");

        let mut hole = square(20.0);
        hole.anchors.reverse();

        Project {
            drawings: vec![Drawing {
                path: PathBuf::from("site-plan.pdf"),
            }],
            calibrations: HashMap::from([(Sheet::new(0, 0), calibration)]),
            measurements: HashMap::from([(
                Sheet::new(0, 0),
                vec![Measurement {
                    name: "Ground floor".to_owned(),
                    outer: square(100.0),
                    holes: vec![hole],
                    colour: Color32::from_rgb(10, 20, 30),
                    visible: false,
                }],
            )]),
        }
    }

    /// Somewhere to write a file that no other test is writing to.
    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(name)
    }

    #[test]
    fn a_saved_project_comes_back_as_it_went_in() {
        let path = scratch("measure-roundtrip.measure.json");
        save(&worked_page(), &path).expect("the project is written");

        let reopened = load(&path).expect("the project is readable");
        let _ = std::fs::remove_file(&path);

        let sheet = Sheet::new(0, 0);
        let calibration = reopened.calibrations[&sheet];
        assert_eq!(calibration.distance, 5.0);
        assert_eq!(calibration.unit, Unit::Metres);
        assert!((calibration.units_per_point() - 0.05).abs() < 1e-12);

        let measurement = &reopened.measurements[&sheet][0];
        assert_eq!(measurement.name, "Ground floor");
        assert_eq!(measurement.colour, Color32::from_rgb(10, 20, 30));
        assert!(!measurement.visible);
        assert_eq!(measurement.outer.anchors.len(), 4);
        assert!(measurement.outer.closed);
        assert_eq!(measurement.outer.anchors[2].pos, Point::new(100.0, 100.0));
        assert_eq!(measurement.holes.len(), 1);

        // The hole still winds against its outline, which is the whole of how
        // it subtracts.
        let outer = geom::signed_area(&measurement.outer);
        let hole = geom::signed_area(&measurement.holes[0]);
        assert!(outer * hole < 0.0);
    }

    /// A drawing is written relative to the project file beside it, so the
    /// pair can be moved or copied anywhere together.
    #[test]
    fn a_drawing_beside_the_project_is_named_relative_to_it() {
        let folder = std::env::temp_dir().join("measure-relative");
        std::fs::create_dir_all(&folder).expect("the scratch folder is writable");

        let mut project = worked_page();
        project.drawings[0].path = folder.join("site-plan.pdf");

        let path = folder.join("site.measure.json");
        save(&project, &path).expect("the project is written");

        let written = std::fs::read_to_string(&path).expect("the project is readable");
        assert!(
            written.contains("\"path\": \"site-plan.pdf\""),
            "the drawing is named relative to the project: {written}"
        );

        // And comes back pointing at the file itself, not at a bare name.
        let reopened = load(&path).expect("the project is readable");
        assert_eq!(reopened.drawings[0].path, folder.join("site-plan.pdf"));

        let _ = std::fs::remove_dir_all(&folder);
    }

    /// Phase one's sidecar names one drawing and keys its work by page. It
    /// comes back as a project of that one drawing.
    #[test]
    fn a_phase_one_sidecar_opens_as_a_one_drawing_project() {
        let folder = std::env::temp_dir().join("measure-phase-one");
        std::fs::create_dir_all(&folder).expect("the scratch folder is writable");

        let path = folder.join("site-plan.measurements.json");
        std::fs::write(
            &path,
            r#"{
                "version": 1,
                "drawing": "site-plan.pdf",
                "calibrations": {
                    "2": {
                        "from": {"x": 0.0, "y": 0.0},
                        "to": {"x": 100.0, "y": 0.0},
                        "distance": 5.0,
                        "unit": "Metres"
                    }
                },
                "measurements": {
                    "2": [
                        {
                            "name": "Ground floor",
                            "outer": {"anchors": [], "closed": true},
                            "holes": [],
                            "colour": [10, 20, 30, 255],
                            "visible": true
                        }
                    ]
                }
            }"#,
        )
        .expect("the scratch file is writable");

        let project = load(&path).expect("a phase one sidecar is readable");
        let _ = std::fs::remove_dir_all(&folder);

        assert_eq!(project.drawings.len(), 1);
        assert_eq!(project.drawings[0].path, folder.join("site-plan.pdf"));
        assert_eq!(project.drawings[0].name(), "site-plan");

        // Page three of that one drawing, and nothing else.
        let sheet = Sheet::new(0, 2);
        assert_eq!(project.worked(), vec![sheet]);
        assert_eq!(project.measurements[&sheet][0].name, "Ground floor");
        assert_eq!(project.calibrations[&sheet].distance, 5.0);
    }

    /// Work on a second drawing is filed under it and comes back under it,
    /// with the two kept apart by which drawing they belong to rather than by
    /// which page.
    #[test]
    fn a_second_drawing_keeps_its_own_work() {
        let mut project = worked_page();
        project.drawings.push(Drawing {
            path: PathBuf::from("services.pdf"),
        });

        // The same page number on the other drawing: nothing about a page
        // number identifies a sheet on its own.
        let other = Sheet::new(1, 0);
        project.measurements.insert(
            other,
            vec![Measurement {
                name: "Verge".to_owned(),
                outer: square(50.0),
                holes: Vec::new(),
                colour: Color32::WHITE,
                visible: true,
            }],
        );

        let path = scratch("measure-two-drawings.measure.json");
        save(&project, &path).expect("the project is written");
        let reopened = load(&path).expect("the project is readable");
        let _ = std::fs::remove_file(&path);

        assert_eq!(reopened.drawings.len(), 2);
        assert_eq!(reopened.drawings[1].name(), "services");
        assert_eq!(reopened.worked(), vec![Sheet::new(0, 0), other]);
        assert_eq!(
            reopened.measurements[&Sheet::new(0, 0)][0].name,
            "Ground floor"
        );
        assert_eq!(reopened.measurements[&other][0].name, "Verge");

        // The second drawing was never calibrated, so it carries no scale and
        // its areas are not exported as though it had one.
        assert!(!reopened.calibrations.contains_key(&other));
    }

    /// A file this reader cannot vouch for is refused, rather than read as far
    /// as it happens to make sense.
    #[test]
    fn a_project_from_a_later_version_is_refused() {
        let path = scratch("measure-from-the-future.measure.json");
        std::fs::write(&path, r#"{"version":9999,"drawings":[],"sheets":[]}"#)
            .expect("the scratch file is writable");

        let refused = load(&path);
        let _ = std::fs::remove_file(&path);

        assert!(refused.is_err());
    }

    /// One row per measurement, naming the drawing and page it was measured
    /// on, with the area and the perimeter of the 100 point square: 25 square
    /// metres less the hole, and 20 metres round.
    #[test]
    fn the_export_writes_a_row_for_each_measurement() {
        let path = scratch("measure-export.csv");
        let rows = export_csv(&worked_page(), &path).expect("the export is written");

        let csv = std::fs::read_to_string(&path).expect("the export is readable");
        let _ = std::fs::remove_file(&path);

        assert_eq!(rows, 1);

        let mut lines = csv.lines();
        assert_eq!(
            lines.next(),
            Some("drawing,page,name,area,area_unit,perimeter,perimeter_unit,holes")
        );
        assert_eq!(
            lines.next(),
            Some("site-plan,1,Ground floor,24.000000,m2,20.000000,m,1")
        );
        assert_eq!(lines.next(), None);
    }
}
