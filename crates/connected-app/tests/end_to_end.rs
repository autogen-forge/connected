//! Computers talking over real TLS connections on localhost, with in-memory input backends
//! standing in for the operating system. Every computer is the same: any can reach any other.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use connected_app::{AppError, Log, MeshBackends, MeshHandle, MeshStart};
use connected_core::{Arrangement, InputEvent, Metrics, Rect};
use connected_net::{Identity, Node, TrustStore};
use connected_platform::mock::{ChannelSource, FixedScreen, SharedRecordingSink, SinkCall};
use connected_proto::{ButtonState, Edge, MouseButton};

const DESK_SCREEN: (i32, i32) = (1920, 1080);
const LAPTOP_SCREEN: (i32, i32) = (2560, 1440);

#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<String>>>);

impl Logs {
    fn writer(&self, who: &'static str) -> Log {
        let lines = Arc::clone(&self.0);
        Arc::new(move |line: &str| lines.lock().unwrap().push(format!("{who}: {line}")))
    }

    fn all(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

fn wait_until(what: &str, logs: &Logs, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}\nlog: {:#?}", logs.all());
        thread::sleep(Duration::from_millis(5));
    }
}

fn new_node(name: &str) -> Node {
    Node::new(Identity::generate(name).unwrap(), TrustStore::in_memory())
}

/// Pairs two nodes so that each trusts the other.
fn pair(a: &Node, b: &Node) {
    let listener = a.listen("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let waiting = thread::spawn(move || listener.accept_for_pairing(|_| true));
    b.pair_with(&addr, |_| true).unwrap();
    waiting.join().unwrap().unwrap();
}

/// One computer running the mesh.
struct Pc {
    handle: MeshHandle,
    /// Local input on this computer.
    input: Sender<InputEvent>,
    /// Whether this computer is withholding input from its own desktop.
    capturing: Arc<AtomicBool>,
    /// Everything put into this computer: what others sent, and its own cursor.
    sink: SharedRecordingSink,
    addr: String,
}

fn start_pc_at(
    node: &Node,
    listen: &str,
    screen: FixedScreen,
    start: MeshStart,
    who: &'static str,
    logs: &Logs,
) -> Pc {
    let (source, input, capturing) = ChannelSource::new();
    let sink = SharedRecordingSink::default();
    let own = sink.clone();
    let handle = MeshHandle::start(
        node.clone(),
        listen,
        start,
        move || {
            Ok(MeshBackends { source: Box::new(source), sink: Box::new(own), screen: Box::new(screen) })
        },
        logs.writer(who),
    )
    .expect("the mesh starts");
    let addr = handle.state().listening_on;
    Pc { handle, input, capturing, sink, addr }
}

fn start_pc(node: &Node, screen: (i32, i32), start: MeshStart, who: &'static str, logs: &Logs) -> Pc {
    start_pc_at(node, "127.0.0.1:0", FixedScreen::new(screen.0, screen.1), start, who, logs)
}

impl Pc {
    fn send(&self, event: InputEvent) {
        self.input.send(event).unwrap();
    }

    fn is_capturing(&self) -> bool {
        self.capturing.load(Ordering::SeqCst)
    }

    fn sees(&self, name: &str) -> bool {
        self.handle.state().peers.iter().any(|peer| peer.name == name)
    }

    fn active_screen(&self) -> String {
        self.handle.state().active_screen
    }

    fn controlled_by(&self) -> Option<String> {
        self.handle.state().controlled_by
    }

    fn calls(&self) -> Vec<SinkCall> {
        self.sink.calls()
    }

    fn knows_address_of(&self, name: &str, other: &Pc) {
        self.handle.set_peer_address(name, Some(other.addr.clone()));
    }
}

/// Lets two computers find each other, each given by its name and itself. Only one of them
/// dials, whichever fingerprint sorts lower; both are told where the other is, as a program
/// that paired with both would have.
fn introduce(a: (&str, &Pc), b: (&str, &Pc)) {
    a.1.knows_address_of(b.0, b.1);
    b.1.knows_address_of(a.0, a.1);
}

fn desk_metrics_arrangement(placements: &[(&str, Edge)]) -> Arrangement {
    let devices = BTreeMap::new();
    let local = [Rect::new(0, 0, DESK_SCREEN.0, DESK_SCREEN.1)];
    let metrics = Metrics { this_computer: "desk", local: &local, devices: &devices };
    let mut arrangement = Arrangement::default();
    for (device, edge) in placements {
        arrangement.place_beside_this_computer(device, *edge, &metrics).unwrap();
    }
    arrangement
}

fn desk_layout(placements: &[(&str, Edge)]) -> MeshStart {
    MeshStart { arrangement: desk_metrics_arrangement(placements), layout_version: 1, ..MeshStart::default() }
}

fn move_mouse(dx: i32, dy: i32) -> InputEvent {
    InputEvent::MouseMove { dx, dy }
}

fn key(usage: u16, state: ButtonState) -> InputEvent {
    InputEvent::Key { usage, state }
}

fn moved_abs_to_x(pc: &Pc, x: i32) -> bool {
    pc.calls().iter().any(|call| matches!(call, SinkCall::MoveAbs(at, _) if *at == x))
}

struct Pair {
    desk: Pc,
    laptop: Pc,
    logs: Logs,
    desk_node: Node,
}

/// A desk and a laptop, paired and connected. The layout is made on the desk, with the laptop
/// beside it on `edge`; the laptop learns it from the desk.
fn linked(edge: Edge) -> Pair {
    let (desk_node, laptop_node) = (new_node("desk"), new_node("laptop"));
    pair(&desk_node, &laptop_node);
    let logs = Logs::default();
    let desk = start_pc(&desk_node, DESK_SCREEN, desk_layout(&[("laptop", edge)]), "desk", &logs);
    let laptop = start_pc(&laptop_node, LAPTOP_SCREEN, MeshStart::default(), "laptop", &logs);
    introduce(("desk", &desk), ("laptop", &laptop));
    wait_until("both to see each other", &logs, || desk.sees("laptop") && laptop.sees("desk"));
    wait_until("the laptop to learn the layout", &logs, || laptop.handle.state().layout_version == 1);
    Pair { desk, laptop, logs, desk_node }
}

#[test]
fn crossing_the_edge_hands_the_cursor_to_the_other_computer_and_back() {
    let Pair { desk, laptop, logs, .. } = linked(Edge::Right);
    assert_eq!(desk.active_screen(), "desk");

    // Push the cursor off the right edge of the desk (it starts in the middle).
    desk.send(move_mouse(1000, 0));
    wait_until("the cursor to appear on the laptop", &logs, || laptop.calls().contains(&SinkCall::MoveAbs(0, 540)));
    assert!(desk.is_capturing(), "the desk must stop receiving its own input");
    assert!(laptop.is_capturing(), "the laptop is held back while it is controlled");
    assert_eq!(desk.active_screen(), "laptop");
    assert_eq!(laptop.controlled_by().as_deref(), Some("desk"));

    // From here on, everything the desk's user does happens on the laptop.
    desk.send(move_mouse(5, 3));
    desk.send(key(0x04, ButtonState::Down));
    desk.send(key(0x04, ButtonState::Up));
    desk.send(InputEvent::MouseButton { button: MouseButton::Left, state: ButtonState::Down });
    desk.send(InputEvent::MouseButton { button: MouseButton::Left, state: ButtonState::Up });
    desk.send(InputEvent::Wheel { dx: 0, dy: -120 });
    wait_until("the laptop to receive the input", &logs, || laptop.calls().len() >= 7);
    assert_eq!(
        laptop.calls()[1..],
        [
            SinkCall::MoveRel(5, 3),
            SinkCall::Key(0x04, ButtonState::Down),
            SinkCall::Key(0x04, ButtonState::Up),
            SinkCall::Button(MouseButton::Left, ButtonState::Down),
            SinkCall::Button(MouseButton::Left, ButtonState::Up),
            SinkCall::Wheel(0, -120),
        ]
    );

    // Walk back across the left edge of the laptop.
    desk.send(move_mouse(-100, 0));
    wait_until("control to come back to the desk", &logs, || !desk.is_capturing());
    wait_until("the laptop to be its own again", &logs, || !laptop.is_capturing());
    assert_eq!(laptop.calls().last(), Some(&SinkCall::ReleaseAll));
    assert_eq!(laptop.controlled_by(), None);
    // The laptop's cursor had moved down 3 to y = 543, which is the same height on the desk,
    // at the desk's right edge.
    wait_until("the desk's cursor to be placed", &logs, || desk.calls().contains(&SinkCall::MoveAbs(1919, 543)));
    assert_eq!(desk.active_screen(), "desk");
}

#[test]
fn either_computer_can_be_the_one_to_cross() {
    let Pair { desk, laptop, logs, .. } = linked(Edge::Right);

    // The laptop's cursor starts in the middle of it; push it off its left edge, onto the desk.
    laptop.send(move_mouse(-3000, 0));
    wait_until("the cursor to appear on the desk", &logs, || moved_abs_to_x(&desk, DESK_SCREEN.0 - 1));
    assert!(laptop.is_capturing());
    assert!(desk.is_capturing());
    assert_eq!(laptop.active_screen(), "desk");
    assert_eq!(desk.controlled_by().as_deref(), Some("laptop"));

    laptop.send(key(0x05, ButtonState::Down));
    wait_until("the desk to receive the key", &logs, || desk.calls().contains(&SinkCall::Key(0x05, ButtonState::Down)));
}

#[test]
fn a_computer_on_the_left_is_reached_by_going_left() {
    let Pair { desk, laptop, logs, .. } = linked(Edge::Left);

    desk.send(move_mouse(-1000, 0));
    wait_until("the cursor to appear on the laptop", &logs, || {
        laptop.calls().contains(&SinkCall::MoveAbs(LAPTOP_SCREEN.0 - 1, 540))
    });
}

#[test]
fn whoever_got_there_first_keeps_priority_until_the_cursor_comes_back() {
    let Pair { desk, laptop, logs, .. } = linked(Edge::Right);
    desk.send(move_mouse(1000, 0));
    wait_until("the laptop to be controlled", &logs, || laptop.controlled_by().is_some());
    let before = laptop.calls().len();

    // The person at the laptop moves their mouse and types; none of it is acted on, and it does
    // not take the desk.
    laptop.send(move_mouse(-3000, 0));
    laptop.send(key(0x04, ButtonState::Down));
    thread::sleep(Duration::from_millis(300));
    assert_eq!(laptop.active_screen(), "laptop");
    assert_eq!(desk.controlled_by(), None);
    assert_eq!(desk.active_screen(), "laptop", "the desk still has the cursor");
    assert_eq!(laptop.calls().len(), before, "the laptop's own input does not reach its own desktop");
}

#[test]
fn the_person_at_the_controlled_computer_can_take_it_back_with_the_escape_chord() {
    let Pair { desk, laptop, logs, .. } = linked(Edge::Right);
    desk.send(move_mouse(1000, 0));
    wait_until("the laptop to be controlled", &logs, || laptop.controlled_by().is_some());

    for modifier in [0xE0, 0xE1, 0xE2] {
        laptop.send(key(modifier, ButtonState::Down));
    }
    laptop.send(key(0x29, ButtonState::Down));

    wait_until("the laptop to be its own again", &logs, || laptop.controlled_by().is_none() && !laptop.is_capturing());
    wait_until("the desk to get the cursor back", &logs, || !desk.is_capturing());
    assert_eq!(desk.active_screen(), "desk");
}

#[test]
fn the_escape_chord_gets_the_desk_back_without_leaking_into_the_laptop() {
    let Pair { desk, laptop, logs, .. } = linked(Edge::Right);
    desk.send(move_mouse(1000, 0));
    wait_until("the cursor to appear on the laptop", &logs, || !laptop.calls().is_empty());

    // Ctrl, Shift and Alt, then Escape.
    for modifier in [0xE0, 0xE1, 0xE2] {
        desk.send(key(modifier, ButtonState::Down));
    }
    desk.send(key(0x29, ButtonState::Down));

    wait_until("control to come back to the desk", &logs, || !desk.is_capturing());
    wait_until("the laptop to let go of its keys", &logs, || laptop.calls().last() == Some(&SinkCall::ReleaseAll));
    assert!(
        !laptop.calls().contains(&SinkCall::Key(0x29, ButtonState::Down)),
        "the chord's Escape belongs to the desk"
    );
    wait_until("the desk's cursor to be placed", &logs, || desk.calls().contains(&SinkCall::MoveAbs(960, 540)));
}

#[test]
fn losing_the_computer_that_has_the_cursor_gives_control_back() {
    let Pair { desk, mut laptop, logs, .. } = linked(Edge::Right);
    desk.send(move_mouse(1000, 0));
    wait_until("the cursor to appear on the laptop", &logs, || !laptop.calls().is_empty());
    assert!(desk.is_capturing());

    laptop.handle.stop();

    wait_until("the desk to notice and take control back", &logs, || !desk.is_capturing());
    wait_until("the desk's cursor to be placed", &logs, || desk.calls().contains(&SinkCall::MoveAbs(960, 540)));
    assert!(!desk.sees("laptop"));
    assert_eq!(desk.active_screen(), "desk");
}

#[test]
fn losing_the_computer_that_controls_this_one_frees_it() {
    let Pair { mut desk, laptop, logs, .. } = linked(Edge::Right);
    desk.send(move_mouse(1000, 0));
    wait_until("the laptop to be controlled", &logs, || laptop.controlled_by().is_some());

    desk.handle.stop();

    wait_until("the laptop to be its own again", &logs, || laptop.controlled_by().is_none());
    wait_until("capture to be off", &logs, || !laptop.is_capturing());
    assert_eq!(laptop.calls().last(), Some(&SinkCall::ReleaseAll));
}

#[test]
fn an_edge_with_nothing_connected_behind_it_is_a_dead_end() {
    let desk_node = new_node("desk");
    let logs = Logs::default();
    let desk = start_pc(&desk_node, DESK_SCREEN, desk_layout(&[("laptop", Edge::Right)]), "desk", &logs);

    desk.send(move_mouse(1000, 0));
    thread::sleep(Duration::from_millis(300));
    assert!(!desk.is_capturing(), "nothing is there to receive the input");
}

#[test]
fn a_paired_device_that_is_not_placed_connects_but_cannot_be_reached_until_it_is() {
    let (desk_node, laptop_node) = (new_node("desk"), new_node("laptop"));
    pair(&desk_node, &laptop_node);
    let logs = Logs::default();
    let desk = start_pc(&desk_node, DESK_SCREEN, MeshStart::default(), "desk", &logs);
    let laptop = start_pc(&laptop_node, LAPTOP_SCREEN, MeshStart::default(), "laptop", &logs);
    introduce(("desk", &desk), ("laptop", &laptop));
    wait_until("both to connect", &logs, || desk.sees("laptop") && laptop.sees("desk"));

    // Connected, so it shows up; but no edge leads to it.
    desk.send(move_mouse(1000, 0));
    thread::sleep(Duration::from_millis(200));
    assert!(!desk.is_capturing());
    assert!(laptop.calls().is_empty());

    // Drag it into the layout while both run.
    desk.handle.set_arrangement(desk_metrics_arrangement(&[("laptop", Edge::Right)]), connected_app::now_ms());
    thread::sleep(Duration::from_millis(100));
    desk.send(move_mouse(1000, 0));
    wait_until("the cursor to appear on the laptop", &logs, || laptop.calls().contains(&SinkCall::MoveAbs(0, 540)));
}

#[test]
fn moving_a_device_to_another_side_while_running_moves_the_way_to_it() {
    let Pair { desk, laptop, logs, .. } = linked(Edge::Right);

    desk.handle.set_arrangement(desk_metrics_arrangement(&[("laptop", Edge::Left)]), connected_app::now_ms());
    thread::sleep(Duration::from_millis(100));

    // The right edge no longer leads anywhere.
    desk.send(move_mouse(1000, 0));
    thread::sleep(Duration::from_millis(150));
    assert!(!desk.is_capturing());
    assert!(laptop.calls().is_empty());

    // The left edge does.
    desk.send(move_mouse(-3000, 0));
    wait_until("the cursor to appear on the laptop", &logs, || {
        laptop.calls().contains(&SinkCall::MoveAbs(LAPTOP_SCREEN.0 - 1, 540))
    });
}

#[test]
fn removing_the_device_the_cursor_is_on_brings_control_home() {
    let Pair { desk, laptop, logs, .. } = linked(Edge::Right);
    desk.send(move_mouse(1000, 0));
    wait_until("the cursor to appear on the laptop", &logs, || !laptop.calls().is_empty());

    desk.handle.set_arrangement(Arrangement::default(), connected_app::now_ms());

    wait_until("control to come back to the desk", &logs, || !desk.is_capturing());
    assert_eq!(desk.active_screen(), "desk");
    wait_until("the laptop to let go", &logs, || laptop.calls().last() == Some(&SinkCall::ReleaseAll));
}

#[test]
fn a_layout_change_on_either_computer_reaches_the_other() {
    let Pair { desk, laptop, logs, .. } = linked(Edge::Right);

    // What the desk made, the laptop has: the desk at the origin, the laptop beside it.
    let seen = laptop.handle.state().arrangement;
    assert_eq!((seen.local.x, seen.local.y), (DESK_SCREEN.0, 0), "the laptop is beside the desk");
    assert!(seen.contains("desk"));

    // The person at the laptop moves the desk; the desk follows.
    let mut moved = laptop.handle.state().arrangement;
    let devices = BTreeMap::from([("desk".to_owned(), vec![Rect::new(0, 0, DESK_SCREEN.0, DESK_SCREEN.1)])]);
    let local = [Rect::new(0, 0, LAPTOP_SCREEN.0, LAPTOP_SCREEN.1)];
    let metrics = Metrics { this_computer: "laptop", local: &local, devices: &devices };
    moved.place("desk", 0, 1500, &metrics).unwrap();
    laptop.handle.set_arrangement(moved, connected_app::now_ms());
    wait_until("the desk to follow", &logs, || {
        let state = desk.handle.state();
        (state.arrangement.local.x, state.arrangement.local.y) == (0, 1500) && state.layout_version > 1
    });
    let on_desk = desk.handle.state();
    assert!(on_desk.arrangement.contains("laptop"));
}

#[test]
fn the_newest_layout_wins_when_computers_meet() {
    let (desk_node, laptop_node) = (new_node("desk"), new_node("laptop"));
    pair(&desk_node, &laptop_node);
    let logs = Logs::default();
    let desk = start_pc(&desk_node, DESK_SCREEN, desk_layout(&[("laptop", Edge::Right)]), "desk", &logs);
    let laptop_layout = MeshStart {
        arrangement: Arrangement::default(),
        layout_version: 5,
        ..MeshStart::default()
    };
    let laptop = start_pc(&laptop_node, LAPTOP_SCREEN, laptop_layout, "laptop", &logs);
    introduce(("desk", &desk), ("laptop", &laptop));
    wait_until("both to connect", &logs, || desk.sees("laptop") && laptop.sees("desk"));
    thread::sleep(Duration::from_millis(300));

    // The laptop's is newer, so the desk took it (it has nobody placed); the desk's old one did not win.
    assert_eq!(desk.handle.state().layout_version, 5);
    assert_eq!(laptop.handle.state().layout_version, 5);
}

#[test]
fn a_computer_with_a_display_left_of_its_main_one_is_entered_where_the_system_expects() {
    let (desk_node, laptop_node) = (new_node("desk"), new_node("laptop"));
    pair(&desk_node, &laptop_node);
    let logs = Logs::default();
    let desk = start_pc(&desk_node, DESK_SCREEN, desk_layout(&[("laptop", Edge::Right)]), "desk", &logs);
    // The system puts the main display at the origin, so the other one is at negative x.
    let screen = FixedScreen::with_displays(vec![
        Rect::new(0, 0, LAPTOP_SCREEN.0, LAPTOP_SCREEN.1),
        Rect::new(-1920, 360, 1920, 1080),
    ]);
    let laptop = start_pc_at(&laptop_node, "127.0.0.1:0", screen, MeshStart::default(), "laptop", &logs);
    introduce(("desk", &desk), ("laptop", &laptop));
    wait_until("the laptop to connect", &logs, || desk.sees("laptop"));
    assert_eq!(desk.handle.state().peers[0].displays.len(), 2);

    // The desk's right edge meets the laptop's left display, which is where the corner of all
    // its displays is, 1920 to the left of the origin the laptop's system measures from.
    desk.send(move_mouse(1000, 0));
    wait_until("the cursor to appear on the laptop", &logs, || laptop.calls().contains(&SinkCall::MoveAbs(-1920, 540)));

    // Across its left display, onto the main one, and back again: always on the laptop.
    desk.send(move_mouse(2000, 0));
    desk.send(move_mouse(-85, 0));
    // The two moves may arrive as one, so it is their total that counts.
    wait_until("the moves to arrive", &logs, || {
        let moved: i32 =
            laptop.calls().iter().map(|call| if let SinkCall::MoveRel(dx, _) = call { *dx } else { 0 }).sum();
        moved == 1915
    });
    assert_eq!(desk.active_screen(), "laptop");

    // Off the left edge of the left display, back to the desk's right edge.
    desk.send(move_mouse(-1925, 0));
    wait_until("control to come back to the desk", &logs, || !desk.is_capturing());
    wait_until("the desk's cursor to be placed", &logs, || desk.calls().contains(&SinkCall::MoveAbs(1919, 540)));
}

#[test]
fn a_third_computer_beyond_the_second_is_reached_through_it() {
    let (desk_node, laptop_node, tablet_node) = (new_node("desk"), new_node("laptop"), new_node("tablet"));
    pair(&desk_node, &laptop_node);
    pair(&desk_node, &tablet_node);
    pair(&laptop_node, &tablet_node);

    let display = vec![Rect::new(0, 0, LAPTOP_SCREEN.0, LAPTOP_SCREEN.1)];
    let devices = BTreeMap::from([("laptop".to_owned(), display.clone()), ("tablet".to_owned(), display)]);
    let local = [Rect::new(0, 0, DESK_SCREEN.0, DESK_SCREEN.1)];
    let metrics = Metrics { this_computer: "desk", local: &local, devices: &devices };
    let mut arrangement = Arrangement::default();
    arrangement.place("laptop", DESK_SCREEN.0, 0, &metrics).unwrap();
    arrangement.place("tablet", DESK_SCREEN.0 + LAPTOP_SCREEN.0, 0, &metrics).unwrap();

    let logs = Logs::default();
    let desk = start_pc(&desk_node, DESK_SCREEN, MeshStart { arrangement, layout_version: 1, ..MeshStart::default() }, "desk", &logs);
    let laptop = start_pc(&laptop_node, LAPTOP_SCREEN, MeshStart::default(), "laptop", &logs);
    let tablet = start_pc(&tablet_node, LAPTOP_SCREEN, MeshStart::default(), "tablet", &logs);
    introduce(("desk", &desk), ("laptop", &laptop));
    introduce(("desk", &desk), ("tablet", &tablet));
    introduce(("laptop", &laptop), ("tablet", &tablet));
    wait_until("everyone to connect", &logs, || {
        desk.sees("laptop") && desk.sees("tablet") && laptop.sees("tablet") && tablet.sees("laptop")
    });
    wait_until("the layout to reach the tablet", &logs, || tablet.handle.state().layout_version == 1);

    desk.send(move_mouse(1000, 0));
    wait_until("the cursor to reach the laptop", &logs, || !laptop.calls().is_empty());
    assert_eq!(desk.active_screen(), "laptop");

    // Straight across the laptop and out of its right edge.
    desk.send(move_mouse(3000, 0));
    wait_until("the cursor to reach the tablet", &logs, || !tablet.calls().is_empty());
    assert_eq!(desk.active_screen(), "tablet");
    assert!(desk.is_capturing(), "capture stays on while hopping between computers");
    wait_until("the laptop to be free", &logs, || {
        laptop.controlled_by().is_none() && laptop.calls().last() == Some(&SinkCall::ReleaseAll)
    });
}

#[test]
fn a_forgotten_device_is_cut_off_and_cannot_come_back() {
    let Pair { desk, laptop, logs, desk_node } = linked(Edge::Right);
    desk.send(move_mouse(1000, 0));
    wait_until("the cursor to appear on the laptop", &logs, || !laptop.calls().is_empty());

    desk_node.forget("laptop").unwrap();
    desk.handle.disconnect("laptop");

    wait_until("the laptop to be cut off", &logs, || !desk.sees("laptop"));
    wait_until("control to come back to the desk", &logs, || !desk.is_capturing());

    // It keeps trying, and keeps being refused.
    thread::sleep(Duration::from_millis(500));
    assert!(!desk.sees("laptop"));
}

#[test]
fn shutting_down_while_the_cursor_is_away_brings_it_home() {
    let Pair { mut desk, laptop, logs, .. } = linked(Edge::Right);
    desk.send(move_mouse(1000, 0));
    wait_until("the cursor to appear on the laptop", &logs, || !laptop.calls().is_empty());

    desk.handle.stop();

    assert!(!desk.is_capturing(), "input capture must be off after shutdown");
    assert!(desk.calls().contains(&SinkCall::MoveAbs(960, 540)));
    assert!(!desk.handle.is_running());
    assert_eq!(desk.handle.state().error, None);
}

#[test]
fn computers_find_each_other_again_when_one_comes_back() {
    let (desk_node, laptop_node) = (new_node("desk"), new_node("laptop"));
    pair(&desk_node, &laptop_node);
    let logs = Logs::default();
    let start = desk_layout(&[("laptop", Edge::Right)]);

    let mut desk = start_pc(&desk_node, DESK_SCREEN, start.clone(), "desk", &logs);
    let addr = desk.addr.clone();
    let laptop = start_pc(&laptop_node, LAPTOP_SCREEN, MeshStart::default(), "laptop", &logs);
    introduce(("desk", &desk), ("laptop", &laptop));
    wait_until("both to connect", &logs, || desk.sees("laptop") && laptop.sees("desk"));

    desk.handle.stop();
    wait_until("the laptop to notice", &logs, || !laptop.sees("desk"));

    // The same address, up again.
    let desk = start_pc_at(&desk_node, &addr, FixedScreen::new(DESK_SCREEN.0, DESK_SCREEN.1), start, "desk", &logs);
    desk.knows_address_of("laptop", &laptop);
    wait_until("they find each other again", &logs, || desk.sees("laptop") && laptop.sees("desk"));

    desk.send(move_mouse(1000, 0));
    wait_until("the cursor to appear on the laptop", &logs, || laptop.calls().contains(&SinkCall::MoveAbs(0, 540)));
}

#[test]
fn a_computer_with_nobody_to_reach_keeps_running() {
    let node = new_node("laptop");
    let logs = Logs::default();
    let laptop = start_pc(&node, LAPTOP_SCREEN, MeshStart::default(), "laptop", &logs);
    // Nothing listens on port 1.
    laptop.handle.set_peer_address("desk", Some("127.0.0.1:1".into()));
    thread::sleep(Duration::from_millis(300));
    assert!(laptop.handle.is_running());
    assert!(laptop.handle.state().peers.is_empty());
}

#[test]
fn a_mesh_that_cannot_get_its_input_backends_does_not_start() {
    let node = new_node("desk");
    let outcome = MeshHandle::start(
        node,
        "127.0.0.1:0",
        MeshStart::default(),
        || Err(AppError::Config("no permission".into())),
        Logs::default().writer("desk"),
    );
    assert!(matches!(outcome, Err(AppError::Config(message)) if message == "no permission"));
}
