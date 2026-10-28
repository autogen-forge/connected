//! The server: captures local input, lets the router decide which machine it belongs to,
//! and sends it to that machine's client.
//!
//! Three kinds of thread are involved. The server thread runs the event loop: it reads
//! local input, runs the router, and acts on what the router says. One accept thread
//! admits clients. Each client then gets a thread of its own that owns the TLS session,
//! because a session is read and written through the same `&mut` handle. They talk over
//! channels: client threads report arrivals and departures to the event loop, and the event
//! loop hands each client thread the messages to send.
//!
//! A program drives all of this through a [`ServerHandle`]: it can change the screen
//! arrangement while the server runs and read what is going on.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use connected_core::{
    Action, Arrangement, Desktop, InputEvent, InputSink, InputSource, Rect, Router, ScreenInfo,
    normalize,
};
use connected_net::{Listener, NetError, Node, Session};
use connected_proto::{ButtonState, Message, Os};

use crate::error::{AppError, Result};
use crate::{HEARTBEAT_TIMEOUT, Log};

const INPUT_POLL: Duration = Duration::from_millis(10);
const ACCEPT_POLL: Duration = Duration::from_millis(200);
/// How long a new client has to announce its screen.
const SCREEN_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a client thread waits for something to send before it looks at the socket.
const OUTGOING_WAIT: Duration = Duration::from_millis(5);
const INBOUND_EVERY: Duration = Duration::from_millis(20);
const PING_EVERY: Duration = Duration::from_secs(2);
/// Larger than any real display; keeps absurd announcements out of the layout maths.
const MAX_SCREEN_SIDE: i32 = 100_000;
/// The most displays a client may announce.
const MAX_DISPLAYS: usize = 16;

const HID_ESCAPE: u16 = 0x29;
/// Ctrl, Shift and Alt, each as the (left, right) pair of HID usages.
const ESCAPE_MODIFIERS: [(u16, u16); 3] = [(0xE0, 0xE4), (0xE1, 0xE5), (0xE2, 0xE6)];

/// What the server needs from the machine it runs on.
pub struct ServerBackends {
    /// Where local input comes from, and how to withhold it from this machine's apps.
    pub source: Box<dyn InputSource>,
    /// Used to put this machine's own cursor back when control returns.
    pub cursor_sink: Box<dyn InputSink>,
    pub screen: Box<dyn ScreenInfo>,
}

/// A client that is connected right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectedClient {
    /// The name it was paired under.
    pub name: String,
    /// Its displays, measured from the corner of all of them.
    pub displays: Vec<Rect>,
    /// How much each display is magnified, in percent, in the order of `displays`.
    pub scales: Vec<u16>,
    /// The system it runs.
    pub os: Os,
    /// Its network address.
    pub from: String,
}

impl ConnectedClient {
    /// The size of the rectangle holding all its displays.
    pub fn size(&self) -> (i32, i32) {
        connected_proto::bounds(&self.displays).map_or((0, 0), |all| (all.width, all.height))
    }
}

/// What a running server is doing, for a screen to show.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerState {
    /// False once the server thread has ended.
    pub running: bool,
    pub listening_on: String,
    pub this_computer: String,
    /// The connected clients, by name.
    pub clients: Vec<ConnectedClient>,
    /// The screen that has the cursor: this computer's name, or a client's.
    pub active_screen: String,
    /// Why the server ended, if it ended because of a failure.
    pub error: Option<String>,
}

enum ServerCommand {
    SetArrangement(Arrangement),
    Disconnect(String),
}

/// A running server. Dropping it stops the server.
pub struct ServerHandle {
    commands: Sender<ServerCommand>,
    stop: Arc<AtomicBool>,
    state: Arc<Mutex<ServerState>>,
    thread: Option<JoinHandle<()>>,
}

