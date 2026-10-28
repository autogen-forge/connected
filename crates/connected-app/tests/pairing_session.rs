//! Pairing driven the way a screen drives it: poll for events, answer the question.

use std::thread;
use std::time::{Duration, Instant};

use connected_app::{PairingEvent, PairingSession};
use connected_net::{Identity, Node, TrustStore};

fn new_node(name: &str) -> Node {
    Node::new(Identity::generate(name).unwrap(), TrustStore::in_memory())
}

/// Polls `session` until `pick` accepts an event, answering any confirmation with `answer`.
fn next_event<T>(
    session: &PairingSession,
    answer: bool,
    mut pick: impl FnMut(PairingEvent) -> Option<T>,
) -> T {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        while let Some(event) = session.poll() {
            if let PairingEvent::Confirm(_) = &event {
                session.answer(answer);
            }
            if let Some(found) = pick(event) {
                return found;
            }
        }
        assert!(Instant::now() < deadline, "timed out waiting for a pairing event");
        thread::sleep(Duration::from_millis(5));
    }
}

fn waiting_address(session: &PairingSession) -> String {
    next_event(session, true, |event| match event {
        PairingEvent::Waiting { addr } => Some(addr),
        _ => None,
    })
}

fn outcome(session: &PairingSession, answer: bool) -> Result<String, String> {
    next_event(session, answer, |event| match event {
        PairingEvent::Finished(result) => Some(result.map(|peer| peer.name)),
        _ => None,
    })
}

#[test]
fn two_sessions_pair_when_both_people_agree() {
    let (desk, laptop) = (new_node("desk"), new_node("laptop"));
    let waiting = PairingSession::listen(desk.clone(), "127.0.0.1:0").unwrap();
    let addr = waiting_address(&waiting);
    let connecting = PairingSession::connect(laptop.clone(), addr);

    // Each side is shown a code; the codes match.
    let mut codes = Vec::new();
    for session in [&waiting, &connecting] {
        let code = next_event(session, true, |event| match event {
            PairingEvent::Confirm(prompt) => Some(prompt.code),
            _ => None,
        });
        codes.push(code);
    }
    assert_eq!(codes[0], codes[1]);

    assert_eq!(outcome(&waiting, true), Ok("laptop".to_owned()));
    assert_eq!(outcome(&connecting, true), Ok("desk".to_owned()));
    assert_eq!(desk.trusted_peers().len(), 1);
    assert_eq!(laptop.trusted_peers().len(), 1);
}

#[test]
fn nothing_is_paired_when_one_person_says_the_codes_differ() {
    let (desk, laptop) = (new_node("desk"), new_node("laptop"));
    let waiting = PairingSession::listen(desk.clone(), "127.0.0.1:0").unwrap();
    let addr = waiting_address(&waiting);
    let connecting = PairingSession::connect(laptop.clone(), addr);

    // The person at the desk agrees; the person at the laptop does not.
    let (at_desk, at_laptop) = both_outcomes((&waiting, true), (&connecting, false));

    assert!(at_desk.is_err());
    assert!(at_laptop.is_err());
    assert!(desk.trusted_peers().is_empty());
    assert!(laptop.trusted_peers().is_empty());
}

/// Follows two sessions at once, each answering confirmations as told, until both end.
fn both_outcomes(
    (first, first_answer): (&PairingSession, bool),
    (second, second_answer): (&PairingSession, bool),
) -> (Result<String, String>, Result<String, String>) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let (mut first_result, mut second_result) = (None, None);
    while first_result.is_none() || second_result.is_none() {
        for (session, answer, result) in [
            (first, first_answer, &mut first_result),
            (second, second_answer, &mut second_result),
        ] {
            while let Some(event) = session.poll() {
                match event {
                    PairingEvent::Confirm(_) => session.answer(answer),
                    PairingEvent::Finished(done) => *result = Some(done.map(|peer| peer.name)),
                    PairingEvent::Waiting { .. } => {}
                }
            }
        }
        assert!(Instant::now() < deadline, "timed out waiting for both sessions to end");
        thread::sleep(Duration::from_millis(5));
    }
    (first_result.unwrap(), second_result.unwrap())
}

#[test]
fn waiting_can_be_cancelled() {
    let desk = new_node("desk");
    let waiting = PairingSession::listen(desk, "127.0.0.1:0").unwrap();
    waiting_address(&waiting);

    waiting.cancel();
    assert_eq!(outcome(&waiting, true), Err("cancelled".to_owned()));
}

#[test]
fn connecting_to_nothing_ends_with_an_error() {
    let laptop = new_node("laptop");
    let session = PairingSession::connect(laptop, "127.0.0.1:1".into());
    assert!(outcome(&session, true).is_err());
}

#[test]
fn listening_on_an_address_in_use_fails_at_once() {
    let desk = new_node("desk");
    let first = PairingSession::listen(desk.clone(), "127.0.0.1:0").unwrap();
    let taken = waiting_address(&first);
    assert!(PairingSession::listen(desk, &taken).is_err());
}
