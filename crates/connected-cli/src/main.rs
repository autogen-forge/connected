use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{ArgGroup, Parser, Subcommand};
use connected_app::{Log, MeshBackends, MeshHandle, MeshStart, Settings, now_ms};
use connected_core::{Arrangement, Metrics, default_displays};
use connected_net::{Identity, Node, PairingPrompt, TrustStore};
use connected_proto::Edge;

mod probe;

const DEFAULT_ADDR: &str = "0.0.0.0:24900";
const TRUST_FILE: &str = "trusted_peers";

#[derive(Parser)]
#[command(name = "connected", version, about = "Share keyboard and mouse across machines")]
struct Cli {
    /// Where this device keeps its identity and the list of paired devices.
    #[arg(long, global = true, env = "CONNECTED_HOME", value_name = "DIR")]
    home: Option<PathBuf>,

    /// Name other devices see for this one. Only used when the identity is first created.
    #[arg(long, global = true, value_name = "NAME")]
    name: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show this device's name and fingerprint.
    Id,

    /// Pair with another device: run `--listen` on one and `--connect` on the other.
    #[command(group(ArgGroup::new("role").required(true).args(["listen", "connect"])))]
    Pair {
        /// Wait for the other device to connect (default address when none is given).
        #[arg(long, num_args = 0..=1, default_missing_value = DEFAULT_ADDR, value_name = "ADDR")]
        listen: Option<String>,

        /// Connect to a device that is waiting with `--listen`.
        #[arg(long, value_name = "ADDR")]
        connect: Option<String>,
    },

    /// List the devices this one is paired with.
    Trusted,

    /// Stop trusting a paired device, named by its name or the start of its fingerprint.
    Forget { who: String },

    /// Connect to every paired device and share keyboard and mouse with all of them.
    ///
    /// Every computer runs this; nobody is the host. Push the cursor off the edge where
    /// another computer sits to move over to it; whichever got there first has priority until
    /// the cursor comes back. To get back without the mouse, press Ctrl+Shift+Alt+Esc.
    /// Ctrl-C stops it. Places and addresses saved by the desktop app are used unless given here.
    Run {
        /// Address to listen on.
        #[arg(long, default_value = DEFAULT_ADDR)]
        listen: String,

        /// A paired device and the side of this screen it sits on, e.g. `laptop=right`.
        /// Give it once per device; each side holds one.
        #[arg(long = "place", value_name = "NAME=EDGE", value_parser = parse_placement)]
        places: Vec<(String, Edge)>,

        /// Where to reach a paired device, e.g. `laptop=192.168.0.7:24900`. Only the device
        /// with the lower fingerprint of each pair dials, but both should know the other.
        #[arg(long = "peer", value_name = "NAME=ADDR", value_parser = parse_peer)]
        peers: Vec<(String, String)>,
    },

    /// Walk through what this computer's input backend can do (reading the mouse, pushing against
    /// an edge, capturing, moving the cursor) and save the result in `connected-test.txt`.
    Test,