impl ServerHandle {
    /// Starts listening on `listen` and serving.
    ///
    /// `make_backends` runs on the server's own thread, because some platforms insist that
    /// input handles are used on the thread that made them. If it fails, so does this call.
    pub fn start(
        node: Node,
        listen: &str,
        arrangement: Arrangement,
        make_backends: impl FnOnce() -> Result<ServerBackends> + Send + 'static,
        log: Log,
    ) -> Result<Self> {
        let listener = node.listen(listen)?;
        let this_computer = node.identity().name().to_owned();
        let state = Arc::new(Mutex::new(ServerState {
            running: true,
            listening_on: listener.local_addr()?.to_string(),
            active_screen: this_computer.clone(),
            this_computer,
            ..ServerState::default()
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let (commands, command_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);

        let thread = thread::Builder::new()
            .name("connected-server".into())
            .spawn({
                let (state, stop) = (Arc::clone(&state), Arc::clone(&stop));
                move || {
                    let backends = match make_backends() {
                        Ok(backends) => {
                            let _ = ready_tx.send(Ok(()));
                            backends
                        }
                        Err(err) => {
                            let _ = ready_tx.send(Err(err));
                            mark_ended(&state, None);
                            return;
                        }
                    };
                    let outcome = serve(
                        node,
                        listener,
                        backends,
                        arrangement,
                        &command_rx,
                        &stop,
                        &state,
                        &log,
                    );
                    mark_ended(&state, outcome.err().map(|err| err.to_string()));
                }
            })
            .map_err(|err| AppError::Config(format!("cannot start the server thread: {err}")))?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self { commands, stop, state, thread: Some(thread) }),
            Ok(Err(err)) => {
                let _ = thread.join();
                Err(err)
            }
            Err(_) => Err(AppError::Config("the server thread ended before it started".into())),
        }
    }

    /// Replaces the screen arrangement. Takes effect at once, including for clients that are
    /// already connected.
    pub fn set_arrangement(&self, arrangement: Arrangement) {
        let _ = self.commands.send(ServerCommand::SetArrangement(arrangement));
    }

    /// Cuts the connection of a client right now. It may connect again, unless it was
    /// forgotten first.
    pub fn disconnect(&self, device: &str) {
        let _ = self.commands.send(ServerCommand::Disconnect(device.to_owned()));
    }

    pub fn state(&self) -> ServerState {
        self.state.lock().map(|state| state.clone()).unwrap_or_default()
    }

    pub fn is_running(&self) -> bool {
        self.state().running
    }

    /// Stops the server and waits for it. The cursor is handed back to this machine and
    /// input capture is switched off first. Safe to call more than once.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

fn mark_ended(state: &Mutex<ServerState>, error: Option<String>) {
    if let Ok(mut state) = state.lock() {
        state.running = false;
        state.clients.clear();
        state.active_screen = state.this_computer.clone();
        state.error = error;
    }
}

enum ClientEvent {
    Connected {
        device: String,
        /// Tells apart a client from an older connection of the same device.
        id: u64,
        displays: Vec<Rect>,
        scales: Vec<u16>,
        os: Os,
        /// Where the connection came from, for the log.
        from: String,
        outgoing: Sender<Message>,
    },
    Gone {
        device: String,
        id: u64,
    },
}

struct ClientEntry {
    id: u64,
    outgoing: Sender<Message>,
    displays: Vec<Rect>,
    scales: Vec<u16>,
    os: Os,
    from: String,
}

type Clients = HashMap<String, ClientEntry>;

#[allow(clippy::too_many_arguments)]
fn serve(
    node: Node,
    listener: Listener,
    backends: ServerBackends,
    arrangement: Arrangement,
    commands: &Receiver<ServerCommand>,
    stop: &Arc<AtomicBool>,
    state: &Mutex<ServerState>,
    log: &Log,
) -> Result<()> {
    let ServerBackends { mut source, mut cursor_sink, screen } = backends;

    // This computer's displays, measured from the corner of all of them; the system reports
    // them, and the cursor, from the main display's corner.
    let (displays, local_origin) = normalize(&screen.displays()?);
    let local = node.identity().name().to_owned();
    let mut desktop = Desktop::new();
    desktop.add(&local, displays);
    let mut router = Router::new(desktop, &local)?;
    if let Ok((x, y)) = screen.cursor() {
        router.set_local_cursor(x - local_origin.0, y - local_origin.1);
    }

    let (events_tx, events_rx) = mpsc::channel();
    let acceptor = {
        let (stop, log) = (Arc::clone(stop), Arc::clone(log));
        thread::spawn(move || accept_clients(node, listener, events_tx, stop, log))
    };

    let mut world = World {
        router,
        local_origin,
        arrangement,
        clients: Clients::new(),
        source: source.as_mut(),
        cursor_sink: cursor_sink.as_mut(),
        state,
        log,
    };
    let result = event_loop(&mut world, screen.as_ref(), &events_rx, commands, stop);

    stop.store(true, Ordering::Relaxed);
    let _ = acceptor.join();
    result
}

/// Everything the event loop changes as things happen.
struct World<'a> {
    router: Router,
    /// Where the corner of this computer's displays is in the system's coordinates.
    local_origin: (i32, i32),
    arrangement: Arrangement,
    clients: Clients,
    source: &'a mut dyn InputSource,
    cursor_sink: &'a mut dyn InputSink,
    state: &'a Mutex<ServerState>,
    log: &'a Log,
}

impl World<'_> {
    /// Carries out what the router asked for.
    fn apply(&mut self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Capture(on) => {
                    if let Err(err) = self.source.set_capture(on) {
                        let how = if on { "on" } else { "off" };
                        (self.log)(&format!("cannot switch input capture {how}: {err}"));
                    }
                }
                Action::SetLocalCursor { x, y } => {
                    if let Err(err) =
                        self.cursor_sink.move_abs(x + self.local_origin.0, y + self.local_origin.1)
                    {
                        (self.log)(&format!("cannot move the cursor: {err}"));
                    }
                }
                Action::Send { to, msg } => {
                    if let Some(client) = self.clients.get(&to) {
                        // A failure means the client thread just ended; its departure is
                        // reported separately.
                        let _ = client.outgoing.send(msg);
                    }
                }
            }
        }
    }

    /// Re-derives the screen layout from the arrangement and who is connected.
    fn rebuild(&mut self) {
        let sizes: BTreeMap<String, Vec<Rect>> = self
            .clients
            .iter()
            .map(|(name, client)| (name.clone(), client.displays.clone()))
            .collect();
        let actions = self.router.rebuild(&self.arrangement, &sizes);
        self.apply(actions);
        self.publish();
    }

    /// Makes what a screen can show match reality.
    fn publish(&self) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.active_screen = self.router.active_screen().to_owned();
        let mut clients: Vec<ConnectedClient> = self
            .clients
            .iter()
            .map(|(name, client)| ConnectedClient {
                name: name.clone(),
                displays: client.displays.clone(),
                scales: client.scales.clone(),
                os: client.os,
                from: client.from.clone(),
            })
            .collect();
        clients.sort_by(|a, b| a.name.cmp(&b.name));
        state.clients = clients;
    }

    fn handle_client_event(&mut self, event: ClientEvent) {
        match event {
            ClientEvent::Connected { device, id, displays, scales, os, from, outgoing } => {
                let placement = if self.arrangement.contains(&device) {
                    "placed in the layout"
                } else {
                    "not placed in the layout yet"
                };
                let description = describe_displays(&displays);
                let entry = ClientEntry { id, outgoing, displays, scales, os, from: from.clone() };
                self.clients.insert(device.clone(), entry);
                self.rebuild();
                // Said only once the router can use it, so "connected" means input can reach it.
                (self.log)(&format!(
                    "'{device}' connected from {from} ({description}), {placement}"
                ));
            }
            ClientEvent::Gone { device, id } => {
                let current = self.clients.get(&device).is_some_and(|client| client.id == id);
                if current {
                    self.clients.remove(&device);
                    self.rebuild();
                }
            }
        }
    }
}

