//! Input injection through `SendInput`.

use std::mem::{size_of, zeroed};

use connected_core::platform::{PlatformError, Result};
use connected_core::InputSink;
use connected_proto::{ButtonState, MouseButton};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    KEYEVENTF_SCANCODE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT, SendInput,
};
use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, SetCursorPos, XBUTTON1, XBUTTON2};

use crate::winkeys;

/// Sends keys, buttons, wheel turns and cursor movement to this computer, remembering what it
/// holds down so that it can let go of it.
pub struct WinSink {
    held_keys: Vec<u16>,
    held_buttons: Vec<MouseButton>,
}

impl WinSink {
    pub fn new() -> Self {
        Self { held_keys: Vec::new(), held_buttons: Vec::new() }
    }
}

fn send(input: INPUT) -> Result<()> {
    // SAFETY: `input` is one fully set-up INPUT, and the size passed is that of the type.
    let sent = unsafe { SendInput(1, &input, size_of::<INPUT>() as i32) };
    if sent == 1 {
        Ok(())
    } else {
        // Windows refuses when something with more rights, such as an administrator's window
        // or the lock screen, has the focus.
        Err(PlatformError::Other("Windows did not accept the input (is a window with more rights in front?)".into()))
    }
}

fn mouse(dx: i32, dy: i32, data: u32, flags: u32) -> INPUT {
    // SAFETY: INPUT is plain data; all-zero is a valid empty value that is filled in below.
    let mut input: INPUT = unsafe { zeroed() };
    input.r#type = INPUT_MOUSE;
    input.Anonymous = INPUT_0 {
        mi: MOUSEINPUT { dx, dy, mouseData: data, dwFlags: flags, time: 0, dwExtraInfo: 0 },
    };
    input
}

fn keyboard(scan: u16, extended: bool, up: bool) -> INPUT {
    // SAFETY: as in `mouse`.
    let mut input: INPUT = unsafe { zeroed() };
    input.r#type = INPUT_KEYBOARD;
    let mut flags = KEYEVENTF_SCANCODE;
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    input.Anonymous = INPUT_0 {
        ki: KEYBDINPUT { wVk: 0, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 },
    };
    input
}

/// The flags and extra data that press or release a mouse button.
fn button_input(button: MouseButton, state: ButtonState) -> INPUT {
    let down = state == ButtonState::Down;
    let (flags, data) = match (button, down) {
        (MouseButton::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
        (MouseButton::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
        (MouseButton::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
        (MouseButton::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
        (MouseButton::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
        (MouseButton::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
        (MouseButton::Back, true) => (MOUSEEVENTF_XDOWN, u32::from(XBUTTON1)),
        (MouseButton::Back, false) => (MOUSEEVENTF_XUP, u32::from(XBUTTON1)),
        (MouseButton::Forward, true) => (MOUSEEVENTF_XDOWN, u32::from(XBUTTON2)),
        (MouseButton::Forward, false) => (MOUSEEVENTF_XUP, u32::from(XBUTTON2)),
    };
    mouse(0, 0, data, flags)
}

impl InputSink for WinSink {
    fn move_abs(&mut self, x: i32, y: i32) -> Result<()> {
        // SAFETY: a plain call.
        if unsafe { SetCursorPos(x, y) } == 0 {
            return Err(PlatformError::Other("Windows could not move the cursor".into()));
        }
        Ok(())
    }

    fn move_rel(&mut self, dx: i32, dy: i32) -> Result<()> {
        // A relative `SendInput` move goes through pointer acceleration, which would stretch the
        // other computer's movement; placing the cursor does not.
        let mut at = POINT { x: 0, y: 0 };
        // SAFETY: `at` is valid for writes.
        if unsafe { GetCursorPos(&mut at) } == 0 {
            return Err(PlatformError::Other("Windows could not read the cursor position".into()));
        }
        self.move_abs(at.x + dx, at.y + dy)
    }

    fn button(&mut self, button: MouseButton, state: ButtonState) -> Result<()> {
        send(button_input(button, state))?;
        match state {
            ButtonState::Down if !self.held_buttons.contains(&button) => self.held_buttons.push(button),
            ButtonState::Up => self.held_buttons.retain(|held| *held != button),
            ButtonState::Down => {}
        }
        Ok(())
    }

    fn wheel(&mut self, dx: i32, dy: i32) -> Result<()> {
        // Both axes are in 120ths of a notch, which is what Windows counts in too.
        if dy != 0 {
            send(mouse(0, 0, dy as u32, MOUSEEVENTF_WHEEL))?;
        }
        if dx != 0 {
            send(mouse(0, 0, dx as u32, MOUSEEVENTF_HWHEEL))?;
        }
        Ok(())
    }

    fn key(&mut self, usage: u16, state: ButtonState) -> Result<()> {
        let Some((scan, extended)) = winkeys::scan_from_hid(usage) else {
            return Err(PlatformError::Other(format!("no Windows key for HID usage {usage:#x}")));
        };
        send(keyboard(scan, extended, state == ButtonState::Up))?;
        match state {
            ButtonState::Down if !self.held_keys.contains(&usage) => self.held_keys.push(usage),
            ButtonState::Up => self.held_keys.retain(|held| *held != usage),
            ButtonState::Down => {}
        }
        Ok(())
    }

    fn release_all(&mut self) -> Result<()> {
        let mut first_error = None;
        for usage in std::mem::take(&mut self.held_keys) {
            if let Some((scan, extended)) = winkeys::scan_from_hid(usage) {
                if let Err(err) = send(keyboard(scan, extended, true)) {
                    first_error.get_or_insert(err);
                }
            }
        }
        for button in std::mem::take(&mut self.held_buttons) {
            if let Err(err) = send(button_input(button, ButtonState::Up)) {
                first_error.get_or_insert(err);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

impl Drop for WinSink {
    fn drop(&mut self) {
        let _ = self.release_all();
    }
}
