//! The client: connects to a server and plays back the input it receives.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use connected_core::platform::Result as PlatformResult;
use connected_core::{InputSink, ScreenInfo, normalize};
use connected_net::{NetError, Node, Session};
use connected_proto::{ButtonState, LayoutScreen, Message, Modifiers};

use crate::error::Result;
use crate::{HEARTBEAT_TIMEOUT, Log};

const POLL: Duration = Duration::from_millis(50);
/// Past this many reports of keys or buttons that cannot be injected, the client stops
/// mentioning them: a missing key mapping would otherwise flood the log.
const MAX_INJECTION_REPORTS: u32 = 20;
/// How finely the wait before reconnecting is sliced, so that stopping is not held up.
const WAIT_SLICE: Duration = Duration::from_millis(100);

/// Connects to the server at `addr` and injects what it sends, until `stop` is set or the
/// connection ends. Keys and buttons still held down are released on the way out.
///
/// What the client tells its owner as things happen.
pub struct Hooks<'a> {
    /// Called with the server's name once the link is up.
    pub on_connected: &'a mut dyn FnMut(&str),
    /// Called with the layout the server sends, now and whenever it changes.
    pub on_layout: &'a mut dyn FnMut(Vec<LayoutScreen>),
}

pub fn run_client(
    node: &Node,
    addr: &str,
    sink: &mut dyn InputSink,
    screen: &dyn ScreenInfo,
    stop: &AtomicBool,
    log: &Log,
    hooks: Hooks<'_>,
) -> Result<()> {
    let mut session = node.connect(addr)?;
    log(&format!("connected to '{}'", session.peer().name));
    (hooks.on_connected)(&session.peer().name);

    // The server measures from the corner of all the displays; the system, from the main one's.
    let (displays, origin) = normalize(&screen.displays()?);
    session.send(&Message::Screen { displays, scales: screen.scales()?, os: screen.os() })?;

    let outcome = receive(&mut session, sink, origin, stop, log, &mut *hooks.on_layout);
    if let Err(err) = sink.release_all() {
        log(&format!("cannot release held keys: {err}"));
    }
    outcome
}

fn receive(
    session: &mut Session,
    sink: &mut dyn InputSink,
    origin: (i32, i32),
    stop: &AtomicBool,
    log: &Log,
    on_layout: &mut dyn FnMut(Vec<LayoutScreen>),
) -> Result<()> {
    let mut last_heard = Instant::now();
    let mut reports = 0u32;

    while !stop.load(Ordering::Relaxed) {
        let Some(message) = session.recv_timeout(POLL)? else {
            if last_heard.elapsed() > HEARTBEAT_TIMEOUT {
                return Err(NetError::Timeout.into());
            }
            continue;
        };
        last_heard = Instant::now();

        if let Message::Ping(n) = message {
            session.send(&Message::Pong(n))?;
        } else if let Message::Layout { screens } = message {
            on_layout(screens);
        } else if let Err(err) = inject(&message, origin, sink) {
            reports += 1;
            if reports <= MAX_INJECTION_REPORTS {
                log(&format!("cannot inject {message:?}: {err}"));
            }
        }
    }
    Ok(())
}

/// Carries out one message. `origin` is where the corner of this computer's displays is in
/// the system's coordinates, which the server's positions are measured from.
fn inject(message: &Message, origin: (i32, i32), sink: &mut dyn InputSink) -> PlatformResult<()> {
    match *message {
        Message::Enter { x, y, modifiers, .. } => {
            sink.move_abs(x + origin.0, y + origin.1)?;
            press_held_modifiers(modifiers, sink)
        }
        Message::Leave => sink.release_all(),
        Message::MouseMove { dx, dy } => sink.move_rel(dx, dy),
        Message::MouseButton { button, state } => sink.button(button, state),
        Message::Wheel { dx, dy } => sink.wheel(dx, dy),
        Message::Key { usage, state } => sink.key(usage, state),
        _ => Ok(()),
    }
}

/// The server's modifier keys were pressed before the cursor came over, so no key-down for
/// them was ever sent. Press them now, so that a Ctrl held while crossing still counts.
fn press_held_modifiers(modifiers: Modifiers, sink: &mut dyn InputSink) -> PlatformResult<()> {
    for bit in 0..8u16 {
        if modifiers.0 & (1 << bit) != 0 {
            sink.key(0xE0 + bit, ButtonState::Down)?;
        }
    }
    Ok(())
}

/// Where a client stands, for a screen to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientStatus {
    /// Trying to reach the server.
    Connecting,
    Connected { server: String },
    /// The link is down; it will be tried again shortly.
    Waiting { reason: String },
    /// The client cannot run at all, for example because input injection is not allowed.
    Failed { reason: String },
    Stopped,
}

/// A client that keeps itself connected: if the link drops it waits a moment and tries
/// again, until it is stopped. Dropping the handle stops the client.
pub struct ClientHandle {
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<ClientStatus>>,
    /// How the sharing computer says the computers are laid out, while connected.
    layout: Arc<Mutex<Option<Vec<LayoutScreen>>>>,
    thread: Option<JoinHandle<()>>,
}

impl ClientHandle {
    /// Starts connecting to the server at `addr`.
    ///
    /// `make_backends` runs on the client's own thread. `retry_after` is how long to wait
    /// between attempts.
    pub fn start(
        node: Node,
        addr: String,
        make_backends: impl FnOnce() -> Result<(Box<dyn InputSink>, Box<dyn ScreenInfo>)> + Send + 'static,
        retry_after: Duration,
        log: Log,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(ClientStatus::Connecting));
        let layout = Arc::new(Mutex::new(None));

