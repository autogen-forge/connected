use std::fmt;

use connected_proto::CodecError;

/// Which side said no to a pairing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    ThisDevice,
    OtherDevice,
}

impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Side::ThisDevice => f.write_str("on this device"),
            Side::OtherDevice => f.write_str("by the other device"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("io: {0}")]
    Io(std::io::Error),
    #[error("tls: {0}")]
    Tls(rustls::Error),
    #[error("message framing: {0}")]
    Codec(#[from] CodecError),
    #[error("certificate: {0}")]
    Certificate(String),
    #[error("trust file: {0}")]
    TrustFile(String),
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("the peer speaks protocol v{theirs}, this build speaks v{ours}")]
    VersionMismatch { ours: u16, theirs: u16 },
    #[error("timed out")]
    Timeout,
    #[error("connection closed by the peer")]
    Closed,
    #[error("pairing declined {0}")]
    Declined(Side),
}

impl From<std::io::Error> for NetError {
    /// rustls reports TLS failures through `io::Error` when it runs inside a `Read`/`Write`
    /// stream; this unwraps them so callers see the real cause.
    fn from(err: std::io::Error) -> Self {
        match err.get_ref().and_then(|inner| inner.downcast_ref::<rustls::Error>()) {
            Some(tls) => NetError::Tls(tls.clone()),
            None => NetError::Io(err),
        }
    }
}

impl From<rustls::Error> for NetError {
    fn from(err: rustls::Error) -> Self {
        NetError::Tls(err)
    }
}

pub type Result<T> = std::result::Result<T, NetError>;
