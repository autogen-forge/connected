//! Windows keyboard scan codes and USB HID usages, for translating between them.
//!
//! A scan code says which physical key was pressed, whatever the layout, which is what the wire
//! carries (HID usages name physical keys too). Windows spreads them over two sets: plain scan
//! codes, and ones sent with an "extended" flag (the arrow keys, the right-hand Ctrl and Alt,
//! the Windows keys, and so on).
//!
//! This table is plain data, so it is built on every system, and its tests run everywhere.

/// `(HID usage, scan code, extended)`.
const TABLE: &[(u16, u16, bool)] = &[
    // Letters.
    (0x04, 0x1E, false), // A
    (0x05, 0x30, false), // B
    (0x06, 0x2E, false), // C
    (0x07, 0x20, false), // D
    (0x08, 0x12, false), // E
    (0x09, 0x21, false), // F
    (0x0A, 0x22, false), // G
    (0x0B, 0x23, false), // H
    (0x0C, 0x17, false), // I
    (0x0D, 0x24, false), // J
    (0x0E, 0x25, false), // K
    (0x0F, 0x26, false), // L
    (0x10, 0x32, false), // M
    (0x11, 0x31, false), // N
    (0x12, 0x18, false), // O
    (0x13, 0x19, false), // P
    (0x14, 0x10, false), // Q
    (0x15, 0x13, false), // R
    (0x16, 0x1F, false), // S
    (0x17, 0x14, false), // T
    (0x18, 0x16, false), // U
    (0x19, 0x2F, false), // V
    (0x1A, 0x11, false), // W
    (0x1B, 0x2D, false), // X
    (0x1C, 0x15, false), // Y
    (0x1D, 0x2C, false), // Z
    // Digits, 1 to 9 and then 0.
    (0x1E, 0x02, false),
    (0x1F, 0x03, false),
    (0x20, 0x04, false),
    (0x21, 0x05, false),
    (0x22, 0x06, false),
    (0x23, 0x07, false),
    (0x24, 0x08, false),
    (0x25, 0x09, false),
    (0x26, 0x0A, false),
    (0x27, 0x0B, false),
    // Editing and punctuation.
    (0x28, 0x1C, false), // Enter
    (0x29, 0x01, false), // Escape
    (0x2A, 0x0E, false), // Backspace
    (0x2B, 0x0F, false), // Tab
    (0x2C, 0x39, false), // Space
    (0x2D, 0x0C, false), // - _
    (0x2E, 0x0D, false), // = +
    (0x2F, 0x1A, false), // [ {
    (0x30, 0x1B, false), // ] }
    (0x31, 0x2B, false), // \ |
    (0x33, 0x27, false), // ; :
    (0x34, 0x28, false), // ' "
    (0x35, 0x29, false), // ` ~
    (0x36, 0x33, false), // , <
    (0x37, 0x34, false), // . >
    (0x38, 0x35, false), // / ?
    (0x39, 0x3A, false), // Caps Lock
    // Function keys.
    (0x3A, 0x3B, false), // F1
    (0x3B, 0x3C, false),
    (0x3C, 0x3D, false),
    (0x3D, 0x3E, false),
    (0x3E, 0x3F, false),
    (0x3F, 0x40, false),
    (0x40, 0x41, false),
    (0x41, 0x42, false),
    (0x42, 0x43, false),
    (0x43, 0x44, false), // F10
    (0x44, 0x57, false), // F11
    (0x45, 0x58, false), // F12
    // Above the arrow keys.
    (0x46, 0x37, true),  // Print Screen
    (0x47, 0x46, false), // Scroll Lock
    (0x49, 0x52, true),  // Insert
    (0x4A, 0x47, true),  // Home
    (0x4B, 0x49, true),  // Page Up
    (0x4C, 0x53, true),  // Delete
    (0x4D, 0x4F, true),  // End
    (0x4E, 0x51, true),  // Page Down
    (0x4F, 0x4D, true),  // Right
    (0x50, 0x4B, true),  // Left
    (0x51, 0x50, true),  // Down
    (0x52, 0x48, true),  // Up
    // The number pad.
    (0x53, 0x45, false), // Num Lock
    (0x54, 0x35, true),  // /
    (0x55, 0x37, false), // *
    (0x56, 0x4A, false), // -
    (0x57, 0x4E, false), // +
    (0x58, 0x1C, true),  // Enter
    (0x59, 0x4F, false), // 1
    (0x5A, 0x50, false), // 2
    (0x5B, 0x51, false), // 3
    (0x5C, 0x4B, false), // 4
    (0x5D, 0x4C, false), // 5
    (0x5E, 0x4D, false), // 6
    (0x5F, 0x47, false), // 7
    (0x60, 0x48, false), // 8
    (0x61, 0x49, false), // 9
    (0x62, 0x52, false), // 0
    (0x63, 0x53, false), // .
    // Others.
    (0x64, 0x56, false), // the extra key next to a short left Shift (< > on many layouts)
    (0x65, 0x5D, true),  // Menu
    // International and Korean keys.
    (0x87, 0x73, false), // ろ / the extra key left of the right Shift
    (0x88, 0x70, false), // Katakana / Hiragana
    (0x89, 0x7D, false), // Yen
    (0x8A, 0x79, false), // Convert
    (0x8B, 0x7B, false), // Non-convert
    (0x90, 0xF2, false), // Hangul / English
    (0x91, 0xF1, false), // Hanja
    // Modifiers.
    (0xE0, 0x1D, false), // Left Ctrl
    (0xE1, 0x2A, false), // Left Shift
    (0xE2, 0x38, false), // Left Alt
    (0xE3, 0x5B, true),  // Left Windows
    (0xE4, 0x1D, true),  // Right Ctrl
    (0xE5, 0x36, false), // Right Shift
    (0xE6, 0x38, true),  // Right Alt
    (0xE7, 0x5C, true),  // Right Windows
];

