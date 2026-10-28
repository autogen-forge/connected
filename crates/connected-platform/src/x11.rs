use connected_core::platform::{Permission, PlatformError, Result};
use connected_core::{InputSink, InputSource, ScreenInfo};

pub const NAME: &str = "linux/x11 (XInput2 / XTest)";

pub fn open_sink() -> Result<Box<dyn InputSink>> {
    Err(PlatformError::Unsupported("X11 sink via XTest is not written yet"))
}

pub fn open_source() -> Result<Box<dyn InputSource>> {
    Err(PlatformError::Unsupported("X11 source via XInput2 is not written yet"))
}

pub fn open_screen() -> Result<Box<dyn ScreenInfo>> {
    Err(PlatformError::Unsupported("X11 screen info is not written yet"))
}

pub fn permissions() -> Vec<Permission> {
    Vec::new()
}

pub fn request_permissions() {}

pub fn declare_gui_app() {}
