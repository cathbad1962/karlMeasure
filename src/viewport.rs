//! The page-to-screen transform: pan, zoom, and fitting a page to a window.
//!
//! Page space is PDF points with the origin at the page's top-left corner and
//! y increasing downward. Screen space is egui logical points. The mapping is
//! `screen = page * zoom + pan`; screen positions are derived, never stored.

use kurbo::{Point, Rect, Size, Vec2};

/// Zoom limits, in screen points per page point.
const MIN_ZOOM: f64 = 0.02;
const MAX_ZOOM: f64 = 64.0;

/// Proportion of the window left clear around the page when fitting.
const FIT_MARGIN: f64 = 0.03;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// Screen position of the page's top-left corner.
    pub pan: Vec2,
    /// Screen points per page point.
    pub zoom: f64,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            pan: Vec2::ZERO,
            zoom: 1.0,
        }
    }
}

impl Viewport {
    /// Centres a page of `page` size within `screen`, scaled to fit inside a
    /// small margin.
    pub fn fit(page: Size, screen: Rect) -> Self {
        if page.width <= 0.0 || page.height <= 0.0 {
            return Self::default();
        }

        let zoom = ((screen.width() / page.width).min(screen.height() / page.height)
            * (1.0 - FIT_MARGIN))
            .clamp(MIN_ZOOM, MAX_ZOOM);
        let top_left = screen.center() - (page.to_vec2() * zoom) / 2.0;

        Self {
            pan: top_left.to_vec2(),
            zoom,
        }
    }

    pub fn page_to_screen(&self, page: Point) -> Point {
        (page.to_vec2() * self.zoom + self.pan).to_point()
    }

    pub fn screen_to_page(&self, screen: Point) -> Point {
        ((screen.to_vec2() - self.pan) / self.zoom).to_point()
    }

    /// The screen rectangle covered by a page-space rectangle.
    pub fn page_rect_to_screen(&self, page: Rect) -> Rect {
        Rect::from_points(
            self.page_to_screen(Point::new(page.x0, page.y0)),
            self.page_to_screen(Point::new(page.x1, page.y1)),
        )
    }

    /// The page-space rectangle visible within a screen rectangle.
    pub fn visible_page_rect(&self, screen: Rect) -> Rect {
        Rect::from_points(
            self.screen_to_page(Point::new(screen.x0, screen.y0)),
            self.screen_to_page(Point::new(screen.x1, screen.y1)),
        )
    }

    pub fn pan_by(&mut self, delta: Vec2) {
        self.pan += delta;
    }

    /// Multiplies the zoom by `factor`, keeping whatever page point lies under
    /// `cursor` under the cursor. The pan follows the clamped zoom, so the
    /// view does not drift once a limit is reached.
    pub fn zoom_about(&mut self, cursor: Point, factor: f64) {
        let zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let applied = zoom / self.zoom;

        self.pan = cursor.to_vec2() - (cursor.to_vec2() - self.pan) * applied;
        self.zoom = zoom;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The page point under the cursor is the anchor a wheel zoom turns about.
    #[test]
    fn zoom_keeps_the_page_point_under_the_cursor() {
        let mut viewport = Viewport {
            pan: Vec2::new(-120.0, 45.0),
            zoom: 0.8,
        };
        let cursor = Point::new(370.0, 210.0);
        let before = viewport.screen_to_page(cursor);

        viewport.zoom_about(cursor, 1.25);

        let after = viewport.screen_to_page(cursor);
        assert!((after.x - before.x).abs() < 1e-9);
        assert!((after.y - before.y).abs() < 1e-9);
    }

    /// Winding the wheel past a zoom limit must not slide the page sideways.
    #[test]
    fn zoom_holds_the_anchor_when_clamped() {
        let mut viewport = Viewport {
            pan: Vec2::new(10.0, -30.0),
            zoom: MAX_ZOOM,
        };
        let cursor = Point::new(200.0, 150.0);
        let before = viewport.screen_to_page(cursor);

        viewport.zoom_about(cursor, 4.0);

        assert_eq!(viewport.zoom, MAX_ZOOM);
        let after = viewport.screen_to_page(cursor);
        assert!((after.x - before.x).abs() < 1e-9);
        assert!((after.y - before.y).abs() < 1e-9);
    }

    #[test]
    fn fit_centres_the_page() {
        let page = Size::new(842.0, 595.0);
        let screen = Rect::new(0.0, 0.0, 1000.0, 800.0);

        let viewport = Viewport::fit(page, screen);
        let centre = viewport.page_to_screen(Point::new(page.width / 2.0, page.height / 2.0));

        assert!((centre.x - screen.center().x).abs() < 1e-9);
        assert!((centre.y - screen.center().y).abs() < 1e-9);
        assert!(
            viewport
                .page_rect_to_screen(Rect::from_origin_size(Point::ZERO, page))
                .width()
                <= screen.width()
        );
    }

    #[test]
    fn screen_and_page_round_trip() {
        let viewport = Viewport {
            pan: Vec2::new(33.0, -17.5),
            zoom: 2.75,
        };
        let page = Point::new(123.5, 400.25);

        let round_tripped = viewport.screen_to_page(viewport.page_to_screen(page));
        assert!((round_tripped.x - page.x).abs() < 1e-9);
        assert!((round_tripped.y - page.y).abs() < 1e-9);
    }
}
