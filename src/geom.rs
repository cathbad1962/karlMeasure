//! Turning stored anchors into a curve, measuring what it encloses, and
//! reshaping it.

use kurbo::{BezPath, CubicBez, ParamCurve, ParamCurveNearest, PathEl, Point, Shape, Vec2};

use crate::doc::{Anchor, AnchorKind, SubPath};

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

/// The area a closed subpath encloses, in square page points.
///
/// Computed analytically from the curve, never from the flattened outline, and
/// returned unsigned: which way round the outline was traced is not something
/// the person tracing it should have to think about.
pub fn area(subpath: &SubPath) -> f64 {
    if !subpath.closed || subpath.anchors.len() < 3 {
        return 0.0;
    }

    bez_path(subpath).area().abs()
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

        assert!((area(&square) - 10_000.0).abs() < 1e-9);
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

        assert!((area(&clockwise) - area(&anticlockwise)).abs() < 1e-9);
    }

    #[test]
    fn a_triangle_encloses_half_its_base_times_its_height() {
        let triangle = corners(&[(0.0, 0.0), (60.0, 0.0), (0.0, 40.0)], true);

        assert!((area(&triangle) - 1_200.0).abs() < 1e-9);
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
        assert!((area(&circle) - expected).abs() / expected < 0.001);
        assert!(area(&circle) > 2.0 * r * r * 1.5, "not the polygon's area");
    }

    #[test]
    fn nothing_is_enclosed_until_the_outline_is_closed() {
        let open = corners(&[(0.0, 0.0), (60.0, 0.0), (0.0, 40.0)], false);
        let two_points = corners(&[(0.0, 0.0), (60.0, 0.0)], true);

        assert_eq!(area(&open), 0.0);
        assert_eq!(area(&two_points), 0.0);
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

        let before = area(&circle);
        let at = insert_anchor(&mut circle, 0, 0.37).expect("segment 0 exists");

        assert_eq!(at, 1);
        assert_eq!(circle.anchors.len(), 4);
        assert!((area(&circle) - before).abs() / before < 1e-9);
    }

    /// A straight edge gains a corner and stays exactly as straight.
    #[test]
    fn inserting_on_a_straight_edge_keeps_it_straight() {
        let mut square = corners(
            &[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)],
            true,
        );

        let before = area(&square);
        let at = insert_anchor(&mut square, 0, 0.5).expect("segment 0 exists");

        assert_eq!(at, 1);
        assert_eq!(square.anchors[1].pos, Point::new(50.0, 0.0));
        assert_eq!(square.anchors[1].kind, AnchorKind::Corner);
        assert_eq!(square.anchors[0].out_handle, Vec2::ZERO);
        assert!((area(&square) - before).abs() < 1e-9);
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
}
