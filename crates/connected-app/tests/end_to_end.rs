//! A server and clients talking over real TLS connections on localhost, with in-memory
//! input backends standing in for the operating system.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use connected_app::{
    AppError, ClientHandle, ClientStatus, Log, ServerBackends, ServerHandle,
};
use connected_core::{Arrangement, InputEvent, InputSink, Metrics, Rect, ScreenInfo};
use connected_net::{Identity, Node, TrustStore};
use connected_platform::mock::{ChannelSource, FixedScreen, SharedRecordingSink, SinkCall};
use connected_proto::{ButtonState, Edge, MouseButton};

const SERVER_SCREEN: (i32, i32) = (1920, 1080);
const CLIENT_SCREEN: (i32, i32) = (2560, 1440);
const RETRY: Duration = Duration::from_millis(100);

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
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}\nlog: {:#?}", logs.all());
        thread::sleep(Duration::from_millis(5));
    }
}

fn new_node(name: &str) -> Node {
    Node::new(Identity::generate(name).unwrap(), TrustStore::in_memory())
}

/// Pairs `client` with `server` so that each trusts the other.
fn pair(server: &Node, client: &Node) {
    let listener = server.listen("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let waiting = thread::spawn(move || listener.accept_for_pairing(|_| true));
    client.pair_with(&addr, |_| true).unwrap();
    waiting.join().unwrap().unwrap();
}

struct Desk {
    handle: ServerHandle,
    /// Local input on the server's machine.
    input: Sender<InputEvent>,
    /// Whether the server is currently withholding input from its own desktop.
    capturing: Arc<AtomicBool>,
    /// What the server did to its own cursor.
    own_cursor: SharedRecordingSink,
    addr: String,
}

fn start_desk_at(node: &Node, listen: &str, arrangement: Arrangement, logs: &Logs) -> Desk {
    let (source, input, capturing) = ChannelSource::new();
    let own_cursor = SharedRecordingSink::default();
    let sink = own_cursor.clone();
    let handle = ServerHandle::start(
        node.clone(),
        listen,
        arrangement,
        move || {
            Ok(ServerBackends {
                source: Box::new(source),
                cursor_sink: Box::new(sink),
                screen: Box::new(FixedScreen::new(SERVER_SCREEN.0, SERVER_SCREEN.1)),
            })
        },
        logs.writer("server"),
    )
    .expect("the server starts");
    let addr = handle.state().listening_on;
    Desk { handle, input, capturing, own_cursor, addr }
}

fn start_desk(node: &Node, arrangement: Arrangement, logs: &Logs) -> Desk {
    start_desk_at(node, "127.0.0.1:0", arrangement, logs)
}

impl Desk {
    fn send(&self, event: InputEvent) {
        self.input.send(event).unwrap();
    }

    fn is_capturing(&self) -> bool {
        self.capturing.load(Ordering::SeqCst)
    }

    fn has_client(&self, name: &str) -> bool {
        self.handle.state().clients.iter().any(|client| client.name == name)
    }

    fn active_screen(&self) -> String {
        self.handle.state().active_screen
    }
}

struct Laptop {
    handle: ClientHandle,
    /// What the client injected into its machine.
    injected: SharedRecordingSink,
}

fn start_laptop(node: &Node, addr: &str, logs: &Logs) -> Laptop {
    start_laptop_with(node, addr, logs, FixedScreen::new(CLIENT_SCREEN.0, CLIENT_SCREEN.1))
}

fn start_laptop_with(node: &Node, addr: &str, logs: &Logs, screen: FixedScreen) -> Laptop {
    let injected = SharedRecordingSink::default();
    let sink = injected.clone();
    let handle = ClientHandle::start(
        node.clone(),
        addr.to_owned(),
        move || {
            Ok((
                Box::new(sink) as Box<dyn InputSink>,
                Box::new(screen.clone()) as Box<dyn ScreenInfo>,
            ))
        },
        RETRY,
        logs.writer("client"),
    );
    Laptop { handle, injected }
}

impl Laptop {
    fn calls(&self) -> Vec<SinkCall> {
        self.injected.calls()
    }

    fn status(&self) -> ClientStatus {
        self.handle.status()
    }
}

fn beside_desk(placements: &[(&str, Edge)]) -> Arrangement {
    let devices = BTreeMap::new();
    let local = [Rect::new(0, 0, SERVER_SCREEN.0, SERVER_SCREEN.1)];
    let metrics = Metrics { this_computer: "desk", local: &local, devices: &devices };
    let mut arrangement = Arrangement::default();
    for (device, edge) in placements {
        arrangement.place_beside_this_computer(device, *edge, &metrics).unwrap();
    }
    arrangement
}

fn move_mouse(dx: i32, dy: i32) -> InputEvent {
    InputEvent::MouseMove { dx, dy }
}

fn key(usage: u16, state: ButtonState) -> InputEvent {
    InputEvent::Key { usage, state }
}

/// A desk with a laptop paired, placed on `edge`, and connected.
fn linked(edge: Edge) -> (Desk, Laptop, Logs, Node, Node) {
    let (desk_node, laptop_node) = (new_node("desk"), new_node("laptop"));
    pair(&desk_node, &laptop_node);
    let logs = Logs::default();
    let desk = start_desk(&desk_node, beside_desk(&[("laptop", edge)]), &logs);
    let laptop = start_laptop(&laptop_node, &desk.addr, &logs);
    wait_until("the laptop to connect", &logs, || desk.has_client("laptop"));
    (desk, laptop, logs, desk_node, laptop_node)
}

#[test]
fn crossing_the_edge_hands_the_cursor_to_the_client_and_back() {
    let (desk, laptop, logs, ..) = linked(Edge::Right);
    assert_eq!(desk.active_screen(), "desk");

    // Push the cursor off the right edge of the desk (it starts in the middle).
    desk.send(move_mouse(1000, 0));
    wait_until("the cursor to appear on the laptop", &logs, || {
        laptop.calls().contains(&SinkCall::MoveAbs(0, 540))
    });
    assert!(desk.is_capturing(), "the desk must stop receiving its own input");
    assert_eq!(desk.active_screen(), "laptop");

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
    wait_until("the laptop to let go", &logs, || laptop.calls().last() == Some(&SinkCall::ReleaseAll));
    // The laptop's cursor had moved down 3 to y = 543, which is the same height on the desk,
    // at the desk's right edge. Capture is released first, then the cursor is placed.
    wait_until("the desk's cursor to be placed", &logs, || {
        desk.own_cursor.calls().contains(&SinkCall::MoveAbs(1919, 543))
    });
    assert_eq!(desk.active_screen(), "desk");
}

#[test]
fn a_client_on_the_left_is_reached_by_going_left() {
    let (desk, laptop, logs, ..) = linked(Edge::Left);

    desk.send(move_mouse(-1000, 0));
    wait_until("the cursor to appear on the laptop", &logs, || {
        laptop.calls().contains(&SinkCall::MoveAbs(CLIENT_SCREEN.0 - 1, 540))
    });
}

#[test]
fn the_escape_chord_gets_the_desk_back_without_leaking_into_the_laptop() {
    let (desk, laptop, logs, ..) = linked(Edge::Right);
    desk.send(move_mouse(1000, 0));
    wait_until("the cursor to appear on the laptop", &logs, || !laptop.calls().is_empty());

    // Ctrl, Shift and Alt, then Escape.
    for modifier in [0xE0, 0xE1, 0xE2] {
        desk.send(key(modifier, ButtonState::Down));
    }
    desk.send(key(0x29, ButtonState::Down));

    wait_until("control to come back to the desk", &logs, || !desk.is_capturing());
    wait_until("the laptop to let go of its keys", &logs, || {
        laptop.calls().last() == Some(&SinkCall::ReleaseAll)
    });
    assert!(
        !laptop.calls().contains(&SinkCall::Key(0x29, ButtonState::Down)),
        "the chord's Escape belongs to the desk"
    );
    wait_until("the desk's cursor to be placed", &logs, || {
        desk.own_cursor.calls().contains(&SinkCall::MoveAbs(960, 540))
    });
}

#[test]
fn losing_the_client_while_it_has_the_cursor_gives_control_back() {
    let (desk, mut laptop, logs, ..) = linked(Edge::Right);
    desk.send(move_mouse(1000, 0));
    wait_until("the cursor to appear on the laptop", &logs, || !laptop.calls().is_empty());
    assert!(desk.is_capturing());

    laptop.handle.stop();

    wait_until("the desk to notice and take control back", &logs, || !desk.is_capturing());
    wait_until("the desk's cursor to be placed", &logs, || {
        desk.own_cursor.calls().contains(&SinkCall::MoveAbs(960, 540))
    });
    assert!(!desk.has_client("laptop"));
    assert_eq!(desk.active_screen(), "desk");
}

#[test]
fn an_edge_with_no_client_behind_it_is_a_dead_end() {
    let desk_node = new_node("desk");
    let logs = Logs::default();
    let desk = start_desk(&desk_node, beside_desk(&[("laptop", Edge::Right)]), &logs);

    desk.send(move_mouse(1000, 0));
    thread::sleep(Duration::from_millis(300));
    assert!(!desk.is_capturing(), "nothing is there to receive the input");
}

#[test]
fn a_paired_device_that_is_not_placed_connects_but_cannot_be_reached_until_it_is() {
    let (desk_node, laptop_node) = (new_node("desk"), new_node("laptop"));
    pair(&desk_node, &laptop_node);
    let logs = Logs::default();
    let desk = start_desk(&desk_node, Arrangement::default(), &logs);
    let laptop = start_laptop(&laptop_node, &desk.addr, &logs);
    wait_until("the laptop to connect", &logs, || desk.has_client("laptop"));

    // Connected, so it shows up; but no edge leads to it.
    desk.send(move_mouse(1000, 0));
    thread::sleep(Duration::from_millis(200));
    assert!(!desk.is_capturing());
    assert!(laptop.calls().is_empty());

    // Drag it into the layout while the server runs.
    desk.handle.set_arrangement(beside_desk(&[("laptop", Edge::Right)]));
    thread::sleep(Duration::from_millis(100));
    desk.send(move_mouse(1000, 0));
    wait_until("the cursor to appear on the laptop", &logs, || {
        laptop.calls().contains(&SinkCall::MoveAbs(0, 540))
    });
}

#[test]
fn moving_a_device_to_another_side_while_running_moves_the_way_to_it() {
    let (desk, laptop, logs, ..) = linked(Edge::Right);

    desk.handle.set_arrangement(beside_desk(&[("laptop", Edge::Left)]));
    thread::sleep(Duration::from_millis(100));

    // The right edge no longer leads anywhere.
    desk.send(move_mouse(1000, 0));
    thread::sleep(Duration::from_millis(150));
    assert!(!desk.is_capturing());
    assert!(laptop.calls().is_empty());

    // The left edge does.
    desk.send(move_mouse(-3000, 0));
    wait_until("the cursor to appear on the laptop", &logs, || {
        laptop.calls().contains(&SinkCall::MoveAbs(CLIENT_SCREEN.0 - 1, 540))
    });
}

#[test]
fn removing_the_device_the_cursor_is_on_brings_control_home() {
    let (desk, laptop, logs, ..) = linked(Edge::Right);
    desk.send(move_mouse(1000, 0));
    wait_until("the cursor to appear on the laptop", &logs, || !laptop.calls().is_empty());

    desk.handle.set_arrangement(Arrangement::default());

    wait_until("control to come back to the desk", &logs, || !desk.is_capturing());
    assert_eq!(desk.active_screen(), "desk");
    wait_until("the laptop to let go", &logs, || laptop.calls().last() == Some(&SinkCall::ReleaseAll));
}

#[test]
fn a_client_with_a_display_left_of_its_main_one_is_entered_where_the_system_expects() {
    let (desk_node, laptop_node) = (new_node("desk"), new_node("laptop"));
    pair(&desk_node, &laptop_node);
    let logs = Logs::default();
    let desk = start_desk(&desk_node, beside_desk(&[("laptop", Edge::Right)]), &logs);
    // The system puts the main display at the origin, so the other one is at negative x.
    let screen = FixedScreen::with_displays(vec![
        Rect::new(0, 0, CLIENT_SCREEN.0, CLIENT_SCREEN.1),
        Rect::new(-1920, 360, 1920, 1080),
    ]);
    let laptop = start_laptop_with(&laptop_node, &desk.addr, &logs, screen);
    wait_until("the laptop to connect", &logs, || desk.has_client("laptop"));
    assert_eq!(desk.handle.state().clients[0].displays.len(), 2);

    // The desk's right edge meets the laptop's left display, which is where the corner of all
    // its displays is, 1920 to the left of the origin the laptop's system measures from.
    desk.send(move_mouse(1000, 0));
    wait_until("the cursor to appear on the laptop", &logs, || {
        laptop.calls().contains(&SinkCall::MoveAbs(-1920, 540))
    });

    // Across its left display, onto the main one, and back again: always on the laptop.
    desk.send(move_mouse(2000, 0));
    desk.send(move_mouse(-85, 0));
    // The two moves may arrive as one, so it is their total that counts.
    wait_until("the moves to arrive", &logs, || {
        let moved: i32 = laptop
            .calls()
            .iter()
            .map(|call| if let SinkCall::MoveRel(dx, _) = call { *dx } else { 0 })
            .sum();
        moved == 1915
    });
    assert_eq!(desk.active_screen(), "laptop");

    // Off the left edge of the left display, back to the desk's right edge.
    desk.send(move_mouse(-1925, 0));
    wait_until("control to come back to the desk", &logs, || !desk.is_capturing());
    wait_until("the desk's cursor to be placed", &logs, || {
        desk.own_cursor.calls().contains(&SinkCall::MoveAbs(1919, 540))
    });
}

#[test]
fn a_third_computer_beyond_the_second_is_reached_through_it() {
    let (desk_node, laptop_node, tablet_node) = (new_node("desk"), new_node("laptop"), new_node("tablet"));
    pair(&desk_node, &laptop_node);
    pair(&desk_node, &tablet_node);

    let client_display = vec![Rect::new(0, 0, CLIENT_SCREEN.0, CLIENT_SCREEN.1)];
    let devices = BTreeMap::from([
        ("laptop".to_owned(), client_display.clone()),
        ("tablet".to_owned(), client_display),
    ]);
    let local = [Rect::new(0, 0, SERVER_SCREEN.0, SERVER_SCREEN.1)];
    let metrics = Metrics { this_computer: "desk", local: &local, devices: &devices };
    let mut arrangement = Arrangement::default();
    arrangement.place("laptop", SERVER_SCREEN.0, 0, &metrics).unwrap();
    arrangement.place("tablet", SERVER_SCREEN.0 + CLIENT_SCREEN.0, 0, &metrics).unwrap();

    let logs = Logs::default();
    let desk = start_desk(&desk_node, arrangement, &logs);
    let laptop = start_laptop(&laptop_node, &desk.addr, &logs);
    let tablet = start_laptop(&tablet_node, &desk.addr, &logs);
    wait_until("both to connect", &logs, || desk.has_client("laptop") && desk.has_client("tablet"));

    desk.send(move_mouse(1000, 0));
    wait_until("the cursor to reach the laptop", &logs, || !laptop.calls().is_empty());
    assert_eq!(desk.active_screen(), "laptop");

    // Straight across the laptop and out of its right edge.
    desk.send(move_mouse(3000, 0));
    wait_until("the cursor to reach the tablet", &logs, || !tablet.calls().is_empty());
    assert_eq!(desk.active_screen(), "tablet");
    assert!(desk.is_capturing(), "capture stays on while hopping between clients");
    assert_eq!(laptop.calls().last(), Some(&SinkCall::ReleaseAll));
}

#[test]
fn a_forgotten_device_is_cut_off_and_cannot_come_back() {
    let (desk, laptop, logs, desk_node, _laptop_node) = linked(Edge::Right);
    desk.send(move_mouse(1000, 0));
    wait_until("the cursor to appear on the laptop", &logs, || !laptop.calls().is_empty());

    desk_node.forget("laptop").unwrap();
    desk.handle.disconnect("laptop");

    wait_until("the laptop to be cut off", &logs, || !desk.has_client("laptop"));
    wait_until("control to come back to the desk", &logs, || !desk.is_capturing());

    // It keeps trying, and keeps being refused.
    thread::sleep(Duration::from_millis(500));
    assert!(!desk.has_client("laptop"));
    assert!(matches!(laptop.status(), ClientStatus::Waiting { .. } | ClientStatus::Connecting));
}

#[test]
fn shutting_down_while_the_cursor_is_away_brings_it_home() {
    let (mut desk, laptop, logs, ..) = linked(Edge::Right);
    desk.send(move_mouse(1000, 0));
    wait_until("the cursor to appear on the laptop", &logs, || !laptop.calls().is_empty());

    desk.handle.stop();

    assert!(!desk.is_capturing(), "input capture must be off after shutdown");
    assert!(desk.own_cursor.calls().contains(&SinkCall::MoveAbs(960, 540)));
    assert!(!desk.handle.is_running());
    assert_eq!(desk.handle.state().error, None);
}

#[test]
fn a_client_reconnects_by_itself_when_the_server_comes_back() {
    let (desk_node, laptop_node) = (new_node("desk"), new_node("laptop"));
    pair(&desk_node, &laptop_node);
    let logs = Logs::default();
    let arrangement = beside_desk(&[("laptop", Edge::Right)]);

    let mut desk = start_desk(&desk_node, arrangement.clone(), &logs);
    let addr = desk.addr.clone();
    let laptop = start_laptop(&laptop_node, &addr, &logs);
    wait_until("the laptop to connect", &logs, || desk.has_client("laptop"));
    assert!(matches!(laptop.status(), ClientStatus::Connected { .. }));

    desk.handle.stop();
    wait_until("the laptop to notice", &logs, || matches!(laptop.status(), ClientStatus::Waiting { .. }));

    // The same server address, up again.
    let desk = start_desk_at(&desk_node, &addr, arrangement, &logs);
    wait_until("the laptop to find it again", &logs, || desk.has_client("laptop"));
    wait_until("the status to say so", &logs, || matches!(laptop.status(), ClientStatus::Connected { .. }));

    desk.send(move_mouse(1000, 0));
    wait_until("the cursor to appear on the laptop", &logs, || {
        laptop.calls().contains(&SinkCall::MoveAbs(0, 540))
    });
}

#[test]
fn a_client_with_no_server_keeps_waiting() {
    let laptop_node = new_node("laptop");
    let logs = Logs::default();
    // Nothing listens on port 1.
    let laptop = start_laptop(&laptop_node, "127.0.0.1:1", &logs);
    wait_until("the client to report trouble", &logs, || {
        matches!(laptop.status(), ClientStatus::Waiting { .. })
    });
}

#[test]
fn a_server_that_cannot_get_its_input_backends_does_not_start() {
    let node = new_node("desk");
    let outcome = ServerHandle::start(
        node,
        "127.0.0.1:0",
        Arrangement::default(),
        || Err(AppError::Config("no permission".into())),
        Logs::default().writer("server"),
    );
    assert!(matches!(outcome, Err(AppError::Config(message)) if message == "no permission"));
}

#[test]
fn a_client_that_cannot_get_its_backends_reports_failure() {
    let node = new_node("laptop");
    let client = ClientHandle::start(
        node,
        "127.0.0.1:1".into(),
        || Err(AppError::Config("no permission".into())),
        RETRY,
        Logs::default().writer("client"),
    );
    wait_until("the failure to show", &Logs::default(), || {
        matches!(client.status(), ClientStatus::Failed { .. })
    });
}
