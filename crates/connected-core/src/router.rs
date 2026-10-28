//! Server-side state machine: decides which screen owns the cursor and what to do about
//! each local input event.
//!
//! The router does no I/O. It consumes [`InputEvent`]s and returns [`Action`]s for the
//! caller to carry out, which keeps it testable without a network or an OS.

use std::collections::BTreeMap;

use connected_proto::{ButtonState, Message, Modifiers};

use connected_proto::Rect;

use crate::arrangement::{Arrangement, Metrics, normalize};
use crate::desktop::{Desktop, Landing, Step};
use crate::platform::InputEvent;

#[derive(Debug, thiserror::Error)]
pub enum LayoutError {
    #[error("unknown screen: {0}")]
    UnknownScreen(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Start or stop suppressing local input.
    Capture(bool),
    /// Put the local cursor at this position.
    SetLocalCursor { x: i32, y: i32 },
    /// Deliver `msg` to the named screen's client.
    Send { to: String, msg: Message },
}

pub struct Router {
    desktop: Desktop,
    local: String,
    /// This computer's displays, measured from the corner of all of them.
    local_displays: Vec<Rect>,
    /// Screen currently owning the cursor; equals `local` when input stays on this machine.
    active: String,
    /// Cursor position on the active screen, relative to its top-left corner.
    pos: (i32, i32),
    seq: u32,
    modifiers: Modifiers,
}

impl Router {
    /// The cursor starts at the center of the local screen.
    pub fn new(desktop: Desktop, local: &str) -> Result<Self, LayoutError> {
        let displays = desktop
            .displays(local)
            .ok_or_else(|| LayoutError::UnknownScreen(local.to_owned()))?;
        let (local_displays, _) = normalize(displays);
        let pos = center_of(&local_displays);
        Ok(Self {
            local: local.to_owned(),
            local_displays,
            active: local.to_owned(),
            desktop,
            pos,
            seq: 0,
            modifiers: Modifiers::default(),
        })
    }

    pub fn active_screen(&self) -> &str {
        &self.active
    }

    /// Whether the cursor currently belongs to another machine.
    pub fn is_remote(&self) -> bool {
        self.active != self.local
    }

    /// Rebuilds the shared desktop from an arrangement and the sizes of the machines that are
    /// connected right now. Only connected machines take part, so a gap in a chain of screens
    /// is a dead end until it fills again.
    ///
    /// If the cursor was on a screen that is no longer there, control returns to the local
    /// screen and the actions for that are returned.
    pub fn rebuild(
        &mut self,
        arrangement: &Arrangement,
        connected: &BTreeMap<String, Vec<Rect>>,
    ) -> Vec<Action> {
        let metrics =
            Metrics { this_computer: &self.local, local: &self.local_displays, devices: connected };
        let desktop = arrangement.desktop_of(&metrics, |name| connected.contains_key(name));

        let actions = if self.is_remote() && desktop.rect(&self.active).is_none() {
            self.force_local()
        } else {
            Vec::new()
        };
        self.desktop = desktop;
        // A computer whose displays changed may no longer contain the cursor.
        self.pos = self.desktop.settle(&self.active, self.pos);
        actions
    }

    /// Tells the router where the real cursor is while input is local.
    pub fn set_local_cursor(&mut self, x: i32, y: i32) {
        if !self.is_remote() {
            self.pos = (x, y);
        }
    }

    /// Emergency return to the local screen, for the escape hotkey or a lost connection.
    /// The screen being left is told to let go of any keys it holds.
    pub fn force_local(&mut self) -> Vec<Action> {
        if !self.is_remote() {
            return Vec::new();
        }
        let center = center_of(&self.local_displays);
        let left = std::mem::replace(&mut self.active, self.local.clone());
        self.pos = center;
        vec![
            Action::Send { to: left, msg: Message::Leave },
            Action::Capture(false),
            Action::SetLocalCursor { x: center.0, y: center.1 },
        ]
    }

