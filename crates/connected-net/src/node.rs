use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use connected_proto::Message;
use rustls::pki_types::ServerName;
use rustls::{ClientConnection, ServerConnection};

use crate::error::{NetError, Result, Side};
use crate::fingerprint::{Fingerprint, pairing_code};
use crate::identity::Identity;
use crate::session::{Peer, Session};
use crate::tls::{self, Mode};
use crate::trust::{TrustStore, TrustedPeer};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long to wait for the other person to answer the pairing question.
const PAIRING_DECISION_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// What a person is asked to confirm when pairing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingPrompt {
    pub peer_name: String,
    pub peer_fingerprint: Fingerprint,
    /// Shown on both devices. The question is whether it is the same on both.
    pub code: String,
}

/// This device on the network: who it is and who it trusts.
#[derive(Clone)]
pub struct Node {
    identity: Arc<Identity>,
    trust: Arc<Mutex<TrustStore>>,
}

impl Node {
    pub fn new(identity: Identity, trust: TrustStore) -> Self {
        Self { identity: Arc::new(identity), trust: Arc::new(Mutex::new(trust)) }
    }

    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    /// Stops trusting the devices that `selector` names: a name, or the start of a
    /// fingerprint. Returns who was removed. Connections they already have stay open until
    /// the owner of the session closes them.
    pub fn forget(&self, selector: &str) -> Result<Vec<TrustedPeer>> {
        self.trust
            .lock()
            .map_err(|_| NetError::TrustFile("trust store lock poisoned".into()))?
            .remove(selector)
    }

    pub fn trusted_peers(&self) -> Vec<TrustedPeer> {
        self.trust
            .lock()
            .map(|mut store| {
                store.refresh();
                store.peers().to_vec()
            })
            .unwrap_or_default()
    }

    /// The name a device was paired under, if it is trusted. Unlike the name a device
    /// announces about itself, this one was confirmed by a person when pairing.
    pub fn trusted_name(&self, fingerprint: &Fingerprint) -> Option<String> {
        let mut store = self.trust.lock().ok()?;
        store.refresh();
        store
            .peers()
            .iter()
            .find(|peer| peer.fingerprint == *fingerprint)
            .map(|peer| peer.name.clone())
    }

    pub fn listen(&self, addr: impl ToSocketAddrs) -> Result<Listener> {
        Ok(Listener { tcp: TcpListener::bind(addr)?, node: self.clone() })
    }

    /// Connects to a device that has been paired with before.
    pub fn connect(&self, addr: impl ToSocketAddrs) -> Result<Session> {
        self.dial(addr, Mode::Trusted(Arc::clone(&self.trust)))
    }

    /// Connects to a device that is waiting in [`Listener::accept_for_pairing`], and pairs.
    ///
    /// `confirm` shows the prompt and returns whether the person agreed that both screens
    /// show the same code.
    pub fn pair_with(
        &self,
        addr: impl ToSocketAddrs,
        confirm: impl FnOnce(&PairingPrompt) -> bool,
    ) -> Result<Peer> {
        let session = self.dial(addr, Mode::Pairing)?;
        self.finish_pairing(session, confirm)
    }

    fn dial(&self, addr: impl ToSocketAddrs, mode: Mode) -> Result<Session> {
        let tcp = connect_tcp(addr)?;
        let config = tls::client_config(&self.identity, mode)?;
        let server_name = ServerName::try_from(tls::SERVER_NAME)
            .map_err(|err| NetError::Protocol(err.to_string()))?;
        let connection = ClientConnection::new(config, server_name)?;
        Session::establish_client(connection, tcp, self.identity.name())
    }

    fn serve(&self, tcp: TcpStream, mode: Mode) -> Result<Session> {
        let config = tls::server_config(&self.identity, mode)?;
        let connection = ServerConnection::new(config)?;
        Session::establish_server(connection, tcp, self.identity.name())
    }

