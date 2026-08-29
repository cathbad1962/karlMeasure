//! Turning stored anchors into a curve, and measuring what it encloses.

use kurbo::{BezPath, PathEl, Point, Shape};

use crate::doc::{Anchor, SubPath};

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
        segment(&mut path, &pair[0], &pair[1]);
    }

    if subpath.closed && subpath.anchors.len() > 1 {
        let last = subpath.anchors.last().expect("checked above");
        segment(&mut path, last, first);
        path.close_path();
    }

    path
}

fn segment(path: &mut BezPath, from: &Anchor, to: &Anchor) {
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
    use crate::doc::Anchor;

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

    #[test]
    fn nothing_is_enclosed_until_the_outline_is_closed() {
        let open = corners(&[(0.0, 0.0), (60.0, 0.0), (0.0, 40.0)], false);
        let two_points = corners(&[(0.0, 0.0), (60.0, 0.0)], true);

        assert_eq!(area(&open), 0.0);
        assert_eq!(area(&two_points), 0.0);
    }

    /// A corner's cubic is a straight line, so a square flattens to its own
    /// four vertices however fine the tolerance.
    #[test]
    fn corners_flatten_to_themselves() {
        let square = corners(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)], true);

        assert_eq!(outline(&square, 0.01).len(), 5);
    }
}
