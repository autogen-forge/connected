//! The Connected desktop app: a window for pairing computers, arranging their screens and
//! sharing a keyboard and mouse between them.
//!
//! The window is a web page (see `ui/`). It asks this program for a [`snapshot::Snapshot`]
//! several times a second and draws it, and sends back what the person does as commands.
//! Everything the app actually does lives in [`model::AppModel`].

pub mod commands;
pub mod demo;
pub mod model;
pub mod snapshot;
pub mod state;

#[cfg(feature = "dev-server")]
mod devserver;

use std::path::PathBuf;
use std::process::exit;

use tauri::Manager;

use crate::model::{AppModel, Backends};
use crate::state::AppState;

/// The one command the window calls; see [`commands::dispatch`].
#[tauri::command(async)]
fn call(
    cmd: String,
    args: Option<serde_json::Value>,
    state: tauri::State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    if std::env::var_os("CONNECTED_TRACE").is_some() {
        eprintln!("call {cmd}");
    }
    commands::dispatch(&state, &cmd, args.unwrap_or(serde_json::Value::Null))
}

struct Args {
    home: Option<PathBuf>,
    demo: bool,
    #[cfg(feature = "dev-server")]
    dev_server: Option<u16>,
}

impl Args {
    fn parse() -> Self {
        let mut args = Args {
            home: None,
            demo: false,
            #[cfg(feature = "dev-server")]
            dev_server: None,
        };
        let mut input = std::env::args().skip(1);
        while let Some(flag) = input.next() {
            match flag.as_str() {
                "--home" => args.home = Some(PathBuf::from(input.next().unwrap_or_else(|| fail("--home needs a folder")))),
                "--demo" => args.demo = true,
                #[cfg(feature = "dev-server")]
                "--dev-server" => {
                    let port = input.next().unwrap_or_else(|| fail("--dev-server needs a port"));
                    args.dev_server = Some(port.parse().unwrap_or_else(|_| fail("--dev-server needs a port number")));
                }
                "--help" | "-h" => {
                    println!(
                        "connected-desktop [--home DIR] [--demo]\n\n\
                         --home DIR  where this computer's identity, paired computers and settings are kept\n\
                         --demo      try the app with stand-in computers, without touching real input"
                    );
                    exit(0);
                }
                // The system may pass its own flags when it opens an app (`-psn_...`).
                _ if flag.starts_with("-psn_") => {}
                other => fail(&format!("unknown option '{other}' (try --help)")),
            }
        }
        args
    }
}

pub(crate) fn fail(message: &str) -> ! {
    eprintln!("{message}");
    exit(1);
}

fn default_home() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("connected")
}

fn open_state(args: &Args) -> AppState {
    if args.demo {
        match demo::build() {
            Ok((model, demo)) => AppState::new(model, Some(demo)),
            Err(err) => fail(&format!("cannot set up the demo: {err}")),
        }
    } else {
        let home = args.home.clone().unwrap_or_else(default_home);
        match AppModel::open(&home, Backends::system()) {
            Ok(model) => AppState::new(model, None),
            Err(err) => fail(&format!("cannot open {}: {err}", home.display())),
        }
    }
}

pub fn run() {
    let args = Args::parse();
    let state = open_state(&args);

    #[cfg(feature = "dev-server")]
    if let Some(port) = args.dev_server {
        devserver::serve(state, port);
        return;
    }

    // This program has windows of its own, which changes how the cursor can be hidden.
    connected_platform::declare_gui_app();

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![call])
        .build(tauri::generate_context!())
        .expect("cannot build the app")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                app.state::<AppState>().shutdown();
            }
        });
}
