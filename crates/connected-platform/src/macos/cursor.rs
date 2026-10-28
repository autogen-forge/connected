//! Hides the cursor while input is captured, and shows it again afterwards.
//!
//! macOS only lets a foreground app hide the cursor. A command-line or background process
//! gets there by making itself a UI-element app and setting a private window-server
//! property, `SetsCursorInBackground`, which Deskflow and Barrier rely on too. Because of
//! the private property this can break with a macOS update; hiding is cosmetic, so every
//! failure here is ignored and capture itself carries on.

use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::Duration;

use core_foundation::base::{CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::display::CGDisplay;

const TO_UI_ELEMENT_APP: u32 = 4;
/// Hides are counted and visibility lags a little behind a show, so a show is retried a few
/// times before giving up.
const MAX_SHOWS: u32 = 5;
const SHOW_SETTLE: Duration = Duration::from_millis(100);

static PREPARE: Once = Once::new();
static IS_GUI_APP: AtomicBool = AtomicBool::new(false);

/// Tells this module that the process is an app with windows. Making such a process a
/// UI-element app would take its Dock icon away, so that step is skipped; the cursor still
/// hides, but other apps keep showing hover effects while it is hidden.
pub fn declare_gui_app() {
    IS_GUI_APP.store(true, Ordering::Relaxed);
}

pub struct HiddenCursor {
    hidden: bool,
}

impl HiddenCursor {
    pub fn new() -> Self {
        Self { hidden: false }
    }

    pub fn hide(&mut self) {
        if self.hidden {
            return;
        }
        PREPARE.call_once(become_app_that_may_hide_the_cursor);
        self.hidden = CGDisplay::main().hide_cursor().is_ok();
    }

    pub fn show(&mut self) {
        if !self.hidden {
            return;
        }
        let display = CGDisplay::main();
        for _ in 0..MAX_SHOWS {
            let _ = display.show_cursor();
            sleep(SHOW_SETTLE);
            if is_visible() {
                break;
            }
        }
        self.hidden = false;
    }
}

impl Drop for HiddenCursor {
    fn drop(&mut self) {
        // The cursor must never stay hidden.
        self.show();
    }
}

fn is_visible() -> bool {
    // SAFETY: argument-free query.
    unsafe { CGCursorIsVisible() != 0 }
}

fn become_app_that_may_hide_the_cursor() {
    let key = CFString::new("SetsCursorInBackground");
    let mut psn = ProcessSerialNumber { high: 0, low: 0 };
    // SAFETY: `key` and the boolean are valid CF objects for the call, and `psn` is a valid
    // out-parameter; the Process Manager calls take no other pointers.
    unsafe {
        let connection = CGSMainConnectionID();
        CGSSetConnectionProperty(
            connection,
            connection,
            key.as_concrete_TypeRef(),
            CFBoolean::true_value().as_CFTypeRef(),
        );
        if !IS_GUI_APP.load(Ordering::Relaxed) {
            GetCurrentProcess(&mut psn);
            TransformProcessType(&psn, TO_UI_ELEMENT_APP);
            SetFrontProcess(&psn);
        }
    }
}

#[repr(C)]
struct ProcessSerialNumber {
    high: u32,
    low: u32,
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGCursorIsVisible() -> u32;
    fn CGSMainConnectionID() -> u32;
    fn CGSSetConnectionProperty(
        connection: u32,
        target: u32,
        key: CFStringRef,
        value: CFTypeRef,
    ) -> i32;
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn GetCurrentProcess(psn: *mut ProcessSerialNumber) -> i32;
    fn TransformProcessType(psn: *const ProcessSerialNumber, transform: u32) -> i32;
    fn SetFrontProcess(psn: *const ProcessSerialNumber) -> i32;
}
