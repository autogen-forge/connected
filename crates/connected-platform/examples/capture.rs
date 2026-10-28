//! Prints local input events as the backend sees them.
//!
//!     cargo run -p connected-platform --example capture -- [seconds] [--grab]
//!
//! With `--grab`, input is also withheld from other apps until Esc is pressed or the time
//! runs out, which is what the server does while the cursor is on another machine.
//! `scripts/grab-check.sh` runs this and checks that the cursor really stayed put.

use std::process::exit;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use connected_core::InputEvent;
use connected_proto::ButtonState;

const HID_ESCAPE: u16 = 0x29;

fn main() {
    let mut seconds = 10u64;
    let mut grab = false;
    for arg in std::env::args().skip(1) {
        if arg == "--grab" {
            grab = true;
        } else if let Ok(value) = arg.parse() {
            seconds = value;
        } else {
            eprintln!("usage: capture [seconds] [--grab]");
            exit(2);
        }
    }

    let mut source = match connected_platform::open_source() {
        Ok(source) => source,
        Err(err) => {
            eprintln!("cannot open input source: {err}");
            exit(1);
        }
    };

    if grab {
        println!(
            "grabbing input at {} ms; press Esc to stop (stops by itself after {seconds}s)",
            now_ms()
        );
        if let Err(err) = source.set_capture(true) {
            eprintln!("cannot start capture: {err}");
            exit(1);
        }
    } else {
        println!("listening for {seconds}s");
    }

    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline {
        match source.next_event_timeout(Duration::from_millis(200)) {
            Ok(Some(event)) => {
                println!("{event:?}");
                let escape = matches!(
                    event,
                    InputEvent::Key { usage: HID_ESCAPE, state: ButtonState::Down }
                );
                if grab && escape {
                    break;
                }
            }
            Ok(None) => {}
            Err(err) => {
                eprintln!("source stopped: {err}");
                break;
            }
        }
    }

    if grab {
        if let Err(err) = source.set_capture(false) {
            eprintln!("cannot stop capture: {err}");
        }
        println!("released at {} ms", now_ms());
    }
    println!("done");
}

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis())
}
