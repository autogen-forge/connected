//! What the app knows and does, apart from how it looks.
//!
//! The screen reads this and tells it what the person asked for. Keeping the two apart
//! means everything the app does can be tested without a window or an operating system's
//! permission to inject input.

use std::collections::{BTreeMap, VecDeque};
use std::net::UdpSocket;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use connected_app::{
    AppError, Log, MeshBackends, MeshHandle, MeshStart, MeshState, PairingEvent, PairingSession,
    Settings, now_ms,
};
use connected_core::{Desktop, Metrics, Os, PlaceError, Rect, default_displays, normalize};
use connected_net::{Fingerprint, Identity, Node, TrustStore, TrustedPeer};

/// The port a computer listens on unless told otherwise.
pub const MESH_PORT: u16 = 24900;
/// The port a computer waits on while pairing. Separate from the mesh's port, so pairing
/// can be done while the computers are connected.
pub const PAIRING_PORT: u16 = 24901;

const TRUST_FILE: &str = "trusted_peers";
const LOG_LINES: usize = 1000;

type MeshFactory = Arc<dyn Fn() -> Result<MeshBackends, AppError> + Send + Sync>;
/// A computer's displays and how much each is magnified, in percent.
pub type LocalScreen = (Vec<Rect>, Vec<u16>);
type ScreenSizeFactory = Arc<dyn Fn() -> Option<LocalScreen> + Send + Sync>;

/// Where input comes from and goes to. The real thing talks to the operating system; tests
/// and the demo use in-memory stand-ins.
#[derive(Clone)]
pub struct Backends {
    pub mesh: MeshFactory,
    /// This computer's displays, if they can be found out.
    pub local_screen: ScreenSizeFactory,
}

impl Backends {
    pub fn system() -> Self {
        Self {
            mesh: Arc::new(|| {
                Ok(MeshBackends {
                    source: connected_platform::open_source()?,
                    sink: connected_platform::open_sink()?,
                    screen: connected_platform::open_screen()?,
                })
            }),
            local_screen: Arc::new(|| {
                let screen = connected_platform::open_screen().ok()?;
                Some((screen.displays().ok()?, screen.scales().ok()?))
            }),
        }
    }
}

/// The activity log: the latest lines, each stamped with the time since the app started.
#[derive(Clone)]
pub struct LogBuffer {
    lines: Arc<Mutex<VecDeque<String>>>,
    started: Instant,
}

impl LogBuffer {
    pub fn new() -> Self {
        Self { lines: Arc::default(), started: Instant::now() }
    }

    pub fn push(&self, line: &str) {
        let elapsed = self.started.elapsed().as_secs();
        let stamped = format!("{:02}:{:02}  {line}", elapsed / 60, elapsed % 60);
        if let Ok(mut lines) = self.lines.lock() {
            if lines.len() == LOG_LINES {
                lines.pop_front();
            }
            lines.push_back(stamped);
        }
    }

    pub fn writer(&self) -> Log {
        let buffer = self.clone();
        Arc::new(move |line: &str| buffer.push(line))
    }

    pub fn snapshot(&self) -> Vec<String> {
        self.lines.lock().map(|lines| lines.iter().cloned().collect()).unwrap_or_default()
    }
}

impl Default for LogBuffer {
    fn default() -> Self {
        Self::new()
    }
}

/// How far a pairing has got, as the screen shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingStage {
    /// Not started: the person picks whether to wait or to connect.
    Choosing,
    /// Listening; the other computer should connect to `addr`.
    Waiting { addr: String },
    Connecting { addr: String },
    /// Both computers are linked. The person compares `code` with the other screen.
    Confirming { peer_name: String, peer_fingerprint: String, code: String },
    /// Over: the name of the computer that was paired, or what went wrong.
    Done(Result<String, String>),
}

pub struct PairingFlow {
    session: Option<PairingSession>,
    pub stage: PairingStage,
}

impl PairingFlow {
    fn new() -> Self {
        Self { session: None, stage: PairingStage::Choosing }
    }

