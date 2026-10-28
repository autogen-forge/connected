//! Does the cursor read back right after it was moved? (macOS experiment)
use std::thread::sleep;
use std::time::Duration;

fn main() {
    let mut sink = connected_platform::open_sink().unwrap();
    let screen = connected_platform::open_screen().unwrap();
    println!("start {:?}", screen.cursor());
    for (x, y) in [(100, 100), (700, 400), (200, 800)] {
        sink.move_abs(x, y).unwrap();
        let now = screen.cursor();
        sleep(Duration::from_millis(30));
        let later = screen.cursor();
        println!("asked ({x},{y}) immediately {now:?} after 30ms {later:?}");
    }
    // relative moves back to back, as a controlling computer sends them
    sink.move_abs(300, 300).unwrap();
    for _ in 0..10 {
        sink.move_rel(10, 0).unwrap();
    }
    sleep(Duration::from_millis(100));
    println!("after 10 x move_rel(10,0) from (300,300): {:?}", screen.cursor());
    for _ in 0..10 {
        sink.move_rel(0, -100).unwrap();
    }
    sleep(Duration::from_millis(100));
    println!("after 10 x move_rel(0,-100): {:?}", screen.cursor());
}
