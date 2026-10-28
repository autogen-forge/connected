//! OS backends for the traits in `connected-core`.
//!
//! Each backend sits in a module compiled only for its OS. The `open_*` functions pick the
//! one for the current target.

pub mod mock;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;
// Plain tables of Windows scan codes, built everywhere so that they are tested everywhere.
#[cfg(any(target_os = "windows", test))]
mod winkeys;
#[cfg(all(unix, not(target_os = "macos")))]
mod x11;

#[cfg(target_os = "macos")]
use macos as backend;
#[cfg(target_os = "windows")]
use windows as backend;
#[cfg(all(unix, not(target_os = "macos")))]
use x11 as backend;

use connected_core::platform::{Permission, Result};
use connected_core::{InputSink, InputSource, ScreenInfo};

/// Name of the backend compiled for this target.
pub const BACKEND: &str = backend::NAME;

/// Opens the injector for this machine (client side).
pub fn open_sink() -> Result<Box<dyn InputSink>> {
    backend::open_sink()
}

/// Opens the input capturer for this machine (server side).
pub fn open_source() -> Result<Box<dyn InputSource>> {
    backend::open_source()
}

/// Opens size and cursor queries for this machine's main screen.
pub fn open_screen() -> Result<Box<dyn ScreenInfo>> {
    backend::open_screen()
}

/// OS permissions the backend needs and whether this process holds them.
pub fn permissions() -> Vec<Permission> {
    backend::permissions()
}

/// Call once at startup from a program that has windows of its own. Some backends behave
/// differently inside an app than inside a command-line tool; macOS, for one, treats how the
/// cursor is hidden differently.
pub fn declare_gui_app() {
    backend::declare_gui_app()
}

/// Asks the OS to prompt for missing permissions, where it has such a prompt.
pub fn request_permissions() {
    backend::request_permissions()
}
