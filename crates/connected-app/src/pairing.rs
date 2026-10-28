//! Pairing as a conversation a screen can follow: waiting for the other device, a code to
//! compare, and an outcome.
//!
//! The work happens on a background thread. The screen polls for events and sends the
//! person's answer back.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use connected_net::{Node, Peer, PairingPrompt};

use crate::error::Result;

/// How long the waiting side listens before it checks whether it was cancelled.
const ACCEPT_SLICE: Duration = Duration::from_millis(200);
/// How long a confirmation waits before it checks whether it was cancelled.
const ANSWER_SLICE: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingEvent {
    /// Listening; the other device should connect to this address.
    Waiting { addr: String },
    /// The other device is connected. Both people should see this code, and say whether
    /// it is the same.
    Confirm(PairingPrompt),
    /// Over: either the device that was paired, or why it did not work out.
    Finished(std::result::Result<Peer, String>),
}

/// One attempt at pairing. Cancelling, or finishing, ends it.
pub struct PairingSession {
    events: Receiver<PairingEvent>,
    answers: Sender<bool>,
    cancel: Arc<AtomicBool>,
}

/// The background thread's end of the conversation.
struct Worker {
    events: Sender<PairingEvent>,
    answers: Receiver<bool>,
    cancel: Arc<AtomicBool>,
}

impl Worker {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Asks the person and waits for the answer. Anything but a yes counts as a no,
    /// including being cancelled.
    fn confirm(&self, prompt: &PairingPrompt) -> bool {
        if self.events.send(PairingEvent::Confirm(prompt.clone())).is_err() {
            return false;
        }
        loop {
            if self.cancelled() {
                return false;
            }
            match self.answers.recv_timeout(ANSWER_SLICE) {
                Ok(answer) => return answer,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return false,
            }
        }
    }

    fn finish(&self, outcome: std::result::Result<Peer, String>) {
        let _ = self.events.send(PairingEvent::Finished(outcome));
    }
}

impl PairingSession {
    /// Waits for another device to connect to `addr`, then pairs with it.
    pub fn listen(node: Node, addr: &str) -> Result<Self> {
        let listener = node.listen(addr)?;
        let waiting_on = listener.local_addr()?.to_string();
        Ok(Self::spawn(move |worker| {
            let _ = worker.events.send(PairingEvent::Waiting { addr: waiting_on });
            loop {
                if worker.cancelled() {
                    return worker.finish(Err("cancelled".into()));
                }
                match listener.accept_for_pairing_timeout(ACCEPT_SLICE, |prompt| worker.confirm(prompt)) {
                    Ok(Some(peer)) => return worker.finish(Ok(peer)),
                    Ok(None) => {}
                    Err(err) => return worker.finish(Err(err.to_string())),
                }
            }
        }))
    }

    /// Connects to a device that is waiting at `addr`, and pairs with it.
    pub fn connect(node: Node, addr: String) -> Self {
        Self::spawn(move |worker| {
            match node.pair_with(&addr, |prompt| worker.confirm(prompt)) {
                Ok(peer) => worker.finish(Ok(peer)),
                Err(err) => worker.finish(Err(err.to_string())),
            }
        })
    }

    fn spawn(work: impl FnOnce(Worker) + Send + 'static) -> Self {
        let (events_tx, events) = mpsc::channel();
        let (answers, answers_rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = Worker { events: events_tx, answers: answers_rx, cancel: Arc::clone(&cancel) };
        thread::spawn(move || work(worker));
        Self { events, answers, cancel }
    }

    /// The next thing that happened, if anything did.
    pub fn poll(&self) -> Option<PairingEvent> {
        self.events.try_recv().ok()
    }

    /// The person's answer to [`PairingEvent::Confirm`]: whether the codes matched.
    pub fn answer(&self, codes_match: bool) {
        let _ = self.answers.send(codes_match);
    }

    /// Gives up. A `Finished` event follows.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for PairingSession {
    fn drop(&mut self) {
        self.cancel();
    }
}