        let thread = thread::spawn({
            let (stop, status, layout) = (Arc::clone(&stop), Arc::clone(&status), Arc::clone(&layout));
            move || {
                let set = |next: ClientStatus| {
                    if let Ok(mut status) = status.lock() {
                        *status = next;
                    }
                };

                let (mut sink, screen) = match make_backends() {
                    Ok(backends) => backends,
                    Err(err) => {
                        log(&format!("cannot start the client: {err}"));
                        set(ClientStatus::Failed { reason: err.to_string() });
                        return;
                    }
                };

                while !stop.load(Ordering::Relaxed) {
                    set(ClientStatus::Connecting);
                    let outcome = run_client(
                        &node,
                        &addr,
                        sink.as_mut(),
                        screen.as_ref(),
                        &stop,
                        &log,
                        Hooks {
                            on_connected: &mut |server| {
                                set(ClientStatus::Connected { server: server.to_owned() });
                            },
                            on_layout: &mut |screens| {
                                if let Ok(mut layout) = layout.lock() {
                                    *layout = Some(screens);
                                }
                            },
                        },
                    );
                    // A layout belongs to the connection it came over.
                    if let Ok(mut layout) = layout.lock() {
                        *layout = None;
                    }
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let reason = match outcome {
                        Ok(()) => "the connection ended".to_owned(),
                        Err(err) => err.to_string(),
                    };
                    log(&format!("not connected: {reason}"));
                    set(ClientStatus::Waiting { reason });

                    let resume = Instant::now() + retry_after;
                    while Instant::now() < resume && !stop.load(Ordering::Relaxed) {
                        thread::sleep(WAIT_SLICE);
                    }
                }
                set(ClientStatus::Stopped);
            }
        });

        Self { stop, status, layout, thread: Some(thread) }
    }

    /// How the sharing computer says the computers are laid out, while connected to it.
    pub fn layout(&self) -> Option<Vec<LayoutScreen>> {
        self.layout.lock().ok().and_then(|layout| layout.clone())
    }

    pub fn status(&self) -> ClientStatus {
        self.status.lock().map(|status| status.clone()).unwrap_or(ClientStatus::Stopped)
    }

    /// Stops the client and waits for it. Safe to call more than once.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for ClientHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use connected_proto::Os;
    use connected_proto::MouseButton;

    #[derive(Default)]
    struct Recorder(Vec<String>);

    impl InputSink for Recorder {
        fn move_abs(&mut self, x: i32, y: i32) -> PlatformResult<()> {
            self.0.push(format!("abs {x} {y}"));
            Ok(())
        }
        fn move_rel(&mut self, dx: i32, dy: i32) -> PlatformResult<()> {
            self.0.push(format!("rel {dx} {dy}"));
            Ok(())
        }
        fn button(&mut self, button: MouseButton, state: ButtonState) -> PlatformResult<()> {
            self.0.push(format!("button {button:?} {state:?}"));
            Ok(())
        }
        fn wheel(&mut self, dx: i32, dy: i32) -> PlatformResult<()> {
            self.0.push(format!("wheel {dx} {dy}"));
            Ok(())
        }
        fn key(&mut self, usage: u16, state: ButtonState) -> PlatformResult<()> {
            self.0.push(format!("key {usage:#x} {state:?}"));
            Ok(())
        }
        fn release_all(&mut self) -> PlatformResult<()> {
            self.0.push("release".into());
            Ok(())
        }
    }

    #[test]
    fn entering_moves_the_cursor_and_presses_the_modifiers_that_were_held() {
        let mut sink = Recorder::default();
        // Left Ctrl (bit 0) and right Shift (bit 5).
        let modifiers = Modifiers(0b0010_0001);
        inject(&Message::Enter { x: 0, y: 720, seq: 1, modifiers }, (0, 0), &mut sink).unwrap();
        assert_eq!(sink.0, vec!["abs 0 720", "key 0xe0 Down", "key 0xe5 Down"]);
    }

    #[test]
    fn entering_is_measured_from_the_corner_of_all_the_displays() {
        let mut sink = Recorder::default();
        // A computer with a display left of its main one: the corner is 1920 left of the origin.
        inject(&Message::Enter { x: 100, y: 50, seq: 1, modifiers: Modifiers(0) }, (-1920, 0), &mut sink)
            .unwrap();
        assert_eq!(sink.0, vec!["abs -1820 50"]);
    }

    #[test]
    fn leaving_releases_everything() {
        let mut sink = Recorder::default();
        inject(&Message::Leave, (0, 0), &mut sink).unwrap();
        assert_eq!(sink.0, vec!["release"]);
    }

    #[test]
    fn input_messages_map_onto_the_sink() {
        let mut sink = Recorder::default();
        inject(&Message::MouseMove { dx: 3, dy: -4 }, (0, 0), &mut sink).unwrap();
        inject(&Message::Wheel { dx: 0, dy: 120 }, (0, 0), &mut sink).unwrap();
        inject(
            &Message::MouseButton { button: MouseButton::Right, state: ButtonState::Up },
            (0, 0),
            &mut sink,
        )
        .unwrap();
        inject(&Message::Key { usage: 0x04, state: ButtonState::Down }, (0, 0), &mut sink).unwrap();
        assert_eq!(
            sink.0,
            vec!["rel 3 -4", "wheel 0 120", "button Right Up", "key 0x4 Down"]
        );
    }

    #[test]
    fn unrelated_messages_are_ignored() {
        let mut sink = Recorder::default();
        inject(&Message::Pong(1), (0, 0), &mut sink).unwrap();
        inject(&Message::Screen { displays: vec![], scales: vec![], os: Os::Other }, (0, 0), &mut sink).unwrap();
        assert!(sink.0.is_empty());
    }
}
