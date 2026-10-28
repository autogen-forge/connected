//! macOS backend: `CGEventTap` captures input, `CGEventPost` injects it.

mod convert;
mod cursor;
mod keymap;
mod sink;
mod source;

use connected_core::platform::{Permission, Result};
use connected_core::{InputSink, InputSource, ScreenInfo};
use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;

pub const NAME: &str = "macos (CGEventTap / CGEventPost)";

/// Written into `EVENT_SOURCE_USER_DATA` of every event this backend injects, so the event
/// tap can tell its own output from real input. ASCII "CONN".
pub(crate) const SYNTHETIC_TAG: i64 = 0x434F_4E4E;

/// See [`cursor::declare_gui_app`].
pub fn declare_gui_app() {
    cursor::declare_gui_app();
}

pub fn open_sink() -> Result<Box<dyn InputSink>> {
    Ok(Box::new(sink::MacSink::open()?))
}

pub fn open_source() -> Result<Box<dyn InputSource>> {
    Ok(Box::new(source::MacSource::open()?))
}

pub fn open_screen() -> Result<Box<dyn ScreenInfo>> {
    Ok(Box::new(sink::MacScreen::open()?))
}

/// The permissions this program needs. Only Accessibility: the event tap is an active one, which
/// may swallow events, and the system asks Accessibility of such a tap and of sending events.
/// Input Monitoring is for taps that only listen, so it is neither needed nor asked for; and the
/// Post Events preflight just follows Accessibility, so it adds nothing to wait for. (The
/// system's own log, `subsystem == "com.apple.TCC"`, shows grants as Accessibility only.)
pub fn permissions() -> Vec<Permission> {
    // SAFETY: an argument-free preflight query.
    unsafe { vec![Permission { name: "Accessibility", granted: AXIsProcessTrusted() }] }
}

/// Asks the system to show its permission prompts for whatever is still missing.
pub fn request_permissions() {
    let prompt_key = CFString::new("AXTrustedCheckOptionPrompt");
    let options = CFDictionary::from_CFType_pairs(&[(
        prompt_key.as_CFType(),
        CFBoolean::true_value().as_CFType(),
    )]);
    // SAFETY: `options` is a valid dictionary for the duration of the call.
    unsafe {
        AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef());
    }
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: core_foundation::dictionary::CFDictionaryRef) -> bool;
}

