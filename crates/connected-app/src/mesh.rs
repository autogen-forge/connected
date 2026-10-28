//! The mesh: every computer runs the same thing, and every paired computer connects to every
//! other. Nobody is the host.
//!
//! Each computer captures its own keyboard and mouse and runs the router. When the cursor
//! goes past the edge of its screen onto another computer's, it starts sending its input to
//! that computer, which injects it. That computer is then *controlled* for as long as the cursor
//! stays there: its own keyboard and mouse are held back (so the two people do not fight), and
//! the computer that got there first keeps priority until its cursor comes back. Either computer
//! can be the one that crosses.
//!
//! A connection is made by one side only, so that two computers do not connect to each other
//! twice: the one whose fingerprint sorts lower dials the other, at the address it learned when
//! they paired (or was told since). The other listens. Once the link is up it is the same in both
//! directions.
//!
//! Three kinds of thread are involved. The mesh thread runs the event loop: it reads local
//! input, runs the router, injects what controlling computers send, and acts on what links report.
//! One accept thread admits links that other computers start and one dial thread starts the ones
//! this computer is to start. Each link has a thread of its own that owns the TLS session,
//! because a session is read and written through the same `&mut` handle. They talk over channels.
//!
//! The layout (where each computer is) is shared too: whoever changes it sends it to everyone,
//! the newest version wins, and a computer passes on what it receives, so every computer shows
//! and uses the same one.
//!
//! A program drives all of this through a [`MeshHandle`].

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use connected_core::platform::Result as PlatformResult;
use connected_core::{
    Action, Arrangement, Desktop, InputEvent, InputSink, InputSource, Metrics, Point, Rect, Router,
    ScreenInfo, Spot, normalize,
};
use connected_net::{Listener, NetError, Node, Session};
use connected_proto::{ButtonState, LayoutScreen, Message, Modifiers, Os};

use crate::error::{AppError, Result};
use crate::{HEARTBEAT_TIMEOUT, Log};

/// How long the event loop waits for local input before looking at what the links and the
/// person's commands have to say. It is also how late a controlling computer's input can arrive.
const INPUT_POLL: Duration = Duration::from_millis(2);
const ACCEPT_POLL: Duration = Duration::from_millis(200);
/// How often the dial thread looks for a computer it should be connected to and is not.
const DIAL_EVERY: Duration = Duration::from_secs(2);
/// How long a new link has to announce its screen.
const SCREEN_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a link thread waits at the socket for the other computer before it looks at what
/// is queued to send. It is also how late anything queued to send can go out.
const INBOUND_WAIT: Duration = Duration::from_millis(2);
const PING_EVERY: Duration = Duration::from_secs(2);
/// Larger than any real display; keeps absurd announcements out of the layout maths.
const MAX_SCREEN_SIDE: i32 = 100_000;
/// The most displays a computer may announce.
const MAX_DISPLAYS: usize = 16;
/// Reports of input that could not be injected stop after this many, so a log is not flooded.
const MAX_INJECTION_REPORTS: u32 = 5;

const HID_ESCAPE: u16 = 0x29;
/// Ctrl, Shift and Alt, each as the (left, right) pair of HID usages.
const ESCAPE_MODIFIERS: [(u16, u16); 3] = [(0xE0, 0xE4), (0xE1, 0xE5), (0xE2, 0xE6)];

/// What the mesh needs from the machine it runs on.
pub struct MeshBackends {
    /// Where local input comes from, and how to withhold it from this machine's apps.
    pub source: Box<dyn InputSource>,
    /// Puts input into this machine: what controlling computers send, and this machine's own
    /// cursor when control returns to it.
    pub sink: Box<dyn InputSink>,
    pub screen: Box<dyn ScreenInfo>,
}

/// What the mesh starts out knowing.
#[derive(Debug, Clone, Default)]
pub struct MeshStart {
    pub arrangement: Arrangement,
    /// The version of that arrangement, a time in milliseconds; 0 if it was never changed.
    pub layout_version: u64,
    /// Where to reach each paired computer, by name, as `address:port`.
    pub peer_addresses: BTreeMap<String, String>,
}

/// A computer that is connected right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectedPeer {
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

impl ConnectedPeer {
    /// The size of the rectangle holding all its displays.
    pub fn size(&self) -> (i32, i32) {
        connected_proto::bounds(&self.displays).map_or((0, 0), |all| (all.width, all.height))
    }
}