fn event_loop(
    world: &mut World<'_>,
    screen: &dyn ScreenInfo,
    events: &Receiver<ClientEvent>,
    commands: &Receiver<ServerCommand>,
    stop: &AtomicBool,
) -> Result<()> {
    let mut held: HashSet<u16> = HashSet::new();

    let result = (|| -> Result<()> {
        while !stop.load(Ordering::Relaxed) {
            while let Ok(event) = events.try_recv() {
                world.handle_client_event(event);
            }
            while let Ok(command) = commands.try_recv() {
                match command {
                    ServerCommand::SetArrangement(arrangement) => {
                        world.arrangement = arrangement;
                        world.rebuild();
                    }
                    ServerCommand::Disconnect(device) => {
                        // Dropping the entry closes the client's queue, which ends its thread.
                        if world.clients.remove(&device).is_some() {
                            world.rebuild();
                        }
                    }
                }
            }

            let Some(input) = world.source.next_event_timeout(INPUT_POLL)? else {
                continue;
            };

            if let InputEvent::Key { usage, state } = input {
                match state {
                    ButtonState::Down => held.insert(usage),
                    ButtonState::Up => held.remove(&usage),
                };
                if state == ButtonState::Down
                    && world.router.is_remote()
                    && is_escape_chord(&held, usage)
                {
                    (world.log)("escape chord pressed: input is back on this machine");
                    let actions = world.router.force_local();
                    world.apply(actions);
                    world.publish();
                    continue;
                }
            }

            let actions = world.router.handle(input);
            let stayed_local = actions.is_empty() && !world.router.is_remote();
            let changed = !actions.is_empty();
            world.apply(actions);
            if changed {
                world.publish();
            }

            // While input is local, the router counts motion itself; the real cursor keeps
            // that count honest (the system stops it at the screen edge, for one).
            if stayed_local {
                if let Ok((x, y)) = screen.cursor() {
                    let (ox, oy) = world.local_origin;
                    world.router.set_local_cursor(x - ox, y - oy);
                }
            }
        }
        Ok(())
    })();

    // Whatever ended the loop, leave this machine's input and cursor as they were.
    let actions = world.router.force_local();
    world.apply(actions);
    if let Err(err) = world.source.set_capture(false) {
        (world.log)(&format!("cannot switch input capture off: {err}"));
    }
    world.publish();
    result
}

