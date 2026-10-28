//! Rectangles in pixels, used for the displays of a computer and for where they sit.

use serde::{Deserialize, Serialize};

use crate::message::Edge;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self { x, y, width, height }
    }

    pub fn right(self) -> i32 {
        self.x + self.width
    }

    pub fn bottom(self) -> i32 {
        self.y + self.height
    }

    pub fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// Whether the two share any area. Rectangles that only touch along an edge do not.
    pub fn overlaps(self, other: Rect) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }

    /// The edge a point lies beyond, if it lies outside. The point is relative to the
    /// rectangle's own top-left corner. A point beyond a corner counts for the horizontal edge.
    pub fn edge_beyond(self, x: i32, y: i32) -> Option<Edge> {
        if x < 0 {
            Some(Edge::Left)
        } else if x >= self.width {
            Some(Edge::Right)
        } else if y < 0 {
            Some(Edge::Top)
        } else if y >= self.height {
            Some(Edge::Bottom)
        } else {
            None
        }
    }

    /// The nearest point inside, for a point relative to the rectangle's top-left corner.
    pub fn clamp(self, x: i32, y: i32) -> (i32, i32) {
        (x.clamp(0, self.width - 1), y.clamp(0, self.height - 1))
    }

    pub fn moved(self, dx: i32, dy: i32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.width, self.height)
    }
}

/// The smallest rectangle holding all of `rects`, or `None` for no rectangles.
pub fn bounds(rects: &[Rect]) -> Option<Rect> {
    let left = rects.iter().map(|r| r.x).min()?;
    let top = rects.iter().map(|r| r.y).min()?;
    let right = rects.iter().map(|r| r.right()).max()?;
    let bottom = rects.iter().map(|r| r.bottom()).max()?;
    Some(Rect::new(left, top, right - left, bottom - top))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangles_know_what_they_contain_and_overlap() {
        let rect = Rect::new(10, 20, 100, 50);
        assert!(rect.contains(10, 20));
        assert!(rect.contains(109, 69));
        assert!(!rect.contains(110, 20));
        assert!(!rect.contains(10, 70));

        assert!(rect.overlaps(Rect::new(50, 40, 10, 10)));
        assert!(rect.overlaps(Rect::new(100, 60, 50, 50)));
        assert!(!rect.overlaps(Rect::new(110, 20, 10, 10)), "touching is not overlapping");
        assert!(!rect.overlaps(Rect::new(10, 70, 100, 10)));
        assert!(!rect.overlaps(Rect::new(500, 500, 1, 1)));
    }

    #[test]
    fn a_point_beyond_the_rectangle_names_its_edge() {
        let rect = Rect::new(100, 100, 200, 100);
        assert_eq!(rect.edge_beyond(50, 50), None);
        assert_eq!(rect.edge_beyond(0, 0), None);
        assert_eq!(rect.edge_beyond(199, 99), None);
        assert_eq!(rect.edge_beyond(-1, 50), Some(Edge::Left));
        assert_eq!(rect.edge_beyond(200, 50), Some(Edge::Right));
        assert_eq!(rect.edge_beyond(50, -1), Some(Edge::Top));
        assert_eq!(rect.edge_beyond(50, 100), Some(Edge::Bottom));
        assert_eq!(rect.edge_beyond(250, 150), Some(Edge::Right), "a corner counts as the side");
        assert_eq!(rect.clamp(-5, 500), (0, 99));
    }

    #[test]
    fn bounds_hold_every_rectangle() {
        assert_eq!(bounds(&[]), None);
        assert_eq!(
            bounds(&[Rect::new(-1920, 0, 1920, 1080), Rect::new(0, 200, 2560, 1440)]),
            Some(Rect::new(-1920, 0, 4480, 1640))
        );
    }
}
