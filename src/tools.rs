//! The tools: the pen that places anchors, and the editor that reshapes them
//! once they are placed.

use kurbo::{Point, Vec2};

use crate::doc::{Anchor, AnchorKind, Measurement, Outline, SubPath};
use crate::geom;

/// The fewest anchors that can enclose an area.
const ENOUGH: usize = 3;

/// An outline being drawn, one anchor per click, in page space.
#[derive(Default)]
pub struct Pen {
    anchors: Vec<Anchor>,
}

impl Pen {
    /// Puts down an anchor where the button went down. It is a corner until a
    /// drag pulls handles out of it.
    pub fn place(&mut self, point: Point) {
        self.anchors.push(Anchor::corner(point));
    }

    /// Pulls handles out of the anchor being placed, from the drag away from
    /// it. While `mirror` holds, the incoming handle is kept equal and
    /// opposite to the outgoing one; once it does not, the incoming handle is
    /// left exactly where the mirror was broken and only the outgoing one
    /// follows.
    pub fn shape(&mut self, out_handle: Vec2, mirror: bool) {
        let Some(anchor) = self.anchors.last_mut() else {
            return;
        };

        anchor.out_handle = out_handle;

        if mirror {
            anchor.in_handle = -out_handle;
            // Dragging back onto the anchor leaves it as sharp as it started.
            anchor.kind = if out_handle == Vec2::ZERO {
                AnchorKind::Corner
            } else {
                AnchorKind::Smooth
            };
        } else {
            anchor.kind = AnchorKind::Asymmetric;
        }
    }

    pub fn anchors(&self) -> &[Anchor] {
        &self.anchors
    }

    /// Closes the outline back to its first anchor, leaving the pen empty and
    /// ready for the next one. `None` while too few anchors have been placed
    /// to enclose anything, which leaves the outline untouched and still
    /// being drawn.
    pub fn close(&mut self) -> Option<SubPath> {
        if self.anchors.len() < ENOUGH {
            return None;
        }

        Some(SubPath {
            anchors: std::mem::take(&mut self.anchors),
            closed: true,
        })
    }

    /// Abandons whatever has been placed so far.
    pub fn clear(&mut self) {
        self.anchors.clear();
    }
}

/// The nearest of `targets` within `radius` of `point`, which is where a
/// placement lands instead of where the hand actually was.
pub fn snap(point: Point, radius: f64, targets: impl Iterator<Item = Point>) -> Option<Point> {
    targets
        .map(|target| (target, (target - point).hypot()))
        .filter(|(_, distance)| *distance <= radius)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(target, _)| target)
}

/// Which anchor, of which outline, of which measurement on the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub measurement: usize,
    pub outline: Outline,
    pub anchor: usize,
}

/// What a drag has hold of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grab {
    Anchor,
    In,
    Out,
}

/// Selecting and reshaping anchors that are already placed.
#[derive(Default)]
pub struct Editor {
    pub selected: Option<Selection>,
    pub grabbed: Option<(Selection, Grab)>,
    /// Where the grabbed anchor sat when the drag began, which is what an
    /// orthogonal constraint holds it against.
    pub origin: Option<Point>,
}

impl Editor {
    /// What lies under `point`, within `radius` page units.
    ///
    /// The selected anchor's handles are tested first, since they are the only
    /// ones drawn and they sit on top of everything else near them.
    pub fn hit(
        &self,
        measurements: &[Measurement],
        within: Option<usize>,
        point: Point,
        radius: f64,
    ) -> Option<(Selection, Grab)> {
        if let Some(selected) = self.selected
            && let Some(anchor) = anchor(measurements, selected)
        {
            for (grab, handle) in [(Grab::In, anchor.in_handle), (Grab::Out, anchor.out_handle)] {
                if handle != Vec2::ZERO && (anchor.pos + handle - point).hypot() <= radius {
                    return Some((selected, grab));
                }
            }
        }

        for (index, measurement) in reachable(measurements, within) {
            for (outline, subpath) in measurement.outlines() {
                for (at, anchor) in subpath.anchors.iter().enumerate() {
                    if (anchor.pos - point).hypot() <= radius {
                        return Some((
                            Selection {
                                measurement: index,
                                outline,
                                anchor: at,
                            },
                            Grab::Anchor,
                        ));
                    }
                }
            }
        }

        None
    }
}