/// What a running mesh is doing, for a screen to show.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MeshState {
    /// False once the mesh thread has ended.
    pub running: bool,
    pub listening_on: String,
    pub this_computer: String,
    /// The connected computers, by name.
    pub peers: Vec<ConnectedPeer>,
    /// The screen that has the cursor: this computer's name, or another's.
    pub active_screen: String,
    /// The computer that is controlling this one right now, if one is.
    pub controlled_by: Option<String>,
    /// The shared layout as this computer has it, and its version.
    pub arrangement: Arrangement,
    pub layout_version: u64,
    /// Why the mesh ended, if it ended because of a failure.
    pub error: Option<String>,
}

enum MeshCommand {
    SetArrangement(Arrangement, u64),
    SetPeerAddress { name: String, address: Option<String> },
    Disconnect(String),
}

/// A running mesh. Dropping it stops it.
pub struct MeshHandle {
    commands: Sender<MeshCommand>,
    stop: Arc<AtomicBool>,
    state: Arc<Mutex<MeshState>>,
    thread: Option<JoinHandle<()>>,
}

impl MeshHandle {
    /// Starts listening on `listen`, connecting to the computers it is to connect to, and
    /// sharing input.
    ///
    /// `make_backends` runs on the mesh's own thread, because some platforms insist that input
    /// handles are used on the thread that made them. If it fails, so does this call.
    pub fn start(
        node: Node,
        listen: &str,
        initial: MeshStart,
        make_backends: impl FnOnce() -> Result<MeshBackends> + Send + 'static,
        log: Log,
    ) -> Result<Self> {
        let listener = node.listen(listen)?;
        let this_computer = node.identity().name().to_owned();
        let state = Arc::new(Mutex::new(MeshState {
            running: true,
            listening_on: listener.local_addr()?.to_string(),
            active_screen: this_computer.clone(),
            this_computer,
            arrangement: initial.arrangement.clone(),
            layout_version: initial.layout_version,
            ..MeshState::default()
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let (commands, command_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);

        let thread = thread::Builder::new()
            .name("connected-mesh".into())
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
                    let run = Run { node, listener, backends, initial, state: &state, stop: &stop, log: &log };
                    let outcome = run.serve(&command_rx);
                    mark_ended(&state, outcome.err().map(|err| err.to_string()));
                }
            })
            .map_err(|err| AppError::Config(format!("cannot start the mesh thread: {err}")))?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self { commands, stop, state, thread: Some(thread) }),
            Ok(Err(err)) => {
                let _ = thread.join();
                Err(err)
            }
            Err(_) => Err(AppError::Config("the mesh thread ended before it started".into())),
        }
    }

    /// Replaces the layout, as the person changing it does. It takes effect at once, and is sent
    /// to every connected computer. `version` is the time of the change in milliseconds; the
    /// mesh keeps it newer than any layout it has seen.
    pub fn set_arrangement(&self, arrangement: Arrangement, version: u64) {
        let _ = self.commands.send(MeshCommand::SetArrangement(arrangement, version));
    }

    /// Sets where to reach a paired computer (`address:port`), or forgets it with `None`.
    pub fn set_peer_address(&self, name: &str, address: Option<String>) {
        let _ = self.commands.send(MeshCommand::SetPeerAddress { name: name.to_owned(), address });
    }

    /// Cuts the connection to a computer right now. It may connect again, unless it was
    /// forgotten first.
    pub fn disconnect(&self, device: &str) {
        let _ = self.commands.send(MeshCommand::Disconnect(device.to_owned()));
    }

    pub fn state(&self) -> MeshState {
        self.state.lock().map(|state| state.clone()).unwrap_or_default()
    }

    pub fn is_running(&self) -> bool {
        self.state().running
    }

    /// Stops the mesh and waits for it. The cursor is handed back to this machine and input
    /// capture is switched off first. Safe to call more than once.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for MeshHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

fn mark_ended(state: &Mutex<MeshState>, error: Option<String>) {
    if let Ok(mut state) = state.lock() {
        state.running = false;
        state.peers.clear();
        state.active_screen = state.this_computer.clone();
        state.controlled_by = None;
        state.error = error;
    }
}

/// What links tell the event loop.
enum LinkEvent {
    Up {
        device: String,
        /// Tells apart a link from an older one to the same device.
        id: u64,
        displays: Vec<Rect>,
        scales: Vec<u16>,
        os: Os,
        /// Where the connection came from or went to, for the log.
        from: String,
        outgoing: Sender<Message>,
    },
    Down {
        device: String,
        id: u64,
    },
    /// Something the other computer sent besides keeping the link alive.
    Message {
        device: String,
        id: u64,
        message: Message,
    },
}

