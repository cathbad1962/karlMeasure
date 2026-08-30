//! Turning stored anchors into a curve, measuring what it encloses, and
//! reshaping it.

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;
use kurbo::{BezPath, CubicBez, ParamCurve, ParamCurveNearest, PathEl, Point, Shape, Vec2};

use crate::doc::{Anchor, AnchorKind, Measurement, SubPath};

/// How precisely to locate the closest point on a curve, in page units. Well
/// below anything a hand can aim at.
const NEAREST_ACCURACY: f64 = 1e-3;

/// The curve a subpath describes, in page space.
///
/// Every segment is a cubic. A corner's handles sit on its own point, which
/// makes the cubic a straight line, so straight and curved edges need no
/// distinction here or anywhere downstream.
pub fn bez_path(subpath: &SubPath) -> BezPath {
    let mut path = BezPath::new();

    let Some(first) = subpath.anchors.first() else {
        return path;
    };
    path.move_to(first.pos);

    for pair in subpath.anchors.windows(2) {
        push_segment(&mut path, &pair[0], &pair[1]);
    }

    if subpath.closed && subpath.anchors.len() > 1 {
        let last = subpath.anchors.last().expect("checked above");
        push_segment(&mut path, last, first);
        path.close_path();
    }

    path
}

fn push_segment(path: &mut BezPath, from: &Anchor, to: &Anchor) {
    path.curve_to(from.pos + from.out_handle, to.pos + to.in_handle, to.pos);
}

/// The signed area a closed subpath encloses, in square page points.
///
/// Computed analytically from the curve, never from the flattened outline. The
/// sign follows the direction it was traced, which is the mechanism by which
/// holes subtract.
pub fn signed_area(subpath: &SubPath) -> f64 {
    if !subpath.closed || subpath.anchors.len() < 3 {
        return 0.0;
    }

    bez_path(subpath).area()
}

/// How closely an arc length is worked out, in page points. Well below the
/// precision any drawing carries.
const LENGTH_ACCURACY: f64 = 1e-4;

/// How far it is round a subpath, in page points: the whole way round when it
/// is closed, and from end to end when it is not.
pub fn perimeter(subpath: &SubPath) -> f64 {
    bez_path(subpath).perimeter(LENGTH_ACCURACY)
}

/// How finely outlines are flattened before they are clipped against one
/// another, in page points. Well below the precision any drawing carries.
const CLIP_FLATNESS: f64 = 0.01;

/// How close a clipped hole has to come to its own area to be treated as
/// lying wholly inside its outline, as a proportion.
const WHOLLY_INSIDE: f64 = 1e-6;

/// What a measurement covers: its outline, less what the holes take off it.
///
/// A hole only takes off the area it actually covers. One lying wholly inside
/// its outline takes off exactly its own area, computed analytically; one that
/// overhangs takes off the part they have in common, which needs the two
/// outlines clipped against each other.
pub fn measurement_area(measurement: &Measurement) -> f64 {
    let outer = signed_area(&measurement.outer).abs();

    let taken: f64 = measurement
        .holes
        .iter()
        .map(|hole| taken_by(&measurement.outer, hole))
        .sum();

    (outer - taken).max(0.0)
}

/// What a hole takes off its outline: the area the two have in common, in
/// square page points.
pub fn taken_by(outer: &SubPath, hole: &SubPath) -> f64 {
    let alone = signed_area(hole).abs();
    if alone == 0.0 {
        return 0.0;
    }

    let subject = vec![contour(outer)];
    let clip = vec![contour(hole)];
    let common = subject.overlay(&clip, OverlayRule::Intersect, FillRule::NonZero);

    let clipped: f64 = common
        .iter()
        .flatten()
        .map(|contour| shoelace(contour))
        .sum::<f64>()
        .abs();

    // Flattening cost the curve a little of its exactness. Where the hole is
    // wholly inside, which is the ordinary case, the exact figure is known
    // and is the one to use.
    if (clipped - alone).abs() <= alone * WHOLLY_INSIDE {
        alone
    } else {
        clipped
    }
}

/// A closed outline as the ring of points the clipper works in.
fn contour(subpath: &SubPath) -> Vec<[f64; 2]> {
    outline(subpath, CLIP_FLATNESS)
        .iter()
        .map(|point| [point.x, point.y])
        .collect()
}

