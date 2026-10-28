//! Input injection through `CGEventPost`.

use std::time::{Duration, Instant};

use connected_core::platform::{PlatformError, Result};
use connected_core::{InputSink, ScreenInfo};
use connected_proto::{ButtonState, MouseButton, Rect};
use core_graphics::display::CGDisplay;
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGMouseButton, EventField,
    ScrollEventUnit,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;

use super::convert::WHEEL_UNITS_PER_LINE;
use super::{SYNTHETIC_TAG, keymap};

/// Two clicks closer than this in time and space form a double click.
const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(500);
const DOUBLE_CLICK_DISTANCE: f64 = 5.0;

struct ButtonSpec {
    down: CGEventType,
    up: CGEventType,
    dragged: CGEventType,
    cg: CGMouseButton,
    number: i64,
}

fn spec(button: MouseButton) -> ButtonSpec {
    use CGEventType::*;
    let (down, up, dragged, cg, number) = match button {
        MouseButton::Left => (LeftMouseDown, LeftMouseUp, LeftMouseDragged, CGMouseButton::Left, 0),
        MouseButton::Right => {
            (RightMouseDown, RightMouseUp, RightMouseDragged, CGMouseButton::Right, 1)
        }
        MouseButton::Middle => {
            (OtherMouseDown, OtherMouseUp, OtherMouseDragged, CGMouseButton::Center, 2)
        }
        MouseButton::Back => {
            (OtherMouseDown, OtherMouseUp, OtherMouseDragged, CGMouseButton::Center, 3)
        }
        MouseButton::Forward => {
            (OtherMouseDown, OtherMouseUp, OtherMouseDragged, CGMouseButton::Center, 4)
        }
    };
    ButtonSpec { down, up, dragged, cg, number }
}

struct Click {
    button: MouseButton,
    at: Instant,
    pos: CGPoint,
    count: i64,
}

pub struct MacSink {
    source: CGEventSource,
    held_keys: Vec<u16>,
    held_buttons: Vec<MouseButton>,
    last_click: Option<Click>,
    /// Scroll remainder (x, y) in wire units that has not yet added up to a whole line.
    scroll_remainder: (i32, i32),
}

// SAFETY: the event source is only used from whichever thread owns the sink, and CoreGraphics
// event sources may be used from any thread.
unsafe impl Send for MacSink {}

impl MacSink {
    pub fn open() -> Result<Self> {
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|()| failure("create event source"))?;
        Ok(Self {
            source,
            held_keys: Vec::new(),
            held_buttons: Vec::new(),
            last_click: None,
            scroll_remainder: (0, 0),
        })
    }

    fn post(&self, event: &CGEvent) {
        // Lets our own event tap recognize and ignore this event.
        event.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, SYNTHETIC_TAG);
        event.post(CGEventTapLocation::HID);
    }

    fn cursor(&self) -> Result<CGPoint> {
        CGEvent::new(self.source.clone())
            .map(|event| event.location())
            .map_err(|()| failure("read the cursor position"))
    }

    /// Modifier flags matching the keys this sink currently holds down.
    fn flags(&self) -> CGEventFlags {
        let bits = self
            .held_keys
            .iter()
            .filter_map(|&usage| keymap::modifier_by_hid(usage))
            .fold(0, |bits, key| bits | key.independent);
        CGEventFlags::from_bits_retain(bits)
    }

    fn post_motion(&self, from: CGPoint, to: CGPoint) -> Result<()> {
        let (kind, button, number) = match self.held_buttons.first() {
            Some(&held) => {
                let spec = spec(held);
                (spec.dragged, spec.cg, Some(spec.number))
            }
            None => (CGEventType::MouseMoved, CGMouseButton::Left, None),
        };
        let event = CGEvent::new_mouse_event(self.source.clone(), kind, to, button)
            .map_err(|()| failure("create a mouse event"))?;
        event.set_integer_value_field(EventField::MOUSE_EVENT_DELTA_X, (to.x - from.x).round() as i64);
        event.set_integer_value_field(EventField::MOUSE_EVENT_DELTA_Y, (to.y - from.y).round() as i64);
        if let Some(number) = number {
            event.set_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER, number);
        }
        event.set_flags(self.flags());
        self.post(&event);
        Ok(())
    }

    fn click_count(&mut self, button: MouseButton, pos: CGPoint, state: ButtonState) -> i64 {
        match state {
            ButtonState::Down => {
                let now = Instant::now();
                let count = match &self.last_click {
                    Some(prev)
                        if prev.button == button
                            && now.duration_since(prev.at) < DOUBLE_CLICK_INTERVAL
                            && (prev.pos.x - pos.x).abs() <= DOUBLE_CLICK_DISTANCE
                            && (prev.pos.y - pos.y).abs() <= DOUBLE_CLICK_DISTANCE =>
                    {
                        prev.count + 1
                    }
                    _ => 1,
                };
                self.last_click = Some(Click { button, at: now, pos, count });
                count
            }
            ButtonState::Up => match &self.last_click {
                Some(prev) if prev.button == button => prev.count,
                _ => 1,
            },
        }
    }
}

impl InputSink for MacSink {
    fn move_abs(&mut self, x: i32, y: i32) -> Result<()> {
        let from = self.cursor()?;
        self.post_motion(from, CGPoint::new(f64::from(x), f64::from(y)))
    }

    fn move_rel(&mut self, dx: i32, dy: i32) -> Result<()> {
        let from = self.cursor()?;
        self.post_motion(from, CGPoint::new(from.x + f64::from(dx), from.y + f64::from(dy)))
    }

