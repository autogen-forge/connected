//! The seam between portable logic and OS code. Backends live in `connected-platform`.

use std::time::Duration;

use connected_proto::{ButtonState, ClipFormat, MouseButton, Os, Rect};

pub type Result<T> = std::result::Result<T, PlatformError>;

#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("not supported: {0}")]
    Unsupported(&'static str),
    #[error("permission denied: {0}")]
    PermissionDenied(&'static str),
    #[error("{0}")]
    Other(String),
}

/// A local input event, in the form the router consumes and the wire carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEvent {
    MouseMove { dx: i32, dy: i32 },
    MouseButton { button: MouseButton, state: ButtonState },
    Wheel { dx: i32, dy: i32 },
    /// `usage` is a HID usage on the keyboard page (0x07).
    Key { usage: u16, state: ButtonState },
}

/// Injects input into the local machine (client side).
pub trait InputSink: Send {
    fn move_abs(&mut self, x: i32, y: i32) -> Result<()>;
    fn move_rel(&mut self, dx: i32, dy: i32) -> Result<()>;
    fn button(&mut self, button: MouseButton, state: ButtonState) -> Result<()>;
    fn wheel(&mut self, dx: i32, dy: i32) -> Result<()>;
    fn key(&mut self, usage: u16, state: ButtonState) -> Result<()>;
    /// Releases every key and button this sink still holds down.
    fn release_all(&mut self) -> Result<()>;
}

/// Observes local input and can suppress it (server side).
pub trait InputSource: Send {
    /// Waits up to `timeout` for the next local event. `None` means the wait timed out.
    fn next_event_timeout(&mut self, timeout: Duration) -> Result<Option<InputEvent>>;

    /// Blocks until the next local event.
    fn next_event(&mut self) -> Result<InputEvent> {
        loop {
            if let Some(event) = self.next_event_timeout(Duration::from_secs(1))? {
                return Ok(event);
            }
        }
    }

    /// While capturing, events reach `next_event` but not the local desktop.
    fn set_capture(&mut self, capture: bool) -> Result<()>;

    /// How this source is reading input, for the log when something does not work.
    fn describe(&self) -> String {
        String::new()
    }
}

/// An OS permission the backend needs, and whether this process has it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permission {
    pub name: &'static str,
    pub granted: bool,
}

pub trait Clipboard: Send {
    /// Increases whenever the clipboard content changes.
    fn sequence(&self) -> Result<u64>;
    fn read(&mut self, format: ClipFormat) -> Result<Option<Vec<u8>>>;
    fn write(&mut self, format: ClipFormat, data: &[u8]) -> Result<()>;
}

pub trait ScreenInfo {
    /// Size of the primary screen in pixels.
    fn size(&self) -> Result<(i32, i32)>;

    /// Every display, in the coordinates [`cursor`](Self::cursor) reports, the main display
    /// first. A computer with one display is that display, from the origin.
    fn displays(&self) -> Result<Vec<Rect>> {
        let (width, height) = self.size()?;
        Ok(vec![Rect::new(0, 0, width, height)])
    }

    /// How much each display is magnified, in percent, in the order of [`displays`](Self::displays).
    fn scales(&self) -> Result<Vec<u16>> {
        Ok(vec![100; self.displays()?.len()])
    }

    /// The system this computer runs.
    fn os(&self) -> Os {
        Os::current()
    }

    fn cursor(&self) -> Result<(i32, i32)>;
}
