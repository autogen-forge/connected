//! Platform-independent logic: screen layout, the server-side router, and the traits
//! every OS backend implements.

pub mod arrangement;
pub mod desktop;
pub mod platform;
pub mod router;

pub use arrangement::{
    Arrangement, DEFAULT_SIZE, Metrics, PlaceError, Point, Spot, default_displays, normalize,
};
pub use connected_proto::Os;
pub use desktop::{Desktop, Landing, Rect, Step};
pub use platform::{
    Clipboard, InputEvent, InputSink, InputSource, Permission, PlatformError, ScreenInfo,
};
pub use router::{Action, LayoutError, Router};
