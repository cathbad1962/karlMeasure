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

/// Everything the user has built on the drawing, keyed by page.
///
/// This is the unit of undo: it is cloned whole before each committed
/// operation. The document is small, and a stack of clones is far easier to
/// keep correct than a set of reversible commands.
#[derive(Clone, Default)]
pub struct Project {
    pub calibrations: HashMap<usize, Calibration>,
    pub measurements: HashMap<usize, Vec<Measurement>>,
}

/// The sidecar format. Raised when a change would stop this reader making
/// sense of a file, so a file from the future is refused rather than
/// half-understood.
const VERSION: u32 = 1;

/// The project as it is written beside the drawing.
///
/// Pages are held in order here, unlike in the project itself, so that saving
/// the same work twice gives the same file.
#[derive(Serialize, Deserialize)]
struct Sidecar {
    version: u32,
    /// The drawing this was measured on. Provenance only: the sidecar is
    /// found by its own name, never by what is written here.
    drawing: String,
    calibrations: BTreeMap<usize, Calibration>,
    measurements: BTreeMap<usize, Vec<Measurement>>,
}

/// Where the sidecar for `drawing` lives: beside it, under the same name.
pub fn sidecar_path(drawing: &Path) -> PathBuf {
    drawing.with_extension("measurements.json")
}

/// Writes the project beside its drawing, and reports where it went.
pub fn save(project: &Project, drawing: &Path) -> Result<PathBuf, String> {
    let sidecar = Sidecar {
        version: VERSION,
        drawing: file_name(drawing),
        calibrations: project.calibrations.iter().map(|(&k, &v)| (k, v)).collect(),
        measurements: project
            .measurements
            .iter()
            .filter(|(_, page)| !page.is_empty())
            .map(|(&k, v)| (k, v.clone()))
            .collect(),
    };

    let path = sidecar_path(drawing);
    let json = serde_json::to_string_pretty(&sidecar)
        .map_err(|error| format!("Could not write the measurements: {error}"))?;

    std::fs::write(&path, json)
        .map_err(|error| format!("Could not write {}: {error}", file_name(&path)))?;

    Ok(path)
}

/// Reads the sidecar beside `drawing`, or `None` when there is not one.
pub fn load(drawing: &Path) -> Result<Option<Project>, String> {
    let path = sidecar_path(drawing);
    let json = match std::fs::read_to_string(&path) {
        Ok(json) => json,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Could not read {}: {error}", file_name(&path))),
    };

    let sidecar: Sidecar = serde_json::from_str(&json)
        .map_err(|error| format!("{} is not readable: {error}", file_name(&path)))?;

    if sidecar.version > VERSION {
        return Err(format!(
            "{} was written by a later version of this application",
            file_name(&path)
        ));
    }

    Ok(Some(Project {
        calibrations: sidecar.calibrations.into_iter().collect(),
        measurements: sidecar.measurements.into_iter().collect(),
    }))
}

/// Writes every measurement to `path` as one row apiece, and reports how many
/// rows that came to.
///
/// Areas and lengths are given in the large unit of the page's own system —
/// square metres and metres, or square feet and feet — so that a column can be
/// summed. A page with no scale has no measurements to report.
pub fn export_csv(project: &Project, path: &Path) -> Result<usize, String> {
    let mut writer = csv::Writer::from_path(path)
        .map_err(|error| format!("Could not write {}: {error}", file_name(path)))?;

    let write = |writer: &mut csv::Writer<std::fs::File>, row: [&str; 7]| {
        writer
            .write_record(row)
            .map_err(|error| format!("Could not write {}: {error}", file_name(path)))
    };

    write(
        &mut writer,
        [
            "page",
            "name",
            "area",
            "area_unit",
            "perimeter",
            "perimeter_unit",
            "holes",
        ],
    )?;

    let mut pages: Vec<usize> = project.measurements.keys().copied().collect();
    pages.sort_unstable();

    let mut rows = 0;

    for page in pages {
        let (Some(calibration), Some(measurements)) = (
            project.calibrations.get(&page),
            project.measurements.get(&page),
        ) else {
            continue;
        };

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
                    &(page + 1).to_string(),
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

    /// A page calibrated at 100 points to 5 metres, with one measurement on
    /// it: a 100 point square, less a 20 point square hole.
    fn worked_page() -> Project {
        let calibration = Calibration::new(Point::ZERO, Point::new(100.0, 0.0), 5.0, Unit::Metres)
            .expect("a positive distance over a real span calibrates");

        let mut hole = square(20.0);
        hole.anchors.reverse();

        Project {
            calibrations: HashMap::from([(0, calibration)]),
            measurements: HashMap::from([(
                0,
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
        let drawing = scratch("measure-roundtrip.pdf");
        let written = save(&worked_page(), &drawing).expect("the sidecar is written");

        let reopened = load(&drawing)
            .expect("the sidecar is readable")
            .expect("the sidecar is there");
        let _ = std::fs::remove_file(&written);

        let calibration = reopened.calibrations[&0];
        assert_eq!(calibration.distance, 5.0);
        assert_eq!(calibration.unit, Unit::Metres);
        assert!((calibration.units_per_point() - 0.05).abs() < 1e-12);

        let measurement = &reopened.measurements[&0][0];
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

    /// A drawing that has never been measured has no sidecar, which is an
    /// ordinary state of affairs rather than a failure.
    #[test]
    fn a_drawing_with_no_sidecar_loads_nothing() {
        let drawing = scratch("measure-never-measured.pdf");
        let _ = std::fs::remove_file(sidecar_path(&drawing));

        assert!(
            load(&drawing)
                .expect("a missing sidecar is not an error")
                .is_none()
        );
    }

    /// A file this reader cannot vouch for is refused, rather than read as far
    /// as it happens to make sense.
    #[test]
    fn a_sidecar_from_a_later_version_is_refused() {
        let drawing = scratch("measure-from-the-future.pdf");
        let path = sidecar_path(&drawing);
        std::fs::write(
            &path,
            r#"{"version":9999,"drawing":"x.pdf","calibrations":{},"measurements":{}}"#,
        )
        .expect("the scratch file is writable");

        let refused = load(&drawing);
        let _ = std::fs::remove_file(&path);

        assert!(refused.is_err());
    }

    /// One row per measurement, with the area and the perimeter of the 100
    /// point square: 25 square metres, less the hole, and 20 metres round.
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
            Some("page,name,area,area_unit,perimeter,perimeter_unit,holes")
        );
        assert_eq!(
            lines.next(),
            Some("1,Ground floor,24.000000,m2,20.000000,m,1")
        );
        assert_eq!(lines.next(), None);
    }
}
