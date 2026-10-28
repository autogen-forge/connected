//! Secure links between devices: identity, pairing, and an authenticated message session.
//!
//! Every device has a self-signed certificate and is known to others by its fingerprint.
//! Two devices that have never met pair once, with both people confirming that the same code
//! appears on both screens; after that they connect over TLS 1.3 and each accepts only
//! certificates it has paired with.

mod error;
mod fingerprint;
mod identity;
mod node;
mod session;
mod tls;
mod trust;

pub use error::{NetError, Result, Side};
pub use fingerprint::{Fingerprint, ParseFingerprintError, pairing_code};
pub use identity::Identity;
pub use node::{Listener, Node, PairingPrompt};
pub use session::{Peer, Session};
pub use trust::{TrustStore, TrustedPeer};