/// The signed area of a ring of points.
fn shoelace(contour: &[[f64; 2]]) -> f64 {
    let mut twice = 0.0;

    for pair in contour.windows(2) {
        twice += pair[0][0] * pair[1][1] - pair[1][0] * pair[0][1];
    }

    if let (Some(last), Some(first)) = (contour.last(), contour.first()) {
        twice += last[0] * first[1] - first[0] * last[1];
    }

    twice / 2.0
}

/// The same subpath traced the other way round: the anchors in reverse, each
/// with its handles swapped, which is what makes the reversal geometric rather
/// than just an ordering.
pub fn reverse(subpath: &SubPath) -> SubPath {
    SubPath {
        anchors: subpath
            .anchors
            .iter()
            .rev()
            .map(|anchor| Anchor {
                pos: anchor.pos,
                in_handle: anchor.out_handle,
                out_handle: anchor.in_handle,
                kind: anchor.kind,
            })
            .collect(),
        closed: subpath.closed,
    }
}

/// `hole` wound against `outer`, so the two signed areas subtract instead of
/// adding. Whichever way round it was traced, it takes area away.
pub fn as_hole(outer: &SubPath, hole: SubPath) -> SubPath {
    if signed_area(outer) * signed_area(&hole) > 0.0 {
        reverse(&hole)
    } else {
        hole
    }
}

/// How many segments a subpath has: one per gap between anchors, plus the one
/// back to the start when it is closed.
pub fn segment_count(subpath: &SubPath) -> usize {
    match subpath.anchors.len() {
        0 | 1 => 0,
        count if subpath.closed => count,
        count => count - 1,
    }
}

/// The cubic leaving anchor `index`, wrapping back to the first anchor on the
/// last segment of a closed subpath.
fn segment(subpath: &SubPath, index: usize) -> Option<CubicBez> {
    let from = subpath.anchors.get(index)?;
    let to = match subpath.anchors.get(index + 1) {
        Some(anchor) => anchor,
        None if subpath.closed => subpath.anchors.first()?,
        None => return None,
    };

    Some(CubicBez::new(
        from.pos,
        from.pos + from.out_handle,
        to.pos + to.in_handle,
        to.pos,
    ))
}

/// Where a point falls on a subpath.
pub struct Nearest {
    pub segment: usize,
    /// The curve parameter, which is not a distance along the segment: even a
    /// straight cubic is not uniformly parameterised.
    pub t: f64,
    /// How far the point is from the curve, in page units.
    pub distance: f64,
}

/// The closest point on the subpath's curve to `point`.
pub fn nearest(subpath: &SubPath, point: Point) -> Option<Nearest> {
    (0..segment_count(subpath))
        .filter_map(|index| {
            let found = segment(subpath, index)?.nearest(point, NEAREST_ACCURACY);

            Some(Nearest {
                segment: index,
                t: found.t,
                distance: found.distance_sq.sqrt(),
            })
        })
        .min_by(|a, b| a.distance.total_cmp(&b.distance))
}

/// Inserts an anchor at `t` along segment `index`, leaving the curve exactly
/// where it was. Returns where the new anchor landed in the subpath.
///
/// A straight edge gains a plain corner and keeps its neighbours untouched:
/// subdividing it would leave handles lying along the edge, which look right
/// until the new anchor is moved and the edge bends. Anything curved is split
/// properly, which is what keeps the reported area from jumping.
pub fn insert_anchor(subpath: &mut SubPath, index: usize, t: f64) -> Option<usize> {
    let curve = segment(subpath, index)?;
    let next = (index + 1) % subpath.anchors.len();

    let straight = subpath.anchors[index].out_handle == Vec2::ZERO
        && subpath.anchors[next].in_handle == Vec2::ZERO;

    let inserted = if straight {
        Anchor::corner(curve.eval(t))
    } else {
        let before = curve.subsegment(0.0..t);
        let after = curve.subsegment(t..1.0);

        subpath.anchors[index].out_handle = before.p1 - before.p0;
        subpath.anchors[next].in_handle = after.p2 - after.p3;

        Anchor {
            pos: before.p3,
            in_handle: before.p2 - before.p3,
            out_handle: after.p1 - after.p0,
            kind: AnchorKind::Smooth,
        }
    };

    subpath.anchors.insert(index + 1, inserted);

    Some(index + 1)
}