    /// Moves along; returns the computer that was paired, if a pairing just finished.
    fn poll(&mut self) -> Option<connected_net::Peer> {
        let events: Vec<PairingEvent> = match &self.session {
            Some(session) => std::iter::from_fn(|| session.poll()).collect(),
            None => return None,
        };
        let mut paired = None;
        for event in events {
            self.stage = match event {
                PairingEvent::Waiting { addr } => PairingStage::Waiting { addr },
                PairingEvent::Confirm(prompt) => PairingStage::Confirming {
                    peer_name: prompt.peer_name,
                    peer_fingerprint: prompt.peer_fingerprint.short(),
                    code: prompt.code,
                },
                PairingEvent::Finished(outcome) => {
                    self.session = None;
                    if let Ok(peer) = &outcome {
                        paired = Some(peer.clone());
                    }
                    PairingStage::Done(outcome.map(|peer| peer.name))
                }
            };
        }
        paired
    }
}

/// A paired computer and whether it is connected to this one right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub name: String,
    pub fingerprint: Fingerprint,
    pub online: bool,
    /// Its displays, while it is connected.
    pub displays: Option<Vec<Rect>>,
    /// Where this computer reaches it, as `address:port`, if it is known.
    pub address: Option<String>,
}

pub struct AppModel {
    home: Option<PathBuf>,
    node: Node,
    backends: Backends,
    pub settings: Settings,
    mesh: Option<MeshHandle>,
    pub pairing: Option<PairingFlow>,
    logs: LogBuffer,
    local_displays: Vec<Rect>,
    local_scales: Vec<u16>,
    /// Whether the system was last seen to be withholding a permission this program needs.
    permissions_missing: bool,
    /// Something the person should be told, such as why starting failed.
    pub notice: Option<String>,
}

impl AppModel {
    /// Opens the app's state in `home`: this computer's identity, who it is paired with, and
    /// its settings. All are created on first use.
    pub fn open(home: &Path, backends: Backends) -> Result<Self, AppError> {
        let name = gethostname::gethostname().to_string_lossy().into_owned();
        let identity = Identity::load_or_create(home, &name)?;
        let trust = TrustStore::load(&home.join(TRUST_FILE))?;
        let (settings, note) = Settings::load(home);
        let mut model = Self::assemble(Some(home.to_owned()), Node::new(identity, trust), settings, backends);
        model.notice = note;
        Ok(model)
    }

    /// State that is never saved, for tests and the demo.
    pub fn in_memory(name: &str, backends: Backends) -> Result<Self, AppError> {
        let node = Node::new(Identity::generate(name)?, TrustStore::in_memory());
        Ok(Self::assemble(None, node, Settings::default(), backends))
    }

    fn assemble(home: Option<PathBuf>, node: Node, settings: Settings, backends: Backends) -> Self {
        let (local_displays, local_scales) = (backends.local_screen)()
            .filter(|(displays, _)| !displays.is_empty())
            .map(|(displays, mut scales)| {
                scales.resize(displays.len(), 100);
                (normalize(&displays).0, scales)
            })
            .unwrap_or_else(|| (default_displays(), vec![100]));
        Self {
            local_displays,
            local_scales,
            permissions_missing: false,
            home,
            node,
            backends,
            settings,
            mesh: None,
            pairing: None,
            logs: LogBuffer::new(),
            notice: None,
        }
    }

    pub fn node(&self) -> &Node {
        &self.node
    }

    /// This computer's displays, measured from the corner of all of them.
    pub fn local_displays(&self) -> &[Rect] {
        &self.local_displays
    }

    /// Takes in whether a permission is missing, as the system says right now. When the last one
    /// is granted, a notice that starting failed is taken down: it was most likely about that,
    /// and the person should not have to dismiss it by hand. Other notices stay.
    pub fn note_permissions(&mut self, missing: bool) {
        let granted_now = self.permissions_missing && !missing;
        self.permissions_missing = missing;
        if granted_now && self.notice.as_deref().is_some_and(is_start_failure) {
            self.notice = None;
            self.log_line("permissions were granted");
        }
    }

    /// How much each of this computer's displays is magnified, in percent.
    pub fn local_scales(&self) -> &[u16] {
        &self.local_scales
    }

