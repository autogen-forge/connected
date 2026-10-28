//! Logs every change of the cursor position, with a wall-clock timestamp in milliseconds.
//!
//!     cargo run -p connected-platform --example watch_cursor -- [seconds]
//!
//! Run it in a second process while another example grabs input: it observes the cursor
//! from outside, unaffected by anything the grabbing process does to its own state.

use std::process::exit;
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn main() {
    let seconds: u64 = std::env::args().nth(1).and_then(|arg| arg.parse().ok()).unwrap_or(30);

    let screen = match connected_platform::open_screen() {
        Ok(screen) => screen,
        Err(err) => {
            eprintln!("cannot open screen info: {err}");
            exit(1);
        }
    };

    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut last = None;
    while Instant::now() < deadline {
        if let Ok(pos) = screen.cursor() {
            if last != Some(pos) {
                let ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
                println!("{ms} {} {}", pos.0, pos.1);
                last = Some(pos);
            }
        }
        sleep(Duration::from_millis(10));
    }
}
