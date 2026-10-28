//! What a running computer is made of: the mesh that shares input with every paired computer,
//! pairing, and the settings that persist.
//!
//! Everything works on the traits from `connected-core`, so it runs against real backends in
//! the program and against in-memory ones in tests. A screen or a command line drives it
//! through handles: [`MeshHandle`] and [`PairingSession`].

mod error;
mod mesh;
mod pairing;
mod settings;

use std::sync::Arc;
use std::time::Duration;

pub use error::{AppError, Result};
pub use pairing::{PairingEvent, PairingSession};
pub use mesh::{ConnectedPeer, MeshBackends, MeshHandle, MeshStart, MeshState, now_ms};
pub use settings::Settings;

/// Where progress and problems are reported. The program prints or collects them.
pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// A computer that has not been heard from for this long is considered gone.
pub(crate) const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(8);