    fn button(&mut self, button: MouseButton, state: ButtonState) -> Result<()> {
        let pos = self.cursor()?;
        let spec = spec(button);
        let kind = match state {
            ButtonState::Down => spec.down,
            ButtonState::Up => spec.up,
        };
        let count = self.click_count(button, pos, state);

        let event = CGEvent::new_mouse_event(self.source.clone(), kind, pos, spec.cg)
            .map_err(|()| failure("create a mouse button event"))?;
        event.set_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER, spec.number);
        event.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, count);
        event.set_flags(self.flags());
        self.post(&event);

        match state {
            ButtonState::Down => {
                if !self.held_buttons.contains(&button) {
                    self.held_buttons.push(button);
                }
            }
            ButtonState::Up => self.held_buttons.retain(|&held| held != button),
        }
        Ok(())
    }

    fn wheel(&mut self, dx: i32, dy: i32) -> Result<()> {
        let unit = WHEEL_UNITS_PER_LINE as i32;
        self.scroll_remainder.0 += dx;
        self.scroll_remainder.1 += dy;
        let lines_x = self.scroll_remainder.0 / unit;
        let lines_y = self.scroll_remainder.1 / unit;
        self.scroll_remainder.0 -= lines_x * unit;
        self.scroll_remainder.1 -= lines_y * unit;
        if lines_x == 0 && lines_y == 0 {
            return Ok(());
        }

        let event = CGEvent::new_scroll_event(
            self.source.clone(),
            ScrollEventUnit::LINE,
            2,
            lines_y,
            lines_x,
            0,
        )
        .map_err(|()| failure("create a scroll event"))?;
        event.set_flags(self.flags());
        self.post(&event);
        Ok(())
    }

    fn key(&mut self, usage: u16, state: ButtonState) -> Result<()> {
        let code = keymap::mac_from_hid(usage).ok_or_else(|| {
            PlatformError::Other(format!("HID usage {usage:#04x} has no macOS keycode"))
        })?;
        let down = state == ButtonState::Down;
        let repeat = down && self.held_keys.contains(&usage);

        if down {
            if !repeat {
                self.held_keys.push(usage);
            }
        } else {
            self.held_keys.retain(|&held| held != usage);
        }

        let event = CGEvent::new_keyboard_event(self.source.clone(), code, down)
            .map_err(|()| failure("create a keyboard event"))?;
        if keymap::modifier_by_hid(usage).is_some() {
            event.set_type(CGEventType::FlagsChanged);
        }
        if repeat {
            event.set_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT, 1);
        }
        event.set_flags(self.flags());
        self.post(&event);
        Ok(())
    }

    fn release_all(&mut self) -> Result<()> {
        let mut first_error = None;
        for usage in std::mem::take(&mut self.held_keys) {
            if let Err(err) = self.key(usage, ButtonState::Up) {
                first_error.get_or_insert(err);
            }
        }
        for button in std::mem::take(&mut self.held_buttons) {
            if let Err(err) = self.button(button, ButtonState::Up) {
                first_error.get_or_insert(err);
            }
        }
        self.scroll_remainder = (0, 0);
        first_error.map_or(Ok(()), Err)
    }
}

impl Drop for MacSink {
    fn drop(&mut self) {
        let _ = self.release_all();
    }
}

/// Displays and cursor, in points.
pub struct MacScreen {
    source: CGEventSource,
}

impl MacScreen {
    pub fn open() -> Result<Self> {
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|()| failure("create event source"))?;
        Ok(Self { source })
    }
}

impl ScreenInfo for MacScreen {
    fn size(&self) -> Result<(i32, i32)> {
        let size = CGDisplay::main().bounds().size;
        Ok((size.width.round() as i32, size.height.round() as i32))
    }

    fn displays(&self) -> Result<Vec<Rect>> {
        Ok(ordered_displays()?.into_iter().map(|(rect, _)| rect).collect())
    }

    fn scales(&self) -> Result<Vec<u16>> {
        Ok(ordered_displays()?.into_iter().map(|(_, scale)| scale).collect())
    }

    fn cursor(&self) -> Result<(i32, i32)> {
        let event = CGEvent::new(self.source.clone()).map_err(|()| failure("read the cursor position"))?;
        let pos = event.location();
        Ok((pos.x.round() as i32, pos.y.round() as i32))
    }
}

/// Every display and how much it is magnified, in percent, the main display first and the
/// rest in the order the system lists them. A display is magnified when it has more pixels
/// than points, as a Retina display does.
fn ordered_displays() -> Result<Vec<(Rect, u16)>> {
    let main = CGDisplay::main().id;
    let ids = CGDisplay::active_displays().map_err(|_| failure("list the displays"))?;
    let mut displays: Vec<(bool, Rect, u16)> = ids
        .into_iter()
        .map(|id| {
            let display = CGDisplay::new(id);
            let bounds = display.bounds();
            let rect = Rect::new(
                bounds.origin.x.round() as i32,
                bounds.origin.y.round() as i32,
                bounds.size.width.round() as i32,
                bounds.size.height.round() as i32,
            );
            let scale = if bounds.size.width > 0.0 {
                (display.pixels_wide() as f64 / bounds.size.width * 100.0).round() as u16
            } else {
                100
            };
            (id != main, rect, scale.max(1))
        })
        .collect();
    displays.sort_by_key(|(not_main, _, _)| *not_main);
    Ok(displays.into_iter().map(|(_, rect, scale)| (rect, scale)).collect())
}

fn failure(what: &str) -> PlatformError {
    PlatformError::Other(format!("CoreGraphics could not {what}"))
}