    pub fn handle(&mut self, event: InputEvent) -> Vec<Action> {
        match event {
            InputEvent::MouseMove { dx, dy } => self.on_move(dx, dy),
            InputEvent::MouseButton { button, state } => {
                self.forward(Message::MouseButton { button, state })
            }
            InputEvent::Wheel { dx, dy } => self.forward(Message::Wheel { dx, dy }),
            InputEvent::Key { usage, state } => {
                self.modifiers.set_usage(usage, state == ButtonState::Down);
                self.forward(Message::Key { usage, state })
            }
        }
    }

    fn forward(&self, msg: Message) -> Vec<Action> {
        if self.is_remote() {
            vec![Action::Send { to: self.active.clone(), msg }]
        } else {
            Vec::new()
        }
    }

    fn on_move(&mut self, dx: i32, dy: i32) -> Vec<Action> {
        let next = (self.pos.0.saturating_add(dx), self.pos.1.saturating_add(dy));

        match self.desktop.step(&self.active, self.pos, next) {
            Step::Cross(landing) => self.switch_to(landing),
            // Either way the cursor is still on this computer. At an edge with nothing beyond
            // it, it stops there.
            Step::Stay(x, y) | Step::Stop(x, y) => {
                self.pos = (x, y);
                self.forward(Message::MouseMove { dx, dy })
            }
        }
    }