struct LinkEntry {
    id: u64,
    outgoing: Sender<Message>,
    displays: Vec<Rect>,
    scales: Vec<u16>,
    os: Os,
    from: String,
}

type Links = HashMap<String, LinkEntry>;

/// What every link thread needs, whichever side started the link.
#[derive(Clone)]
struct LinkContext {
    node: Node,
    /// What this computer tells the other when a link comes up.
    announce: Message,
    events: Sender<LinkEvent>,
    stop: Arc<AtomicBool>,
    log: Log,
    ids: Arc<AtomicU64>,
}

/// One run of the mesh: what `serve` needs, gathered so that it does not take a dozen arguments.
struct Run<'a> {
    node: Node,
    listener: Listener,
    backends: MeshBackends,
    initial: MeshStart,
    state: &'a Arc<Mutex<MeshState>>,
    stop: &'a Arc<AtomicBool>,
    log: &'a Log,
}

impl Run<'_> {
    fn serve(self, commands: &Receiver<MeshCommand>) -> Result<()> {
        let Run { node, listener, backends, initial, state, stop, log } = self;
        let MeshBackends { mut source, mut sink, screen } = backends;

        // This computer's displays, measured from the corner of all of them; the system reports
        // them, and the cursor, from the main display's corner.
        let (displays, local_origin) = normalize(&screen.displays()?);
        let local = node.identity().name().to_owned();
        let scales = screen.scales().unwrap_or_else(|_| vec![100; displays.len()]);
        let announce = Message::Screen { displays: displays.clone(), scales: scales.clone(), os: screen.os() };
        let mut desktop = Desktop::new();
        desktop.add(&local, displays.clone());
        let mut router = Router::new(desktop, &local)?;
        if let Ok((x, y)) = screen.cursor() {
            router.set_local_cursor(x - local_origin.0, y - local_origin.1);
        }

        let (events_tx, events_rx) = mpsc::channel();
        let context = LinkContext {
            node: node.clone(),
            announce,
            events: events_tx,
            stop: Arc::clone(stop),
            log: Arc::clone(log),
            ids: Arc::new(AtomicU64::new(0)),
        };
        let targets = Arc::new(Mutex::new(initial.peer_addresses.clone()));
        let acceptor = {
            let context = context.clone();
            thread::spawn(move || accept_links(listener, &context))
        };
        let dialer = {
            let (context, targets, state) = (context.clone(), Arc::clone(&targets), Arc::clone(state));
            thread::spawn(move || dial_links(&context, &targets, &state))
        };
        drop(context);

        let mut world = World {
            router,
            this_computer: ThisComputer { name: local, displays },
            local_origin,
            arrangement: initial.arrangement,
            layout_version: initial.layout_version,
            links: Links::new(),
            controlled_by: None,
            source: source.as_mut(),
            sink: sink.as_mut(),
            screen: screen.as_ref(),
            node,
            targets,
            state,
            log,
            reports: 0,
        };
        let result = event_loop(&mut world, &events_rx, commands, stop);

        stop.store(true, Ordering::Relaxed);
        let _ = acceptor.join();
        let _ = dialer.join();
        result
    }
}

/// Whether a computer is connected, according to what the state shows.
fn is_connected(state: &Mutex<MeshState>, name: &str) -> bool {
    state.lock().map(|state| state.peers.iter().any(|peer| peer.name == name)).unwrap_or(false)
}

/// This computer, as the layout sees it.
struct ThisComputer {
    name: String,
    displays: Vec<Rect>,
}

/// Everything the event loop changes as things happen.
struct World<'a> {
    router: Router,
    this_computer: ThisComputer,
    /// Where the corner of this computer's displays is in the system's coordinates.
    local_origin: (i32, i32),
    arrangement: Arrangement,
    layout_version: u64,
    links: Links,
    /// The computer whose cursor is on this one, if there is one.
    controlled_by: Option<String>,
    source: &'a mut dyn InputSource,
    sink: &'a mut dyn InputSink,
    screen: &'a dyn ScreenInfo,
    node: Node,
    targets: Arc<Mutex<BTreeMap<String, String>>>,
    state: &'a Mutex<MeshState>,
    log: &'a Log,
    reports: u32,
}