    pub fn logs(&self) -> Vec<String> {
        self.logs.snapshot()
    }

    pub fn log_line(&self, line: &str) {
        self.logs.push(line);
    }

    pub fn this_computer(&self) -> &str {
        self.node.identity().name()
    }

    pub fn fingerprint(&self) -> Fingerprint {
        self.node.identity().fingerprint()
    }

    // ---- running ----

    pub fn is_running(&self) -> bool {
        self.mesh.is_some()
    }

    pub fn mesh_state(&self) -> Option<MeshState> {
        self.mesh.as_ref().map(MeshHandle::state)
    }

    /// Starts listening for the paired computers and connecting to them.
    pub fn start(&mut self) {
        if self.is_running() {
            return;
        }
        self.notice = None;
        let factory = Arc::clone(&self.backends.mesh);
        let start = MeshStart {
            arrangement: self.settings.arrangement.clone(),
            layout_version: self.settings.layout_version,
            peer_addresses: self.settings.peer_addresses.clone(),
        };
        let started = MeshHandle::start(
            self.node.clone(),
            &self.settings.listen,
            start,
            move || factory(),
            self.logs.writer(),
        );
        match started {
            Ok(mesh) => {
                self.log_line(&format!("started, listening on {}", mesh.state().listening_on));
                self.mesh = Some(mesh);
            }
            Err(err) => self.notice = Some(format!("연결을 시작하지 못했습니다: {err}")),
        }
    }

    pub fn stop(&mut self) {
        if let Some(mut mesh) = self.mesh.take() {
            mesh.stop();
            self.log_line("stopped");
        }
    }

    /// Sets where to reach a paired computer. An empty address forgets it.
    pub fn set_peer_address(&mut self, device: &str, address: &str) {
        let address = address.trim();
        let address = (!address.is_empty()).then(|| with_default_port(address, MESH_PORT));
        match &address {
            Some(address) => self.settings.peer_addresses.insert(device.to_owned(), address.clone()),
            None => self.settings.peer_addresses.remove(device),
        };
        if let Some(mesh) = &self.mesh {
            mesh.set_peer_address(device, address);
        }
        self.save();
    }

    // ---- layout ----

    /// Puts a paired computer with its top-left corner at (`x`, `y`) on the shared desktop, or
    /// moves it there.
    pub fn place_device(&mut self, device: &str, x: i32, y: i32) {
        let (name, sizes, local) = (self.this_computer().to_owned(), self.known_displays(), self.local_displays.clone());
        let metrics = Metrics { this_computer: &name, local: &local, devices: &sizes };
        match self.settings.arrangement.place(device, x, y, &metrics) {
            Ok(()) => self.arrangement_changed(),
            Err(err) => self.notice = Some(place_error_text(&err)),
        }
    }

    /// Moves this computer's own screen on the shared desktop.
    pub fn move_this_computer(&mut self, x: i32, y: i32) {
        let (name, sizes, local) = (self.this_computer().to_owned(), self.known_displays(), self.local_displays.clone());
        let metrics = Metrics { this_computer: &name, local: &local, devices: &sizes };
        match self.settings.arrangement.move_this_computer(x, y, &metrics) {
            Ok(()) => self.arrangement_changed(),
            Err(err) => self.notice = Some(place_error_text(&err)),
        }
    }

    /// Moves several computers at once, by the same amount. `local` is whether this computer's
    /// own screen is among them.
    pub fn move_group(&mut self, devices: &[String], local: bool, dx: i32, dy: i32) {
        let (name, sizes, own) = (self.this_computer().to_owned(), self.known_displays(), self.local_displays.clone());
        let metrics = Metrics { this_computer: &name, local: &own, devices: &sizes };
        match self.settings.arrangement.translate(devices, local, dx, dy, &metrics) {
            Ok(()) => self.arrangement_changed(),
            Err(err) => self.notice = Some(place_error_text(&err)),
        }
    }

