//! Computers placed in one shared coordinate space, the way an operating system's display
//! settings place monitors.
//!
//! A computer has one display or several, each a rectangle in pixels. The cursor leaves a
//! display through one of its edges and arrives on whatever display is just beyond that edge,
//! at the same spot in the shared space. Where nothing is beyond the edge, because the
//! neighbor is smaller or offset, or because there is no neighbor, the cursor stops.
//!
//! Positions that belong to one computer, such as where its cursor is, are measured from the
//! top-left corner of the smallest rectangle holding all of its displays.

use connected_proto::bounds;
pub use connected_proto::Rect;

/// Where the cursor lands after leaving a computer, relative to the arrival computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Landing {
    pub screen: String,
    pub x: i32,
    pub y: i32,
}

/// What a cursor movement comes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// It stays on its computer, at this position. That includes moving onto another display
    /// of the same computer.
    Stay(i32, i32),
    /// It leaves for another computer.
    Cross(Landing),
    /// It ran into an edge with nothing beyond it, and stops at this position.
    Stop(i32, i32),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Desktop {
    computers: Vec<(String, Vec<Rect>)>,
}

impl Desktop {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a computer with its displays at their places on the shared desktop, replacing any
    /// earlier one with the same name. A computer with no displays is left out.
    pub fn add(&mut self, name: impl Into<String>, displays: Vec<Rect>) -> &mut Self {
        let name = name.into();
        self.computers.retain(|(existing, _)| *existing != name);
        if !displays.is_empty() {
            self.computers.push((name, displays));
        }
        self
    }

    /// Adds a computer that has a single display.
    pub fn add_one(&mut self, name: impl Into<String>, rect: Rect) -> &mut Self {
        self.add(name, vec![rect])
    }

    pub fn displays(&self, name: &str) -> Option<&[Rect]> {
        self.computers.iter().find(|(existing, _)| existing == name).map(|(_, d)| d.as_slice())
    }

    /// The smallest rectangle holding all of a computer's displays.
    pub fn rect(&self, name: &str) -> Option<Rect> {
        bounds(self.displays(name)?)
    }

    pub fn screens(&self) -> &[(String, Vec<Rect>)] {
        &self.computers
    }

    /// What happens when the cursor of `from` goes from `pos` to `next`, both measured from
    /// the top-left corner of that computer's displays.
    pub fn step(&self, from: &str, pos: (i32, i32), next: (i32, i32)) -> Step {
        let Some(displays) = self.displays(from) else {
            return Step::Stay(next.0, next.1);
        };
        let origin = bounds(displays).expect("a computer has displays");
        let local: Vec<Rect> = displays.iter().map(|d| d.moved(-origin.x, -origin.y)).collect();

        if local.iter().any(|d| d.contains(next.0, next.1)) {
            return Step::Stay(next.0, next.1);
        }

        // The display being left, and the edge of it that the cursor went through.
        let current = local
            .iter()
            .find(|d| d.contains(pos.0, pos.1))
            .copied()
            .unwrap_or_else(|| nearest(&local, pos));
        let edge = current.edge_beyond(next.0 - current.x, next.1 - current.y);
        let Some(edge) = edge else {
            return Step::Stay(next.0, next.1);
        };

        // Where the cursor would be in the shared space, kept within the edge it is leaving by.
        let source = current.moved(origin.x, origin.y);
        let along_x = (origin.x + next.0).clamp(source.x, source.right() - 1);
        let along_y = (origin.y + next.1).clamp(source.y, source.bottom() - 1);
        let probe = match edge {
            connected_proto::Edge::Right => (source.right(), along_y),
            connected_proto::Edge::Left => (source.x - 1, along_y),
            connected_proto::Edge::Bottom => (along_x, source.bottom()),
            connected_proto::Edge::Top => (along_x, source.y - 1),
        };

        for (name, displays) in &self.computers {
            if !displays.iter().any(|d| *d != source && d.contains(probe.0, probe.1)) {
                continue;
            }
            let target = bounds(displays).expect("a computer has displays");
            if name == from {
                return Step::Stay(probe.0 - target.x, probe.1 - target.y);
            }
            return Step::Cross(Landing {
                screen: name.clone(),
                x: probe.0 - target.x,
                y: probe.1 - target.y,
            });
        }
        let (x, y) = current.clamp(next.0 - current.x, next.1 - current.y);
        Step::Stop(current.x + x, current.y + y)
    }
}

impl Desktop {
    /// `pos`, a position on the named computer, if it is on one of its displays, and otherwise
    /// the nearest position that is.
    pub fn settle(&self, name: &str, pos: (i32, i32)) -> (i32, i32) {
        let Some(displays) = self.displays(name) else {
            return pos;
        };
        let origin = bounds(displays).expect("a computer has displays");
        let local: Vec<Rect> = displays.iter().map(|d| d.moved(-origin.x, -origin.y)).collect();
        if local.iter().any(|d| d.contains(pos.0, pos.1)) {
            return pos;
        }
        let near = nearest(&local, pos);
        let (x, y) = near.clamp(pos.0 - near.x, pos.1 - near.y);
        (near.x + x, near.y + y)
    }
}

