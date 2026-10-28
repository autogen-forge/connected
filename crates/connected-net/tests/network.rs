//! End-to-end tests over real localhost sockets: pairing, trusted connections, and the ways
//! a connection must be refused.

use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use connected_net::{Identity, Listener, NetError, Node, Peer, Session, Side, TrustStore};
use connected_proto::{ClipFormat, Message};

const SHORT: Duration = Duration::from_millis(200);
const LONG: Duration = Duration::from_secs(10);

fn node(name: &str) -> Node {
    Node::new(Identity::generate(name).unwrap(), TrustStore::in_memory())
}

fn listen(node: &Node) -> (Listener, String) {
    let listener = node.listen("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    (listener, addr)
}

struct PairingOutcome {
    listener_side: Result<Peer, NetError>,
    connector_side: Result<Peer, NetError>,
    listener_code: String,
    connector_code: String,
}

/// `a` waits to be paired, `b` connects. Each side's person answers as told.
fn try_pairing(a: &Node, b: &Node, a_says: bool, b_says: bool) -> PairingOutcome {
    let (listener, addr) = listen(a);
    let (code_tx, code_rx) = mpsc::channel();
    let waiting: JoinHandle<Result<Peer, NetError>> = thread::spawn(move || {
        listener.accept_for_pairing(|prompt| {
            code_tx.send(prompt.code.clone()).unwrap();
            a_says
        })
    });

    let mut connector_code = String::new();
    let connector_side = b.pair_with(&addr, |prompt| {
        connector_code = prompt.code.clone();
        b_says
    });
    PairingOutcome {
        listener_side: waiting.join().unwrap(),
        connector_side,
        listener_code: code_rx.recv_timeout(LONG).unwrap_or_default(),
        connector_code,
    }
}

fn paired_nodes() -> (Node, Node) {
    let (a, b) = (node("desk"), node("laptop"));
    let outcome = try_pairing(&a, &b, true, true);
    outcome.listener_side.unwrap();
    outcome.connector_side.unwrap();
    (a, b)
}

/// Runs `serve` on the first paired device that connects to `server`, and returns the
/// client's side of that connection.
fn connect_paired(
    server: &Node,
    client: &Node,
    serve: impl FnOnce(Session) + Send + 'static,
) -> (Session, JoinHandle<()>) {
    let (listener, addr) = listen(server);
    let handle = thread::spawn(move || serve(listener.accept().unwrap()));
    (client.connect(&addr).unwrap(), handle)
}

#[test]
fn pairing_shows_the_same_code_on_both_devices_and_trusts_both_ways() {
    let (a, b) = (node("desk"), node("laptop"));
    let outcome = try_pairing(&a, &b, true, true);

    let peer_seen_by_a = outcome.listener_side.unwrap();
    let peer_seen_by_b = outcome.connector_side.unwrap();
    assert_eq!(outcome.listener_code, outcome.connector_code);
    assert!(!outcome.listener_code.is_empty());

    assert_eq!(peer_seen_by_a.name, "laptop");
    assert_eq!(peer_seen_by_a.fingerprint, b.identity().fingerprint());
    assert_eq!(peer_seen_by_b.name, "desk");
    assert_eq!(peer_seen_by_b.fingerprint, a.identity().fingerprint());

    assert_eq!(a.trusted_peers().len(), 1);
    assert_eq!(a.trusted_peers()[0].fingerprint, b.identity().fingerprint());
    assert_eq!(b.trusted_peers()[0].fingerprint, a.identity().fingerprint());
}

#[test]
fn pairing_is_refused_when_the_connecting_side_says_no() {
    let (a, b) = (node("desk"), node("laptop"));
    let outcome = try_pairing(&a, &b, true, false);

    assert!(matches!(outcome.connector_side, Err(NetError::Declined(Side::ThisDevice))));
    assert!(matches!(outcome.listener_side, Err(NetError::Declined(Side::OtherDevice))));
    assert!(a.trusted_peers().is_empty());
    assert!(b.trusted_peers().is_empty());
}

#[test]
fn pairing_is_refused_when_the_waiting_side_says_no() {
    let (a, b) = (node("desk"), node("laptop"));
    let outcome = try_pairing(&a, &b, false, true);

    assert!(matches!(outcome.listener_side, Err(NetError::Declined(Side::ThisDevice))));
    assert!(matches!(outcome.connector_side, Err(NetError::Declined(Side::OtherDevice))));
    assert!(a.trusted_peers().is_empty());
    assert!(b.trusted_peers().is_empty());
}

#[test]
fn paired_devices_connect_and_exchange_messages() {
    let (desk, laptop) = paired_nodes();
    let (mut client, server) = connect_paired(&desk, &laptop, |mut session| {
        assert_eq!(session.peer().name, "laptop");
        while let Ok(message) = session.recv() {
            if let Message::Ping(n) = message {
                session.send(&Message::Pong(n)).unwrap();
            }
        }
    });

    assert_eq!(client.peer().name, "desk");
    assert_eq!(client.peer().fingerprint, desk.identity().fingerprint());

    for n in [1, 2, 3] {
        client.send(&Message::Ping(n)).unwrap();
        assert_eq!(client.recv_timeout(LONG).unwrap(), Some(Message::Pong(n)));
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn large_messages_survive_being_split_across_reads() {
    let (desk, laptop) = paired_nodes();
    let payload = vec![0xAB; 300_000];
    let expected = Message::ClipData {
        id: 1,
        format: ClipFormat::Png,
        offset: 0,
        data: payload,
        last: true,
    };

    let sent = expected.clone();
    let (mut client, server) = connect_paired(&desk, &laptop, move |mut session| {
        session.send(&sent).unwrap();
    });

    assert_eq!(client.recv_timeout(LONG).unwrap(), Some(expected));
    server.join().unwrap();
}

#[test]
fn an_idle_session_times_out_and_stays_usable() {
    let (desk, laptop) = paired_nodes();
    let (mut client, server) = connect_paired(&desk, &laptop, |mut session| {
        let message = session.recv().unwrap();
        session.send(&message).unwrap();
    });

    assert_eq!(client.recv_timeout(SHORT).unwrap(), None);
    assert_eq!(client.recv_timeout(SHORT).unwrap(), None);

    client.send(&Message::Ping(9)).unwrap();
    assert_eq!(client.recv_timeout(LONG).unwrap(), Some(Message::Ping(9)));
    server.join().unwrap();
}

#[test]
fn a_closed_connection_is_reported_as_closed() {
    let (desk, laptop) = paired_nodes();
    let (mut client, server) = connect_paired(&desk, &laptop, drop);
    server.join().unwrap();

    assert!(matches!(client.recv_timeout(LONG), Err(NetError::Closed)));
}

#[test]
fn a_device_that_was_never_paired_cannot_connect() {
    let desk = node("desk");
    let stranger = node("stranger");

    let (listener, addr) = listen(&desk);
    let server = thread::spawn(move || listener.accept().map(|_| ()));

    // The stranger does not trust the server either, so it refuses first.
    assert!(stranger.connect(&addr).is_err());
    assert!(server.join().unwrap().is_err());
}

#[test]
fn the_server_turns_away_a_client_it_does_not_know() {
    let (desk, _laptop) = paired_nodes();

    // The stranger is willing to talk to the desk, so only the desk's check is in play.
    let stranger = {
        let mut trust = TrustStore::in_memory();
        trust
            .add(connected_net::TrustedPeer {
                name: "desk".into(),
                fingerprint: desk.identity().fingerprint(),
            })
            .unwrap();
        Node::new(Identity::generate("stranger").unwrap(), trust)
    };

    let (listener, addr) = listen(&desk);
    let server = thread::spawn(move || listener.accept().map(|_| ()));

    assert!(stranger.connect(&addr).is_err());
    assert!(matches!(server.join().unwrap(), Err(NetError::Tls(_))));
}

#[test]
fn a_client_does_not_talk_to_a_server_it_has_not_paired_with() {
    let (_desk, laptop) = paired_nodes();
    let impostor_server = node("desk");

    let (listener, addr) = listen(&impostor_server);
    let server = thread::spawn(move || listener.accept().map(|_| ()));

    assert!(laptop.connect(&addr).is_err());
    let _ = server.join();
}

#[test]
fn an_impostor_using_a_paired_devices_name_is_refused() {
    let (desk, _laptop) = paired_nodes();

    // Same name as the paired laptop, but a different key pair. It trusts the desk.
    let mut trust = TrustStore::in_memory();
    trust
        .add(connected_net::TrustedPeer {
            name: "desk".into(),
            fingerprint: desk.identity().fingerprint(),
        })
        .unwrap();
    let impostor = Node::new(Identity::generate("laptop").unwrap(), trust);

    let (listener, addr) = listen(&desk);
    let server = thread::spawn(move || listener.accept().map(|_| ()));

    assert!(impostor.connect(&addr).is_err());
    assert!(server.join().unwrap().is_err());
}

/// Both devices keep their trusted list in a file, as the real program does.
fn paired_nodes_with_files(label: &str) -> (Node, Node, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("connected-net-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let make = |name: &str| {
        let trust = TrustStore::load(&dir.join(format!("{name}-trusted"))).unwrap();
        Node::new(Identity::generate(name).unwrap(), trust)
    };
    let (desk, laptop) = (make("desk"), make("laptop"));
    let outcome = try_pairing(&desk, &laptop, true, true);
    outcome.listener_side.unwrap();
    outcome.connector_side.unwrap();
    (desk, laptop, dir)
}

#[test]
fn forgetting_a_device_in_another_process_locks_it_out_of_a_running_server() {
    let (desk, laptop, dir) = paired_nodes_with_files("forget");
    let (listener, addr) = listen(&desk);
    let server = thread::spawn(move || {
        let first = listener.accept().map(|_| ());
        let second = listener.accept().map(|_| ());
        (first, second)
    });

    laptop.connect(&addr).expect("paired device is welcome");

    // `connected forget laptop`, run as a separate program against the same file.
    let mut elsewhere = TrustStore::load(&dir.join("desk-trusted")).unwrap();
    assert_eq!(elsewhere.remove("laptop").unwrap().len(), 1);

    assert!(laptop.connect(&addr).is_err(), "forgotten device must be refused");
    let (first, second) = server.join().unwrap();
    assert!(first.is_ok());
    assert!(second.is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn adding_a_device_in_another_process_lets_it_into_a_running_server() {
    let (desk, _laptop, dir) = paired_nodes_with_files("learn");

    // A phone that already trusts the desk but is not yet known to it.
    let phone_identity = Identity::generate("phone").unwrap();
    let phone_fingerprint = phone_identity.fingerprint();
    let mut phone_trust = TrustStore::in_memory();
    phone_trust
        .add(connected_net::TrustedPeer {
            name: "desk".into(),
            fingerprint: desk.identity().fingerprint(),
        })
        .unwrap();
    let phone = Node::new(phone_identity, phone_trust);

    let (listener, addr) = listen(&desk);
    let server = thread::spawn(move || {
        let first = listener.accept().map(|_| ());
        let second = listener.accept().map(|_| ());
        (first, second)
    });

    assert!(phone.connect(&addr).is_err(), "not paired yet");

    // `connected pair`, run as a separate program against the same file.
    let mut elsewhere = TrustStore::load(&dir.join("desk-trusted")).unwrap();
    elsewhere
        .add(connected_net::TrustedPeer { name: "phone".into(), fingerprint: phone_fingerprint })
        .unwrap();

    phone.connect(&addr).expect("newly paired device is welcome without a restart");
    let (first, second) = server.join().unwrap();
    assert!(first.is_err());
    assert!(second.is_ok());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn trust_is_remembered_across_connections() {
    let (desk, laptop) = paired_nodes();
    let (listener, addr) = listen(&desk);
    let server = thread::spawn(move || {
        for _ in 0..2 {
            let mut session = listener.accept().unwrap();
            session.send(&Message::Pong(1)).unwrap();
        }
    });

    for _ in 0..2 {
        let mut client = laptop.connect(&addr).unwrap();
        assert_eq!(client.recv_timeout(LONG).unwrap(), Some(Message::Pong(1)));
    }
    server.join().unwrap();
}