    /// Every placed screen, this computer's included, as the router will see them if all
    /// of those computers are connected.
    pub fn desktop(&self) -> Desktop {
        let sizes = self.known_displays();
        let metrics = Metrics {
            this_computer: self.this_computer(),
            local: &self.local_displays,
            devices: &sizes,
        };
        self.settings.arrangement.desktop(&metrics)
    }

    /// How much each display of each paired computer is magnified: as it says now if it is
    /// connected, as it said last time otherwise.
    pub fn known_scales(&self) -> BTreeMap<String, Vec<u16>> {
        let mut known = self.settings.known_scales.clone();
        for peer in self.mesh_state().map(|state| state.peers).unwrap_or_default() {
            known.insert(peer.name, peer.scales);
        }
        known
    }

    /// The system each paired computer runs: as it says now if it is connected, as it said
    /// last time otherwise.
    pub fn known_systems(&self) -> BTreeMap<String, Os> {
        let mut known = self.settings.known_os.clone();
        for peer in self.mesh_state().map(|state| state.peers).unwrap_or_default() {
            known.insert(peer.name, peer.os);
        }
        known
    }

    /// The displays of each paired computer: what it has now if it is connected, what it
    /// had last time otherwise.
    pub fn known_displays(&self) -> BTreeMap<String, Vec<Rect>> {
        let mut known = self.settings.known_screens.clone();
        for peer in self.mesh_state().map(|state| state.peers).unwrap_or_default() {
            known.insert(peer.name, peer.displays);
        }
        known
    }

    pub fn remove_from_layout(&mut self, device: &str) {
        if self.settings.arrangement.remove(device) {
            self.arrangement_changed();
        }
    }

    /// The person changed the layout: it gets a newer version, goes to every connected computer,
    /// and is kept.
    fn arrangement_changed(&mut self) {
        self.settings.layout_version = now_ms().max(self.settings.layout_version + 1);
        if let Some(mesh) = &self.mesh {
            mesh.set_arrangement(self.settings.arrangement.clone(), self.settings.layout_version);
        }
        self.save();
    }

    /// Takes in a layout that another computer sent, if it is newer than this one's.
    fn adopt_shared_layout(&mut self) {
        let Some(state) = self.mesh_state() else {
            return;
        };
        if state.layout_version > self.settings.layout_version {
            self.settings.arrangement = state.arrangement;
            self.settings.layout_version = state.layout_version;
            self.save();
        }
    }

    // ---- paired computers ----

    /// Everyone this computer is paired with, with who is connected right now.
    pub fn devices(&self) -> Vec<Device> {
        let connected = self.mesh_state().map(|state| state.peers).unwrap_or_default();
        let mut devices: Vec<Device> = self
            .node
            .trusted_peers()
            .into_iter()
            .map(|TrustedPeer { name, fingerprint }| {
                let peer = connected.iter().find(|peer| peer.name == name);
                Device {
                    online: peer.is_some(),
                    displays: peer.map(|peer| peer.displays.clone()),
                    address: self.settings.peer_addresses.get(&name).cloned(),
                    name,
                    fingerprint,
                }
            })
            .collect();
        devices.sort_by(|a, b| a.name.cmp(&b.name));
        devices
    }

    /// Stops trusting a computer, takes it off the layout and cuts its connection.
    pub fn forget_device(&mut self, name: &str) {
        match self.node.forget(name) {
            Ok(removed) if !removed.is_empty() => {
                self.log_line(&format!("'{name}' is no longer paired"));
            }
            Ok(_) => return,
            Err(err) => {
                self.notice = Some(format!("기기를 삭제하지 못했습니다: {err}"));
                return;
            }
        }
        if let Some(mesh) = &self.mesh {
            mesh.set_peer_address(name, None);
            mesh.disconnect(name);
        }
        self.settings.peer_addresses.remove(name);
        self.settings.known_screens.remove(name);
        self.settings.known_os.remove(name);
        self.settings.known_scales.remove(name);
        self.remove_from_layout(name);
        self.save();
    }

    // ---- pairing ----

    /// Opens the pairing window, ready to choose how.
    pub fn open_pairing(&mut self) {
        self.cancel_pairing();
        self.pairing = Some(PairingFlow::new());
    }

