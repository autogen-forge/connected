use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use connected_proto::codec::{encode, try_decode};
use connected_proto::{Message, PROTOCOL_VERSION};
use rustls::{ClientConnection, ServerConnection, StreamOwned};

use crate::error::{NetError, Result};
use crate::fingerprint::Fingerprint;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const READ_CHUNK: usize = 16 * 1024;

/// The other device, as seen from an open session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub name: String,
    pub fingerprint: Fingerprint,
    /// The network address the other device is at, as this one sees it.
    pub address: Option<std::net::IpAddr>,
}

enum Tls {
    Client(Box<StreamOwned<ClientConnection, TcpStream>>),
    Server(Box<StreamOwned<ServerConnection, TcpStream>>),
}

impl Tls {
    fn tcp(&self) -> &TcpStream {
        match self {
            Tls::Client(stream) => &stream.sock,
            Tls::Server(stream) => &stream.sock,
        }
    }

    /// Runs the TLS handshake to completion.
    fn handshake(&mut self) -> io::Result<()> {
        match self {
            Tls::Client(stream) => {
                while stream.conn.is_handshaking() {
                    stream.conn.complete_io(&mut stream.sock)?;
                }
            }
            Tls::Server(stream) => {
                while stream.conn.is_handshaking() {
                    stream.conn.complete_io(&mut stream.sock)?;
                }
            }
        }
        Ok(())
    }

    fn peer_fingerprint(&self) -> Option<Fingerprint> {
        let certificates = match self {
            Tls::Client(stream) => stream.conn.peer_certificates(),
            Tls::Server(stream) => stream.conn.peer_certificates(),
        };
        certificates.and_then(|chain| chain.first()).map(|cert| Fingerprint::of_certificate(cert))
    }
}

impl Read for Tls {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Tls::Client(stream) => stream.read(buf),
            Tls::Server(stream) => stream.read(buf),
        }
    }
}

impl Write for Tls {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Tls::Client(stream) => stream.write(buf),
            Tls::Server(stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Tls::Client(stream) => stream.flush(),
            Tls::Server(stream) => stream.flush(),
        }
    }
}

/// An encrypted, mutually authenticated connection that carries [`Message`]s.
///
/// Reads and writes both go through `&mut self`, so one thread owns a session and polls it
/// with [`recv_timeout`](Session::recv_timeout) between sends.
pub struct Session {
    tls: Tls,
    inbox: Vec<u8>,
    peer: Peer,
}

impl Session {
    /// Finishes TLS on a freshly connected socket and swaps `Hello`s with the peer.
    pub(crate) fn establish_client(
        connection: ClientConnection,
        tcp: TcpStream,
        my_name: &str,
    ) -> Result<Self> {
        let tls = Tls::Client(Box::new(StreamOwned::new(connection, tcp)));
        Self::establish(tls, my_name)
    }

    pub(crate) fn establish_server(
        connection: ServerConnection,
        tcp: TcpStream,
        my_name: &str,
    ) -> Result<Self> {
        let tls = Tls::Server(Box::new(StreamOwned::new(connection, tcp)));
        Self::establish(tls, my_name)
    }

    fn establish(mut tls: Tls, my_name: &str) -> Result<Self> {
        tls.tcp().set_nodelay(true)?;
        tls.tcp().set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
        tls.tcp().set_write_timeout(Some(WRITE_TIMEOUT))?;
        tls.handshake()?;

        let fingerprint = tls
            .peer_fingerprint()
            .ok_or_else(|| NetError::Protocol("the peer sent no certificate".into()))?;

        let address = tls.tcp().peer_addr().ok().map(|addr| addr.ip());
        let mut session = Self {
            tls,
            inbox: Vec::new(),
            peer: Peer { name: String::new(), fingerprint, address },
        };
        session.send(&Message::Hello { version: PROTOCOL_VERSION, name: my_name.to_owned() })?;
        match session.recv_timeout(HANDSHAKE_TIMEOUT)? {
            Some(Message::Hello { version, name }) if version == PROTOCOL_VERSION => {
                session.peer.name = name;
                Ok(session)
            }
            Some(Message::Hello { version, .. }) => {
                Err(NetError::VersionMismatch { ours: PROTOCOL_VERSION, theirs: version })
            }
            Some(other) => Err(NetError::Protocol(format!("expected Hello, got {other:?}"))),
            None => Err(NetError::Timeout),
        }
    }

    pub fn peer(&self) -> &Peer {
        &self.peer
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.tls.tcp().peer_addr()
    }

    pub fn send(&mut self, message: &Message) -> Result<()> {
        let frame = encode(message)?;
        self.tls.write_all(&frame)?;
        self.tls.flush()?;
        Ok(())
    }

    /// Waits up to `timeout` for the next message; `None` means nothing arrived in time.
    pub fn recv_timeout(&mut self, timeout: Duration) -> Result<Option<Message>> {
        let deadline = Instant::now() + timeout;
        let mut chunk = [0u8; READ_CHUNK];
        loop {
            if let Some((message, used)) = try_decode(&self.inbox)? {
                self.inbox.drain(..used);
                return Ok(Some(message));
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            // A zero timeout would mean "block forever" to the socket.
            self.tls.tcp().set_read_timeout(Some(remaining.max(Duration::from_millis(1))))?;

            match self.tls.read(&mut chunk) {
                Ok(0) => return Err(NetError::Closed),
                Ok(read) => self.inbox.extend_from_slice(&chunk[..read]),
                Err(err)
                    if matches!(err.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) =>
                {
                    return Ok(None);
                }
                // Peers that just close the socket without a TLS goodbye are still "closed".
                Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => {
                    return Err(NetError::Closed);
                }
                Err(err) => return Err(err.into()),
            }
        }
    }

    /// Blocks until the next message arrives.
    pub fn recv(&mut self) -> Result<Message> {
        loop {
            if let Some(message) = self.recv_timeout(Duration::from_secs(60))? {
                return Ok(message);
            }
        }
    }
}