    fn finish_pairing(
        &self,
        mut session: Session,
        confirm: impl FnOnce(&PairingPrompt) -> bool,
    ) -> Result<Peer> {
        let peer = session.peer().clone();
        let prompt = PairingPrompt {
            code: pairing_code(&self.identity.fingerprint(), &peer.fingerprint),
            peer_name: peer.name.clone(),
            peer_fingerprint: peer.fingerprint,
        };

        let accepted = confirm(&prompt);
        session.send(&Message::PairDecision { accepted })?;
        let theirs = match session.recv_timeout(PAIRING_DECISION_TIMEOUT)? {
            Some(Message::PairDecision { accepted }) => accepted,
            Some(other) => {
                return Err(NetError::Protocol(format!("expected PairDecision, got {other:?}")));
            }
            None => return Err(NetError::Timeout),
        };

        if !accepted {
            return Err(NetError::Declined(Side::ThisDevice));
        }
        if !theirs {
            return Err(NetError::Declined(Side::OtherDevice));
        }

        self.trust
            .lock()
            .map_err(|_| NetError::TrustFile("trust store lock poisoned".into()))?
            .add(TrustedPeer { name: peer.name.clone(), fingerprint: peer.fingerprint })?;
        Ok(peer)
    }
}

pub struct Listener {
    tcp: TcpListener,
    node: Node,
}

impl Listener {
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.tcp.local_addr()
    }

    /// Waits for a paired device to connect. A device that is not paired is turned away at
    /// the TLS handshake and shows up here as an error; call `accept` again to keep going.
    pub fn accept(&self) -> Result<Session> {
        let (tcp, _) = self.tcp.accept()?;
        self.node.serve(tcp, Mode::Trusted(Arc::clone(&self.node.trust)))
    }

    /// Like [`accept`](Self::accept), but gives up after `timeout` and returns `None`, so a
    /// loop can notice that it should stop.
    pub fn accept_timeout(&self, timeout: Duration) -> Result<Option<Session>> {
        match self.accept_tcp(timeout)? {
            Some(tcp) => {
                self.node.serve(tcp, Mode::Trusted(Arc::clone(&self.node.trust))).map(Some)
            }
            None => Ok(None),
        }
    }

    /// Waits for one device to connect in order to pair, and pairs with it.
    pub fn accept_for_pairing(
        &self,
        confirm: impl FnOnce(&PairingPrompt) -> bool,
    ) -> Result<Peer> {
        let (tcp, _) = self.tcp.accept()?;
        let session = self.node.serve(tcp, Mode::Pairing)?;
        self.node.finish_pairing(session, confirm)
    }

    /// Like [`accept_for_pairing`](Self::accept_for_pairing), but gives up after `timeout`
    /// and returns `None`, so a waiting screen can be cancelled.
    pub fn accept_for_pairing_timeout(
        &self,
        timeout: Duration,
        confirm: impl FnOnce(&PairingPrompt) -> bool,
    ) -> Result<Option<Peer>> {
        let Some(tcp) = self.accept_tcp(timeout)? else {
            return Ok(None);
        };
        let session = self.node.serve(tcp, Mode::Pairing)?;
        self.node.finish_pairing(session, confirm).map(Some)
    }

    /// Accepts a TCP connection, waiting at most `timeout`.
    fn accept_tcp(&self, timeout: Duration) -> Result<Option<TcpStream>> {
        let deadline = Instant::now() + timeout;
        self.tcp.set_nonblocking(true)?;
        let accepted = loop {
            match self.tcp.accept() {
                Ok((tcp, _)) => break Ok(tcp),
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        break Err(None);
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(err) => break Err(Some(err)),
            }
        };
        self.tcp.set_nonblocking(false)?;

        match accepted {
            Ok(tcp) => {
                // A socket accepted from a non-blocking listener can inherit that mode.
                tcp.set_nonblocking(false)?;
                Ok(Some(tcp))
            }
            Err(None) => Ok(None),
            Err(Some(err)) => Err(err.into()),
        }
    }
}

fn connect_tcp(addr: impl ToSocketAddrs) -> Result<TcpStream> {
    let mut last_error = None;
    for candidate in addr.to_socket_addrs()? {
        match TcpStream::connect_timeout(&candidate, CONNECT_TIMEOUT) {
            Ok(tcp) => return Ok(tcp),
            Err(err) => last_error = Some(err),
        }
    }
    Err(last_error
        .unwrap_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no address to connect to"))
        .into())
}
