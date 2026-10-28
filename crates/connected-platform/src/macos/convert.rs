//! Turns CGEvents seen by the tap into [`InputEvent`]s.

use connected_core::InputEvent;
use connected_proto::{ButtonState, MouseButton};
use core_graphics::event::{CGEvent, CGEventType, EventField};

use super::keymap;

/// Wire unit for scrolling: 1/120 of a notch.
pub const WHEEL_UNITS_PER_LINE: f64 = 120.0;

pub fn convert(kind: CGEventType, event: &CGEvent) -> Vec<InputEvent> {
    use CGEventType::*;
    match kind {
        MouseMoved | LeftMouseDragged | RightMouseDragged | OtherMouseDragged => {
            let dx = event.get_integer_value_field(EventField::MOUSE_EVENT_DELTA_X) as i32;
            let dy = event.get_integer_value_field(EventField::MOUSE_EVENT_DELTA_Y) as i32;
            if dx == 0 && dy == 0 {
                Vec::new()
            } else {
                vec![InputEvent::MouseMove { dx, dy }]
            }
        }
        LeftMouseDown => vec![button(MouseButton::Left, ButtonState::Down)],
        LeftMouseUp => vec![button(MouseButton::Left, ButtonState::Up)],
        RightMouseDown => vec![button(MouseButton::Right, ButtonState::Down)],
        RightMouseUp => vec![button(MouseButton::Right, ButtonState::Up)],
        OtherMouseDown | OtherMouseUp => {
            let state = if matches!(kind, OtherMouseDown) {
                ButtonState::Down
            } else {
                ButtonState::Up
            };
            match event.get_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER) {
                2 => vec![button(MouseButton::Middle, state)],
                3 => vec![button(MouseButton::Back, state)],
                4 => vec![button(MouseButton::Forward, state)],
                _ => Vec::new(),
            }
        }
        ScrollWheel => {
            // The fixed-point fields carry fractional lines, so trackpad scrolling stays smooth.
            let lines = |field| event.get_double_value_field(field);
            let dy = (lines(EventField::SCROLL_WHEEL_EVENT_FIXED_POINT_DELTA_AXIS_1)
                * WHEEL_UNITS_PER_LINE)
                .round() as i32;
            let dx = (lines(EventField::SCROLL_WHEEL_EVENT_FIXED_POINT_DELTA_AXIS_2)
                * WHEEL_UNITS_PER_LINE)
                .round() as i32;
            if dx == 0 && dy == 0 {
                Vec::new()
            } else {
                vec![InputEvent::Wheel { dx, dy }]
            }
        }
        KeyDown | KeyUp => {
            let code = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
            let state = if matches!(kind, KeyDown) {
                ButtonState::Down
            } else {
                ButtonState::Up
            };
            match keymap::hid_from_mac(code) {
                Some(usage) => vec![InputEvent::Key { usage, state }],
                None => Vec::new(),
            }
        }
        FlagsChanged => modifier_change(event),
        _ => Vec::new(),
    }
}

fn button(button: MouseButton, state: ButtonState) -> InputEvent {
    InputEvent::MouseButton { button, state }
}

