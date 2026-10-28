//! Wire protocol shared by server and client.

pub mod codec;
pub mod geometry;
pub mod message;

pub use codec::{CodecError, MAX_FRAME_LEN};
pub use geometry::{Rect, bounds};
pub use message::{
    ButtonState, ClipFormat, ClipOffer, Edge, LayoutScreen, Message, Modifiers, MouseButton, Os,
};

/// Bumped whenever a message change breaks compatibility.
pub const PROTOCOL_VERSION: u16 = 1;
