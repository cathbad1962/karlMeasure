//! What the user builds on top of a drawing: the scale of a page, and the
//! areas measured on it.

use eframe::egui::Color32;
use kurbo::{Point, Vec2};

/// Millimetres in one PDF point: 72 points to the inch, 25.4 mm to the inch.
const MM_PER_POINT: f64 = 25.4 / 72.0;

/// The unit a real-world distance was given in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
#[derive(Clone, Copy, Debug)]
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
}

// The data model below is fixed in full. Slice 4 only places corners into it
// and only reads the outline back, so the parts that curves, holes, naming and
// visibility will use are not read yet.

/// How an anchor's two handles relate to each other.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnchorKind {
    Corner,
    Smooth,
    Asymmetric,
}

/// A point on a path, with the handles that shape the curve either side of it.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
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

#[derive(Clone, Debug, Default)]
pub struct SubPath {
    pub anchors: Vec<Anchor>,
    pub closed: bool,
}

/// A named area on a page: an outline, and later the holes punched in it.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct Measurement {
    pub name: String,
    pub outer: SubPath,
    pub holes: Vec<SubPath>,
    pub colour: Color32,
    pub visible: bool,
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
}