/// Whether what a client announced as its displays can be believed.
fn usable_displays(displays: &[Rect]) -> bool {
    !displays.is_empty()
        && displays.len() <= MAX_DISPLAYS
        && displays.iter().all(|d| {
            (1..=MAX_SCREEN_SIDE).contains(&d.width)
                && (1..=MAX_SCREEN_SIDE).contains(&d.height)
                && d.x.abs() <= MAX_SCREEN_SIDE
                && d.y.abs() <= MAX_SCREEN_SIDE
        })
}

/// "2560x1440", or "2 displays, 4480x1440" for a computer with several.
fn describe_displays(displays: &[Rect]) -> String {
    let (width, height) =
        connected_proto::bounds(displays).map_or((0, 0), |all| (all.width, all.height));
    match displays.len() {
        1 => format!("{width}x{height}"),
        count => format!("{count} displays, {width}x{height}"),
    }
}

fn is_escape_chord(held: &HashSet<u16>, usage: u16) -> bool {
    usage == HID_ESCAPE
        && ESCAPE_MODIFIERS
            .iter()
            .all(|(left, right)| held.contains(left) || held.contains(right))
}

fn accept_clients(
    node: Node,
    listener: Listener,
    events: Sender<ClientEvent>,
    stop: Arc<AtomicBool>,
    log: Log,
) {
    let mut next_id = 0u64;
    let mut threads: Vec<JoinHandle<()>> = Vec::new();

    while !stop.load(Ordering::Relaxed) {
        match listener.accept_timeout(ACCEPT_POLL) {
            Ok(Some(session)) => {
                next_id += 1;
                let node = node.clone();
                let (events, stop, log) = (events.clone(), Arc::clone(&stop), Arc::clone(&log));
                let id = next_id;
                threads.push(thread::spawn(move || {
                    serve_client(session, &node, id, &events, &stop, &log);
                }));
                threads.retain(|thread| !thread.is_finished());
            }
            Ok(None) => {}
            Err(err) => log(&format!("refused a connection: {err}")),
        }
    }
    for thread in threads {
        let _ = thread.join();
    }
}