/// The measurements a click can reach, keeping the index each one is filed
/// under: `within` is the one in hand, which is the only one an anchor tool
/// reaches while there is one. A hidden outline is not there to be grabbed.
fn reachable(
    measurements: &[Measurement],
    within: Option<usize>,
) -> impl Iterator<Item = (usize, &Measurement)> {
    measurements
        .iter()
        .enumerate()
        .filter(move |(index, measurement)| {
            measurement.visible && within.is_none_or(|only| only == *index)
        })
}

fn anchor(measurements: &[Measurement], selection: Selection) -> Option<&Anchor> {
    measurements
        .get(selection.measurement)?
        .outline(selection.outline)?
        .anchors
        .get(selection.anchor)
}

fn anchor_mut(measurements: &mut [Measurement], selection: Selection) -> Option<&mut Anchor> {
    measurements
        .get_mut(selection.measurement)?
        .outline_mut(selection.outline)?
        .anchors
        .get_mut(selection.anchor)
}

/// Moves whatever is grabbed to `point`. A smooth anchor's opposite handle
/// mirrors the one being dragged until `mirror` stops holding, at which point
/// the pair comes apart for good.
pub fn move_to(
    measurements: &mut [Measurement],
    selection: Selection,
    grab: Grab,
    point: Point,
    mirror: bool,
) {
    let Some(anchor) = anchor_mut(measurements, selection) else {
        return;
    };

    match grab {
        Grab::Anchor => anchor.pos = point,
        Grab::In => {
            anchor.in_handle = point - anchor.pos;

            if !mirror {
                anchor.kind = AnchorKind::Asymmetric;
            } else if anchor.kind == AnchorKind::Smooth {
                anchor.out_handle = -anchor.in_handle;
            }
        }
        Grab::Out => {
            anchor.out_handle = point - anchor.pos;

            if !mirror {
                anchor.kind = AnchorKind::Asymmetric;
            } else if anchor.kind == AnchorKind::Smooth {
                anchor.in_handle = -anchor.out_handle;
            }
        }
    }
}

/// Inserts an anchor where `point` falls on an outline, if it falls within
/// `radius` of one, and says which anchor to select next.
pub fn insert(
    measurements: &mut [Measurement],
    within: Option<usize>,
    point: Point,
    radius: f64,
) -> Option<Selection> {
    let (index, outline, found) = reachable(measurements, within)
        .flat_map(|(index, measurement)| {
            measurement
                .outlines()
                .filter_map(move |(outline, subpath)| {
                    Some((index, outline, geom::nearest(subpath, point)?))
                })
        })
        .min_by(|a, b| a.2.distance.total_cmp(&b.2.distance))?;

    if found.distance > radius {
        return None;
    }

    let subpath = measurements[index].outline_mut(outline)?;
    let at = geom::insert_anchor(subpath, found.segment, found.t)?;

    Some(Selection {
        measurement: index,
        outline,
        anchor: at,
    })
}

/// Removes the selected anchor, unless doing so would leave too few to enclose
/// anything.
pub fn delete(measurements: &mut [Measurement], selection: Selection) -> Option<()> {
    let outline = measurements
        .get_mut(selection.measurement)?
        .outline_mut(selection.outline)?;

    if outline.anchors.len() <= ENOUGH || selection.anchor >= outline.anchors.len() {
        return None;
    }

    outline.anchors.remove(selection.anchor);

    Some(())
}

