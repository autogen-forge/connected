//! A made-up setup for trying the app without a second computer.
//!
//! A few stand-in computers are paired with the app and connect to its server over real TLS
//! on this machine. Input is simulated, so nothing here touches the real keyboard or mouse.

use std::thread;
use std::time::Duration;

use connected_app::{AppError, ClientHandle, ServerBackends};
use connected_core::{InputSink, Metrics, Os, Rect, ScreenInfo};
use connected_net::{Identity, Node, TrustStore};
use connected_platform::mock::{ChannelSource, FixedScreen, SharedRecordingSink};

use crate::model::{AppModel, Backends};

/// Keeps the stand-in computers alive for as long as the demo runs.
pub struct Demo {
    _clients: Vec<ClientHandle>,
}

/// A made-up computer.
struct Machine {
    name: &'static str,
    /// Its displays, the main one first.
    displays: &'static [Rect],
    /// How much each display is magnified, in percent.
    scales: &'static [u16],
    os: Os,
    /// Where its top-left corner is on the shared desktop, if it is on the layout.
    at: Option<(i32, i32)>,
    /// Whether it is switched on and connected.
    connects: bool,
}

const MACHINES: &[Machine] = &[
    Machine { name: "MacBook Air", displays: &[Rect::new(0, 0, 1470, 956)], scales: &[200], os: Os::Mac, at: Some((1512, 120)), connects: true },
    // Two monitors: the main one, and a smaller one to its right, lower down.
    Machine {
        name: "Windows PC",
        displays: &[Rect::new(0, 0, 2560, 1440), Rect::new(2560, 360, 1920, 1080)],
        scales: &[100, 150],
        os: Os::Windows,
        at: Some((2982, -200)),
        connects: true,
    },
    Machine { name: "거실 TV", displays: &[Rect::new(0, 0, 1920, 1080)], scales: &[100], os: Os::Linux, at: Some((-1920, -80)), connects: false },
    Machine { name: "Linux 노트북", displays: &[Rect::new(0, 0, 1920, 1080)], scales: &[100], os: Os::Linux, at: None, connects: false },
];

pub fn build() -> Result<(AppModel, Demo), AppError> {
    let backends = Backends {
        server: std::sync::Arc::new(|| {
            let (source, _input, _capture) = ChannelSource::new();
            Ok(ServerBackends {
                source: Box::new(source),
                cursor_sink: Box::new(SharedRecordingSink::default()),
                screen: Box::new(FixedScreen::new(1512, 982)),
            })
        }),
        local_screen: std::sync::Arc::new(|| Some((vec![Rect::new(0, 0, 1512, 982)], vec![200]))),
        client: std::sync::Arc::new(|| {
            Ok((
                Box::new(SharedRecordingSink::default()) as Box<dyn InputSink>,
                Box::new(FixedScreen::new(1920, 1080)) as Box<dyn ScreenInfo>,
            ))
        }),
    };

    let mut model = AppModel::in_memory("Studio Mac", backends)?;
    model.settings.listen = "127.0.0.1:0".to_owned();

    let mut machines = Vec::new();
    for machine in MACHINES {
        let node = Node::new(Identity::generate(machine.name)?, TrustStore::in_memory());
        pair(model.node(), &node)?;
        model.settings.known_screens.insert(machine.name.to_owned(), machine.displays.to_vec());
        model.settings.known_os.insert(machine.name.to_owned(), machine.os);
        model.settings.known_scales.insert(machine.name.to_owned(), machine.scales.to_vec());
        if let Some((x, y)) = machine.at {
            let sizes = model.settings.known_screens.clone();
            let local = [Rect::new(0, 0, 1512, 982)];
            let metrics = Metrics { this_computer: "Studio Mac", local: &local, devices: &sizes };
            model.settings.arrangement.place(machine.name, x, y, &metrics).ok();
        }
        machines.push((node, machine.displays, machine.scales, machine.os, machine.connects));
    }

    model.start();
    let address = model.server_state().map(|state| state.listening_on).unwrap_or_default();

    let mut clients = Vec::new();
    for (node, displays, scales, os, connects) in machines {
        if !connects {
            continue;
        }
        clients.push(ClientHandle::start(
            node,
            address.clone(),
            move || {
                Ok((
                    Box::new(SharedRecordingSink::default()) as Box<dyn InputSink>,
                    Box::new(FixedScreen::with_displays(displays.to_vec()).with_scales(scales.to_vec()).with_os(os)) as Box<dyn ScreenInfo>,
                ))
            },
            Duration::from_secs(1),
            std::sync::Arc::new(|_: &str| {}),
        ));
    }
    Ok((model, Demo { _clients: clients }))
}

/// Pairs two nodes with each other, both answering yes.
fn pair(server: &Node, client: &Node) -> Result<(), AppError> {
    let listener = server.listen("127.0.0.1:0")?;
    let address = listener.local_addr()?.to_string();
    let waiting = thread::spawn(move || listener.accept_for_pairing(|_| true));
    client.pair_with(&address, |_| true)?;
    waiting
        .join()
        .map_err(|_| AppError::Config("the demo pairing thread panicked".into()))??;
    Ok(())
}
