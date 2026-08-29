//! The pen: placing anchors until an outline is closed.

use kurbo::{Point, Vec2};

use crate::doc::{Anchor, AnchorKind, SubPath};

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

    #[test]
    fn two_anchors_enclose_nothing_and_stay_put() {
        let mut pen = Pen::default();
        pen.place(Point::new(0.0, 0.0));
        pen.place(Point::new(10.0, 0.0));

        assert!(pen.close().is_none());
        assert_eq!(pen.anchors().len(), 2, "the outline is still being drawn");
    }
}