fn serve_client(
    mut session: Session,
    node: &Node,
    id: u64,
    events: &Sender<ClientEvent>,
    stop: &AtomicBool,
    log: &Log,
) {
    // The name a person confirmed when pairing, not the one the device claims right now.
    let Some(device) = node.trusted_name(&session.peer().fingerprint) else {
        log("a connection arrived from a device that is no longer paired; dropped");
        return;
    };

    let (displays, scales, os) = match session.recv_timeout(SCREEN_TIMEOUT) {
        Ok(Some(Message::Screen { displays, scales, os })) if usable_displays(&displays) => {
            // One scale for each display; a missing one is taken as none, a stray one dropped.
            let mut scales = scales;
            scales.resize(displays.len(), 100);
            (normalize(&displays).0, scales, os)
        }
        Ok(other) => {
            log(&format!("'{device}' did not announce usable displays ({other:?}); disconnecting it"));
            return;
        }
        Err(err) => {
            log(&format!("'{device}' went away before announcing its screen: {err}"));
            return;
        }
    };

    let (outgoing, queue) = mpsc::channel();
    let from = session.peer_addr().map(|addr| addr.to_string()).unwrap_or_default();
    let connected =
        ClientEvent::Connected { device: device.clone(), id, displays, scales, os, from, outgoing };
    if events.send(connected).is_err() {
        return;
    }

    let outcome = pump(&mut session, &queue, stop);
    let _ = events.send(ClientEvent::Gone { device: device.clone(), id });
    match outcome {
        Ok(()) => log(&format!("'{device}' disconnected")),
        Err(err) => log(&format!("'{device}' lost: {err}")),
    }
}