    /// Report which platform backends work on this machine.
    Check {
        /// Ask the OS to show its permission prompts for anything missing.
        #[arg(long)]
        request_permissions: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let home = match cli.home {
        Some(home) => home,
        None => dirs::config_dir().context("no config directory on this system; pass --home")?.join("connected"),
    };

    match cli.command {
        Command::Id => show_identity(&home, cli.name),
        Command::Pair { listen, connect } => pair(&home, cli.name, listen, connect),
        Command::Trusted => list_trusted(&home),
        Command::Forget { who } => forget(&home, &who),
        Command::Run { listen, places, peers } => run(&home, cli.name, &listen, places, peers),
        Command::Test => probe::run(),
        Command::Check { request_permissions } => check(request_permissions),
    }
}

fn open_node(home: &Path, name: Option<String>) -> Result<Node> {
    let name = name.unwrap_or_else(|| gethostname::gethostname().to_string_lossy().into_owned());
    let identity = Identity::load_or_create(home, &name)
        .with_context(|| format!("cannot load or create the identity in {}", home.display()))?;
    let trust = TrustStore::load(&home.join(TRUST_FILE)).context("cannot read the trusted devices")?;
    Ok(Node::new(identity, trust))
}

fn show_identity(home: &Path, name: Option<String>) -> Result<()> {
    let node = open_node(home, name)?;
    println!("name:        {}", node.identity().name());
    println!("fingerprint: {}", node.identity().fingerprint());
    println!("kept in:     {}", home.display());
    Ok(())
}

fn pair(home: &Path, name: Option<String>, listen: Option<String>, connect: Option<String>) -> Result<()> {
    let node = open_node(home, name)?;
    println!("This device: {} ({})", node.identity().name(), node.identity().fingerprint().short());

    let peer = match (listen, connect) {
        (Some(addr), None) => {
            let listener = node.listen(&addr).with_context(|| format!("cannot listen on {addr}"))?;
            println!("Waiting for a device to pair, on {} ...", listener.local_addr()?);
            listener.accept_for_pairing(ask_for_confirmation)
        }
        (None, Some(addr)) => {
            println!("Connecting to {addr} ...");
            node.pair_with(&addr, ask_for_confirmation)
        }
        _ => bail!("give exactly one of --listen and --connect"),
    }
    .context("pairing did not complete")?;

    println!("\nPaired with '{}' ({}).", peer.name, peer.fingerprint.short());
    Ok(())
}

fn ask_for_confirmation(prompt: &PairingPrompt) -> bool {
    println!("\nPairing request from '{}' ({}).", prompt.peer_name, prompt.peer_fingerprint.short());
    println!("Confirmation code:  {}\n", prompt.code);
    println!("If the other device shows a different code, someone may be intercepting: answer no.");
    print!("Does the other device show the same code? [y/N] ");
    let _ = io::stdout().flush();

    let mut answer = String::new();
    if io::stdin().lock().read_line(&mut answer).is_err() {
        return false;
    }
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

fn list_trusted(home: &Path) -> Result<()> {
    let trust = TrustStore::load(&home.join(TRUST_FILE)).context("cannot read the trusted devices")?;
    if trust.peers().is_empty() {
        println!("No paired devices. Pair one with `connected pair`.");
    }
    for peer in trust.peers() {
        println!("{}  {}", peer.fingerprint, peer.name);
    }
    Ok(())
}

fn forget(home: &Path, who: &str) -> Result<()> {
    let mut trust = TrustStore::load(&home.join(TRUST_FILE)).context("cannot read the trusted devices")?;
    let removed = trust.remove(who)?;
    if removed.is_empty() {
        bail!("no paired device matches '{who}' (use a name, or at least 4 characters of a fingerprint)");
    }
    for peer in removed {
        println!("No longer trusting '{}' ({}).", peer.name, peer.fingerprint.short());
    }
    Ok(())
}

fn parse_placement(text: &str) -> std::result::Result<(String, Edge), String> {
    let (name, edge) = text.split_once('=').ok_or("expected NAME=EDGE, for example laptop=right")?;
    let edge = match edge.trim().to_ascii_lowercase().as_str() {
        "left" => Edge::Left,
        "right" => Edge::Right,
        "top" | "up" => Edge::Top,
        "bottom" | "down" => Edge::Bottom,
        other => return Err(format!("unknown edge '{other}': use left, right, top or bottom")),
    };
    let name = name.trim();
    if name.is_empty() {
        return Err("the device name is empty".into());
    }
    Ok((name.to_owned(), edge))
}

fn parse_peer(text: &str) -> std::result::Result<(String, String), String> {
    let (name, address) = text.split_once('=').ok_or("expected NAME=ADDR, for example laptop=192.168.0.7:24900")?;
    let (name, address) = (name.trim(), address.trim());
    if name.is_empty() || address.is_empty() {
        return Err("the device name and its address must both be given".into());
    }
    Ok((name.to_owned(), address.to_owned()))
}

fn run(
    home: &Path,
    name: Option<String>,
    addr: &str,
    places: Vec<(String, Edge)>,
    peers: Vec<(String, String)>,
) -> Result<()> {
    let (saved, problem) = Settings::load(home);
    if let Some(problem) = problem {
        eprintln!("warning: {problem}");
    }
    let mut start = MeshStart {
        arrangement: saved.arrangement,
        layout_version: saved.layout_version,
        peer_addresses: saved.peer_addresses,
    };

    if !places.is_empty() {
        let mut arrangement = Arrangement::default();
        // Real sizes are only known once a computer connects; until then, one of the usual size
        // is assumed, which is enough to tell that two cannot share a side.
        let devices = BTreeMap::new();
        let local = default_displays();
        let metrics = Metrics { this_computer: "this computer", local: &local, devices: &devices };
        for (device, edge) in places {
            if arrangement.contains(&device) {
                bail!("'{device}' is placed twice");
            }
            arrangement
                .place_beside_this_computer(&device, edge, &metrics)
                .map_err(|err| anyhow::anyhow!("cannot place '{device}' on the {edge:?} side: {err}"))?;
        }
        start.arrangement = arrangement;
        start.layout_version = now_ms();
    }
    start.peer_addresses.extend(peers);

    let node = open_node(home, name)?;
    warn_about_missing_permissions();
    let stop = stop_on_ctrl_c()?;

    let mut mesh = MeshHandle::start(
        node.clone(),
        addr,
        start,
        || {
            Ok(MeshBackends {
                source: connected_platform::open_source()?,
                sink: connected_platform::open_sink()?,
                screen: connected_platform::open_screen()?,
            })
        },
        printer(),
    )
    .context("cannot start")?;

    println!(
        "'{}' is listening on {}. Ctrl-C stops it; Ctrl+Shift+Alt+Esc brings the cursor back.",
        node.identity().name(),
        mesh.state().listening_on
    );
    while !stop.load(Ordering::SeqCst) && mesh.is_running() {
        sleep(Duration::from_millis(100));
    }
    mesh.stop();

    match mesh.state().error {
        Some(error) => bail!("stopped: {error}"),
        None => Ok(()),
    }
}

fn printer() -> Log {
    Arc::new(|line: &str| println!("{line}"))
}

fn stop_on_ctrl_c() -> Result<Arc<AtomicBool>> {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst))
        .context("cannot install the Ctrl-C handler")?;
    Ok(stop)
}