impl World<'_> {
    /// Carries out what the router asked for.
    fn apply(&mut self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Capture(on) => self.capture(on),
                Action::SetLocalCursor { x, y } => {
                    if let Err(err) = self.sink.move_abs(x + self.local_origin.0, y + self.local_origin.1) {
                        (self.log)(&format!("cannot move the cursor: {err}"));
                    }
                }
                Action::Send { to, msg } => {
                    if matches!(msg, Message::Enter { .. }) {
                        (self.log)(&format!("cursor moved onto '{to}'"));
                    }
                    self.send_to(&to, msg);
                }
            }
        }
    }

    fn capture(&mut self, on: bool) {
        if let Err(err) = self.source.set_capture(on) {
            let how = if on { "on" } else { "off" };
            (self.log)(&format!("cannot switch input capture {how}: {err}"));
        }
    }

    fn send_to(&self, to: &str, message: Message) {
        if let Some(link) = self.links.get(to) {
            // A failure means the link thread just ended; its departure is reported separately.
            let _ = link.outgoing.send(message);
        }
    }

    /// Sends to every connected computer but `except`.
    fn broadcast(&self, message: &Message, except: Option<&str>) {
        for (name, link) in &self.links {
            if Some(name.as_str()) != except {
                let _ = link.outgoing.send(message.clone());
            }
        }
    }

    /// Re-derives the screen layout from the arrangement and who is connected.
    fn rebuild(&mut self) {
        let sizes = self.connected_displays();
        let actions = self.router.rebuild(&self.arrangement, &sizes);
        self.apply(actions);
        self.publish();
    }

    fn connected_displays(&self) -> BTreeMap<String, Vec<Rect>> {
        self.links.iter().map(|(name, link)| (name.clone(), link.displays.clone())).collect()
    }

    /// Makes what a screen can show match reality.
    fn publish(&self) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.active_screen = self.router.active_screen().to_owned();
        state.controlled_by.clone_from(&self.controlled_by);
        let mut peers: Vec<ConnectedPeer> = self
            .links
            .iter()
            .map(|(name, link)| ConnectedPeer {
                name: name.clone(),
                displays: link.displays.clone(),
                scales: link.scales.clone(),
                os: link.os,
                from: link.from.clone(),
            })
            .collect();
        peers.sort_by(|a, b| a.name.cmp(&b.name));
        state.peers = peers;
    }

    /// Shows the layout, for whoever saves it.
    fn publish_arrangement(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.arrangement = self.arrangement.clone();
            state.layout_version = self.layout_version;
        }
    }

    // ---- the shared layout ----

    /// Where each placed computer is, by fingerprint, as everyone knows them.
    fn layout_doc(&self) -> Vec<LayoutScreen> {
        let sizes = self.connected_displays();
        let metrics = Metrics {
            this_computer: &self.this_computer.name,
            local: &self.this_computer.displays,
            devices: &sizes,
        };
        let desktop = self.arrangement.desktop(&metrics);
        let fingerprints = self.fingerprints();

        let mut screens = vec![LayoutScreen {
            fingerprint: self.node.identity().fingerprint().to_string(),
            x: self.arrangement.local.x,
            y: self.arrangement.local.y,
        }];
        for name in self.arrangement.remote.keys() {
            let (Some(fingerprint), Some(rect)) = (fingerprints.get(name), desktop.rect(name)) else {
                continue;
            };
            screens.push(LayoutScreen { fingerprint: fingerprint.clone(), x: rect.x, y: rect.y });
        }
        screens
    }

    /// The paired computers' fingerprints, by the name they were paired under.
    fn fingerprints(&self) -> BTreeMap<String, String> {
        self.node
            .trusted_peers()
            .into_iter()
            .map(|peer| (peer.name, peer.fingerprint.to_string()))
            .collect()
    }

    fn layout_message(&self) -> Message {
        Message::Layout { version: self.layout_version, screens: self.layout_doc() }
    }

    /// The layout was changed here, by the person: it gets a newer version, and goes to everyone.
    fn change_layout(&mut self, arrangement: Arrangement, version: u64) {
        self.arrangement = arrangement;
        self.layout_version = version.max(self.layout_version + 1);
        self.rebuild();
        self.publish_arrangement();
        let message = self.layout_message();
        self.broadcast(&message, None);
    }

    /// A computer sent its layout. If it is newer than ours, it replaces ours and is passed on;
    /// if it is older, the sender is told what is newer.
    fn on_layout(&mut self, from: &str, version: u64, screens: Vec<LayoutScreen>) {
        if version < self.layout_version {
            let message = self.layout_message();
            self.send_to(from, message);
            return;
        }
        if version == self.layout_version {
            return;
        }

        let relay = screens.clone();
        let local_fingerprint = self.node.identity().fingerprint().to_string();
        let names: HashMap<String, String> =
            self.fingerprints().into_iter().map(|(name, fingerprint)| (fingerprint, name)).collect();
        let mut arrangement = Arrangement::default();
        for screen in screens {
            if screen.fingerprint == local_fingerprint {
                arrangement.local = Point { x: screen.x, y: screen.y };
            } else if let Some(name) = names.get(&screen.fingerprint) {
                arrangement.remote.insert(name.clone(), Spot::At { x: screen.x, y: screen.y });
            }
            // A computer this one is not paired with cannot be placed here, and is left out.
        }

        self.arrangement = arrangement;
        self.layout_version = version;
        self.rebuild();
        self.publish_arrangement();
        // Passed on as it came, so that computers this one is not paired with are not lost.
        self.broadcast(&Message::Layout { version, screens: relay }, Some(from));
        (self.log)(&format!("took the layout '{from}' sent"));
    }

    // ---- being controlled ----

    /// A computer's cursor has come onto this one.
    fn on_enter(&mut self, from: &str, x: i32, y: i32, modifiers: Modifiers) {
        let busy = self.router.is_remote() || self.controlled_by.as_deref().is_some_and(|other| other != from);
        if busy {
            // This computer's cursor is elsewhere, or another computer has it: whoever got there
            // first keeps priority. Say so, and the sender takes its cursor back.
            (self.log)(&format!("'{from}' tried to take control, but this computer is busy"));
            self.send_to(from, Message::Leave);
            return;
        }
        (self.log)(&format!("'{from}' took control of this computer"));
        self.controlled_by = Some(from.to_owned());
        self.capture(true);
        let message = Message::Enter { x, y, seq: 0, modifiers };
        self.inject(&message);
        self.publish();
    }

    /// The computer that controlled this one lets go (or, if this one controlled it, refuses).
    fn on_leave(&mut self, from: &str) {
        if self.controlled_by.as_deref() == Some(from) {
            self.end_control();
        } else if self.router.active_screen() == from {
            let actions = self.router.force_local();
            self.apply(actions);
            self.publish();
        }
    }

    /// This computer is its own again: its keys are let go and its own keyboard and mouse work.
    fn end_control(&mut self) {
        let Some(controller) = self.controlled_by.take() else {
            return;
        };
        (self.log)(&format!("'{controller}' let go of this computer"));
        if let Err(err) = self.sink.release_all() {
            (self.log)(&format!("cannot release held keys: {err}"));
        }
        self.capture(false);
        // The cursor moved while it was controlled; the router has to start from where it is.
        if let Ok((x, y)) = self.screen.cursor() {
            self.router.set_local_cursor(x - self.local_origin.0, y - self.local_origin.1);
        }
        self.publish();
    }

    /// Puts what a controlling computer sent into this one.
    fn inject(&mut self, message: &Message) {
        if let Err(err) = inject(message, self.local_origin, self.sink) {
            self.reports += 1;
            if self.reports <= MAX_INJECTION_REPORTS {
                (self.log)(&format!("cannot inject {message:?}: {err}"));
            }
        }
    }

    fn on_message(&mut self, from: &str, message: Message) {
        match message {
            Message::Layout { version, screens } => self.on_layout(from, version, screens),
            Message::Enter { x, y, modifiers, .. } => self.on_enter(from, x, y, modifiers),
            Message::Leave => self.on_leave(from),
            // Only the computer that has the cursor here is listened to.
            Message::MouseMove { .. }
            | Message::MouseButton { .. }
            | Message::Wheel { .. }
            | Message::Key { .. }
                if self.controlled_by.as_deref() == Some(from) =>
            {
                self.inject(&message);
            }
            _ => {}
        }
    }

    fn on_link_event(&mut self, event: LinkEvent) {
        match event {
            LinkEvent::Up { device, id, displays, scales, os, from, outgoing } => {
                let placement = if self.arrangement.contains(&device) {
                    "placed in the layout"
                } else {
                    "not placed in the layout yet"
                };
                let description = describe_displays(&displays);
                self.links.insert(device.clone(), LinkEntry { id, outgoing, displays, scales, os, from: from.clone() });
                self.rebuild();
                // Said only once the router can use it, so "connected" means input can reach it.
                (self.log)(&format!("'{device}' connected ({from}, {description}), {placement}"));
                let message = self.layout_message();
                self.send_to(&device, message);
            }
            LinkEvent::Down { device, id } => {
                if self.links.get(&device).is_some_and(|link| link.id == id) {
                    self.links.remove(&device);
                    if self.controlled_by.as_deref() == Some(device.as_str()) {
                        self.end_control();
                    }
                    self.rebuild();
                }
            }
            LinkEvent::Message { device, id, message } => {
                if self.links.get(&device).is_some_and(|link| link.id == id) {
                    self.on_message(&device, message);
                }
            }
        }
    }

    fn on_command(&mut self, command: MeshCommand) {
        match command {
            MeshCommand::SetArrangement(arrangement, version) => self.change_layout(arrangement, version),
            MeshCommand::SetPeerAddress { name, address } => {
                if let Ok(mut targets) = self.targets.lock() {
                    match address {
                        Some(address) => targets.insert(name, address),
                        None => targets.remove(&name),
                    };
                }
            }
            MeshCommand::Disconnect(device) => {
                // Dropping the entry closes the link's queue, which ends its thread.
                if self.links.remove(&device).is_some() {
                    if self.controlled_by.as_deref() == Some(device.as_str()) {
                        self.end_control();
                    }
                    self.rebuild();
                }
            }
        }
    }
}

