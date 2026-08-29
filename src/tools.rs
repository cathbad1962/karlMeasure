//! The pen: placing anchors until an outline is closed.

use kurbo::Point;

use crate::doc::{Anchor, SubPath};

/// The fewest anchors that can enclose an area.
const ENOUGH: usize = 3;

/// An outline being drawn, one anchor per click, in page space.
#[derive(Default)]
pub struct Pen {
    anchors: Vec<Anchor>,
}

impl Pen {
    pub fn place(&mut self, point: Point) {
        self.anchors.push(Anchor::corner(point));
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
    fn two_anchors_enclose_nothing_and_stay_put() {
        let mut pen = Pen::default();
        pen.place(Point::new(0.0, 0.0));
        pen.place(Point::new(10.0, 0.0));

        assert!(pen.close().is_none());
        assert_eq!(pen.anchors().len(), 2, "the outline is still being drawn");
    }
}
