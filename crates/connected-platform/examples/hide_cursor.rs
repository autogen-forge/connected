//! Tries to hide the mouse cursor from a command-line process (macOS only).
//!
//!     cargo run -q -p connected-platform --example hide_cursor [-- VARIANT]
//!
//! Hiding the cursor normally needs a foreground app. Three variants are tried one after
//! another (or only VARIANT, 1 to 4), each for a few seconds; watch the screen and note in
//! which of them the cursor disappears. The cursor is always shown again afterwards.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("this experiment only runs on macOS");
}

#[cfg(target_os = "macos")]
fn main() {
    lab::run();
}

#[cfg(target_os = "macos")]
mod lab {
    use std::thread::sleep;
    use std::time::Duration;

    use core_foundation::base::{CFTypeRef, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::string::{CFString, CFStringRef};
    use core_graphics::display::CGDisplay;

    const HOLD: Duration = Duration::from_secs(4);
    const GAP: Duration = Duration::from_secs(2);
    const TO_UI_ELEMENT_APP: u32 = 4;
    const MAX_SHOWS: u32 = 5;

    pub fn run() {
        let only: Option<u32> = std::env::args().nth(1).and_then(|arg| arg.parse().ok());
        let wanted = |n: u32| only.is_none_or(|only| only == n);

        println!(
            "Move the mouse around during each variant and watch whether the cursor vanishes.\n\
             Also press a letter key while it is hidden: if it still lands in the app you were \
             typing in, focus was kept.\n"
        );

        if wanted(1) {
            variant("1. plain hide", || {});
        }
        if wanted(2) {
            variant("2. hide with SetsCursorInBackground", allow_background_cursor_control);
        }
        if wanted(3) {
            variant("3. hide as a UI-element app, brought to the front", || {
                allow_background_cursor_control();
                become_ui_element_app(true);
            });
        }
        if wanted(4) {
            variant("4. hide as a UI-element app, left in the background", || {
                allow_background_cursor_control();
                become_ui_element_app(false);
            });
        }
    }

    fn variant(name: &str, prepare: impl FnOnce()) {
        println!("{name}");
        prepare();

        let display = CGDisplay::main();
        let hidden = display.hide_cursor();
        println!("   hide_cursor -> {hidden:?}, CGCursorIsVisible = {}", is_visible());

        sleep(HOLD);

        // Always undo, whatever happened above. Hides are counted, so a show may not be enough
        // when an earlier hide went unnoticed; repeat until the cursor is back.
        let mut shows = 0;
        loop {
            let _ = display.show_cursor();
            shows += 1;
            sleep(Duration::from_millis(150));
            if is_visible() || shows >= MAX_SHOWS {
                break;
            }
        }
        println!("   show_cursor x{shows} -> CGCursorIsVisible = {}\n", is_visible());
        sleep(GAP);
    }

    fn is_visible() -> bool {
        // SAFETY: argument-free query.
        unsafe { CGCursorIsVisible() != 0 }
    }

    fn allow_background_cursor_control() {
        let key = CFString::new("SetsCursorInBackground");
        // SAFETY: `key` and the boolean are valid CF objects for the call.
        let status = unsafe {
            let connection = CGSMainConnectionID();
            CGSSetConnectionProperty(
                connection,
                connection,
                key.as_concrete_TypeRef(),
                CFBoolean::true_value().as_CFTypeRef(),
            )
        };
        println!("   SetsCursorInBackground -> {status}");
    }

    #[repr(C)]
    struct ProcessSerialNumber {
        high: u32,
        low: u32,
    }

    fn become_ui_element_app(bring_to_front: bool) {
        let mut psn = ProcessSerialNumber { high: 0, low: 0 };
        // SAFETY: `psn` is a valid out-parameter; these Process Manager calls take no other
        // pointers.
        let status = unsafe {
            GetCurrentProcess(&mut psn);
            let status = TransformProcessType(&psn, TO_UI_ELEMENT_APP);
            if bring_to_front {
                SetFrontProcess(&psn);
            }
            status
        };
        println!("   TransformProcessType -> {status}");
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGCursorIsVisible() -> u32;
        fn CGSMainConnectionID() -> u32;
        fn CGSSetConnectionProperty(
            connection: u32,
            target: u32,
            key: CFStringRef,
            value: CFTypeRef,
        ) -> i32;
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn GetCurrentProcess(psn: *mut ProcessSerialNumber) -> i32;
        fn TransformProcessType(psn: *const ProcessSerialNumber, transform: u32) -> i32;
        fn SetFrontProcess(psn: *const ProcessSerialNumber) -> i32;
    }
}
