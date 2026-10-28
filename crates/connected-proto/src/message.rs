use serde::{Deserialize, Serialize};

use crate::geometry::Rect;

/// Side of a screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    pub fn opposite(self) -> Edge {
        match self {
            Edge::Left => Edge::Right,
            Edge::Right => Edge::Left,
            Edge::Top => Edge::Bottom,
            Edge::Bottom => Edge::Top,
        }
    }
}

/// The operating system a computer runs, for showing which is which.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Mac,
    Windows,
    Linux,
    #[default]
    Other,
}

impl Os {
    /// The system this program was built for.
    pub fn current() -> Os {
        if cfg!(target_os = "macos") {
            Os::Mac
        } else if cfg!(target_os = "windows") {
            Os::Windows
        } else if cfg!(target_os = "linux") {
            Os::Linux
        } else {
            Os::Other
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ButtonState {
    Down,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

/// Held modifier keys, laid out like the USB HID modifier byte:
/// bit `n` is HID usage `0xE0 + n` (LCtrl, LShift, LAlt, LGui, RCtrl, RShift, RAlt, RGui).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Modifiers(pub u8);

impl Modifiers {
    const FIRST_USAGE: u16 = 0xE0;
    const LAST_USAGE: u16 = 0xE7;

    /// Records a key transition. Returns false if `usage` is not a modifier key.
    pub fn set_usage(&mut self, usage: u16, down: bool) -> bool {
        if !(Self::FIRST_USAGE..=Self::LAST_USAGE).contains(&usage) {
            return false;
        }
        let bit = 1u8 << (usage - Self::FIRST_USAGE);
        if down {
            self.0 |= bit;
        } else {
            self.0 &= !bit;
        }
        true
    }

    pub fn ctrl(self) -> bool {
        self.0 & 0x11 != 0
    }

    pub fn shift(self) -> bool {
        self.0 & 0x22 != 0
    }

    pub fn alt(self) -> bool {
        self.0 & 0x44 != 0
    }

    /// Cmd on macOS, Win on Windows, Super on Linux.
    pub fn meta(self) -> bool {
        self.0 & 0x88 != 0
    }
}

/// Clipboard payload kinds. Every payload is normalized so platforms can exchange it:
/// text is UTF-8, images are PNG, files are a newline-separated URI list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClipFormat {
    Text,
    Png,
    FileList,
}

/// One available format of a clipboard entry, with its size in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipOffer {
    pub format: ClipFormat,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Message {
    Hello {
        version: u16,
        name: String,
    },

    /// The cursor entered this screen at (`x`, `y`); input follows until `Leave`.
    Enter {
        x: i32,
        y: i32,
        seq: u32,
        modifiers: Modifiers,
    },
    Leave,

    MouseMove {
        dx: i32,
        dy: i32,
    },
    MouseButton {
        button: MouseButton,
        state: ButtonState,
    },
    /// Scroll amount in 1/120 of a wheel notch (the Windows `WHEEL_DELTA` unit).
    /// Positive `dy` scrolls up (wheel turned away from the user), positive `dx` scrolls right.
    Wheel {
        dx: i32,
        dy: i32,
    },
    /// `usage` is a HID usage on the keyboard page (0x07).
    Key {
        usage: u16,
        state: ButtonState,
    },

    /// The sender's clipboard changed. Data is pulled with `ClipRequest`.
    ClipAnnounce {
        id: u64,
        offers: Vec<ClipOffer>,
    },
    ClipRequest {
        id: u64,
        format: ClipFormat,
    },
    ClipData {
        id: u64,
        format: ClipFormat,
        offset: u64,
        data: Vec<u8>,
        last: bool,
    },

    Ping(u64),
    Pong(u64),

    /// Sent once during pairing: whether this side's user confirmed that both screens show
    /// the same code. Pairing succeeds only if both sides accept.
    PairDecision {
        accepted: bool,
    },

    /// A client's displays and system, sent right after `Hello`. The server needs them to know where
    /// the cursor can cross over and where it lands. The positions are relative to the top-left
    /// corner of the smallest rectangle holding all of them, so that corner is (0, 0).
    Screen {
        displays: Vec<Rect>,
        /// How much each display is magnified, in percent (100 is none, 200 is a Retina
        /// display), in the same order as `displays`.
        scales: Vec<u16>,
        os: Os,
    },
}
