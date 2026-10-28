//! Everything the window needs to draw itself, as one value.
//!
//! The window asks for a snapshot several times a second and draws whatever it gets, so it
//! holds no state of its own about connections. All wording is left to the window; this only
//! says what is true.

use std::collections::BTreeMap;

use connected_core::{Os, Rect, default_displays};
use serde::Serialize;

use crate::model::{
    AppModel, Device, MESH_PORT, PAIRING_PORT, PairingStage, lan_address,
};
use crate::state::AppState;

/// The log lines that are sent along; the window shows the latest ones.
const LOG_LINES: usize = 300;

#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub this_computer: String,
    pub fingerprint: String,
    pub fingerprint_short: String,
    /// Whether this computer is connecting to the others.
    pub running: bool,
    pub mesh_port: u16,
    pub pairing_port: u16,
    /// This computer's address on the local network, if it has one.
    pub lan_address: Option<String>,
    /// True when the app is running with stand-in computers.
    pub demo: bool,
    pub mesh: Option<MeshView>,
    /// This computer's screen and the placed ones, on the shared desktop.
    pub screens: Vec<ScreenView>,
    pub devices: Vec<DeviceView>,
    pub pairing: Option<PairingView>,
    pub notice: Option<String>,
    pub permissions: Vec<PermissionView>,
    pub logs: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ScreenView {
    pub name: String,
    /// The top-left corner on the shared desktop, in pixels.
    pub x: i32,
    pub y: i32,
    /// The size of the rectangle holding all its displays.
    pub width: i32,
    pub height: i32,
    /// Whether it is this computer's own screen.
    pub local: bool,
    /// Its displays, measured from (`x`, `y`).
    pub displays: Vec<Rect>,
    /// How much each of its displays is magnified, in percent.
    pub scales: Vec<u16>,
    /// The system it runs.
    pub os: Os,
    /// Whether it is connected. This computer always is.
    pub online: bool,
}

#[derive(Debug, Serialize)]
pub struct MeshView {
    pub listening_on: String,
    /// The screen that has the cursor right now: this computer's name or another's.
    pub active_screen: String,
    /// The computer whose cursor is on this one, if there is one.
    pub controlled_by: Option<String>,
    pub peers: Vec<PeerView>,
}

#[derive(Debug, Serialize)]
pub struct PeerView {
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub displays: Vec<Rect>,
    pub from: String,
}

