//! macOS virtual keycodes <-> USB HID usages (keyboard page 0x07).
//!
//! Virtual keycodes name physical key positions, which is what HID usages do too, so the
//! mapping is a plain table. Keys with no HID counterpart (Fn) are left out.

pub const CAPS_LOCK: u16 = 0x39;

/// `(macOS keycode, HID usage)`
const TABLE: &[(u16, u16)] = &[
    (0x00, 0x04), // A
    (0x01, 0x16), // S
    (0x02, 0x07), // D
    (0x03, 0x09), // F
    (0x04, 0x0B), // H
    (0x05, 0x0A), // G
    (0x06, 0x1D), // Z
    (0x07, 0x1B), // X
    (0x08, 0x06), // C
    (0x09, 0x19), // V
    (0x0A, 0x64), // ISO section / non-US backslash
    (0x0B, 0x05), // B
    (0x0C, 0x14), // Q
    (0x0D, 0x1A), // W
    (0x0E, 0x08), // E
    (0x0F, 0x15), // R
    (0x10, 0x1C), // Y
    (0x11, 0x17), // T
    (0x12, 0x1E), // 1
    (0x13, 0x1F), // 2
    (0x14, 0x20), // 3
    (0x15, 0x21), // 4
    (0x16, 0x23), // 6
    (0x17, 0x22), // 5
    (0x18, 0x2E), // =
    (0x19, 0x26), // 9
    (0x1A, 0x24), // 7
    (0x1B, 0x2D), // -
    (0x1C, 0x25), // 8
    (0x1D, 0x27), // 0
    (0x1E, 0x30), // ]
    (0x1F, 0x12), // O
    (0x20, 0x18), // U
    (0x21, 0x2F), // [
    (0x22, 0x0C), // I
    (0x23, 0x13), // P
    (0x24, 0x28), // Return
    (0x25, 0x0F), // L
    (0x26, 0x0D), // J
    (0x27, 0x34), // '
    (0x28, 0x0E), // K
    (0x29, 0x33), // ;
    (0x2A, 0x31), // backslash
    (0x2B, 0x36), // ,
    (0x2C, 0x38), // /
    (0x2D, 0x11), // N
    (0x2E, 0x10), // M
    (0x2F, 0x37), // .
    (0x30, 0x2B), // Tab
    (0x31, 0x2C), // Space
    (0x32, 0x35), // `
    (0x33, 0x2A), // Backspace
    (0x35, 0x29), // Escape
    (0x36, 0xE7), // Right Cmd
    (0x37, 0xE3), // Left Cmd
    (0x38, 0xE1), // Left Shift
    (0x39, 0x39), // Caps Lock
    (0x3A, 0xE2), // Left Option
    (0x3B, 0xE0), // Left Control
    (0x3C, 0xE5), // Right Shift
    (0x3D, 0xE6), // Right Option
    (0x3E, 0xE4), // Right Control
    (0x40, 0x6C), // F17
    (0x41, 0x63), // Keypad .
    (0x43, 0x55), // Keypad *
    (0x45, 0x57), // Keypad +
    (0x47, 0x53), // Keypad Clear / Num Lock
    (0x48, 0x80), // Volume Up
    (0x49, 0x81), // Volume Down
    (0x4A, 0x7F), // Mute
    (0x4B, 0x54), // Keypad /
    (0x4C, 0x58), // Keypad Enter
    (0x4E, 0x56), // Keypad -
    (0x4F, 0x6D), // F18
    (0x50, 0x6E), // F19
    (0x51, 0x67), // Keypad =
    (0x52, 0x62), // Keypad 0
    (0x53, 0x59), // Keypad 1
    (0x54, 0x5A), // Keypad 2
    (0x55, 0x5B), // Keypad 3
    (0x56, 0x5C), // Keypad 4
    (0x57, 0x5D), // Keypad 5
    (0x58, 0x5E), // Keypad 6
    (0x59, 0x5F), // Keypad 7
    (0x5A, 0x6F), // F20
    (0x5B, 0x60), // Keypad 8
    (0x5C, 0x61), // Keypad 9
    (0x5D, 0x89), // JIS Yen (International3)
    (0x5E, 0x87), // JIS underscore (International1)
    (0x5F, 0x85), // JIS keypad comma
    (0x60, 0x3E), // F5
    (0x61, 0x3F), // F6
    (0x62, 0x40), // F7
    (0x63, 0x3C), // F3
    (0x64, 0x41), // F8
    (0x65, 0x42), // F9
    (0x66, 0x91), // JIS Eisu (Lang2)
    (0x67, 0x44), // F11
    (0x68, 0x90), // JIS Kana (Lang1)
    (0x69, 0x68), // F13
    (0x6A, 0x6B), // F16
    (0x6B, 0x69), // F14
    (0x6D, 0x43), // F10
    (0x6E, 0x65), // Application (context menu)
    (0x6F, 0x45), // F12
    (0x71, 0x6A), // F15
    (0x72, 0x49), // Help -> Insert
    (0x73, 0x4A), // Home
    (0x74, 0x4B), // Page Up
    (0x75, 0x4C), // Forward Delete
    (0x76, 0x3D), // F4
    (0x77, 0x4D), // End
    (0x78, 0x3B), // F2
    (0x79, 0x4E), // Page Down
    (0x7A, 0x3A), // F1
    (0x7B, 0x50), // Left
    (0x7C, 0x4F), // Right
    (0x7D, 0x51), // Down
    (0x7E, 0x52), // Up
];