/// The rectangle closest to `pos`.
fn nearest(rects: &[Rect], pos: (i32, i32)) -> Rect {
    let distance = |r: &Rect| {
        let dx = (r.x - pos.0).max(0).max(pos.0 - (r.right() - 1));
        let dy = (r.y - pos.1).max(0).max(pos.1 - (r.bottom() - 1));
        i64::from(dx) * i64::from(dx) + i64::from(dy) * i64::from(dy)
    };
    *rects.iter().min_by_key(|r| distance(r)).expect("a computer has displays")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn landing(screen: &str, x: i32, y: i32) -> Option<Landing> {
        Some(Landing { screen: screen.into(), x, y })
    }

    /// Where the cursor of a computer with one display lands when it tries to reach (x, y),
    /// which is outside that display, from the nearest point inside it.
    fn leave(desktop: &Desktop, from: &str, x: i32, y: i32) -> Option<Landing> {
        let rect = desktop.rect(from).unwrap();
        let pos = rect.clamp(x, y);
        match desktop.step(from, pos, (x, y)) {
            Step::Cross(landing) => Some(landing),
            _ => None,
        }
    }

    fn side_by_side() -> Desktop {
        let mut desktop = Desktop::new();
        desktop
            .add_one("mac", Rect::new(0, 0, 1920, 1080))
            .add_one("win", Rect::new(1920, 0, 2560, 1440));
        desktop
    }

    #[test]
    fn the_cursor_arrives_at_the_same_height_it_left() {
        let desktop = side_by_side();
        assert_eq!(leave(&desktop, "mac", 1925, 540), landing("win", 0, 540));
        assert_eq!(leave(&desktop, "win", -3, 540), landing("mac", 1919, 540));
    }

    #[test]
    fn a_taller_neighbor_is_only_reached_where_the_screens_meet() {
        let desktop = side_by_side();
        // Win is 1440 tall and mac 1080: the lower part of win's left edge faces nothing.
        assert_eq!(leave(&desktop, "win", -1, 1079), landing("mac", 1919, 1079));
        assert_eq!(leave(&desktop, "win", -1, 1080), None);
        assert_eq!(leave(&desktop, "win", -1, 1400), None);
    }

    #[test]
    fn a_shorter_neighbor_is_only_reached_where_it_reaches() {
        let mut desktop = Desktop::new();
        desktop
            .add_one("mac", Rect::new(0, 0, 1920, 1080))
            .add_one("tv", Rect::new(1920, 0, 1280, 720));
        assert_eq!(leave(&desktop, "mac", 1920, 700), landing("tv", 0, 700));
        assert_eq!(leave(&desktop, "mac", 1920, 900), None);
    }

    #[test]
    fn an_offset_neighbor_arrives_at_the_right_height_for_it() {
        let mut desktop = Desktop::new();
        desktop
            .add_one("mac", Rect::new(0, 0, 1920, 1080))
            .add_one("tv", Rect::new(1920, 300, 1280, 720));
        assert_eq!(leave(&desktop, "mac", 1920, 100), None, "above where tv starts");
        assert_eq!(leave(&desktop, "mac", 1920, 400), landing("tv", 0, 100));
        assert_eq!(leave(&desktop, "tv", -1, 50), landing("mac", 1919, 350));
    }

    #[test]
    fn neighbors_above_and_below_are_reached_through_the_top_and_bottom_edges() {
        let mut desktop = Desktop::new();
        desktop
            .add_one("mac", Rect::new(0, 0, 1920, 1080))
            .add_one("above", Rect::new(-960, -2160, 3840, 2160))
            .add_one("below", Rect::new(0, 1080, 1000, 500));
        assert_eq!(leave(&desktop, "mac", 960, -1), landing("above", 1920, 2159));
        assert_eq!(leave(&desktop, "mac", 600, 1080), landing("below", 600, 0));
        assert_eq!(leave(&desktop, "mac", 1500, 1080), None, "below ends at x = 1000");
    }

    #[test]
    fn one_edge_can_face_several_screens() {
        let mut desktop = Desktop::new();
        desktop
            .add_one("mac", Rect::new(0, 0, 1920, 1080))
            .add_one("upper", Rect::new(1920, 0, 1000, 500))
            .add_one("lower", Rect::new(1920, 500, 1000, 580));
        assert_eq!(leave(&desktop, "mac", 1920, 100), landing("upper", 0, 100));
        assert_eq!(leave(&desktop, "mac", 1920, 700), landing("lower", 0, 200));
    }

    #[test]
    fn a_gap_between_screens_is_a_dead_end() {
        let mut desktop = Desktop::new();
        desktop.add_one("a", Rect::new(0, 0, 100, 100)).add_one("b", Rect::new(150, 0, 100, 100));
        assert_eq!(leave(&desktop, "a", 100, 50), None);
    }

    #[test]
    fn a_screen_is_never_its_own_neighbor() {
        let mut desktop = Desktop::new();
        desktop.add_one("a", Rect::new(0, 0, 100, 100));
        assert_eq!(leave(&desktop, "a", 100, 50), None);
        assert_eq!(desktop.step("nobody", (0, 0), (5, 5)), Step::Stay(5, 5));
    }

    #[test]
    fn adding_a_computer_again_replaces_it() {
        let mut desktop = side_by_side();
        desktop.add_one("win", Rect::new(1920, 0, 1000, 500));
        assert_eq!(desktop.screens().len(), 2);
        assert_eq!(desktop.rect("win"), Some(Rect::new(1920, 0, 1000, 500)));
        desktop.add("win", vec![]);
        assert!(desktop.rect("win").is_none(), "no displays, no computer");
    }

    #[test]
    fn leaving_through_a_corner_uses_the_part_of_the_edge_that_exists() {
        let desktop = side_by_side();
        // Beyond both the right and bottom edges of mac: the crossing height is held to mac's.
        assert_eq!(leave(&desktop, "mac", 1925, 1500), landing("win", 0, 1079));
    }

    /// Two displays side by side, the right one lower: an L-shaped computer.
    fn stepped() -> Desktop {
        let mut desktop = Desktop::new();
        desktop.add(
            "pc",
            vec![Rect::new(1000, 0, 1000, 1000), Rect::new(2000, 500, 1000, 500)],
        );
        desktop
    }

    #[test]
    fn moving_between_the_displays_of_one_computer_does_not_leave_it() {
        let desktop = stepped();
        // Measured from the corner of all displays, which is (1000, 0).
        assert_eq!(desktop.step("pc", (990, 700), (1010, 700)), Step::Stay(1010, 700));
        assert_eq!(desktop.step("pc", (500, 100), (505, 100)), Step::Stay(505, 100));
    }

    #[test]
    fn a_notch_between_displays_is_a_dead_end() {
        let desktop = stepped();
        // The right display starts at y = 500, so above that the left one's edge faces nothing.
        assert_eq!(desktop.step("pc", (995, 200), (1005, 200)), Step::Stop(999, 200));
        // The same holds going up out of the right display, into the notch above it.
        assert_eq!(desktop.step("pc", (1500, 505), (1500, 495)), Step::Stop(1500, 500));
    }

    #[test]
    fn leaving_a_group_by_its_second_display_lands_relative_to_the_neighbor() {
        let mut desktop = stepped();
        desktop.add_one("tv", Rect::new(3000, 600, 800, 400));
        // From the right display (x 1000..2000 here), going on right at y = 700 reaches the tv.
        let step = desktop.step("pc", (1995, 700), (2005, 700));
        assert_eq!(step, Step::Cross(Landing { screen: "tv".into(), x: 0, y: 100 }));
        // The tv's left edge faces the right display only where they meet.
        assert_eq!(leave(&desktop, "tv", -1, 50), landing("pc", 1999, 650));
    }

    #[test]
    fn leaving_a_group_whose_corner_is_not_the_origin_uses_the_corner() {
        let mut desktop = Desktop::new();
        desktop.add("pc", vec![Rect::new(-1920, 100, 1920, 1080), Rect::new(0, 0, 2560, 1440)]);
        desktop.add_one("mac", Rect::new(2560, 0, 1000, 1000));
        // The group's corner is (-1920, 0): its right display runs from x = 1920 to 4480 there.
        let step = desktop.step("pc", (4475, 300), (4485, 300));
        assert_eq!(step, Step::Cross(Landing { screen: "mac".into(), x: 0, y: 300 }));
    }

    #[test]
    fn a_position_is_settled_onto_the_nearest_display() {
        let desktop = stepped();
        assert_eq!(desktop.settle("pc", (500, 500)), (500, 500));
        assert_eq!(desktop.settle("pc", (1500, 100)), (1500, 500), "in the notch, above the right display");
        assert_eq!(desktop.settle("pc", (5000, 5000)), (1999, 999));
        assert_eq!(desktop.settle("nobody", (7, 8)), (7, 8));
    }

    #[test]
    fn a_cursor_outside_every_display_is_taken_to_the_nearest_one() {
        let desktop = stepped();
        // A position left over from before the displays changed.
        assert_eq!(desktop.step("pc", (5000, 5000), (5010, 5000)), Step::Stop(1999, 999));
    }
}