/// Handles for an anchor being smoothed: along the line joining its
/// neighbours, reaching a third of the way towards each.
pub fn smooth_handles(previous: Point, pos: Point, next: Point) -> (Vec2, Vec2) {
    let along = next - previous;
    let length = along.hypot();

    if length <= f64::EPSILON {
        return (Vec2::ZERO, Vec2::ZERO);
    }

    let direction = along / length;

    (
        direction * -((pos - previous).hypot() / 3.0),
        direction * ((next - pos).hypot() / 3.0),
    )
}

/// The direction a constrained drag is held to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

impl Axis {
    /// Whichever axis the movement so far is more along.
    pub fn of(from: Point, point: Point) -> Self {
        let delta = point - from;

        if delta.x.abs() >= delta.y.abs() {
            Self::Horizontal
        } else {
            Self::Vertical
        }
    }

    /// `point` pulled onto this axis through `from`.
    pub fn hold(self, from: Point, point: Point) -> Point {
        match self {
            Self::Horizontal => Point::new(point.x, from.y),
            Self::Vertical => Point::new(from.x, point.y),
        }
    }
}

/// Where a caption for this subpath sits: the centre of its bounding box.
pub fn centre(subpath: &SubPath) -> Point {
    bez_path(subpath).bounding_box().center()
}