fn warn_about_missing_permissions() {
    for permission in connected_platform::permissions() {
        if !permission.granted {
            eprintln!(
                "warning: the '{}' permission is missing for this terminal or app; input may not work (see `connected check`)",
                permission.name
            );
        }
    }
}

fn check(request_permissions: bool) -> Result<()> {
    println!("backend: {}", connected_platform::BACKEND);
    println!("protocol: v{}", connected_proto::PROTOCOL_VERSION);

    for permission in connected_platform::permissions() {
        let state = if permission.granted { "granted" } else { "MISSING" };
        println!("permission {}: {state}", permission.name);
    }
    if request_permissions {
        connected_platform::request_permissions();
        println!("asked the OS to prompt for missing permissions; re-run check afterwards");
    }

    report("input sink  ", connected_platform::open_sink().map(|_| ()));
    report("input source", connected_platform::open_source().map(|_| ()));
    match connected_platform::open_screen() {
        Ok(screen) => match (screen.size(), screen.cursor()) {
            (Ok((w, h)), Ok((x, y))) => println!("screen      : {w}x{h}, cursor at {x},{y}"),
            (Err(err), _) | (_, Err(err)) => println!("screen      : {err}"),
        },
        Err(err) => println!("screen      : {err}"),
    }
    Ok(())
}

fn report(label: &str, result: connected_core::platform::Result<()>) {
    match result {
        Ok(()) => println!("{label}: ok"),
        Err(err) => println!("{label}: {err}"),
    }
}
