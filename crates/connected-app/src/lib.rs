//! What a running session is made of: the server that captures input and decides where it
//! goes, the client that injects what it receives, pairing, and the settings that persist.
//!
//! Everything works on the traits from `connected-core`, so it runs against real backends in
//! the program and against in-memory ones in tests. A screen or a command line drives it
//! through handles: [`ServerHandle`], [`ClientHandle`] and [`PairingSession`].

mod client;
mod error;
mod pairing;
mod server;
mod settings;

use std::sync::Arc;
use std::time::Duration;

pub use client::{ClientHandle, ClientStatus, Hooks, run_client};
pub use error::{AppError, Result};
pub use pairing::{PairingEvent, PairingSession};
pub use server::{ConnectedClient, ServerBackends, ServerHandle, ServerState};
pub use settings::{Mode, Settings};

/// Where progress and problems are reported. The program prints or collects them.
pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// A client that has not been heard from for this long is considered gone.
pub(crate) const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(8);