    /// Waits for another computer to connect and pair.
    pub fn pair_by_waiting(&mut self) {
        let address = format!("0.0.0.0:{PAIRING_PORT}");
        let outcome = PairingSession::listen(self.node.clone(), &address);
        self.begin_pairing(outcome.map_err(|err| format!("대기를 시작하지 못했습니다: {err}")), None);
    }

    /// Connects to a computer that is waiting, and pairs.
    pub fn pair_by_connecting(&mut self, address: &str) {
        let address = with_default_port(address.trim(), PAIRING_PORT);
        let session = PairingSession::connect(self.node.clone(), address.clone());
        self.begin_pairing(Ok(session), Some(address));
    }

    fn begin_pairing(&mut self, session: Result<PairingSession, String>, connecting_to: Option<String>) {
        let flow = self.pairing.get_or_insert_with(PairingFlow::new);
        match session {
            Ok(session) => {
                flow.session = Some(session);
                if let Some(addr) = connecting_to {
                    flow.stage = PairingStage::Connecting { addr };
                }
            }
            Err(message) => flow.stage = PairingStage::Done(Err(message)),
        }
    }

    /// The person's answer to "do both screens show the same code?".
    pub fn answer_pairing(&mut self, codes_match: bool) {
        if let Some(PairingFlow { session: Some(session), .. }) = &self.pairing {
            session.answer(codes_match);
        }
    }

    /// Gives up on the pairing in progress, if any.
    pub fn cancel_pairing(&mut self) {
        if let Some(PairingFlow { session: Some(session), .. }) = &self.pairing {
            session.cancel();
        }
    }

    pub fn close_pairing(&mut self) {
        self.cancel_pairing();
        if let Some(PairingFlow { stage: PairingStage::Done(Ok(name)), .. }) = &self.pairing {
            self.logs.push(&format!("paired with '{name}'"));
        }
        self.pairing = None;
    }

    // ---- housekeeping ----

    /// Call regularly: moves along a pairing, takes in layouts other computers sent, and
    /// notices when the mesh ended on its own.
    pub fn tick(&mut self) {
        let paired = self.pairing.as_mut().and_then(PairingFlow::poll);
        if let Some(peer) = paired {
            self.learn_address(&peer);
        }

        if let Some(mesh) = &self.mesh {
            if !mesh.is_running() {
                let error = mesh.state().error;
                self.mesh = None;
                if let Some(error) = error {
                    self.notice = Some(format!("연결이 멈췄습니다: {error}"));
                }
            }
        }
        self.adopt_shared_layout();
        self.remember_screen_sizes();
    }

    /// A computer was just paired: remember where it is, so the two can find each other again.
    fn learn_address(&mut self, peer: &connected_net::Peer) {
        let Some(ip) = peer.address else {
            return;
        };
        let address = std::net::SocketAddr::new(ip, MESH_PORT).to_string();
        self.log_line(&format!("'{}' is at {address}", peer.name));
        self.settings.peer_addresses.insert(peer.name.clone(), address.clone());
        if let Some(mesh) = &self.mesh {
            mesh.set_peer_address(&peer.name, Some(address));
        }
        self.save();
    }

    /// Keeps the displays and system of each connected computer for when it is switched off.
    fn remember_screen_sizes(&mut self) {
        let known = self.known_displays();
        let systems = self.known_systems();
        let scales = self.known_scales();
        if known != self.settings.known_screens
            || systems != self.settings.known_os
            || scales != self.settings.known_scales
        {
            self.settings.known_screens = known;
            self.settings.known_os = systems;
            self.settings.known_scales = scales;
            self.save();
        }
    }

    fn save(&mut self) {
        let Some(home) = &self.home else {
            return;
        };
        if let Err(err) = self.settings.save(home) {
            self.notice = Some(err.to_string());
        }
    }
}

impl Drop for AppModel {
    fn drop(&mut self) {
        self.stop();
        self.cancel_pairing();
    }
}

/// Whether a notice says that connecting could not be started.
fn is_start_failure(notice: &str) -> bool {
    notice.starts_with("연결을 시작하지 못했습니다")
}

