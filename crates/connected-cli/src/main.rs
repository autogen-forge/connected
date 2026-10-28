use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{ArgGroup, Parser, Subcommand};
use connected_app::{ClientHandle, ClientStatus, Log, ServerBackends, ServerHandle};
use connected_core::{Arrangement, Metrics, default_displays};
use connected_net::{Identity, Node, PairingPrompt, TrustStore};
use connected_proto::Edge;

const DEFAULT_ADDR: &str = "0.0.0.0:24900";
const TRUST_FILE: &str = "trusted_peers";

#[derive(Parser)]
#[command(name = "connected", version, about = "Share keyboard, mouse, clipboard and files across machines")]
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

    /// Capture this machine's keyboard and mouse and share them with paired clients.
    ///
    /// Push the cursor off the edge where a client sits to move over to it. To get back
    /// without the mouse, press Ctrl+Shift+Alt+Esc. Ctrl-C stops the server.
    Server {
        /// Address to listen on.
        #[arg(long, default_value = DEFAULT_ADDR)]
        listen: String,

        /// A paired device and the side of this screen it sits on, e.g. `laptop=right`.
        /// Give it once per client; each side holds one.
        #[arg(long = "client", value_name = "NAME=EDGE", value_parser = parse_placement, required = true)]
        clients: Vec<(String, Edge)>,
    },

    /// Connect to a server and play back the keyboard and mouse it sends. Ctrl-C stops it.
    Client {
        /// Server address, host:port.
        #[arg(long)]
        server: String,
    },

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
        Command::Server { listen, clients } => serve(&home, cli.name, &listen, clients),
        Command::Client { server } => follow(&home, cli.name, &server),
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

fn serve(home: &Path, name: Option<String>, addr: &str, clients: Vec<(String, Edge)>) -> Result<()> {
    let mut arrangement = Arrangement::default();
    // Real sizes are only known once a computer connects; until then, one of the usual size
    // is assumed, which is enough to tell that two cannot share a side.
    let devices = BTreeMap::new();
    let local = default_displays();
    let metrics = Metrics { this_computer: "this computer", local: &local, devices: &devices };
    for (device, edge) in clients {
        if arrangement.contains(&device) {
            bail!("'{device}' is placed twice");
        }
        arrangement
            .place_beside_this_computer(&device, edge, &metrics)
            .map_err(|err| anyhow::anyhow!("cannot place '{device}' on the {edge:?} side: {err}"))?;
    }

    let node = open_node(home, name)?;
    warn_about_missing_permissions();
    let stop = stop_on_ctrl_c()?;

    let mut server = ServerHandle::start(
        node.clone(),
        addr,
        arrangement,
        || {
            Ok(ServerBackends {
                source: connected_platform::open_source()?,
                cursor_sink: connected_platform::open_sink()?,
                screen: connected_platform::open_screen()?,
            })
        },
        printer(),
    )
    .context("cannot start the server")?;

    println!(
        "'{}' is serving on {}. Ctrl-C stops it; Ctrl+Shift+Alt+Esc brings the cursor back.",
        node.identity().name(),
        server.state().listening_on
    );
    while !stop.load(Ordering::SeqCst) && server.is_running() {
        sleep(Duration::from_millis(100));
    }
    server.stop();

    match server.state().error {
        Some(error) => bail!("the server stopped: {error}"),
        None => Ok(()),
    }
}

fn follow(home: &Path, name: Option<String>, server: &str) -> Result<()> {
    let node = open_node(home, name)?;
    warn_about_missing_permissions();
    let stop = stop_on_ctrl_c()?;

    println!("Connecting to {server}, and reconnecting if the link drops. Ctrl-C stops it.");
    let mut client = ClientHandle::start(
        node,
        server.to_owned(),
        || Ok((connected_platform::open_sink()?, connected_platform::open_screen()?)),
        Duration::from_secs(3),
        printer(),
    );
    while !stop.load(Ordering::SeqCst) {
        if let ClientStatus::Failed { reason } = client.status() {
            bail!("the client cannot run: {reason}");
        }
        sleep(Duration::from_millis(100));
    }
    client.stop();
    Ok(())
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