/// The time in milliseconds, which is what a layout's version is.
pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_millis() as u64)
}

fn event_loop(
    world: &mut World<'_>,
    events: &Receiver<LinkEvent>,
    commands: &Receiver<MeshCommand>,
    stop: &AtomicBool,
) -> Result<()> {
    let mut held: HashSet<u16> = HashSet::new();
    let mut heard_input = false;

    let result = (|| -> Result<()> {
        while !stop.load(Ordering::Relaxed) {
            while let Ok(event) = events.try_recv() {
                world.on_link_event(event);
            }
            while let Ok(command) = commands.try_recv() {
                world.on_command(command);
            }

            let Some(input) = world.source.next_event_timeout(INPUT_POLL)? else {
                continue;
            };
            if !heard_input {
                heard_input = true;
                (world.log)("this computer's input is being read");
            }

            if let InputEvent::Key { usage, state } = input {
                match state {
                    ButtonState::Down => held.insert(usage),
                    ButtonState::Up => held.remove(&usage),
                };
                if state == ButtonState::Down && is_escape_chord(&held, usage) {
                    if world.router.is_remote() {
                        (world.log)("escape chord pressed: input is back on this machine");
                        let actions = world.router.force_local();
                        world.apply(actions);
                        world.publish();
                        continue;
                    }
                    if let Some(controller) = world.controlled_by.clone() {
                        // The person at this computer takes it back from the one controlling it.
                        (world.log)("escape chord pressed: this machine is its own again");
                        world.send_to(&controller, Message::Leave);
                        world.end_control();
                        continue;
                    }
                }
            }

            // While another computer controls this one, what is typed or moved here is held
            // back by the capture and not acted on: the computer that got here first has priority.
            if world.controlled_by.is_some() {
                continue;
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
                if let Ok((x, y)) = world.screen.cursor() {
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
    world.end_control();
    world.capture(false);
    world.publish();
    result
}

/// Carries out one message from a controlling computer. `origin` is where the corner of this
/// computer's displays is in the system's coordinates, which the positions it is sent are
/// measured from.
fn inject(message: &Message, origin: (i32, i32), sink: &mut dyn InputSink) -> PlatformResult<()> {
    match *message {
        Message::Enter { x, y, modifiers, .. } => {
            sink.move_abs(x + origin.0, y + origin.1)?;
            press_held_modifiers(modifiers, sink)
        }
        Message::Leave => sink.release_all(),
        Message::MouseMove { dx, dy } => sink.move_rel(dx, dy),
        Message::MouseButton { button, state } => sink.button(button, state),
        Message::Wheel { dx, dy } => sink.wheel(dx, dy),
        Message::Key { usage, state } => sink.key(usage, state),
        _ => Ok(()),
    }
}

/// The controlling computer's modifier keys were pressed before the cursor came over, so no
/// key-down for them was ever sent. Press them now, so that a Ctrl held while crossing still counts.
fn press_held_modifiers(modifiers: Modifiers, sink: &mut dyn InputSink) -> PlatformResult<()> {
    for bit in 0..8u16 {
        if modifiers.0 & (1 << bit) != 0 {
            sink.key(0xE0 + bit, ButtonState::Down)?;
        }
    }
    Ok(())
}

/// Whether what a computer announced as its displays can be believed.
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

// ---- starting links ----

/// Admits the links that other computers start.
fn accept_links(listener: Listener, context: &LinkContext) {
    let mut threads: Vec<JoinHandle<()>> = Vec::new();

    while !context.stop.load(Ordering::Relaxed) {
        match listener.accept_timeout(ACCEPT_POLL) {
            Ok(Some(session)) => {
                let context = context.clone();
                threads.push(thread::spawn(move || run_link(session, &context, "accepted")));
                threads.retain(|thread| !thread.is_finished());
            }
            Ok(None) => {}
            Err(err) => (context.log)(&format!("refused a connection: {err}")),
        }
    }
    for thread in threads {
        let _ = thread.join();
    }
}

/// Starts the links this computer is to start: to each paired computer whose fingerprint sorts
/// after its own, whenever one is not connected, at the address it knows for it.
fn dial_links(context: &LinkContext, targets: &Mutex<BTreeMap<String, String>>, state: &Mutex<MeshState>) {
    let mine = context.node.identity().fingerprint().to_string();
    let dialing: Arc<Mutex<HashSet<String>>> = Arc::default();
    // What went wrong last time with each computer, so that the same trouble is said once.
    let last_problem: Arc<Mutex<HashMap<String, String>>> = Arc::default();
    let mut threads: Vec<JoinHandle<()>> = Vec::new();

    while !context.stop.load(Ordering::Relaxed) {
        for peer in context.node.trusted_peers() {
            if mine >= peer.fingerprint.to_string() {
                continue; // the other computer is the one to dial
            }
            let address = targets.lock().ok().and_then(|targets| targets.get(&peer.name).cloned());
            let Some(address) = address else {
                continue;
            };
            if is_connected(state, &peer.name) {
                continue;
            }
            let started = dialing.lock().map(|mut dialing| dialing.insert(peer.name.clone())).unwrap_or(false);
            if !started {
                continue;
            }

            let (context, dialing, last_problem) = (context.clone(), Arc::clone(&dialing), Arc::clone(&last_problem));
            threads.push(thread::spawn(move || {
                match context.node.connect(address.as_str()) {
                    Ok(session) => {
                        if let Ok(mut problems) = last_problem.lock() {
                            problems.remove(&peer.name);
                        }
                        run_link(session, &context, "dialed");
                    }
                    Err(err) => {
                        let text = err.to_string();
                        let new = last_problem
                            .lock()
                            .map(|mut problems| problems.insert(peer.name.clone(), text.clone()).as_deref() != Some(text.as_str()))
                            .unwrap_or(true);
                        if new {
                            (context.log)(&format!("cannot connect to '{}' at {address}: {text}", peer.name));
                        }
                    }
                }
                if let Ok(mut dialing) = dialing.lock() {
                    dialing.remove(&peer.name);
                }
            }));
            threads.retain(|thread| !thread.is_finished());
        }

        // Wait before looking again, in slices so that stopping is not held up.
        let until = Instant::now() + DIAL_EVERY;
        while Instant::now() < until && !context.stop.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(50));
        }
    }
    for thread in threads {
        let _ = thread.join();
    }
}

/// Runs one link, from the moment its TLS session is up until it ends. Both sides do the same:
/// say what their screens are, and then pass messages both ways.
fn run_link(mut session: Session, context: &LinkContext, how: &str) {
    let LinkContext { node, announce, events, stop, log, ids } = context;

    // The name a person confirmed when pairing, not the one the device claims right now.
    let Some(device) = node.trusted_name(&session.peer().fingerprint) else {
        log("a connection arrived from a device that is no longer paired; dropped");
        return;
    };
    if let Err(err) = session.send(announce) {
        log(&format!("'{device}' went away before this computer could announce its screen: {err}"));
        return;
    }

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

    let id = ids.fetch_add(1, Ordering::Relaxed) + 1;
    let (outgoing, queue) = mpsc::channel();
    let from = session.peer_addr().map(|addr| addr.to_string()).unwrap_or_else(|_| how.to_owned());
    let up = LinkEvent::Up { device: device.clone(), id, displays, scales, os, from, outgoing };
    if events.send(up).is_err() {
        return;
    }

    let outcome = pump(&mut session, &queue, stop, events, &device, id);
    let _ = events.send(LinkEvent::Down { device: device.clone(), id });
    match outcome {
        Ok(()) => log(&format!("'{device}' disconnected")),
        Err(err) => log(&format!("'{device}' lost: {err}")),
    }
}

/// Sends what the event loop queues for one link, hands what the other computer sends to the
/// event loop, and keeps the connection alive.
fn pump(
    session: &mut Session,
    queue: &Receiver<Message>,
    stop: &AtomicBool,
    events: &Sender<LinkEvent>,
    device: &str,
    id: u64,
) -> std::result::Result<(), NetError> {
    let mut last_heard = Instant::now();
    let mut last_ping = last_heard;
    let mut pings = 0u64;

    while !stop.load(Ordering::Relaxed) {
        loop {
            match queue.try_recv() {
                Ok(first) => {
                    for message in coalesce(first, queue) {
                        session.send(&message)?;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                // The event loop dropped its end: the mesh is stopping, or this link was
                // replaced by a newer one to the same device.
                Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
        }

        // Waits here for the other computer; a message that comes is dealt with at once.
        if let Some(message) = session.recv_timeout(INBOUND_WAIT)? {
            last_heard = Instant::now();
            match message {
                Message::Ping(n) => session.send(&Message::Pong(n))?,
                Message::Pong(_) => {}
                other => {
                    let event = LinkEvent::Message { device: device.to_owned(), id, message: other };
                    if events.send(event).is_err() {
                        return Ok(());
                    }
                }
            }
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


    #[derive(Default)]
    struct Recorder(Vec<String>);

    impl InputSink for Recorder {
        fn move_abs(&mut self, x: i32, y: i32) -> PlatformResult<()> {
            self.0.push(format!("abs {x} {y}"));
            Ok(())
        }
        fn move_rel(&mut self, dx: i32, dy: i32) -> PlatformResult<()> {
            self.0.push(format!("rel {dx} {dy}"));
            Ok(())
        }
        fn button(&mut self, button: MouseButton, state: ButtonState) -> PlatformResult<()> {
            self.0.push(format!("button {button:?} {state:?}"));
            Ok(())
        }
        fn wheel(&mut self, dx: i32, dy: i32) -> PlatformResult<()> {
            self.0.push(format!("wheel {dx} {dy}"));
            Ok(())
        }
        fn key(&mut self, usage: u16, state: ButtonState) -> PlatformResult<()> {
            self.0.push(format!("key {usage:#x} {state:?}"));
            Ok(())
        }
        fn release_all(&mut self) -> PlatformResult<()> {
            self.0.push("release".into());
            Ok(())
        }
    }

    #[test]
    fn entering_moves_the_cursor_and_presses_the_modifiers_that_were_held() {
        let mut sink = Recorder::default();
        // Left Ctrl (bit 0) and right Shift (bit 5).
        let modifiers = Modifiers(0b0010_0001);
        inject(&Message::Enter { x: 0, y: 720, seq: 1, modifiers }, (0, 0), &mut sink).unwrap();
        assert_eq!(sink.0, vec!["abs 0 720", "key 0xe0 Down", "key 0xe5 Down"]);
    }

    #[test]
    fn entering_is_measured_from_the_corner_of_all_the_displays() {
        let mut sink = Recorder::default();
        // A computer with a display left of its main one: the corner is 1920 left of the origin.
        inject(&Message::Enter { x: 100, y: 50, seq: 1, modifiers: Modifiers(0) }, (-1920, 0), &mut sink)
            .unwrap();
        assert_eq!(sink.0, vec!["abs -1820 50"]);
    }

    #[test]
    fn leaving_releases_everything() {
        let mut sink = Recorder::default();
        inject(&Message::Leave, (0, 0), &mut sink).unwrap();
        assert_eq!(sink.0, vec!["release"]);
    }

    #[test]
    fn input_messages_map_onto_the_sink() {
        let mut sink = Recorder::default();
        inject(&Message::MouseMove { dx: 3, dy: -4 }, (0, 0), &mut sink).unwrap();
        inject(&Message::Wheel { dx: 0, dy: 120 }, (0, 0), &mut sink).unwrap();
        inject(
            &Message::MouseButton { button: MouseButton::Right, state: ButtonState::Up },
            (0, 0),
            &mut sink,
        )
        .unwrap();
        inject(&Message::Key { usage: 0x04, state: ButtonState::Down }, (0, 0), &mut sink).unwrap();
        assert_eq!(
            sink.0,
            vec!["rel 3 -4", "wheel 0 120", "button Right Up", "key 0x4 Down"]
        );
    }

    #[test]
    fn unrelated_messages_are_ignored() {
        let mut sink = Recorder::default();
        inject(&Message::Pong(1), (0, 0), &mut sink).unwrap();
        inject(&Message::Screen { displays: vec![], scales: vec![], os: Os::Other }, (0, 0), &mut sink).unwrap();
        assert!(sink.0.is_empty());
    }
}
