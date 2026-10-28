//! Windows backend: low-level hooks and raw input capture input, `SendInput` injects it.

mod screen;
mod sink;
mod source;

use connected_core::platform::{Permission, Result};
use connected_core::{InputSink, InputSource, ScreenInfo};

pub const NAME: &str = "windows (low-level hooks / SendInput)";

/// Asks Windows to keep its clock ticking every millisecond instead of every 15. Waits on the
/// network and between messages are rounded up to a tick, which would make the cursor of another
/// computer move in jumps.
fn fine_timer() {
    use std::sync::Once;
    use windows_sys::Win32::Media::timeBeginPeriod;

    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: a plain call; it lasts until the program ends.
        unsafe {
            timeBeginPeriod(1);
        }
    });
}

/// Makes this process see the real pixels of every monitor. Without it, Windows shows a
/// program that does not say it can cope with scaling a pretend, scaled-down desktop, and the
/// cursor positions and monitor sizes it reads would not match what the other computers expect.
/// Harmless if it was already set, for instance by the program's manifest.
fn see_real_pixels() {
    use std::sync::Once;
    use windows_sys::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
    };

    fine_timer();
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: a plain call with a constant; it fails harmlessly when already set.
        unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
    });
}

pub fn open_sink() -> Result<Box<dyn InputSink>> {
    see_real_pixels();
    Ok(Box::new(sink::WinSink::new()))
}

pub fn open_source() -> Result<Box<dyn InputSource>> {
    see_real_pixels();
    Ok(Box::new(source::WinSource::open()?))
}

pub fn open_screen() -> Result<Box<dyn ScreenInfo>> {
    see_real_pixels();
    Ok(Box::new(screen::WinScreen))
}

/// Windows asks for no permission to read or send input.
pub fn permissions() -> Vec<Permission> {
    Vec::new()
}

pub fn request_permissions() {}

pub fn declare_gui_app() {
    see_real_pixels();
}