#[derive(Debug, Serialize)]
pub struct DeviceView {
    pub name: String,
    pub fingerprint: String,
    pub fingerprint_short: String,
    pub online: bool,
    /// The size of all its displays together while it is connected.
    pub screen: Option<(i32, i32)>,
    /// Its displays now, or the last time it connected; a usual one if never seen. Measured
    /// from the corner of all of them.
    pub displays: Vec<Rect>,
    /// The system it runs, as last reported.
    pub os: Os,
    /// Where this computer reaches it, as `address:port`, if that is known.
    pub address: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PairingView {
    /// `choosing`, `waiting`, `connecting`, `confirming` or `done`.
    pub stage: &'static str,
    pub addr: Option<String>,
    pub peer_name: Option<String>,
    pub peer_fingerprint: Option<String>,
    pub code: Option<String>,
    /// How it ended, once `stage` is `done`.
    pub ok: Option<bool>,
    /// The name that was paired, or what went wrong.
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PermissionView {
    pub name: &'static str,
    pub granted: bool,
}

/// Takes a snapshot. This is also what keeps the app moving: the model has to be ticked
/// regularly to take in what its background threads report, such as pairing progress, and
/// the window asks for a snapshot every fraction of a second.
pub fn snapshot(state: &AppState) -> Snapshot {
    let permissions = state.permissions();
    let mut model = state.model.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    model.tick();
    if state.checks_permissions() {
        model.note_permissions(permissions.iter().any(|permission| !permission.granted));
    }
    build(&model, state.demo, permissions)
}

fn build(
    model: &AppModel,
    demo: bool,
    permissions: Vec<connected_core::Permission>,
) -> Snapshot {
    let logs = model.logs();
    let skipped = logs.len().saturating_sub(LOG_LINES);
    let fingerprint = model.fingerprint();
    let systems = model.known_systems();
    let scales = model.known_scales();
    let device_list = model.devices();
    let devices_online: std::collections::HashMap<&str, bool> =
        device_list.iter().map(|device| (device.name.as_str(), device.online)).collect();

    Snapshot {
        this_computer: model.this_computer().to_owned(),
        fingerprint: fingerprint.to_string(),
        fingerprint_short: fingerprint.short(),
        running: model.is_running(),
        mesh_port: MESH_PORT,
        pairing_port: PAIRING_PORT,
        lan_address: lan_address(),
        demo,
        mesh: model.mesh_state().map(|state| MeshView {
            listening_on: state.listening_on,
            active_screen: state.active_screen,
            controlled_by: state.controlled_by,
            peers: state
                .peers
                .into_iter()
                .map(|peer| {
                    let (width, height) = peer.size();
                    PeerView { name: peer.name, width, height, displays: peer.displays, from: peer.from }
                })
                .collect(),
        }),
        screens: model
            .desktop()
            .screens()
            .iter()
            .filter_map(|(name, displays)| {
                let all = connected_proto::bounds(displays)?;
                Some(ScreenView {
                    name: name.clone(),
                    x: all.x,
                    y: all.y,
                    width: all.width,
                    height: all.height,
                    local: name == model.this_computer(),
                    displays: displays.iter().map(|d| d.moved(-all.x, -all.y)).collect(),
                    scales: if name == model.this_computer() {
                        model.local_scales().to_vec()
                    } else {
                        scales.get(name).cloned().unwrap_or_default()
                    },
                    os: if name == model.this_computer() {
                        Os::current()
                    } else {
                        systems.get(name).copied().unwrap_or_default()
                    },
                    online: name == model.this_computer()
                        || devices_online.get(name.as_str()).copied().unwrap_or(false),
                })
            })
            .collect(),
        devices: {
            let known = model.known_displays();
            device_list.iter().cloned().map(|device| device_view(device, &known, &systems)).collect()
        },
        pairing: model.pairing.as_ref().map(|flow| pairing_view(&flow.stage)),
        notice: model.notice.clone(),
        permissions: permissions
            .into_iter()
            .map(|permission| PermissionView { name: permission.name, granted: permission.granted })
            .collect(),
        logs: logs.into_iter().skip(skipped).collect(),
    }
}

fn device_view(
    device: Device,
    known: &BTreeMap<String, Vec<Rect>>,
    systems: &BTreeMap<String, Os>,
) -> DeviceView {
    DeviceView {
        os: systems.get(&device.name).copied().unwrap_or_default(),
        address: device.address.clone(),
        displays: known.get(&device.name).cloned().unwrap_or_else(default_displays),
        fingerprint: device.fingerprint.to_string(),
        fingerprint_short: device.fingerprint.short(),
        name: device.name,
        online: device.online,
        screen: device.displays.as_deref().and_then(connected_proto::bounds).map(|all| (all.width, all.height)),
    }
}

fn pairing_view(stage: &PairingStage) -> PairingView {
    let mut view = PairingView {
        stage: "choosing",
        addr: None,
        peer_name: None,
        peer_fingerprint: None,
        code: None,
        ok: None,
        message: None,
    };
    match stage {
        PairingStage::Choosing => {}
        PairingStage::Waiting { addr } => {
            view.stage = "waiting";
            view.addr = Some(addr.clone());
        }
        PairingStage::Connecting { addr } => {
            view.stage = "connecting";
            view.addr = Some(addr.clone());
        }
        PairingStage::Confirming { peer_name, peer_fingerprint, code } => {
            view.stage = "confirming";
            view.peer_name = Some(peer_name.clone());
            view.peer_fingerprint = Some(peer_fingerprint.clone());
            view.code = Some(code.clone());
        }
        PairingStage::Done(outcome) => {
            view.stage = "done";
            view.ok = Some(outcome.is_ok());
            view.message = Some(match outcome {
                Ok(name) => name.clone(),
                Err(reason) => reason.clone(),
            });
        }
    }
    view
}

/// For tests: the snapshot as the window receives it, a JSON value.
#[cfg(test)]
pub(crate) fn as_json(state: &AppState) -> serde_json::Value {
    serde_json::to_value(snapshot(state)).expect("a snapshot is always valid JSON")
}
