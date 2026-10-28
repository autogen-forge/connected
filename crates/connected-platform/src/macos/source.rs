//! Local input capture through a `CGEventTap`.
//!
//! The tap lives on its own thread with its own run loop. The callback converts each event,
//! queues it for `next_event_timeout`, and drops the original while capture is on. Dropping
//! mouse motion also keeps the cursor where it is, while the motion deltas still reach us.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use connected_core::platform::{PlatformError, Result};
use connected_core::{InputEvent, InputSource};
use core_foundation::base::TCFType;
use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes};
use core_graphics::event::{
    CGEvent, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
    CallbackResult, EventField,
};

use super::cursor::HiddenCursor;
use super::{SYNTHETIC_TAG, convert};

const WATCHED: [CGEventType; 14] = [
    CGEventType::MouseMoved,
    CGEventType::LeftMouseDown,
    CGEventType::LeftMouseUp,
    CGEventType::LeftMouseDragged,
    CGEventType::RightMouseDown,
    CGEventType::RightMouseUp,
    CGEventType::RightMouseDragged,
    CGEventType::OtherMouseDown,
    CGEventType::OtherMouseUp,
    CGEventType::OtherMouseDragged,
    CGEventType::ScrollWheel,
    CGEventType::KeyDown,
    CGEventType::KeyUp,
    CGEventType::FlagsChanged,
];

/// `CFRunLoop` is a thread-safe object (`CFRunLoopStop` may be called from any thread), but
/// the binding does not say so.
struct SendRunLoop(CFRunLoop);
unsafe impl Send for SendRunLoop {}

pub struct MacSource {
    events: Receiver<InputEvent>,
    capture: Arc<AtomicBool>,
    cursor: HiddenCursor,
    run_loop: Option<SendRunLoop>,
    thread: Option<JoinHandle<()>>,
}

impl MacSource {
    pub fn open() -> Result<Self> {
        let (event_tx, events) = mpsc::channel();
        let capture = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);

        let thread = std::thread::Builder::new()
            .name("connected-event-tap".into())
            .spawn({
                let capture = Arc::clone(&capture);
                move || spawn_tap(event_tx, capture, ready_tx)
            })
            .map_err(|err| PlatformError::Other(format!("cannot start tap thread: {err}")))?;

        let run_loop = match ready_rx.recv() {
            Ok(Ok(run_loop)) => run_loop,
            Ok(Err(err)) => {
                let _ = thread.join();
                return Err(err);
            }
            Err(_) => {
                let _ = thread.join();
                return Err(PlatformError::Other("event tap thread exited early".into()));
            }
        };

        Ok(Self {
            events,
            capture,
            cursor: HiddenCursor::new(),
            run_loop: Some(run_loop),
            thread: Some(thread),
        })
    }
}

fn spawn_tap(
    events: Sender<InputEvent>,
    capture: Arc<AtomicBool>,
    ready: mpsc::SyncSender<Result<SendRunLoop>>,
) {
    // The callback must re-enable the tap when the system disables it, but the tap does not
    // exist yet when the callback is built, so it finds the port through this slot.
    let port_slot: Arc<AtomicPtr<c_void>> = Arc::new(AtomicPtr::new(std::ptr::null_mut()));

    let callback = {
        let port_slot = Arc::clone(&port_slot);
        move |_proxy, kind: CGEventType, event: &CGEvent| {
            if matches!(
                kind,
                CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
            ) {
                let port = port_slot.load(Ordering::Acquire);
                if !port.is_null() {
                    // SAFETY: the slot holds the live mach port of this tap, which outlives
                    // every callback invocation.
                    unsafe { CGEventTapEnable(port, true) };
                }
                return CallbackResult::Keep;
            }

            // Events we injected ourselves must pass through untouched.
            if event.get_integer_value_field(EventField::EVENT_SOURCE_USER_DATA) == SYNTHETIC_TAG {
                return CallbackResult::Keep;
            }

            for converted in convert::convert(kind, event) {
                // The receiver is gone only while the source is being dropped.
                let _ = events.send(converted);
            }

            if capture.load(Ordering::Acquire) {
                CallbackResult::Drop
            } else {
                CallbackResult::Keep
            }
        }
    };

    let tap = match CGEventTap::new(
        CGEventTapLocation::Session,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::Default,
        WATCHED.to_vec(),
        callback,
    ) {
        Ok(tap) => tap,
        Err(()) => {
            let _ = ready.send(Err(PlatformError::PermissionDenied(
                "cannot create the event tap; allow this app under System Settings > \
                 Privacy & Security > Accessibility",
            )));
            return;
        }
    };

    let Ok(loop_source) = tap.mach_port().create_runloop_source(0) else {
        let _ = ready.send(Err(PlatformError::Other("cannot create run loop source".into())));
        return;
    };
    port_slot.store(tap.mach_port().as_concrete_TypeRef() as *mut c_void, Ordering::Release);

    let run_loop = CFRunLoop::get_current();
    // SAFETY: `kCFRunLoopCommonModes` is a constant exported by CoreFoundation.
    run_loop.add_source(&loop_source, unsafe { kCFRunLoopCommonModes });
    tap.enable();

    if ready.send(Ok(SendRunLoop(run_loop))).is_err() {
        return;
    }
    CFRunLoop::run_current();
    // `tap` drops here, which invalidates the mach port.
}

impl InputSource for MacSource {
    fn next_event_timeout(&mut self, timeout: Duration) -> Result<Option<InputEvent>> {
        match self.events.recv_timeout(timeout) {
            Ok(event) => Ok(Some(event)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => {
                Err(PlatformError::Other("event tap stopped".into()))
            }
        }
    }

    fn set_capture(&mut self, capture: bool) -> Result<()> {
        if capture {
            self.cursor.hide();
        }
        self.capture.store(capture, Ordering::Release);
        if !capture {
            self.cursor.show();
        }
        Ok(())
    }

    fn set_blocking(&mut self, block: bool) -> Result<()> {
        self.block(block);
        // The other computer's mouse moves this cursor, so it has to be seen.
        if block && !self.cursor.ensure_visible() {
            return Err(PlatformError::Other("the cursor stays hidden".into()));
        }
        Ok(())
    }
}

impl MacSource {
    /// Swallows this computer's own input without hiding the cursor.
    fn block(&mut self, block: bool) {
        self.capture.store(block, Ordering::Release);
        if !block {
            self.cursor.show();
        }
    }
}

impl Drop for MacSource {
    fn drop(&mut self) {
        self.capture.store(false, Ordering::Release);
        if let Some(run_loop) = self.run_loop.take() {
            run_loop.0.stop();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapEnable(tap: *mut c_void, enable: bool);
}