/// Sends what the event loop queues for one client, and keeps the connection alive.
fn pump(
    session: &mut Session,
    queue: &Receiver<Message>,
    stop: &AtomicBool,
) -> std::result::Result<(), NetError> {
    let started = Instant::now();
    let (mut last_heard, mut last_ping, mut last_poll) = (started, started, started);
    let mut pings = 0u64;

    while !stop.load(Ordering::Relaxed) {
        match queue.recv_timeout(OUTGOING_WAIT) {
            Ok(first) => {
                for message in coalesce(first, queue) {
                    session.send(&message)?;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            // The event loop dropped its end: the server is stopping, or this client was
            // replaced by a newer connection of the same device.
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }

        if last_poll.elapsed() >= INBOUND_EVERY {
            while let Some(message) = session.recv_timeout(Duration::from_millis(1))? {
                last_heard = Instant::now();
                if let Message::Ping(n) = message {
                    session.send(&Message::Pong(n))?;
                }
            }
            last_poll = Instant::now();
        }

        if last_ping.elapsed() >= PING_EVERY {
            pings += 1;
            session.send(&Message::Ping(pings))?;
            last_ping = Instant::now();
        }
        if last_heard.elapsed() > HEARTBEAT_TIMEOUT {
            return Err(NetError::Timeout);
        }
    }
    Ok(())
}

/// Takes everything that is already queued behind `first`, merging runs of mouse motion and
/// scrolling into one message each. When the network falls behind, this keeps the cursor
/// moving by the right total instead of replaying a long queue of tiny steps.
fn coalesce(first: Message, queue: &Receiver<Message>) -> Vec<Message> {
    let mut batch: Vec<Message> = Vec::new();
    let mut next = Some(first);
    while let Some(message) = next.take() {
        let merged = match (batch.last_mut(), &message) {
            (Some(Message::MouseMove { dx, dy }), Message::MouseMove { dx: more_x, dy: more_y })
            | (Some(Message::Wheel { dx, dy }), Message::Wheel { dx: more_x, dy: more_y }) => {
                *dx = dx.saturating_add(*more_x);
                *dy = dy.saturating_add(*more_y);
                true
            }
            _ => false,
        };
        if !merged {
            batch.push(message);
        }
        next = queue.try_recv().ok();
    }
    batch
}

#[cfg(test)]
mod tests {
    use super::*;
    use connected_proto::MouseButton;

    fn queue_of(messages: Vec<Message>) -> (Message, Receiver<Message>) {
        let (sender, receiver) = mpsc::channel();
        let mut messages = messages.into_iter();
        let first = messages.next().expect("at least one message");
        for message in messages {
            sender.send(message).unwrap();
        }
        (first, receiver)
    }

    #[test]
    fn runs_of_motion_are_merged_into_one_step() {
        let (first, rest) = queue_of(vec![
            Message::MouseMove { dx: 1, dy: 2 },
            Message::MouseMove { dx: 3, dy: -1 },
            Message::MouseMove { dx: 4, dy: 0 },
        ]);
        assert_eq!(coalesce(first, &rest), vec![Message::MouseMove { dx: 8, dy: 1 }]);
    }

    #[test]
    fn scrolling_is_merged_but_not_with_motion() {
        let (first, rest) = queue_of(vec![
            Message::Wheel { dx: 0, dy: -120 },
            Message::Wheel { dx: 0, dy: -60 },
            Message::MouseMove { dx: 5, dy: 5 },
        ]);
        assert_eq!(
            coalesce(first, &rest),
            vec![Message::Wheel { dx: 0, dy: -180 }, Message::MouseMove { dx: 5, dy: 5 }]
        );
    }

    #[test]
    fn other_messages_keep_their_order_and_split_motion_runs() {
        let click = Message::MouseButton { button: MouseButton::Left, state: ButtonState::Down };
        let (first, rest) = queue_of(vec![
            Message::MouseMove { dx: 1, dy: 0 },
            click.clone(),
            Message::MouseMove { dx: 2, dy: 0 },
            Message::MouseMove { dx: 2, dy: 0 },
        ]);
        assert_eq!(
            coalesce(first, &rest),
            vec![Message::MouseMove { dx: 1, dy: 0 }, click, Message::MouseMove { dx: 4, dy: 0 }]
        );
    }

    #[test]
    fn merged_motion_does_not_overflow() {
        let (first, rest) = queue_of(vec![
            Message::MouseMove { dx: i32::MAX, dy: 0 },
            Message::MouseMove { dx: 10, dy: 0 },
        ]);
        assert_eq!(coalesce(first, &rest), vec![Message::MouseMove { dx: i32::MAX, dy: 0 }]);
    }

    #[test]
    fn the_escape_chord_needs_all_three_modifiers_and_escape() {
        let mut held: HashSet<u16> = HashSet::new();
        assert!(!is_escape_chord(&held, HID_ESCAPE));

        held.extend([0xE0, 0xE1]); // left Ctrl, left Shift
        assert!(!is_escape_chord(&held, HID_ESCAPE));

        held.insert(0xE6); // right Alt counts as Alt
        assert!(is_escape_chord(&held, HID_ESCAPE));
        assert!(!is_escape_chord(&held, 0x04), "only Escape completes the chord");
    }
}