fn modifier_change(event: &CGEvent) -> Vec<InputEvent> {
    let code = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;

    // The system reports Caps Lock only when it toggles, never as separate down and up.
    if code == keymap::CAPS_LOCK {
        let usage = keymap::CAPS_LOCK;
        return vec![
            InputEvent::Key { usage, state: ButtonState::Down },
            InputEvent::Key { usage, state: ButtonState::Up },
        ];
    }

    let Some(key) = keymap::modifier_by_mac(code) else {
        return Vec::new();
    };
    let flags = event.get_flags().bits();
    let down = if flags & key.family != 0 {
        flags & key.device != 0
    } else {
        flags & key.independent != 0
    };
    let state = if down { ButtonState::Down } else { ButtonState::Up };
    vec![InputEvent::Key { usage: key.hid, state }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_graphics::event::{CGEventFlags, CGMouseButton, ScrollEventUnit};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use core_graphics::geometry::CGPoint;

    fn source() -> CGEventSource {
        CGEventSource::new(CGEventSourceStateID::Private).expect("event source")
    }

    fn mouse(kind: CGEventType) -> CGEvent {
        CGEvent::new_mouse_event(source(), kind, CGPoint::new(10.0, 10.0), CGMouseButton::Left)
            .expect("mouse event")
    }

    fn flags_changed(keycode: u16, raw_flags: u64) -> CGEvent {
        let event = CGEvent::new_keyboard_event(source(), keycode, true).expect("key event");
        event.set_type(CGEventType::FlagsChanged);
        event.set_flags(CGEventFlags::from_bits_retain(raw_flags));
        event
    }

    fn key(usage: u16, state: ButtonState) -> InputEvent {
        InputEvent::Key { usage, state }
    }

    #[test]
    fn mouse_motion_uses_the_delta_fields() {
        let event = mouse(CGEventType::MouseMoved);
        event.set_integer_value_field(EventField::MOUSE_EVENT_DELTA_X, 7);
        event.set_integer_value_field(EventField::MOUSE_EVENT_DELTA_Y, -3);
        assert_eq!(
            convert(CGEventType::MouseMoved, &event),
            vec![InputEvent::MouseMove { dx: 7, dy: -3 }]
        );
    }

    #[test]
    fn zero_motion_is_dropped() {
        let event = mouse(CGEventType::MouseMoved);
        assert!(convert(CGEventType::MouseMoved, &event).is_empty());
    }

    #[test]
    fn dragging_counts_as_motion() {
        let event = mouse(CGEventType::LeftMouseDragged);
        event.set_integer_value_field(EventField::MOUSE_EVENT_DELTA_X, 2);
        assert_eq!(
            convert(CGEventType::LeftMouseDragged, &event),
            vec![InputEvent::MouseMove { dx: 2, dy: 0 }]
        );
    }

    #[test]
    fn buttons_map_by_event_type_and_number() {
        let event = mouse(CGEventType::LeftMouseDown);
        assert_eq!(
            convert(CGEventType::LeftMouseDown, &event),
            vec![button(MouseButton::Left, ButtonState::Down)]
        );

        let other = mouse(CGEventType::OtherMouseUp);
        other.set_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER, 3);
        assert_eq!(
            convert(CGEventType::OtherMouseUp, &other),
            vec![button(MouseButton::Back, ButtonState::Up)]
        );

        other.set_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER, 9);
        assert!(convert(CGEventType::OtherMouseUp, &other).is_empty());
    }

    #[test]
    fn scroll_is_reported_in_120ths_of_a_notch() {
        let event = CGEvent::new_scroll_event(source(), ScrollEventUnit::LINE, 2, 3, -1, 0)
            .expect("scroll event");
        assert_eq!(
            convert(CGEventType::ScrollWheel, &event),
            vec![InputEvent::Wheel { dx: -120, dy: 360 }]
        );
    }

    #[test]
    fn keys_translate_to_hid_usages() {
        let down = CGEvent::new_keyboard_event(source(), 0x00, true).unwrap();
        assert_eq!(
            convert(CGEventType::KeyDown, &down),
            vec![key(0x04, ButtonState::Down)]
        );
        let up = CGEvent::new_keyboard_event(source(), 0x7B, false).unwrap();
        assert_eq!(
            convert(CGEventType::KeyUp, &up),
            vec![key(0x50, ButtonState::Up)]
        );
    }

    #[test]
    fn keys_without_a_hid_usage_are_ignored() {
        let fn_key = CGEvent::new_keyboard_event(source(), 0x3F, true).unwrap();
        assert!(convert(CGEventType::KeyDown, &fn_key).is_empty());
    }

    #[test]
    fn modifier_press_and_release_follow_the_device_bits() {
        const LSHIFT_DEVICE: u64 = 0x2;
        const RSHIFT_DEVICE: u64 = 0x4;
        const SHIFT_INDEPENDENT: u64 = 0x2_0000;

        let both = flags_changed(0x38, SHIFT_INDEPENDENT | LSHIFT_DEVICE | RSHIFT_DEVICE);
        assert_eq!(
            convert(CGEventType::FlagsChanged, &both),
            vec![key(0xE1, ButtonState::Down)]
        );

        // Left shift released while right shift is still held.
        let left_up = flags_changed(0x38, SHIFT_INDEPENDENT | RSHIFT_DEVICE);
        assert_eq!(
            convert(CGEventType::FlagsChanged, &left_up),
            vec![key(0xE1, ButtonState::Up)]
        );

        let none = flags_changed(0x3C, 0);
        assert_eq!(
            convert(CGEventType::FlagsChanged, &none),
            vec![key(0xE5, ButtonState::Up)]
        );
    }

    #[test]
    fn modifier_without_device_bits_falls_back_to_the_independent_flag() {
        let down = flags_changed(0x37, 0x10_0000);
        assert_eq!(
            convert(CGEventType::FlagsChanged, &down),
            vec![key(0xE3, ButtonState::Down)]
        );
    }

    #[test]
    fn caps_lock_toggle_becomes_a_tap() {
        let event = flags_changed(0x39, 0x1_0000);
        assert_eq!(
            convert(CGEventType::FlagsChanged, &event),
            vec![key(0x39, ButtonState::Down), key(0x39, ButtonState::Up)]
        );
    }
}
