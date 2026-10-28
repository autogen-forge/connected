//! Monitors and the cursor position.

use std::mem::size_of;
use std::ptr::null;

use connected_core::platform::{PlatformError, Result};
use connected_core::{Rect, ScreenInfo};
use windows_sys::Win32::Foundation::{LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN,
};

/// The flag in `MONITORINFO` that marks the main monitor.
const MONITORINFOF_PRIMARY: u32 = 1;

/// What this computer's monitors are and where the cursor is, in pixels. The origin is the
/// top-left corner of the main monitor, and other monitors can be at negative positions.
pub struct WinScreen;

impl ScreenInfo for WinScreen {
    fn size(&self) -> Result<(i32, i32)> {
        // SAFETY: argument-free metric queries.
        let (width, height) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        if width <= 0 || height <= 0 {
            return Err(PlatformError::Other("Windows reported no screen".into()));
        }
        Ok((width, height))
    }

    fn displays(&self) -> Result<Vec<Rect>> {
        let mut found: Vec<(bool, Rect)> = Vec::new();
        // SAFETY: `collect` only runs during this call, and `found` outlives it.
        let ok = unsafe {
            EnumDisplayMonitors(std::ptr::null_mut(), null(), Some(collect), &mut found as *mut _ as LPARAM)
        };
        if ok == 0 || found.is_empty() {
            return Err(PlatformError::Other("Windows could not list the monitors".into()));
        }
        // The main monitor first, the rest in the order Windows lists them.
        found.sort_by_key(|(primary, _)| !*primary);
        Ok(found.into_iter().map(|(_, rect)| rect).collect())
    }

    fn cursor(&self) -> Result<(i32, i32)> {
        let mut point = POINT { x: 0, y: 0 };
        // SAFETY: `point` is valid for writes.
        if unsafe { GetCursorPos(&mut point) } == 0 {
            return Err(PlatformError::Other("Windows could not read the cursor position".into()));
        }
        Ok((point.x, point.y))
    }
}

/// Called by Windows once for each monitor.
unsafe extern "system" fn collect(
    monitor: HMONITOR,
    _context: HDC,
    _rect: *mut RECT,
    found: LPARAM,
) -> i32 {
    // SAFETY: `found` is the vector passed to `EnumDisplayMonitors` above.
    let found = unsafe { &mut *(found as *mut Vec<(bool, Rect)>) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        rcMonitor: RECT { left: 0, top: 0, right: 0, bottom: 0 },
        rcWork: RECT { left: 0, top: 0, right: 0, bottom: 0 },
        dwFlags: 0,
    };
    // SAFETY: `info` has its size set, as the call requires.
    if unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
        let r = info.rcMonitor;
        found.push((
            info.dwFlags & MONITORINFOF_PRIMARY != 0,
            Rect::new(r.left, r.top, r.right - r.left, r.bottom - r.top),
        ));
    }
    1 // keep going
}
