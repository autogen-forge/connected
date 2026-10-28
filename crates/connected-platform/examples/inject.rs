//! Moves the cursor around a small square and checks that it ended up where it started.
//!
//!     cargo run -p connected-platform --example inject [-- --type]
//!
//! With `--type`, it also types "hello" into whatever app has keyboard focus.

use std::process::exit;
use std::thread::sleep;
use std::time::Duration;

use connected_proto::ButtonState;

// HID usages for h, e, l, l, o.
const HELLO: [u16; 5] = [0x0B, 0x08, 0x0F, 0x0F, 0x12];

fn main() {
    let type_text = std::env::args().skip(1).any(|arg| arg == "--type");

    let (mut sink, screen) = match (connected_platform::open_sink(), connected_platform::open_screen()) {
        (Ok(sink), Ok(screen)) => (sink, screen),
        (Err(err), _) | (_, Err(err)) => {
            eprintln!("cannot open backend: {err}");
            exit(1);
        }
    };

    let start = screen.cursor().expect("cursor position");
    let mut step = |dx: i32, dy: i32| {
        for _ in 0..10 {
            sink.move_rel(dx / 10, dy / 10).expect("move cursor");
            sleep(Duration::from_millis(15));
        }
    };
    for (dx, dy) in [(40, 0), (0, 40), (-40, 0), (0, -40)] {
        step(dx, dy);
    }
    sleep(Duration::from_millis(100));
    let end = screen.cursor().expect("cursor position");
    println!("cursor {start:?} -> {end:?}");
    if start == end {
        println!("back where it started (if it never visibly moved, injection is blocked)");
    } else {
        println!("cursor drifted by ({}, {})", end.0 - start.0, end.1 - start.1);
    }

    if type_text {
        for usage in HELLO {
            sink.key(usage, ButtonState::Down).expect("key down");
            sink.key(usage, ButtonState::Up).expect("key up");
            sleep(Duration::from_millis(30));
        }
        println!("typed hello");
    }
}