/// Why a screen cannot be put in a spot, in words for the person using the app.
fn place_error_text(error: &PlaceError) -> String {
    match error {
        PlaceError::Overlaps(name) => format!("‘{name}’ 화면과 겹치게 놓을 수 없습니다."),
    }
}

/// This computer's address on the local network, if it has one. Found by asking the system
/// which interface it would use to reach an outside address; nothing is sent.
pub fn lan_address() -> Option<String> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    Some(socket.local_addr().ok()?.ip().to_string())
}

/// Adds `port` to an address that has none, so that people can type just `192.168.0.5`.
pub fn with_default_port(address: &str, port: u16) -> String {
    let address = address.trim();
    let has_port = match address.strip_prefix('[') {
        // [::1]:1234
        Some(rest) => rest.contains("]:"),
        None => address.matches(':').count() == 1,
    };
    if has_port || address.is_empty() { address.to_owned() } else { format!("{address}:{port}") }
}

#[cfg(test)]
pub(crate) mod testing;

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::testing::{mock_backends, model, pair, wait_until};
    use super::*;
    use connected_core::{DEFAULT_SIZE, Point, Spot};

    fn failing_backends() -> Backends {
        Backends {
            mesh: Arc::new(|| Err(AppError::Config("input is not allowed".into()))),
            local_screen: Arc::new(|| None),
        }
    }

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("connected-gui-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_new_model_is_idle_with_no_devices() {
        let model = model("desk");
        assert!(!model.is_running());
        assert!(model.devices().is_empty());
        assert_eq!(model.this_computer(), "desk");
        assert!(model.mesh_state().is_none());
    }

    #[test]
    fn connecting_starts_and_stops() {
        let mut model = model("desk");
        model.start();
        assert!(model.is_running());
        let state = model.mesh_state().unwrap();
        assert!(state.running);
        assert_eq!(state.this_computer, "desk");
        assert!(model.notice.is_none());

        model.stop();
        assert!(!model.is_running());
        assert!(model.logs().iter().any(|line| line.contains("started")));
        assert!(model.logs().iter().any(|line| line.contains("stopped")));
    }

    #[test]
    fn a_failure_to_start_is_explained() {
        let mut model = AppModel::in_memory("desk", failing_backends()).unwrap();
        model.settings.listen = "127.0.0.1:0".into();
        model.start();
        assert!(!model.is_running());
        assert!(model.notice.as_deref().unwrap().contains("input is not allowed"));
    }

    #[test]
    fn granting_the_last_permission_takes_down_the_notice_that_starting_failed() {
        let mut model = model("desk");
        model.notice = Some("연결을 시작하지 못했습니다: permission denied".into());
        model.note_permissions(true);
        assert!(model.notice.is_some(), "still missing, so the notice stays");
        model.note_permissions(false);
        assert!(model.notice.is_none());
    }

    #[test]
    fn granting_a_permission_leaves_other_notices_alone() {
        let mut model = model("desk");
        model.note_permissions(true);
        model.notice = Some("‘laptop’ 화면과 겹치게 놓을 수 없습니다.".into());
        model.note_permissions(false);
        assert!(model.notice.is_some());
    }

    #[test]
    fn a_notice_is_not_taken_down_if_nothing_was_ever_missing() {
        let mut model = model("desk");
        model.notice = Some("연결을 시작하지 못했습니다: refused".into());
        model.note_permissions(false);
        assert!(model.notice.is_some());
    }

    #[test]
    fn devices_are_placed_moved_and_removed() {
        let mut model = model("desk");
        model.place_device("laptop", 1920, 0);
        assert_eq!(model.settings.arrangement.spot_of("laptop"), Some(Spot::At { x: 1920, y: 0 }));

        model.place_device("laptop", -1920, 200);
        assert_eq!(model.settings.arrangement.spot_of("laptop"), Some(Spot::At { x: -1920, y: 200 }));

        model.remove_from_layout("laptop");
        assert_eq!(model.settings.arrangement.spot_of("laptop"), None);
        assert!(model.notice.is_none());
    }

    #[test]
    fn placing_on_top_of_another_screen_is_refused_with_a_message() {
        let mut model = model("desk");
        model.place_device("laptop", 1920, 0);
        model.place_device("tablet", 2000, 100);
        assert_eq!(model.settings.arrangement.spot_of("tablet"), None);
        assert!(model.notice.as_deref().unwrap().contains("laptop"));

        model.notice = None;
        model.place_device("tablet", 100, 100);
        assert!(model.notice.as_deref().unwrap().contains("desk"), "this computer's own screen too");
    }

    #[test]
    fn this_computer_can_be_moved_but_not_onto_a_device() {
        let mut model = model("desk");
        model.place_device("laptop", 1920, 0);
        model.move_this_computer(0, 500);
        assert_eq!(model.settings.arrangement.local, Point { x: 0, y: 500 });
        assert!(model.notice.is_none());
        model.move_this_computer(1500, 0);
        assert!(model.notice.is_some());
        assert_eq!(model.settings.arrangement.local, Point { x: 0, y: 500 });
    }

    #[test]
    fn the_desktop_has_this_computer_and_the_placed_devices_to_scale() {
        let mut model = model("desk");
        model.place_device("laptop", 1920, 100);
        model.place_device("tv", -1920, 0);
        let desktop = model.desktop();
        assert_eq!(desktop.rect("desk"), Some(Rect::new(0, 0, 1920, 1080)));
        // Not connected, so its size is a guess.
        assert_eq!(desktop.rect("laptop"), Some(Rect::new(1920, 100, DEFAULT_SIZE.0, DEFAULT_SIZE.1)));
        assert_eq!(desktop.screens().len(), 3);
    }

    #[test]
    fn layout_and_settings_are_saved_and_come_back() {
        let home = temp_dir("persist");
        {
            let mut model = AppModel::open(&home, mock_backends()).unwrap();
            model.place_device("laptop", 1920, 40);
            model.set_peer_address("laptop", "10.0.0.5");
        }
        let model = AppModel::open(&home, mock_backends()).unwrap();
        assert_eq!(model.settings.arrangement.spot_of("laptop"), Some(Spot::At { x: 1920, y: 40 }));
        assert_eq!(model.settings.peer_addresses.get("laptop").map(String::as_str), Some("10.0.0.5:24900"));
        assert!(model.settings.layout_version > 0, "a change gets a version");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn the_identity_is_kept_between_runs() {
        let home = temp_dir("identity");
        let first = AppModel::open(&home, mock_backends()).unwrap().fingerprint();
        let second = AppModel::open(&home, mock_backends()).unwrap().fingerprint();
        assert_eq!(first, second);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn pairing_adds_a_device_to_both_lists() {
        let (mut desk, mut laptop) = (model("desk"), model("laptop"));
        pair(&mut desk, &mut laptop);

        assert_eq!(
            desk.pairing.as_ref().unwrap().stage,
            PairingStage::Done(Ok("laptop".to_owned()))
        );
        assert_eq!(desk.devices().len(), 1);
        assert_eq!(desk.devices()[0].name, "laptop");
        assert!(!desk.devices()[0].online);
        assert_eq!(laptop.devices()[0].name, "desk");

        desk.close_pairing();
        assert!(desk.pairing.is_none());
        assert!(desk.logs().iter().any(|line| line.contains("paired with 'laptop'")));
    }

    #[test]
    fn a_waiting_pairing_can_be_cancelled() {
        let mut desk = model("desk");
        desk.open_pairing();
        let session = PairingSession::listen(desk.node.clone(), "127.0.0.1:0").unwrap();
        desk.pairing.as_mut().unwrap().session = Some(session);
        desk.cancel_pairing();
        wait_until("the cancellation to show", || {
            desk.tick();
            matches!(desk.pairing.as_ref().map(|flow| &flow.stage), Some(PairingStage::Done(Err(_))))
        });
    }

    #[test]
    fn forgetting_a_device_unpairs_it_and_clears_its_place() {
        let (mut desk, mut laptop) = (model("desk"), model("laptop"));
        pair(&mut desk, &mut laptop);
        desk.place_device("laptop", 1920, 0);
        desk.settings.known_screens.insert("laptop".into(), vec![Rect::new(0, 0, 800, 600)]);

        desk.forget_device("laptop");
        assert!(desk.devices().is_empty());
        assert_eq!(desk.settings.arrangement.spot_of("laptop"), None);
        assert!(!desk.settings.known_screens.contains_key("laptop"));
    }

    #[test]
    fn pairing_remembers_where_the_other_computer_is() {
        let (mut desk, mut laptop) = (model("desk"), model("laptop"));
        pair(&mut desk, &mut laptop);
        // Both are on this machine, so each reaches the other at the loopback address.
        assert_eq!(desk.devices()[0].address.as_deref(), Some("127.0.0.1:24900"));
        assert_eq!(laptop.devices()[0].address.as_deref(), Some("127.0.0.1:24900"));
    }

    #[test]
    fn a_connected_device_shows_as_online_and_shares_the_layout() {
        let (mut desk, mut laptop) = (model("desk"), model("laptop"));
        pair(&mut desk, &mut laptop);
        desk.place_device("laptop", 1920, 0);
        desk.start();
        laptop.start();
        desk.set_peer_address("laptop", &laptop.mesh_state().unwrap().listening_on);
        laptop.set_peer_address("desk", &desk.mesh_state().unwrap().listening_on);

        let deadline = Instant::now() + Duration::from_secs(8);
        while !(desk.devices().iter().any(|d| d.online) && laptop.devices().iter().any(|d| d.online)) {
            assert!(
                Instant::now() < deadline,
                "they never showed as online\ndesk log: {:#?}\nlaptop log: {:#?}",
                desk.logs(),
                laptop.logs(),
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let device = desk.devices().remove(0);
        assert_eq!(device.displays, Some(vec![Rect::new(0, 0, 1920, 1080)]));

        // The laptop has the layout the desk made, with itself beside the desk.
        wait_until("the laptop to take the layout", || {
            laptop.tick();
            laptop.settings.arrangement.contains("desk")
        });
        assert_eq!((laptop.settings.arrangement.local.x, laptop.settings.arrangement.local.y), (1920, 0));
        assert_eq!(laptop.settings.layout_version, desk.settings.layout_version);
        desk.tick();
        assert_eq!(desk.settings.known_screens.get("laptop"), Some(&vec![Rect::new(0, 0, 1920, 1080)]), "remembered for later");

        // Forgetting it cuts it off at once.
        desk.forget_device("laptop");
        wait_until("the laptop to be cut off", || {
            desk.mesh_state().is_some_and(|state| state.peers.is_empty())
        });
    }

    #[test]
    fn the_log_keeps_only_the_latest_lines() {
        let logs = LogBuffer::new();
        for n in 0..(LOG_LINES + 25) {
            logs.push(&format!("line {n}"));
        }
        let lines = logs.snapshot();
        assert_eq!(lines.len(), LOG_LINES);
        assert!(lines[0].ends_with("line 25"));
        assert!(lines.last().unwrap().ends_with(&format!("line {}", LOG_LINES + 24)));
        assert!(lines[0].starts_with("00:00"));
    }

    #[test]
    fn a_missing_port_gets_the_default() {
        assert_eq!(with_default_port("192.168.0.5", 24900), "192.168.0.5:24900");
        assert_eq!(with_default_port("192.168.0.5:9000", 24900), "192.168.0.5:9000");
        assert_eq!(with_default_port("desk.local", 24901), "desk.local:24901");
        assert_eq!(with_default_port("  10.0.0.1  ", 24900), "10.0.0.1:24900");
        assert_eq!(with_default_port("[::1]", 24900), "[::1]:24900");
        assert_eq!(with_default_port("[::1]:5", 24900), "[::1]:5");
        assert_eq!(with_default_port("", 24900), "");
    }

    #[test]
    fn this_computer_has_a_lan_address_or_none() {
        // Sandboxes may have no network; either answer is fine, but it must not panic.
        if let Some(address) = lan_address() {
            assert!(address.parse::<std::net::IpAddr>().is_ok());
        }
    }
}