    fn switch_to(&mut self, landing: Landing) -> Vec<Action> {
        let was_remote = self.is_remote();

        let mut actions = Vec::new();
        if was_remote {
            actions.push(Action::Send { to: self.active.clone(), msg: Message::Leave });
        }

        self.active = landing.screen.clone();
        self.pos = (landing.x, landing.y);

        if self.is_remote() {
            if !was_remote {
                actions.push(Action::Capture(true));
            }
            self.seq += 1;
            actions.push(Action::Send {
                to: landing.screen,
                msg: Message::Enter {
                    x: landing.x,
                    y: landing.y,
                    seq: self.seq,
                    modifiers: self.modifiers,
                },
            });
        } else {
            actions.push(Action::Capture(false));
            actions.push(Action::SetLocalCursor { x: landing.x, y: landing.y });
        }
        actions
    }
}

/// The middle of the first display, which is the main one.
fn center_of(displays: &[Rect]) -> (i32, i32) {
    displays.first().map_or((0, 0), |d| (d.x + d.width / 2, d.y + d.height / 2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use connected_proto::{Edge, MouseButton};

    const LOCAL: [Rect; 1] = [Rect::new(0, 0, 1920, 1080)];

    /// mac (1920x1080) | win (2560x1440) | linux (1920x1080), left to right, tops aligned.
    fn router() -> Router {
        let mut desktop = Desktop::new();
        desktop
            .add_one("mac", Rect::new(0, 0, 1920, 1080))
            .add_one("win", Rect::new(1920, 0, 2560, 1440))
            .add_one("linux", Rect::new(4480, 0, 1920, 1080));
        Router::new(desktop, "mac").unwrap()
    }

    fn mv(dx: i32, dy: i32) -> InputEvent {
        InputEvent::MouseMove { dx, dy }
    }

    fn send(to: &str, msg: Message) -> Action {
        Action::Send { to: to.into(), msg }
    }

    fn enter(x: i32, y: i32, seq: u32) -> Message {
        Message::Enter { x, y, seq, modifiers: Modifiers::default() }
    }

    #[test]
    fn movement_inside_the_local_screen_does_nothing() {
        let mut r = router();
        assert!(r.handle(mv(10, -4)).is_empty());
        assert_eq!(r.active_screen(), "mac");
    }

    #[test]
    fn unknown_local_screen_is_an_error() {
        assert!(matches!(
            Router::new(Desktop::new(), "mac"),
            Err(LayoutError::UnknownScreen(_))
        ));
    }

    #[test]
    fn crossing_the_edge_captures_input_and_enters_the_neighbor() {
        let mut r = router();
        r.set_local_cursor(1919, 540);

        let actions = r.handle(mv(5, 0));
        assert_eq!(actions, vec![Action::Capture(true), send("win", enter(0, 540, 1))]);
        assert_eq!(r.active_screen(), "win");
    }

    #[test]
    fn remote_input_is_forwarded() {
        let mut r = router();
        r.set_local_cursor(1919, 540);
        r.handle(mv(5, 0));

        assert_eq!(r.handle(mv(7, 3)), vec![send("win", Message::MouseMove { dx: 7, dy: 3 })]);
        assert_eq!(
            r.handle(InputEvent::MouseButton {
                button: MouseButton::Left,
                state: ButtonState::Down
            }),
            vec![send(
                "win",
                Message::MouseButton { button: MouseButton::Left, state: ButtonState::Down }
            )]
        );
        assert_eq!(
            r.handle(InputEvent::Key { usage: 0x04, state: ButtonState::Down }),
            vec![send("win", Message::Key { usage: 0x04, state: ButtonState::Down })]
        );
    }

    #[test]
    fn local_buttons_and_keys_are_not_forwarded() {
        let mut r = router();
        assert!(r
            .handle(InputEvent::Key { usage: 0x04, state: ButtonState::Down })
            .is_empty());
        assert!(r.handle(InputEvent::Wheel { dx: 0, dy: 1 }).is_empty());
    }

    #[test]
    fn walking_back_returns_to_the_local_screen() {
        let mut r = router();
        r.set_local_cursor(1919, 540);
        r.handle(mv(5, 0));

        let actions = r.handle(mv(-3, 0));
        assert_eq!(
            actions,
            vec![
                send("win", Message::Leave),
                Action::Capture(false),
                Action::SetLocalCursor { x: 1919, y: 540 },
            ]
        );
        assert_eq!(r.active_screen(), "mac");
    }

    #[test]
    fn hopping_between_remote_screens_keeps_capture_on() {
        let mut r = router();
        r.set_local_cursor(1919, 540);
        r.handle(mv(5, 0));

        // Cross win's right edge (x = 2560) from its entry point at x = 0.
        let actions = r.handle(mv(2565, 0));
        assert_eq!(actions, vec![send("win", Message::Leave), send("linux", enter(0, 540, 2))]);
        assert_eq!(r.active_screen(), "linux");
    }

    #[test]
    fn edge_without_a_neighbor_stops_the_cursor() {
        let mut r = router();
        r.set_local_cursor(5, 5);
        assert!(r.handle(mv(-100, 0)).is_empty());
        assert!(r.handle(mv(0, -100)).is_empty());
        assert_eq!(r.active_screen(), "mac");
    }

    #[test]
    fn the_part_of_an_edge_that_faces_nothing_stops_the_cursor() {
        let mut r = router();
        // Win is taller than mac, so mac's right edge is fully covered, but linux ends at 1080.
        r.set_local_cursor(1919, 540);
        r.handle(mv(5, 0));
        r.handle(mv(0, 700)); // down to y = 1240 on win
        assert_eq!(r.handle(mv(2565, 0)), vec![send("win", Message::MouseMove { dx: 2565, dy: 0 })]);
        assert_eq!(r.active_screen(), "win", "linux does not reach down to y = 1240");
    }

    #[test]
    fn enter_reports_held_modifiers() {
        let mut r = router();
        r.set_local_cursor(1919, 540);
        r.handle(InputEvent::Key { usage: 0xE1, state: ButtonState::Down }); // LShift

        let actions = r.handle(mv(5, 0));
        let Action::Send { msg: Message::Enter { modifiers, .. }, .. } = &actions[1] else {
            panic!("expected Enter, got {actions:?}");
        };
        assert!(modifiers.shift());
        assert!(!modifiers.ctrl());
    }

    #[test]
    fn force_local_recovers_from_a_remote_screen() {
        let mut r = router();
        r.set_local_cursor(1919, 540);
        r.handle(mv(5, 0));

        let actions = r.force_local();
        assert_eq!(
            actions,
            vec![
                send("win", Message::Leave),
                Action::Capture(false),
                Action::SetLocalCursor { x: 960, y: 540 },
            ]
        );
        assert_eq!(r.active_screen(), "mac");
        assert!(r.force_local().is_empty());
    }

    /// A router that knows only the local screen, as the real server starts out.
    fn bare_router() -> Router {
        let mut desktop = Desktop::new();
        desktop.add_one("mac", Rect::new(0, 0, 1920, 1080));
        Router::new(desktop, "mac").unwrap()
    }

    fn sizes(connected: &[(&str, (i32, i32))]) -> BTreeMap<String, Vec<Rect>> {
        connected
            .iter()
            .map(|(name, size)| ((*name).to_owned(), vec![Rect::new(0, 0, size.0, size.1)]))
            .collect()
    }

    fn metrics(devices: &BTreeMap<String, Vec<Rect>>) -> Metrics<'_> {
        Metrics { this_computer: "mac", local: &LOCAL, devices }
    }

    /// Devices flush against the "mac" screen, sized like the real ones used below.
    fn arranged(placements: &[(&str, Edge)]) -> Arrangement {
        let known = sizes(&[
            ("win", (2560, 1440)),
            ("linux", (1920, 1080)),
            ("tv", (960, 540)),
            ("mac", (100, 100)),
        ]);
        let mut arrangement = Arrangement::default();
        for (device, edge) in placements {
            arrangement.place_beside_this_computer(device, *edge, &metrics(&known)).unwrap();
        }
        arrangement
    }

    /// Devices at fixed positions, sized like the real ones used below.
    fn placed(placements: &[(&str, i32, i32)]) -> Arrangement {
        let known = sizes(&[("win", (2560, 1440)), ("linux", (1920, 1080)), ("tv", (960, 540))]);
        let mut arrangement = Arrangement::default();
        for (device, x, y) in placements {
            arrangement.place(device, *x, *y, &metrics(&known)).unwrap();
        }
        arrangement
    }

    #[test]
    fn an_edge_leads_nowhere_until_the_client_connects() {
        let mut r = bare_router();
        let arrangement = arranged(&[("win", Edge::Right)]);

        // Arranged, but not connected.
        r.rebuild(&arrangement, &sizes(&[]));
        r.set_local_cursor(1919, 540);
        assert!(r.handle(mv(5, 0)).is_empty());
        assert!(!r.is_remote());

        r.rebuild(&arrangement, &sizes(&[("win", (2560, 1440))]));
        r.set_local_cursor(1919, 540);
        let actions = r.handle(mv(5, 0));
        assert_eq!(actions[0], Action::Capture(true));
        assert!(r.is_remote());
    }

    #[test]
    fn a_client_on_the_left_is_reached_by_going_left() {
        let mut r = bare_router();
        r.rebuild(&arranged(&[("tv", Edge::Left)]), &sizes(&[("tv", (960, 540))]));
        r.set_local_cursor(0, 270);

        let actions = r.handle(mv(-5, 0));
        assert_eq!(actions[1], send("tv", enter(959, 270, 1)));
    }

    #[test]
    fn a_smaller_client_is_not_reached_below_where_it_ends() {
        let mut r = bare_router();
        r.rebuild(&arranged(&[("tv", Edge::Left)]), &sizes(&[("tv", (960, 540))]));
        r.set_local_cursor(0, 800);

        assert!(r.handle(mv(-5, 0)).is_empty());
        assert!(!r.is_remote());
    }

    #[test]
    fn a_client_above_is_reached_by_going_up() {
        let mut r = bare_router();
        r.rebuild(&placed(&[("win", 0, -1440)]), &sizes(&[("win", (2560, 1440))]));
        r.set_local_cursor(960, 0);

        let actions = r.handle(mv(0, -5));
        assert_eq!(actions[1], send("win", enter(960, 1439, 1)));
    }

    #[test]
    fn a_client_placed_off_center_is_entered_at_the_matching_height() {
        let mut r = bare_router();
        r.rebuild(&placed(&[("win", 1920, 300)]), &sizes(&[("win", (2560, 1440))]));

        r.set_local_cursor(1919, 100);
        assert!(r.handle(mv(5, 0)).is_empty(), "above where win starts");

        r.set_local_cursor(1919, 400);
        let actions = r.handle(mv(5, 0));
        assert_eq!(actions[1], send("win", enter(0, 100, 1)));
    }

    #[test]
    fn screens_can_be_chained_through_a_middle_client() {
        let mut r = bare_router();
        let arrangement = placed(&[("win", 1920, 0), ("linux", 4480, 0)]);
        r.rebuild(&arrangement, &sizes(&[("win", (2560, 1440)), ("linux", (1920, 1080))]));

        r.set_local_cursor(1919, 540);
        r.handle(mv(5, 0));
        assert_eq!(r.active_screen(), "win");

        // 2565 steps right from the win screen's left edge crosses its right edge.
        r.handle(mv(2565, 0));
        assert_eq!(r.active_screen(), "linux");
    }

    #[test]
    fn a_gap_in_a_chain_is_a_dead_end() {
        let mut r = bare_router();
        let arrangement = placed(&[("win", 1920, 0), ("linux", 4480, 0)]);
        // Only the far screen is connected: the middle one is missing.
        r.rebuild(&arrangement, &sizes(&[("linux", (1920, 1080))]));

        r.set_local_cursor(1919, 540);
        assert!(r.handle(mv(5, 0)).is_empty());
        assert!(!r.is_remote());
    }

    #[test]
    fn a_gap_between_placed_screens_is_a_dead_end() {
        let mut r = bare_router();
        r.rebuild(&placed(&[("win", 2000, 0)]), &sizes(&[("win", (2560, 1440))]));

        r.set_local_cursor(1919, 540);
        assert!(r.handle(mv(5, 0)).is_empty());
        assert!(!r.is_remote());
    }

    #[test]
    fn losing_the_screen_the_cursor_is_on_brings_control_home() {
        let mut r = bare_router();
        let arrangement = arranged(&[("win", Edge::Right)]);
        r.rebuild(&arrangement, &sizes(&[("win", (2560, 1440))]));
        r.set_local_cursor(1919, 540);
        r.handle(mv(5, 0));

        let actions = r.rebuild(&arrangement, &sizes(&[]));
        assert_eq!(
            actions,
            vec![
                send("win", Message::Leave),
                Action::Capture(false),
                Action::SetLocalCursor { x: 960, y: 540 },
            ]
        );
        assert!(!r.is_remote());

        // And the edge is a dead end again.
        r.set_local_cursor(1919, 540);
        assert!(r.handle(mv(5, 0)).is_empty());
    }

    #[test]
    fn losing_another_screen_leaves_the_active_one_alone() {
        let mut r = bare_router();
        let arrangement = arranged(&[("win", Edge::Right), ("linux", Edge::Left)]);
        let both = sizes(&[("win", (2560, 1440)), ("linux", (1920, 1080))]);
        r.rebuild(&arrangement, &both);
        r.set_local_cursor(1919, 540);
        r.handle(mv(5, 0));

        assert!(r.rebuild(&arrangement, &sizes(&[("win", (2560, 1440))])).is_empty());
        assert_eq!(r.active_screen(), "win");
    }

    #[test]
    fn moving_the_screen_the_cursor_is_on_to_another_side_keeps_it_there() {
        let mut r = bare_router();
        let win = sizes(&[("win", (2560, 1440))]);
        r.rebuild(&arranged(&[("win", Edge::Right)]), &win);
        r.set_local_cursor(1919, 540);
        r.handle(mv(5, 0));
        assert!(r.is_remote());

        // The same screen, now on the left. The cursor is still on it, so it stays there.
        assert!(r.rebuild(&arranged(&[("win", Edge::Left)]), &win).is_empty());
        assert_eq!(r.active_screen(), "win");

        // Going back toward the desk now means going right, off the win screen's right edge.
        let actions = r.handle(mv(3000, 0));
        assert_eq!(r.active_screen(), "mac");
        assert!(actions.contains(&Action::Capture(false)));
    }

    #[test]
    fn a_resized_screen_changes_how_much_of_the_edge_leads_to_it() {
        let mut r = bare_router();
        let arrangement = arranged(&[("win", Edge::Right)]);
        r.rebuild(&arrangement, &sizes(&[("win", (2560, 1440))]));
        r.set_local_cursor(1919, 900);
        assert!(!r.handle(mv(5, 0)).is_empty(), "y = 900 is on a 1440-tall screen");
        r.force_local();

        r.rebuild(&arrangement, &sizes(&[("win", (1280, 720))]));
        r.set_local_cursor(1919, 900);
        assert!(r.handle(mv(5, 0)).is_empty(), "but not on a 720-tall one");

        r.set_local_cursor(1919, 540);
        let actions = r.handle(mv(5, 0));
        assert_eq!(actions[1], send("win", enter(0, 540, 2)));
    }

    #[test]
    fn a_resize_that_leaves_the_cursor_outside_pulls_it_back_in() {
        let mut r = bare_router();
        let arrangement = arranged(&[("win", Edge::Right)]);
        r.rebuild(&arrangement, &sizes(&[("win", (2560, 1440))]));
        r.set_local_cursor(1919, 1000);
        r.handle(mv(5, 0));
        r.handle(mv(100, 0)); // still on win, y = 1000

        r.rebuild(&arrangement, &sizes(&[("win", (1280, 720))]));
        // Moving 0 keeps the clamped position: now on the bottom row of the smaller screen.
        let actions = r.handle(mv(0, 1));
        assert_eq!(actions, vec![send("win", Message::MouseMove { dx: 0, dy: 1 })]);
        assert_eq!(r.active_screen(), "win");
    }

    #[test]
    fn a_device_named_like_this_computer_is_left_out() {
        let mut r = bare_router();
        let arrangement = arranged(&[("mac", Edge::Right)]);
        r.rebuild(&arrangement, &sizes(&[("mac", (100, 100))]));

        r.set_local_cursor(1919, 540);
        assert!(r.handle(mv(5, 0)).is_empty());
    }

    /// A computer with a main display and a smaller one to its right, lower down.
    fn two_displays() -> BTreeMap<String, Vec<Rect>> {
        BTreeMap::from([(
            "pc".to_owned(),
            vec![Rect::new(0, 0, 2560, 1440), Rect::new(2560, 400, 1920, 1080)],
        )])
    }

    #[test]
    fn the_cursor_crosses_between_the_displays_of_a_client_without_leaving_it() {
        let mut r = bare_router();
        r.rebuild(&placed(&[("pc", 1920, 0)]), &two_displays());
        r.set_local_cursor(1919, 600);
        r.handle(mv(5, 0));
        assert_eq!(r.active_screen(), "pc");

        // Straight across the main display and onto the second one: still the client's own.
        assert_eq!(r.handle(mv(2560, 0)), vec![send("pc", Message::MouseMove { dx: 2560, dy: 0 })]);
        assert_eq!(r.active_screen(), "pc");
    }

    #[test]
    fn the_cursor_stops_where_a_client_display_has_nothing_beyond_it() {
        let mut r = bare_router();
        r.rebuild(&placed(&[("pc", 1920, 0)]), &two_displays());
        r.set_local_cursor(1919, 100);
        r.handle(mv(5, 0));

        // The second display starts at y = 400, so above that the main display's right edge
        // faces nothing. The cursor stops, and the client is told to move by the same amount.
        let actions = r.handle(mv(3000, 0));
        assert_eq!(actions, vec![send("pc", Message::MouseMove { dx: 3000, dy: 0 })]);
        assert_eq!(r.active_screen(), "pc");
        // Back at the left it returns to this computer: x was held at the edge, not 3000.
        r.handle(mv(-2570, 0));
        assert_eq!(r.active_screen(), "mac");
    }

    #[test]
    fn a_client_with_two_displays_hands_the_cursor_back_from_its_main_display() {
        let mut r = bare_router();
        r.rebuild(&placed(&[("pc", 1920, 0)]), &two_displays());
        r.set_local_cursor(1919, 900);
        let actions = r.handle(mv(5, 0));
        assert_eq!(actions[1], send("pc", enter(0, 900, 1)));
        let back = r.handle(mv(-5, 0));
        assert!(back.contains(&Action::SetLocalCursor { x: 1919, y: 900 }));
    }

    #[test]
    fn leaving_through_the_second_display_enters_the_next_computer_at_the_matching_height() {
        let mut r = bare_router();
        let arrangement = placed(&[("pc", 1920, 0), ("tv", 6400, 700)]);
        let mut connected = two_displays();
        connected.extend(sizes(&[("tv", (960, 540))]));
        r.rebuild(&arrangement, &connected);

        r.set_local_cursor(1919, 900);
        r.handle(mv(5, 0)); // enters pc at (0, 900)
        r.handle(mv(2561, 0)); // onto its second display, at x = 2566
        let actions = r.handle(mv(1930, 0)); // off its right edge at x = 4480
        assert_eq!(actions, vec![send("pc", Message::Leave), send("tv", enter(0, 200, 2))]);
    }

    #[test]
    fn this_computer_can_have_several_displays_too() {
        let mut desktop = Desktop::new();
        desktop.add("mac", vec![Rect::new(0, 0, 1920, 1080), Rect::new(1920, 0, 1000, 1080)]);
        let mut r = Router::new(desktop, "mac").unwrap();
        r.set_local_cursor(1910, 500);
        assert!(r.handle(mv(100, 0)).is_empty(), "onto its own second display");
        assert!(!r.is_remote());

        // The arrangement and the pc beside both displays together.
        let devices = two_displays();
        let local = [Rect::new(0, 0, 1920, 1080), Rect::new(1920, 0, 1000, 1080)];
        let mut arrangement = Arrangement::default();
        let metrics = Metrics { this_computer: "mac", local: &local, devices: &devices };
        arrangement.place_beside_this_computer("pc", Edge::Right, &metrics).unwrap();
        r.rebuild(&arrangement, &devices);
        r.set_local_cursor(2915, 500);
        let actions = r.handle(mv(10, 0));
        assert_eq!(actions[1], send("pc", enter(0, 500, 1)));
    }

    #[test]
    fn a_new_arrangement_pulls_a_stranded_cursor_back_onto_a_display() {
        let mut r = bare_router();
        r.rebuild(&placed(&[("pc", 1920, 0)]), &two_displays());
        r.set_local_cursor(1919, 900);
        r.handle(mv(5, 0));
        r.handle(mv(2561, 0)); // on the second display, which then goes away

        let one = sizes(&[("pc", (1280, 720))]);
        r.rebuild(&placed(&[("pc", 1920, 0)]), &one);
        // Still on pc, and the next move starts from a position that exists.
        assert_eq!(r.active_screen(), "pc");
        assert_eq!(r.handle(mv(0, 1)), vec![send("pc", Message::MouseMove { dx: 0, dy: 1 })]);
    }
}
