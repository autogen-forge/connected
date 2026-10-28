//! Where each computer's screen sits relative to the others.
//!
//! A screen is either at a position in the shared desktop (the way display settings place
//! monitors) or just beside this computer's screen. The second form is for programs that
//! cannot know a screen's size in advance, such as the command line: it is resolved to a
//! position once the real size is known.

use std::collections::BTreeMap;

use connected_proto::{Edge, bounds};
use serde::{Deserialize, Serialize};

use crate::desktop::{Desktop, Rect};

/// The size assumed for a computer whose displays are not known, such as one that has not
/// connected yet.
pub const DEFAULT_SIZE: (i32, i32) = (1920, 1080);

/// The displays assumed for a computer that has not connected yet.
pub fn default_displays() -> Vec<Rect> {
    vec![Rect::new(0, 0, DEFAULT_SIZE.0, DEFAULT_SIZE.1)]
}

/// The displays moved so that the smallest rectangle holding them has its corner at (0, 0),
/// and how far they were moved: the old position of that corner.
pub fn normalize(displays: &[Rect]) -> (Vec<Rect>, (i32, i32)) {
    match bounds(displays) {
        Some(all) => (displays.iter().map(|d| d.moved(-all.x, -all.y)).collect(), (all.x, all.y)),
        None => (Vec::new(), (0, 0)),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Spot {
    /// The corner of the computer's displays is at this position in the shared desktop.
    At { x: i32, y: i32 },
    /// Flush against this computer's screen, on this side.
    Beside { edge: Edge },
}

/// The displays of each computer, which decide how big they are on the shared desktop.
pub struct Metrics<'a> {
    pub this_computer: &'a str,
    /// This computer's own displays.
    pub local: &'a [Rect],
    /// Other computers' displays, where known.
    pub devices: &'a BTreeMap<String, Vec<Rect>>,
}

impl Metrics<'_> {
    pub fn device(&self, name: &str) -> Vec<Rect> {
        self.devices.get(name).filter(|d| !d.is_empty()).cloned().unwrap_or_else(default_displays)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlaceError {
    #[error("it would overlap {0}")]
    Overlaps(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Arrangement {
    /// Where this computer's own screen is. Usually the origin, but it can be moved.
    pub local: Point,
    /// The other computers, by the name they were paired under.
    pub remote: BTreeMap<String, Spot>,
}

impl Default for Arrangement {
    fn default() -> Self {
        Self { local: Point { x: 0, y: 0 }, remote: BTreeMap::new() }
    }
}

impl Arrangement {
    pub fn spot_of(&self, device: &str) -> Option<Spot> {
        self.remote.get(device).copied()
    }

    pub fn contains(&self, device: &str) -> bool {
        self.remote.contains_key(device)
    }

    pub fn remove(&mut self, device: &str) -> bool {
        self.remote.remove(device).is_some()
    }

    /// Puts `device` with its top-left corner at (`x`, `y`), unless that would overlap another
    /// screen. Moves it if it was already placed.
    pub fn place(&mut self, device: &str, x: i32, y: i32, metrics: &Metrics) -> Result<(), PlaceError> {
        self.place_spot(device, Spot::At { x, y }, metrics)
    }

    /// Puts `device` flush against this computer's screen, on the given side.
    pub fn place_beside_this_computer(
        &mut self,
        device: &str,
        edge: Edge,
        metrics: &Metrics,
    ) -> Result<(), PlaceError> {
        self.place_spot(device, Spot::Beside { edge }, metrics)
    }

    fn place_spot(&mut self, device: &str, spot: Spot, metrics: &Metrics) -> Result<(), PlaceError> {
        let mut candidate = self.clone();
        candidate.remote.insert(device.to_owned(), spot);
        if let Some(other) = candidate.overlap_of(device, metrics) {
            return Err(PlaceError::Overlaps(other));
        }
        *self = candidate;
        Ok(())
    }

    /// Moves this computer's own screen, unless that would overlap another screen.
    pub fn move_this_computer(&mut self, x: i32, y: i32, metrics: &Metrics) -> Result<(), PlaceError> {
        let mut candidate = self.clone();
        candidate.local = Point { x, y };
        if let Some(other) = candidate.overlap_of(metrics.this_computer, metrics) {
            return Err(PlaceError::Overlaps(other));
        }
        *self = candidate;
        Ok(())
    }

    /// Moves several computers by the same amount at once, keeping their places relative to each
    /// other. `include_local` moves this computer's own screen too. The group may not end up on
    /// top of a computer that is not part of it, and then nothing moves.
    ///
    /// Computers that stay behind keep their places on the desktop, even those that were placed
    /// beside this computer, which would otherwise follow it.
    pub fn translate(
        &mut self,
        devices: &[String],
        include_local: bool,
        dx: i32,
        dy: i32,
        metrics: &Metrics,
    ) -> Result<(), PlaceError> {
        let before = self.desktop(metrics);
        let mut candidate = self.clone();

        for name in self.remote.keys() {
            let Some(rect) = before.rect(name) else {
                continue;
            };
            let spot = if devices.contains(name) {
                Spot::At { x: rect.x + dx, y: rect.y + dy }
            } else if include_local {
                Spot::At { x: rect.x, y: rect.y }
            } else {
                continue;
            };
            candidate.remote.insert(name.clone(), spot);
        }
        if include_local {
            candidate.local = Point { x: self.local.x + dx, y: self.local.y + dy };
        }

        let desktop = candidate.desktop(metrics);
        let moved = |name: &str| {
            (include_local && name == metrics.this_computer) || devices.iter().any(|d| d == name)
        };
        for (name, _) in desktop.screens().iter().filter(|(name, _)| moved(name)) {
            let Some(mine) = desktop.rect(name) else {
                continue;
            };
            let blocker = desktop
                .screens()
                .iter()
                .filter(|(other, _)| !moved(other))
                .find(|(other, _)| desktop.rect(other).is_some_and(|rect| mine.overlaps(rect)));
            if let Some((other, _)) = blocker {
                return Err(PlaceError::Overlaps(other.clone()));
            }
        }
        *self = candidate;
        Ok(())
    }

    /// Every placed computer on the shared desktop, whether or not it is connected.
    pub fn desktop(&self, metrics: &Metrics) -> Desktop {
        self.desktop_of(metrics, |_| true)
    }

    /// The shared desktop with only the computers `present` accepts, besides this one.
    pub fn desktop_of(&self, metrics: &Metrics, present: impl Fn(&str) -> bool) -> Desktop {
        let (local, _) = normalize(metrics.local);
        let local: Vec<Rect> = local.iter().map(|d| d.moved(self.local.x, self.local.y)).collect();
        let local_bounds = bounds(&local);
        let mut desktop = Desktop::new();
        desktop.add(metrics.this_computer, local);

        for (name, spot) in &self.remote {
            // A computer named like this one would be indistinguishable from it.
            if name == metrics.this_computer || !present(name) {
                continue;
            }
            let (displays, _) = normalize(&metrics.device(name));
            let Some(size) = bounds(&displays) else {
                continue;
            };
            let (x, y) = match (*spot, local_bounds) {
                (Spot::At { x, y }, _) => (x, y),
                (Spot::Beside { edge }, Some(here)) => match edge {
                    Edge::Right => (here.right(), here.y),
                    Edge::Left => (here.x - size.width, here.y),
                    Edge::Bottom => (here.x, here.bottom()),
                    Edge::Top => (here.x, here.y - size.height),
                },
                (Spot::Beside { .. }, None) => (0, 0),
            };
            desktop.add(name.clone(), displays.iter().map(|d| d.moved(x, y)).collect());
        }
        desktop
    }

    /// Another computer that `screen` overlaps, if any. A computer takes up the whole
    /// rectangle holding its displays, empty corners included.
    fn overlap_of(&self, screen: &str, metrics: &Metrics) -> Option<String> {
        let desktop = self.desktop(metrics);
        let mine = desktop.rect(screen)?;
        desktop
            .screens()
            .iter()
            .filter(|(name, _)| name != screen)
            .find(|(name, _)| desktop.rect(name).is_some_and(|other| mine.overlaps(other)))
            .map(|(name, _)| name.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCAL: [Rect; 1] = [Rect::new(0, 0, 1920, 1080)];

    fn metrics<'a>(devices: &'a BTreeMap<String, Vec<Rect>>) -> Metrics<'a> {
        Metrics { this_computer: "desk", local: &LOCAL, devices }
    }

    /// Computers with one display each, of the given sizes.
    fn sizes(entries: &[(&str, (i32, i32))]) -> BTreeMap<String, Vec<Rect>> {
        entries
            .iter()
            .map(|(name, size)| ((*name).to_owned(), vec![Rect::new(0, 0, size.0, size.1)]))
            .collect()
    }

    #[test]
    fn this_computer_starts_alone_at_the_origin() {
        let arrangement = Arrangement::default();
        assert_eq!(arrangement.local, Point { x: 0, y: 0 });
        assert!(arrangement.remote.is_empty());
    }

    #[test]
    fn a_device_is_placed_by_its_top_left_corner() {
        let devices = sizes(&[("laptop", (1470, 956))]);
        let mut arrangement = Arrangement::default();
        arrangement.place("laptop", 1920, 100, &metrics(&devices)).unwrap();

        assert_eq!(arrangement.spot_of("laptop"), Some(Spot::At { x: 1920, y: 100 }));
        let desktop = arrangement.desktop(&metrics(&devices));
        assert_eq!(desktop.rect("laptop"), Some(Rect::new(1920, 100, 1470, 956)));
        assert_eq!(desktop.rect("desk"), Some(Rect::new(0, 0, 1920, 1080)));
    }

    #[test]
    fn placing_a_device_again_moves_it() {
        let devices = sizes(&[("laptop", (800, 600))]);
        let mut arrangement = Arrangement::default();
        arrangement.place("laptop", 1920, 0, &metrics(&devices)).unwrap();
        arrangement.place("laptop", -800, 0, &metrics(&devices)).unwrap();
        assert_eq!(arrangement.remote.len(), 1);
        assert_eq!(arrangement.spot_of("laptop"), Some(Spot::At { x: -800, y: 0 }));
    }

    #[test]
    fn screens_that_only_touch_are_allowed_and_ones_that_overlap_are_not() {
        let devices = sizes(&[("laptop", (800, 600)), ("tablet", (800, 600))]);
        let mut arrangement = Arrangement::default();
        arrangement.place("laptop", 1920, 0, &metrics(&devices)).unwrap();

        assert_eq!(
            arrangement.place("tablet", 2000, 100, &metrics(&devices)),
            Err(PlaceError::Overlaps("laptop".into()))
        );
        assert!(!arrangement.contains("tablet"), "a refused placement changes nothing");

        assert_eq!(
            arrangement.place("tablet", 1000, 500, &metrics(&devices)),
            Err(PlaceError::Overlaps("desk".into())),
            "this computer's own screen counts too"
        );
        assert!(arrangement.place("tablet", 2720, 0, &metrics(&devices)).is_ok());
    }

    #[test]
    fn a_device_can_be_put_back_where_it_already_is() {
        let devices = sizes(&[("laptop", (800, 600))]);
        let mut arrangement = Arrangement::default();
        arrangement.place("laptop", 1920, 0, &metrics(&devices)).unwrap();
        assert!(arrangement.place("laptop", 1920, 0, &metrics(&devices)).is_ok());
        assert!(arrangement.place("laptop", 1950, 20, &metrics(&devices)).is_ok(), "it does not collide with itself");
    }

    #[test]
    fn this_computer_can_move_but_not_onto_a_device() {
        let devices = sizes(&[("laptop", (800, 600))]);
        let mut arrangement = Arrangement::default();
        arrangement.place("laptop", 1920, 0, &metrics(&devices)).unwrap();

        assert!(arrangement.move_this_computer(0, 700, &metrics(&devices)).is_ok());
        assert_eq!(arrangement.local, Point { x: 0, y: 700 });
        assert_eq!(
            arrangement.move_this_computer(1500, 0, &metrics(&devices)),
            Err(PlaceError::Overlaps("laptop".into()))
        );
        assert_eq!(arrangement.local, Point { x: 0, y: 700 });
    }

    #[test]
    fn unknown_sizes_are_assumed() {
        let devices = BTreeMap::new();
        let mut arrangement = Arrangement::default();
        arrangement.place("laptop", 1920, 0, &metrics(&devices)).unwrap();
        let rect = arrangement.desktop(&metrics(&devices)).rect("laptop").unwrap();
        assert_eq!((rect.width, rect.height), DEFAULT_SIZE);
    }

    #[test]
    fn beside_this_computer_is_flush_against_it_whatever_the_size() {
        let devices = sizes(&[("r", (800, 600)), ("l", (500, 400)), ("t", (700, 300)), ("b", (900, 200))]);
        let mut arrangement = Arrangement::default();
        for (name, edge) in [("r", Edge::Right), ("l", Edge::Left), ("t", Edge::Top), ("b", Edge::Bottom)] {
            arrangement.place_beside_this_computer(name, edge, &metrics(&devices)).unwrap();
        }
        let desktop = arrangement.desktop(&metrics(&devices));
        assert_eq!(desktop.rect("r"), Some(Rect::new(1920, 0, 800, 600)));
        assert_eq!(desktop.rect("l"), Some(Rect::new(-500, 0, 500, 400)));
        assert_eq!(desktop.rect("t"), Some(Rect::new(0, -300, 700, 300)));
        assert_eq!(desktop.rect("b"), Some(Rect::new(0, 1080, 900, 200)));
    }

    #[test]
    fn two_devices_beside_the_same_edge_would_overlap() {
        let devices = BTreeMap::new();
        let mut arrangement = Arrangement::default();
        arrangement.place_beside_this_computer("a", Edge::Right, &metrics(&devices)).unwrap();
        assert_eq!(
            arrangement.place_beside_this_computer("b", Edge::Right, &metrics(&devices)),
            Err(PlaceError::Overlaps("a".into()))
        );
    }

    #[test]
    fn beside_follows_this_computer_when_it_moves() {
        let devices = sizes(&[("laptop", (800, 600))]);
        let mut arrangement = Arrangement::default();
        arrangement.place_beside_this_computer("laptop", Edge::Right, &metrics(&devices)).unwrap();
        arrangement.move_this_computer(0, 1000, &metrics(&devices)).unwrap();
        assert_eq!(
            arrangement.desktop(&metrics(&devices)).rect("laptop"),
            Some(Rect::new(1920, 1000, 800, 600))
        );
    }

    #[test]
    fn only_present_devices_are_on_the_desktop_of_connected_ones() {
        let devices = sizes(&[("a", (100, 100)), ("b", (100, 100))]);
        let mut arrangement = Arrangement::default();
        arrangement.place("a", 1920, 0, &metrics(&devices)).unwrap();
        arrangement.place("b", 2020, 0, &metrics(&devices)).unwrap();

        let desktop = arrangement.desktop_of(&metrics(&devices), |name| name == "b");
        assert!(desktop.rect("a").is_none());
        assert!(desktop.rect("b").is_some());
        assert!(desktop.rect("desk").is_some());
    }

    #[test]
    fn a_device_named_like_this_computer_is_left_out() {
        let devices = BTreeMap::new();
        let mut arrangement = Arrangement::default();
        arrangement.remote.insert("desk".into(), Spot::At { x: 5000, y: 0 });
        let desktop = arrangement.desktop(&metrics(&devices));
        assert_eq!(desktop.screens().len(), 1);
        assert_eq!(desktop.rect("desk"), Some(Rect::new(0, 0, 1920, 1080)));
    }

    #[test]
    fn removing_reports_whether_the_device_was_there() {
        let devices = BTreeMap::new();
        let mut arrangement = Arrangement::default();
        arrangement.place("laptop", 1920, 0, &metrics(&devices)).unwrap();
        assert!(arrangement.remove("laptop"));
        assert!(!arrangement.remove("laptop"));
    }

    fn group(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn several_computers_move_together_and_keep_their_places_relative_to_each_other() {
        let devices = sizes(&[("a", (500, 500)), ("b", (500, 500))]);
        let mut arrangement = Arrangement::default();
        arrangement.place("a", 2000, 0, &metrics(&devices)).unwrap();
        arrangement.place("b", 2500, 100, &metrics(&devices)).unwrap();

        arrangement.translate(&group(&["a", "b"]), false, 300, 700, &metrics(&devices)).unwrap();
        assert_eq!(arrangement.spot_of("a"), Some(Spot::At { x: 2300, y: 700 }));
        assert_eq!(arrangement.spot_of("b"), Some(Spot::At { x: 2800, y: 800 }));
        assert_eq!(arrangement.local, Point { x: 0, y: 0 });
    }

    #[test]
    fn a_group_may_not_land_on_a_computer_that_stays_behind() {
        let devices = sizes(&[("a", (500, 500)), ("b", (500, 500)), ("c", (500, 500))]);
        let mut arrangement = Arrangement::default();
        arrangement.place("a", 2000, 0, &metrics(&devices)).unwrap();
        arrangement.place("b", 2500, 0, &metrics(&devices)).unwrap();
        arrangement.place("c", 2000, 1000, &metrics(&devices)).unwrap();
        let before = arrangement.clone();

        assert_eq!(
            arrangement.translate(&group(&["a", "b"]), false, 0, 800, &metrics(&devices)),
            Err(PlaceError::Overlaps("c".into()))
        );
        assert_eq!(arrangement, before, "a refused move changes nothing");
    }

    #[test]
    fn members_of_a_group_do_not_collide_with_each_other_however_far_it_goes() {
        let devices = sizes(&[("a", (500, 500)), ("b", (500, 500))]);
        let mut arrangement = Arrangement::default();
        arrangement.place("a", 2000, 0, &metrics(&devices)).unwrap();
        arrangement.place("b", 2000, 500, &metrics(&devices)).unwrap();
        // Moving only "a" down would land on "b"; moving both is fine.
        assert!(arrangement.clone().translate(&group(&["a"]), false, 0, 300, &metrics(&devices)).is_err());
        assert!(arrangement.translate(&group(&["a", "b"]), false, 0, 300, &metrics(&devices)).is_ok());
    }

    #[test]
    fn this_computer_can_go_with_the_group_while_the_others_stay_put() {
        let devices = sizes(&[("a", (500, 500)), ("b", (500, 500))]);
        let mut arrangement = Arrangement::default();
        arrangement.place_beside_this_computer("a", Edge::Right, &metrics(&devices)).unwrap();
        arrangement.place("b", 0, 1080, &metrics(&devices)).unwrap();
        // "a" is beside this computer, which it would follow; here it is among those that stay.
        arrangement.translate(&[], true, 0, 2000, &metrics(&devices)).unwrap();

        let desktop = arrangement.desktop(&metrics(&devices));
        assert_eq!(desktop.rect("desk"), Some(Rect::new(0, 2000, 1920, 1080)));
        assert_eq!(desktop.rect("a"), Some(Rect::new(1920, 0, 500, 500)), "stayed where it was");
        assert_eq!(desktop.rect("b"), Some(Rect::new(0, 1080, 500, 500)));
    }

    #[test]
    fn a_selected_device_placed_beside_this_computer_becomes_a_fixed_place() {
        let devices = sizes(&[("a", (500, 500))]);
        let mut arrangement = Arrangement::default();
        arrangement.place_beside_this_computer("a", Edge::Right, &metrics(&devices)).unwrap();
        arrangement.translate(&group(&["a"]), false, 0, 600, &metrics(&devices)).unwrap();
        assert_eq!(arrangement.spot_of("a"), Some(Spot::At { x: 1920, y: 600 }));
    }

    #[test]
    fn unknown_names_in_a_group_are_ignored() {
        let devices = sizes(&[("a", (500, 500))]);
        let mut arrangement = Arrangement::default();
        arrangement.place("a", 2000, 0, &metrics(&devices)).unwrap();
        arrangement.translate(&group(&["a", "ghost"]), false, 10, 0, &metrics(&devices)).unwrap();
        assert_eq!(arrangement.spot_of("a"), Some(Spot::At { x: 2010, y: 0 }));
        assert!(!arrangement.contains("ghost"));
    }

    #[test]
    fn a_computer_with_two_displays_is_placed_by_the_corner_of_both() {
        let mut devices = BTreeMap::new();
        // As a computer reports them: its main display, and another to the left of it.
        devices.insert(
            "pc".to_owned(),
            vec![Rect::new(0, 0, 2560, 1440), Rect::new(-1920, 360, 1920, 1080)],
        );
        let mut arrangement = Arrangement::default();
        arrangement.place("pc", 1920, 0, &metrics(&devices)).unwrap();

        let desktop = arrangement.desktop(&metrics(&devices));
        assert_eq!(
            desktop.displays("pc"),
            Some([Rect::new(3840, 0, 2560, 1440), Rect::new(1920, 360, 1920, 1080)].as_slice())
        );
        assert_eq!(desktop.rect("pc"), Some(Rect::new(1920, 0, 4480, 1440)));
    }

    #[test]
    fn a_group_takes_up_the_whole_rectangle_around_its_displays() {
        let mut devices = sizes(&[("tablet", (500, 500))]);
        devices.insert("pc".to_owned(), vec![Rect::new(0, 0, 1000, 500), Rect::new(0, 500, 500, 500)]);
        let mut arrangement = Arrangement::default();
        arrangement.place("pc", 1920, 0, &metrics(&devices)).unwrap();

        // The empty corner of the L shape counts as part of the pc.
        assert_eq!(
            arrangement.place("tablet", 2420, 500, &metrics(&devices)),
            Err(PlaceError::Overlaps("pc".into()))
        );
        // Beside it is fine, touching the rectangle.
        assert!(arrangement.place("tablet", 2920, 500, &metrics(&devices)).is_ok());
    }

    #[test]
    fn beside_uses_the_size_of_all_the_displays() {
        let mut devices = BTreeMap::new();
        devices.insert("pc".to_owned(), vec![Rect::new(0, 0, 1000, 800), Rect::new(1000, 0, 1000, 600)]);
        let mut arrangement = Arrangement::default();
        arrangement.place_beside_this_computer("pc", Edge::Left, &metrics(&devices)).unwrap();
        assert_eq!(arrangement.desktop(&metrics(&devices)).rect("pc"), Some(Rect::new(-2000, 0, 2000, 800)));
    }

    #[test]
    fn displays_are_moved_to_start_at_the_origin() {
        let (moved, origin) = normalize(&[Rect::new(-1920, 100, 1920, 1080), Rect::new(0, 0, 2560, 1440)]);
        assert_eq!(origin, (-1920, 0));
        assert_eq!(moved, vec![Rect::new(0, 100, 1920, 1080), Rect::new(1920, 0, 2560, 1440)]);
        assert_eq!(normalize(&[]), (vec![], (0, 0)));
    }

    #[test]
    fn spots_have_a_stable_json_form() {
        let at = serde_json::to_string(&Spot::At { x: 5, y: -3 }).unwrap();
        let beside = serde_json::to_string(&Spot::Beside { edge: Edge::Left }).unwrap();
        assert_eq!(at, r#"{"kind":"at","x":5,"y":-3}"#);
        assert_eq!(beside, r#"{"kind":"beside","edge":"Left"}"#);
        assert_eq!(serde_json::from_str::<Spot>(&at).unwrap(), Spot::At { x: 5, y: -3 });
    }
}