/// The scan code and whether it is an extended one, for a HID usage.
pub fn scan_from_hid(usage: u16) -> Option<(u16, bool)> {
    TABLE.iter().find(|(hid, _, _)| *hid == usage).map(|(_, scan, extended)| (*scan, *extended))
}

/// The HID usage for a scan code, as Windows reports it.
pub fn hid_from_scan(scan: u16, extended: bool) -> Option<u16> {
    TABLE
        .iter()
        .find(|(_, code, ext)| *code == scan && *ext == extended)
        .map(|(hid, _, _)| *hid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_goes_there_and_back() {
        for (hid, scan, extended) in TABLE {
            assert_eq!(scan_from_hid(*hid), Some((*scan, *extended)), "usage {hid:#x}");
            assert_eq!(hid_from_scan(*scan, *extended), Some(*hid), "scan {scan:#x}");
        }
    }

    #[test]
    fn no_usage_and_no_scan_code_appears_twice() {
        for (index, (hid, scan, extended)) in TABLE.iter().enumerate() {
            for (other_hid, other_scan, other_extended) in &TABLE[index + 1..] {
                assert_ne!(hid, other_hid, "usage {hid:#x} twice");
                assert!(
                    !(scan == other_scan && extended == other_extended),
                    "scan code {scan:#x} (extended: {extended}) twice"
                );
            }
        }
    }

    #[test]
    fn the_keys_people_press_most_are_there() {
        assert_eq!(scan_from_hid(0x04), Some((0x1E, false))); // A
        assert_eq!(scan_from_hid(0x28), Some((0x1C, false))); // Enter
        assert_eq!(scan_from_hid(0x58), Some((0x1C, true))); // keypad Enter
        assert_eq!(scan_from_hid(0x4F), Some((0x4D, true))); // Right arrow
        assert_eq!(scan_from_hid(0xE4), Some((0x1D, true))); // Right Ctrl
        assert_eq!(hid_from_scan(0x1D, false), Some(0xE0)); // Left Ctrl
        assert_eq!(hid_from_scan(0x38, true), Some(0xE6)); // Right Alt
    }

    #[test]
    fn unknown_keys_are_not_guessed() {
        assert_eq!(scan_from_hid(0xFFFF), None);
        assert_eq!(hid_from_scan(0x7F, false), None);
    }

    #[test]
    fn the_escape_chord_keys_can_be_sent() {
        // Ctrl, Shift, Alt and Escape, on both sides where there are two.
        for usage in [0xE0, 0xE1, 0xE2, 0xE4, 0xE5, 0xE6, 0x29] {
            assert!(scan_from_hid(usage).is_some(), "usage {usage:#x}");
        }
    }
}