/// Flips the selected anchor between a corner and a smooth point. Smoothing
/// takes its handles from the neighbours either side; sharpening collapses
/// them, which leaves both edges straight.
pub fn toggle(measurements: &mut [Measurement], selection: Selection) -> Option<()> {
    let outline = measurements
        .get_mut(selection.measurement)?
        .outline_mut(selection.outline)?;
    let count = outline.anchors.len();

    if count < ENOUGH || selection.anchor >= count {
        return None;
    }

    let current = outline.anchors[selection.anchor];

    let (in_handle, out_handle, kind) = if current.kind == AnchorKind::Corner {
        let previous = outline.anchors[(selection.anchor + count - 1) % count].pos;
        let next = outline.anchors[(selection.anchor + 1) % count].pos;
        let (in_handle, out_handle) = geom::smooth_handles(previous, current.pos, next);

        (in_handle, out_handle, AnchorKind::Smooth)
    } else {
        (Vec2::ZERO, Vec2::ZERO, AnchorKind::Corner)
    };

    let anchor = &mut outline.anchors[selection.anchor];
    anchor.in_handle = in_handle;
    anchor.out_handle = out_handle;
    anchor.kind = kind;

    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_anchors_close_into_an_outline() {
        let mut pen = Pen::default();
        pen.place(Point::new(0.0, 0.0));
        pen.place(Point::new(10.0, 0.0));
        pen.place(Point::new(10.0, 10.0));

        let closed = pen.close().expect("three anchors enclose an area");

        assert!(closed.closed);
        assert_eq!(closed.anchors.len(), 3);
        assert!(
            pen.anchors().is_empty(),
            "the pen is ready for the next one"
        );
    }

    #[test]
    fn dragging_mirrors_the_handles() {
        let mut pen = Pen::default();
        pen.place(Point::new(10.0, 10.0));
        pen.shape(Vec2::new(4.0, 2.0), true);

        let anchor = pen.anchors().last().expect("an anchor was placed");
        assert_eq!(anchor.out_handle, Vec2::new(4.0, 2.0));
        assert_eq!(anchor.in_handle, Vec2::new(-4.0, -2.0));
        assert_eq!(anchor.kind, AnchorKind::Smooth);
    }

    #[test]
    fn breaking_the_mirror_leaves_the_incoming_handle_where_it_was() {
        let mut pen = Pen::default();
        pen.place(Point::new(10.0, 10.0));
        pen.shape(Vec2::new(4.0, 0.0), true);
        pen.shape(Vec2::new(0.0, 6.0), false);

        let anchor = pen.anchors().last().expect("an anchor was placed");
        assert_eq!(anchor.in_handle, Vec2::new(-4.0, 0.0));
        assert_eq!(anchor.out_handle, Vec2::new(0.0, 6.0));
        assert_eq!(anchor.kind, AnchorKind::Asymmetric);
    }

    #[test]
    fn dragging_back_onto_the_anchor_leaves_a_corner() {
        let mut pen = Pen::default();
        pen.place(Point::new(10.0, 10.0));
        pen.shape(Vec2::new(4.0, 2.0), true);
        pen.shape(Vec2::ZERO, true);

        let anchor = pen.anchors().last().expect("an anchor was placed");
        assert_eq!(anchor.kind, AnchorKind::Corner);
    }

    fn square() -> Vec<Measurement> {
        let mut pen = Pen::default();
        pen.place(Point::new(0.0, 0.0));
        pen.place(Point::new(100.0, 0.0));
        pen.place(Point::new(100.0, 100.0));
        pen.place(Point::new(0.0, 100.0));

        vec![Measurement {
            name: "Area 1".to_owned(),
            outer: pen.close().expect("four anchors enclose an area"),
            holes: Vec::new(),
            colour: eframe::egui::Color32::WHITE,
            visible: true,
        }]
    }

    /// A square with a smaller square taken out of the middle of it.
    fn square_with_a_hole() -> Vec<Measurement> {
        let mut measurements = square();
        let mut pen = Pen::default();

        pen.place(Point::new(40.0, 40.0));
        pen.place(Point::new(60.0, 40.0));
        pen.place(Point::new(60.0, 60.0));
        pen.place(Point::new(40.0, 60.0));

        let hole = pen.close().expect("four anchors enclose an area");
        measurements[0].holes.push(hole);

        measurements
    }

    fn at(measurement: usize, anchor: usize) -> Selection {
        Selection {
            measurement,
            outline: Outline::Outer,
            anchor,
        }
    }

    fn in_hole(measurement: usize, hole: usize, anchor: usize) -> Selection {
        Selection {
            measurement,
            outline: Outline::Hole(hole),
            anchor,
        }
    }

    #[test]
    fn snapping_takes_the_nearest_target_in_reach() {
        let targets = [
            Point::new(0.0, 0.0),
            Point::new(100.0, 0.0),
            Point::new(104.0, 3.0),
        ];

        assert_eq!(
            snap(Point::new(101.0, 1.0), 5.0, targets.into_iter()),
            Some(Point::new(100.0, 0.0))
        );
        assert_eq!(snap(Point::new(50.0, 50.0), 5.0, targets.into_iter()), None);
    }

    #[test]
    fn an_anchor_is_hit_within_the_radius_and_not_beyond_it() {
        let measurements = square();
        let editor = Editor::default();

        assert_eq!(
            editor.hit(&measurements, None, Point::new(103.0, 2.0), 5.0),
            Some((at(0, 1), Grab::Anchor))
        );
        assert_eq!(
            editor.hit(&measurements, None, Point::new(50.0, 50.0), 5.0),
            None
        );
    }

    /// A handle is only reachable once its anchor is selected, because that is
    /// the only time it is drawn.
    #[test]
    fn a_handle_is_hit_only_while_its_anchor_is_selected() {
        let mut measurements = square();
        measurements[0].outer.anchors[0].out_handle = Vec2::new(20.0, 0.0);
        measurements[0].outer.anchors[0].kind = AnchorKind::Smooth;

        let unselected = Editor::default();
        assert_eq!(
            unselected.hit(&measurements, None, Point::new(20.0, 0.0), 5.0),
            None
        );

        let selected = Editor {
            selected: Some(at(0, 0)),
            ..Editor::default()
        };
        assert_eq!(
            selected.hit(&measurements, None, Point::new(20.0, 0.0), 5.0),
            Some((at(0, 0), Grab::Out))
        );
    }

    #[test]
    fn dragging_a_smooth_handle_mirrors_until_the_mirror_is_broken() {
        let mut measurements = square();
        measurements[0].outer.anchors[0].kind = AnchorKind::Smooth;

        move_to(
            &mut measurements,
            at(0, 0),
            Grab::Out,
            Point::new(0.0, 9.0),
            true,
        );
        assert_eq!(
            measurements[0].outer.anchors[0].in_handle,
            Vec2::new(0.0, -9.0)
        );

        move_to(
            &mut measurements,
            at(0, 0),
            Grab::Out,
            Point::new(4.0, 4.0),
            false,
        );
        assert_eq!(
            measurements[0].outer.anchors[0].in_handle,
            Vec2::new(0.0, -9.0)
        );
        assert_eq!(
            measurements[0].outer.anchors[0].kind,
            AnchorKind::Asymmetric
        );
    }

    #[test]
    fn an_area_keeps_the_three_anchors_it_needs() {
        let mut measurements = square();

        assert_eq!(delete(&mut measurements, at(0, 0)), Some(()));
        assert_eq!(measurements[0].outer.anchors.len(), 3);

        assert_eq!(delete(&mut measurements, at(0, 0)), None);
        assert_eq!(measurements[0].outer.anchors.len(), 3);
    }

    /// Smoothing an anchor and sharpening it again returns the straight edges
    /// it started with.
    #[test]
    fn toggling_twice_returns_the_anchor_to_a_corner() {
        let mut measurements = square();

        toggle(&mut measurements, at(0, 1)).expect("a corner smooths");
        assert_eq!(measurements[0].outer.anchors[1].kind, AnchorKind::Smooth);
        assert_ne!(measurements[0].outer.anchors[1].out_handle, Vec2::ZERO);

        toggle(&mut measurements, at(0, 1)).expect("a smooth anchor sharpens");
        assert_eq!(measurements[0].outer.anchors[1].kind, AnchorKind::Corner);
        assert_eq!(measurements[0].outer.anchors[1].in_handle, Vec2::ZERO);
        assert_eq!(measurements[0].outer.anchors[1].out_handle, Vec2::ZERO);
    }

    /// A hole's anchors are as reachable as an outline's, and addressed by
    /// which hole they belong to.
    #[test]
    fn a_hole_can_be_reshaped_like_any_other_outline() {
        let mut measurements = square_with_a_hole();
        let editor = Editor::default();

        assert_eq!(
            editor.hit(&measurements, None, Point::new(60.0, 40.0), 5.0),
            Some((in_hole(0, 0, 1), Grab::Anchor))
        );

        move_to(
            &mut measurements,
            in_hole(0, 0, 1),
            Grab::Anchor,
            Point::new(70.0, 40.0),
            true,
        );
        assert_eq!(
            measurements[0].holes[0].anchors[1].pos,
            Point::new(70.0, 40.0)
        );

        assert_eq!(toggle(&mut measurements, in_hole(0, 0, 1)), Some(()));
        assert_eq!(measurements[0].holes[0].anchors[1].kind, AnchorKind::Smooth);

        assert_eq!(delete(&mut measurements, in_hole(0, 0, 1)), Some(()));
        assert_eq!(measurements[0].holes[0].anchors.len(), 3);

        // Three is as few as a hole can have, just as for an outline.
        assert_eq!(delete(&mut measurements, in_hole(0, 0, 0)), None);
        assert_eq!(
            measurements[0].outer.anchors.len(),
            4,
            "the outline is untouched"
        );
    }

    /// A click near a hole's edge adds to the hole, not to the outline it is
    /// punched in.
    #[test]
    fn an_anchor_lands_on_the_outline_it_was_aimed_at() {
        let mut measurements = square_with_a_hole();

        assert_eq!(
            insert(&mut measurements, None, Point::new(50.0, 41.0), 5.0),
            Some(in_hole(0, 0, 1))
        );
        assert_eq!(measurements[0].holes[0].anchors.len(), 5);
        assert_eq!(measurements[0].outer.anchors.len(), 4);
    }

    #[test]
    fn an_anchor_is_only_inserted_near_an_outline() {
        let mut measurements = square();

        assert_eq!(
            insert(&mut measurements, None, Point::new(500.0, 500.0), 5.0),
            None
        );
        assert_eq!(measurements[0].outer.anchors.len(), 4);

        assert_eq!(
            insert(&mut measurements, None, Point::new(50.0, 2.0), 5.0),
            Some(at(0, 1))
        );
        assert_eq!(measurements[0].outer.anchors.len(), 5);
        assert_eq!(measurements[0].outer.anchors[1].pos, Point::new(50.0, 0.0));
    }

    #[test]
    fn two_anchors_enclose_nothing_and_stay_put() {
        let mut pen = Pen::default();
        pen.place(Point::new(0.0, 0.0));
        pen.place(Point::new(10.0, 0.0));

        assert!(pen.close().is_none());
        assert_eq!(pen.anchors().len(), 2, "the outline is still being drawn");
    }

    /// Two squares side by side, sharing the edge at x = 100.
    fn two_squares() -> Vec<Measurement> {
        let mut measurements = square();
        let mut pen = Pen::default();

        pen.place(Point::new(100.0, 0.0));
        pen.place(Point::new(200.0, 0.0));
        pen.place(Point::new(200.0, 100.0));
        pen.place(Point::new(100.0, 100.0));

        measurements.push(Measurement {
            name: "Area 2".to_owned(),
            outer: pen.close().expect("four anchors enclose an area"),
            holes: Vec::new(),
            colour: eframe::egui::Color32::WHITE,
            visible: true,
        });

        measurements
    }

    /// With one area in hand, a click by the neighbour's anchor reaches
    /// nothing: an anchor tool only takes hold of what it is showing.
    #[test]
    fn an_anchor_is_only_hit_within_the_area_in_hand() {
        let measurements = two_squares();
        let editor = Editor::default();
        let corner = Point::new(200.0, 0.0);

        assert_eq!(
            editor.hit(&measurements, Some(1), corner, 5.0),
            Some((at(1, 1), Grab::Anchor))
        );
        assert_eq!(editor.hit(&measurements, Some(0), corner, 5.0), None);
        assert_eq!(
            editor.hit(&measurements, None, corner, 5.0),
            Some((at(1, 1), Grab::Anchor))
        );
    }

    /// The two squares share an edge, so a point on it is as near to one as to
    /// the other. Which one gains the anchor is decided by what is in hand,
    /// not by which happens to be nearest.
    #[test]
    fn an_anchor_lands_in_the_area_in_hand_on_a_shared_edge() {
        let on_the_shared_edge = Point::new(100.0, 50.0);

        let mut measurements = two_squares();
        assert_eq!(
            insert(&mut measurements, Some(0), on_the_shared_edge, 5.0),
            Some(at(0, 2))
        );
        assert_eq!(measurements[0].outer.anchors.len(), 5);
        assert_eq!(measurements[1].outer.anchors.len(), 4);

        let mut measurements = two_squares();
        assert_eq!(
            insert(&mut measurements, Some(1), on_the_shared_edge, 5.0),
            Some(at(1, 4))
        );
        assert_eq!(measurements[0].outer.anchors.len(), 4);
        assert_eq!(measurements[1].outer.anchors.len(), 5);
    }

    /// A click nowhere near the area in hand adds nothing, even when it lands
    /// squarely on another area's edge.
    #[test]
    fn no_anchor_lands_outside_the_area_in_hand() {
        let mut measurements = two_squares();

        assert_eq!(
            insert(&mut measurements, Some(0), Point::new(150.0, 0.0), 5.0),
            None
        );
        assert_eq!(measurements[0].outer.anchors.len(), 4);
        assert_eq!(measurements[1].outer.anchors.len(), 4);
    }
}