/// The subpath as a polyline in page space, for painting and hit-testing.
/// `tolerance` is in page units, so callers divide by the zoom to keep the
/// error constant on screen.
pub fn outline(subpath: &SubPath, tolerance: f64) -> Vec<Point> {
    let mut points = Vec::new();

    kurbo::flatten(bez_path(subpath), tolerance, |element| match element {
        PathEl::MoveTo(point) | PathEl::LineTo(point) => points.push(point),
        _ => {}
    });

    points
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{Anchor, AnchorKind};
    use kurbo::Vec2;

    fn corners(points: &[(f64, f64)], closed: bool) -> SubPath {
        SubPath {
            anchors: points
                .iter()
                .map(|&(x, y)| Anchor::corner(Point::new(x, y)))
                .collect(),
            closed,
        }
    }

    #[test]
    fn a_square_encloses_its_side_squared() {
        let square = corners(
            &[(10.0, 10.0), (110.0, 10.0), (110.0, 110.0), (10.0, 110.0)],
            true,
        );

        assert!((signed_area(&square).abs() - 10_000.0).abs() < 1e-9);
    }

    /// Tracing the other way round negates the signed area, not the answer.
    #[test]
    fn winding_direction_does_not_change_the_area() {
        let clockwise = corners(
            &[(10.0, 10.0), (110.0, 10.0), (110.0, 110.0), (10.0, 110.0)],
            true,
        );
        let anticlockwise = corners(
            &[(10.0, 110.0), (110.0, 110.0), (110.0, 10.0), (10.0, 10.0)],
            true,
        );

        assert!((signed_area(&clockwise).abs() - signed_area(&anticlockwise).abs()).abs() < 1e-9);
    }

    #[test]
    fn a_triangle_encloses_half_its_base_times_its_height() {
        let triangle = corners(&[(0.0, 0.0), (60.0, 0.0), (0.0, 40.0)], true);

        assert!((signed_area(&triangle).abs() - 1_200.0).abs() < 1e-9);
    }

    /// Four smooth anchors with the standard handle length trace a circle.
    /// Its area is pi r squared; the square through the same four anchors
    /// would be a third short, which is what computing area from anchors
    /// rather than from the curve would give.
    #[test]
    fn a_traced_circle_encloses_pi_r_squared() {
        const KAPPA: f64 = 0.552_284_749_831;
        let r: f64 = 50.0;
        let pull = KAPPA * r;

        let smooth = |pos: Point, in_handle: Vec2, out_handle: Vec2| Anchor {
            pos,
            in_handle,
            out_handle,
            kind: AnchorKind::Smooth,
        };

        let circle = SubPath {
            anchors: vec![
                smooth(
                    Point::new(r, 0.0),
                    Vec2::new(0.0, -pull),
                    Vec2::new(0.0, pull),
                ),
                smooth(
                    Point::new(0.0, r),
                    Vec2::new(pull, 0.0),
                    Vec2::new(-pull, 0.0),
                ),
                smooth(
                    Point::new(-r, 0.0),
                    Vec2::new(0.0, pull),
                    Vec2::new(0.0, -pull),
                ),
                smooth(
                    Point::new(0.0, -r),
                    Vec2::new(-pull, 0.0),
                    Vec2::new(pull, 0.0),
                ),
            ],
            closed: true,
        };

        let expected = std::f64::consts::PI * r * r;
        assert!((signed_area(&circle).abs() - expected).abs() / expected < 0.001);
        assert!(
            signed_area(&circle).abs() > 2.0 * r * r * 1.5,
            "not the polygon's area"
        );
    }

    #[test]
    fn nothing_is_enclosed_until_the_outline_is_closed() {
        let open = corners(&[(0.0, 0.0), (60.0, 0.0), (0.0, 40.0)], false);
        let two_points = corners(&[(0.0, 0.0), (60.0, 0.0)], true);

        assert_eq!(signed_area(&open).abs(), 0.0);
        assert_eq!(signed_area(&two_points).abs(), 0.0);
    }

    fn measurement(outer: SubPath, holes: Vec<SubPath>) -> Measurement {
        Measurement {
            name: "Area 1".to_owned(),
            outer,
            holes,
            colour: eframe::egui::Color32::WHITE,
            visible: true,
        }
    }

    /// A hundred-unit square with a twenty-unit square taken out of it.
    #[test]
    fn a_hole_comes_off_the_area() {
        let outer = corners(
            &[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)],
            true,
        );
        let hole = corners(
            &[(10.0, 10.0), (30.0, 10.0), (30.0, 30.0), (10.0, 30.0)],
            true,
        );

        let wound = as_hole(&outer, hole);
        let measured = measurement(outer, vec![wound]);

        assert!((measurement_area(&measured) - 9_600.0).abs() < 1e-9);
    }

    /// Which way round the hole was traced is not the tracer's problem.
    #[test]
    fn a_hole_subtracts_whichever_way_round_it_was_traced() {
        let outer = corners(
            &[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)],
            true,
        );
        let clockwise = corners(
            &[(10.0, 10.0), (30.0, 10.0), (30.0, 30.0), (10.0, 30.0)],
            true,
        );
        let anticlockwise = reverse(&clockwise);

        let one = measurement(outer.clone(), vec![as_hole(&outer, clockwise)]);
        let other = measurement(outer.clone(), vec![as_hole(&outer, anticlockwise)]);

        assert!((measurement_area(&one) - measurement_area(&other)).abs() < 1e-9);
        assert!((measurement_area(&one) - 9_600.0).abs() < 1e-9);
    }

    /// A hole hanging over the edge only takes off the part that is inside.
    #[test]
    fn a_hole_takes_off_only_what_it_covers() {
        let outer = corners(
            &[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)],
            true,
        );
        // Half in, half out: 20 wide, straddling the right-hand edge.
        let straddling = corners(
            &[(90.0, 40.0), (110.0, 40.0), (110.0, 60.0), (90.0, 60.0)],
            true,
        );

        let measured = measurement(outer.clone(), vec![as_hole(&outer, straddling)]);

        // 400 square units of hole, of which 200 lie inside.
        assert!((measurement_area(&measured) - 9_800.0).abs() < 1e-6);
    }

    /// A hole swallowing the whole outline leaves nothing, not less than
    /// nothing.
    #[test]
    fn a_hole_cannot_take_off_more_than_there_is() {
        let outer = corners(
            &[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)],
            true,
        );
        let swallowing = corners(
            &[
                (-50.0, -50.0),
                (150.0, -50.0),
                (150.0, 150.0),
                (-50.0, 150.0),
            ],
            true,
        );

        let measured = measurement(outer.clone(), vec![as_hole(&outer, swallowing)]);

        assert!(measurement_area(&measured).abs() < 1e-6);
    }

    /// A hole that misses its outline altogether takes off nothing.
    #[test]
    fn a_hole_beside_the_outline_takes_off_nothing() {
        let outer = corners(
            &[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)],
            true,
        );
        let elsewhere = corners(
            &[
                (200.0, 200.0),
                (220.0, 200.0),
                (220.0, 220.0),
                (200.0, 220.0),
            ],
            true,
        );

        let measured = measurement(outer.clone(), vec![as_hole(&outer, elsewhere)]);

        assert!((measurement_area(&measured) - 10_000.0).abs() < 1e-6);
    }

    #[test]
    fn every_hole_subtracts() {
        let outer = corners(
            &[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)],
            true,
        );
        let first = corners(
            &[(10.0, 10.0), (30.0, 10.0), (30.0, 30.0), (10.0, 30.0)],
            true,
        );
        let second = corners(
            &[(50.0, 50.0), (60.0, 50.0), (60.0, 60.0), (50.0, 60.0)],
            true,
        );

        let measured = measurement(
            outer.clone(),
            vec![as_hole(&outer, first), as_hole(&outer, second)],
        );

        assert!((measurement_area(&measured) - 9_500.0).abs() < 1e-9);
    }

    /// Reversing a subpath negates what it encloses without moving it.
    #[test]
    fn reversing_negates_the_signed_area_and_nothing_else() {
        const KAPPA: f64 = 0.552_284_749_831;
        let pull = KAPPA * 20.0;

        let curved = SubPath {
            anchors: vec![
                Anchor {
                    pos: Point::new(20.0, 0.0),
                    in_handle: Vec2::new(0.0, -pull),
                    out_handle: Vec2::new(0.0, pull),
                    kind: AnchorKind::Smooth,
                },
                Anchor {
                    pos: Point::new(0.0, 20.0),
                    in_handle: Vec2::new(pull, 0.0),
                    out_handle: Vec2::new(-pull, 0.0),
                    kind: AnchorKind::Smooth,
                },
                Anchor {
                    pos: Point::new(-20.0, 0.0),
                    in_handle: Vec2::new(0.0, pull),
                    out_handle: Vec2::new(0.0, -pull),
                    kind: AnchorKind::Smooth,
                },
            ],
            closed: true,
        };

        let backwards = reverse(&curved);

        assert!((signed_area(&curved) + signed_area(&backwards)).abs() < 1e-9);
        assert!((signed_area(&curved).abs() - signed_area(&backwards).abs()).abs() < 1e-9);
    }

    /// Splitting a curve is only worth anything if it does not move it, which
    /// the area is the sharpest witness to.
    #[test]
    fn inserting_on_a_curve_leaves_the_area_alone() {
        const KAPPA: f64 = 0.552_284_749_831;
        let pull = KAPPA * 50.0;

        let mut circle = SubPath {
            anchors: vec![
                Anchor {
                    pos: Point::new(50.0, 0.0),
                    in_handle: Vec2::new(0.0, -pull),
                    out_handle: Vec2::new(0.0, pull),
                    kind: AnchorKind::Smooth,
                },
                Anchor {
                    pos: Point::new(0.0, 50.0),
                    in_handle: Vec2::new(pull, 0.0),
                    out_handle: Vec2::new(-pull, 0.0),
                    kind: AnchorKind::Smooth,
                },
                Anchor {
                    pos: Point::new(-50.0, 0.0),
                    in_handle: Vec2::new(0.0, pull),
                    out_handle: Vec2::new(0.0, -pull),
                    kind: AnchorKind::Smooth,
                },
            ],
            closed: true,
        };

        let before = signed_area(&circle).abs();
        let at = insert_anchor(&mut circle, 0, 0.37).expect("segment 0 exists");

        assert_eq!(at, 1);
        assert_eq!(circle.anchors.len(), 4);
        assert!((signed_area(&circle).abs() - before).abs() / before < 1e-9);
    }

    /// A straight edge gains a corner and stays exactly as straight.
    #[test]
    fn inserting_on_a_straight_edge_keeps_it_straight() {
        let mut square = corners(
            &[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)],
            true,
        );

        let before = signed_area(&square).abs();
        let at = insert_anchor(&mut square, 0, 0.5).expect("segment 0 exists");

        assert_eq!(at, 1);
        assert_eq!(square.anchors[1].pos, Point::new(50.0, 0.0));
        assert_eq!(square.anchors[1].kind, AnchorKind::Corner);
        assert_eq!(square.anchors[0].out_handle, Vec2::ZERO);
        assert!((signed_area(&square).abs() - before).abs() < 1e-9);
    }

    /// The segment back to the first anchor is the last one, and inserting on
    /// it appends rather than disturbing the anchors already indexed.
    #[test]
    fn the_closing_segment_can_be_split_too() {
        let mut square = corners(
            &[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)],
            true,
        );

        assert_eq!(segment_count(&square), 4);
        let at = insert_anchor(&mut square, 3, 0.5).expect("the closing segment exists");

        assert_eq!(at, 4);
        assert_eq!(square.anchors[4].pos, Point::new(0.0, 50.0));
    }

    #[test]
    fn nearest_finds_the_segment_the_point_lies_against() {
        let square = corners(
            &[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)],
            true,
        );

        let found =
            nearest(&square, Point::new(101.0, 30.0)).expect("a closed square has segments");

        assert_eq!(found.segment, 1);
        assert!((found.distance - 1.0).abs() < 1e-6);

        // `t` is a curve parameter, not a distance along the edge: even a
        // straight cubic is not uniformly parameterised, so what there is to
        // check is the point it names.
        let on_curve = segment(&square, found.segment)
            .expect("segment 1 exists")
            .eval(found.t);
        assert!((on_curve - Point::new(100.0, 30.0)).hypot() < 1e-3);
    }

    /// The constraint holds whichever axis the hand was already closer to.
    #[test]
    fn orthogonal_holds_the_nearer_axis() {
        let from = Point::new(100.0, 100.0);

        assert_eq!(
            Axis::of(from, Point::new(180.0, 112.0)).hold(from, Point::new(180.0, 112.0)),
            Point::new(180.0, 100.0)
        );
        assert_eq!(
            Axis::of(from, Point::new(112.0, 180.0)).hold(from, Point::new(112.0, 180.0)),
            Point::new(100.0, 180.0)
        );
    }

    /// Smoothing points the handles along the line joining the neighbours, so
    /// the curve passes through without a kink.
    #[test]
    fn smooth_handles_lie_along_the_neighbours() {
        let (in_handle, out_handle) = smooth_handles(
            Point::new(0.0, 0.0),
            Point::new(30.0, 0.0),
            Point::new(90.0, 0.0),
        );

        assert_eq!(in_handle, Vec2::new(-10.0, 0.0));
        assert_eq!(out_handle, Vec2::new(20.0, 0.0));
    }

    /// A corner's cubic is a straight line, so a square flattens to its own
    /// four vertices however fine the tolerance.
    #[test]
    fn corners_flatten_to_themselves() {
        let square = corners(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)], true);

        assert_eq!(outline(&square, 0.01).len(), 5);
    }

    /// A closed square measures its four sides; the same corners left open
    /// measure only the three that are drawn.
    #[test]
    fn perimeter_goes_the_whole_way_round_a_closed_subpath() {
        let points = [(10.0, 10.0), (110.0, 10.0), (110.0, 110.0), (10.0, 110.0)];

        assert!((perimeter(&corners(&points, true)) - 400.0).abs() < 1e-6);
        assert!((perimeter(&corners(&points, false)) - 300.0).abs() < 1e-6);
    }

    /// A circle drawn as four cubics with the usual handle length comes within
    /// a whisker of its true circumference.
    #[test]
    fn perimeter_follows_the_curve_rather_than_the_chords() {
        let radius = 50.0;
        let handle = radius * 4.0 / 3.0 * (std::f64::consts::FRAC_PI_8).tan();
        let circle = SubPath {
            anchors: vec![
                Anchor {
                    pos: Point::new(radius, 0.0),
                    in_handle: Vec2::new(0.0, -handle),
                    out_handle: Vec2::new(0.0, handle),
                    kind: AnchorKind::Smooth,
                },
                Anchor {
                    pos: Point::new(0.0, radius),
                    in_handle: Vec2::new(handle, 0.0),
                    out_handle: Vec2::new(-handle, 0.0),
                    kind: AnchorKind::Smooth,
                },
                Anchor {
                    pos: Point::new(-radius, 0.0),
                    in_handle: Vec2::new(0.0, handle),
                    out_handle: Vec2::new(0.0, -handle),
                    kind: AnchorKind::Smooth,
                },
                Anchor {
                    pos: Point::new(0.0, -radius),
                    in_handle: Vec2::new(-handle, 0.0),
                    out_handle: Vec2::new(handle, 0.0),
                    kind: AnchorKind::Smooth,
                },
            ],
            closed: true,
        };

        let circumference = std::f64::consts::TAU * radius;

        let measured = perimeter(&circle);

        // The four chords would come out a tenth short; the cubics themselves
        // are a shade long, which is the approximation's own error, not the
        // measuring of it.
        assert!(
            (measured - circumference).abs() < circumference * 1e-3,
            "measured {measured} against {circumference}"
        );
    }
}