pub fn hid_from_mac(code: u16) -> Option<u16> {
    TABLE.iter().find(|&&(mac, _)| mac == code).map(|&(_, hid)| hid)
}

pub fn mac_from_hid(usage: u16) -> Option<u16> {
    TABLE.iter().find(|&&(_, hid)| hid == usage).map(|&(mac, _)| mac)
}

/// One physical modifier key and the event-flag bits that reflect it.
///
/// A flags-changed event carries only the new flag state, so telling whether the key went
/// down or up means looking at its bit. The device bits (`NX_DEVICE*KEYMASK`) tell left
/// from right; the independent bit is the fallback when a device does not set them.
pub struct ModifierKey {
    pub mac: u16,
    pub hid: u16,
    pub device: u64,
    /// Device bits of both keys of this modifier.
    pub family: u64,
    pub independent: u64,
}

const CTRL: u64 = 0x0004_0000;
const SHIFT: u64 = 0x0002_0000;
const ALT: u64 = 0x0008_0000;
const CMD: u64 = 0x0010_0000;

pub const MODIFIERS: [ModifierKey; 8] = [
    ModifierKey { mac: 0x3B, hid: 0xE0, device: 0x0000_0001, family: 0x0000_2001, independent: CTRL },
    ModifierKey { mac: 0x38, hid: 0xE1, device: 0x0000_0002, family: 0x0000_0006, independent: SHIFT },
    ModifierKey { mac: 0x3A, hid: 0xE2, device: 0x0000_0020, family: 0x0000_0060, independent: ALT },
    ModifierKey { mac: 0x37, hid: 0xE3, device: 0x0000_0008, family: 0x0000_0018, independent: CMD },
    ModifierKey { mac: 0x3E, hid: 0xE4, device: 0x0000_2000, family: 0x0000_2001, independent: CTRL },
    ModifierKey { mac: 0x3C, hid: 0xE5, device: 0x0000_0004, family: 0x0000_0006, independent: SHIFT },
    ModifierKey { mac: 0x3D, hid: 0xE6, device: 0x0000_0040, family: 0x0000_0060, independent: ALT },
    ModifierKey { mac: 0x36, hid: 0xE7, device: 0x0000_0010, family: 0x0000_0018, independent: CMD },
];

pub fn modifier_by_mac(code: u16) -> Option<&'static ModifierKey> {
    MODIFIERS.iter().find(|m| m.mac == code)
}

pub fn modifier_by_hid(usage: u16) -> Option<&'static ModifierKey> {
    MODIFIERS.iter().find(|m| m.hid == usage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn table_has_no_duplicates() {
        let macs: HashSet<_> = TABLE.iter().map(|&(m, _)| m).collect();
        let hids: HashSet<_> = TABLE.iter().map(|&(_, h)| h).collect();
        assert_eq!(macs.len(), TABLE.len());
        assert_eq!(hids.len(), TABLE.len());
    }

    #[test]
    fn lookups_round_trip() {
        for &(mac, hid) in TABLE {
            assert_eq!(hid_from_mac(mac), Some(hid));
            assert_eq!(mac_from_hid(hid), Some(mac));
        }
    }

    #[test]
    fn known_keys() {
        assert_eq!(hid_from_mac(0x00), Some(0x04)); // A
        assert_eq!(hid_from_mac(0x24), Some(0x28)); // Return
        assert_eq!(hid_from_mac(0x7A), Some(0x3A)); // F1
        assert_eq!(hid_from_mac(0x7E), Some(0x52)); // Up
        assert_eq!(mac_from_hid(0x2C), Some(0x31)); // Space
        assert_eq!(hid_from_mac(0x3F), None); // Fn has no HID usage
    }

    #[test]
    fn letters_and_digits_are_complete() {
        // HID 0x04..=0x27 is a-z then 1-9,0.
        for usage in 0x04..=0x27u16 {
            assert!(mac_from_hid(usage).is_some(), "usage {usage:#x} unmapped");
        }
    }

    #[test]
    fn every_hid_modifier_is_covered_and_consistent() {
        for usage in 0xE0..=0xE7u16 {
            let m = modifier_by_hid(usage).expect("modifier row");
            assert_eq!(hid_from_mac(m.mac), Some(usage));
            assert_eq!(m.device & m.family, m.device);
        }
    }
}
