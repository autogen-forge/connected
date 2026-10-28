//! `connected test`: a guided check of what this computer's input backend can do, for finding out
//! why sharing does not work here. It reads the mouse, pushes against a screen edge, captures
//! input, and injects it, then writes everything to `connected-test.txt` to be sent back.

use std::fmt::Write as _;
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use connected_core::{InputEvent, InputSink, InputSource, ScreenInfo};

const REPORT_FILE: &str = "connected-test.txt";
const READ_FOR: Duration = Duration::from_secs(6);
const POLL: Duration = Duration::from_millis(20);

/// Everything said is printed at once and kept for the file.
struct Report {
    text: String,
}

impl Report {
    fn say(&mut self, line: impl AsRef<str>) {
        let line = line.as_ref();
        println!("{line}");
        let _ = writeln!(self.text, "{line}");
    }

    fn verdict(&mut self, ok: bool, line: impl AsRef<str>) {
        let mark = if ok { "[ok]  " } else { "[FAIL]" };
        self.say(format!("{mark} {}", line.as_ref()));
    }

    fn info(&mut self, line: impl AsRef<str>) {
        self.say(format!("[info] {}", line.as_ref()));
    }
}

pub fn run() -> Result<()> {
    let mut report = Report { text: String::new() };
    report.say(format!("connected {} test, backend: {}", env!("CARGO_PKG_VERSION"), connected_platform::BACKEND));
    report.say("");

    let outcome = steps(&mut report);
    if let Err(err) = &outcome {
        report.verdict(false, format!("stopped: {err:#}"));
    }

    match std::fs::write(REPORT_FILE, &report.text) {
        Ok(()) => {
            let place = std::env::current_dir().map(|dir| dir.join(REPORT_FILE)).unwrap_or_else(|_| Path::new(REPORT_FILE).into());
            println!("\nThe result is saved in {}. Send it, or copy what is above.", place.display());
        }
        Err(err) => println!("\nCould not save {REPORT_FILE}: {err}. Copy what is above."),
    }
    print!("Press Enter to close. ");
    let _ = io::stdout().flush();
    let _ = io::stdin().lock().lines().next();
    outcome
}

fn steps(report: &mut Report) -> Result<()> {
    let screen = connected_platform::open_screen().context("cannot read the screens")?;
    screens(report, screen.as_ref());

    let mut source = connected_platform::open_source().context("cannot open the input source")?;
    report.verdict(true, "the input source opened");
    let mut sink = connected_platform::open_sink().context("cannot open the input sink")?;
    report.verdict(true, "the input sink opened");

    reading(report, source.as_mut(), screen.as_ref());
    edge(report, source.as_mut(), screen.as_ref());
    capture(report, source.as_mut(), screen.as_ref());
    injecting(report, sink.as_mut(), screen.as_ref());
    Ok(())
}

fn screens(report: &mut Report, screen: &dyn ScreenInfo) {
    report.say("1. Screens");
    match (screen.displays(), screen.scales()) {
        (Ok(displays), Ok(scales)) => {
            for (n, (display, scale)) in displays.iter().zip(&scales).enumerate() {
                report.info(format!(
                    "display {n}: {}x{} at ({}, {}), scale {scale}%{}",
                    display.width,
                    display.height,
                    display.x,
                    display.y,
                    if n == 0 { " (main)" } else { "" }
                ));
            }
        }
        (Err(err), _) | (_, Err(err)) => report.verdict(false, format!("cannot list the displays: {err}")),
    }
    match screen.cursor() {
        Ok((x, y)) => report.info(format!("cursor at ({x}, {y})")),
        Err(err) => report.verdict(false, format!("cannot read the cursor: {err}")),
    }
    report.say("");
}

/// Counts down so that the person is ready, then returns.
fn get_ready(what: &str) {
    println!("   {what}");
    for left in (1..=3).rev() {
        print!("   starting in {left}... ");
        let _ = io::stdout().flush();
        sleep(Duration::from_secs(1));
    }
    println!("NOW");
}

/// Reads and drops whatever input is waiting, so a step starts from nothing.
fn drain(source: &mut dyn InputSource) {
    while let Ok(Some(_)) = source.next_event_timeout(Duration::from_millis(1)) {}
}

fn reading(report: &mut Report, source: &mut dyn InputSource, screen: &dyn ScreenInfo) {
    report.say("2. Reading the mouse");
    get_ready(&format!("Move the mouse around, in circles, for {} seconds.", READ_FOR.as_secs()));
    drain(source);
    let start = screen.cursor().ok();
    let (mut events, mut travelled) = (0u32, 0i64);
    let deadline = Instant::now() + READ_FOR;
    while Instant::now() < deadline {
        if let Ok(Some(InputEvent::MouseMove { dx, dy })) = source.next_event_timeout(POLL) {
            events += 1;
            travelled += i64::from(dx.abs() + dy.abs());
        }
    }
    let end = screen.cursor().ok();
    report.verdict(events > 0, format!("{events} mouse movements read, {travelled} counts in all"));
    report.info(format!("cursor went from {start:?} to {end:?}"));
    report.info(source.describe());
    report.say("");
}

fn edge(report: &mut Report, source: &mut dyn InputSource, screen: &dyn ScreenInfo) {
    report.say("3. Pushing against a screen edge");
    get_ready(&format!(
        "Push the mouse against the outer edge of a screen (any edge) and keep pushing for {} seconds.",
        READ_FOR.as_secs()
    ));
    drain(source);
    let (mut events, mut still) = (0u32, 0u32);
    let mut last = screen.cursor().ok();
    let deadline = Instant::now() + READ_FOR;
    while Instant::now() < deadline {
        if let Ok(Some(InputEvent::MouseMove { .. })) = source.next_event_timeout(POLL) {
            events += 1;
            let now = screen.cursor().ok();
            if now == last {
                still += 1;
            }
            last = now;
        }
    }
    report.info(format!("{events} movements read, {still} of them while the cursor did not move"));
    report.verdict(
        still >= 5,
        "movement keeps coming when the cursor is stopped at an edge (needed to cross to another computer)",
    );
    report.say("");
}

fn capture(report: &mut Report, source: &mut dyn InputSource, screen: &dyn ScreenInfo) {
    report.say("4. Capturing input (what happens while another computer is controlled)");
    get_ready(&format!(
        "Move the mouse and press a few letter keys for {} seconds. The cursor should disappear, \
         stay put, and nothing should be typed.",
        READ_FOR.as_secs()
    ));
    drain(source);
    if let Err(err) = source.set_capture(true) {
        report.verdict(false, format!("cannot start capturing: {err}"));
        report.say("");
        return;
    }
    sleep(Duration::from_millis(300));
    let before = screen.cursor().ok();
    let (mut moves, mut keys, mut dx_sum, mut dy_sum) = (0u32, 0u32, 0i64, 0i64);
    let deadline = Instant::now() + READ_FOR;
    while Instant::now() < deadline {
        match source.next_event_timeout(POLL) {
            Ok(Some(InputEvent::MouseMove { dx, dy })) => {
                moves += 1;
                dx_sum += i64::from(dx);
                dy_sum += i64::from(dy);
            }
            Ok(Some(InputEvent::Key { .. })) => keys += 1,
            _ => {}
        }
    }
    let after = screen.cursor().ok();
    if let Err(err) = source.set_capture(false) {
        report.verdict(false, format!("cannot stop capturing: {err}"));
    }

    report.verdict(moves > 0, format!("{moves} mouse movements read while capturing (net {dx_sum}, {dy_sum})"));
    report.verdict(before == after, format!("the cursor stayed put: {before:?} then {after:?}"));
    report.info(format!("{keys} key events read; nothing should have been typed anywhere"));
    report.info(source.describe());
    let answer = ask("Did the cursor disappear during this step? (y/n) ");
    report.info(format!("cursor hidden while capturing: {answer}"));
    report.say("");
}

fn injecting(report: &mut Report, sink: &mut dyn InputSink, screen: &dyn ScreenInfo) {
    report.say("5. Moving the cursor (do not touch the mouse)");
    let Ok(displays) = screen.displays() else {
        report.verdict(false, "no displays to move the cursor on");
        return;
    };
    let main = displays[0];
    let (cx, cy) = (main.x + main.width / 2, main.y + main.height / 2);
    sleep(Duration::from_secs(1));

    for (x, y) in [(cx - 100, cy - 100), (cx + 100, cy - 100), (cx + 100, cy + 100), (cx - 100, cy + 100)] {
        let result = sink.move_abs(x, y);
        sleep(Duration::from_millis(80));
        let at = screen.cursor().ok();
        let near = at.is_some_and(|(ax, ay)| (ax - x).abs() <= 2 && (ay - y).abs() <= 2);
        report.verdict(result.is_ok() && near, format!("moved to ({x}, {y}): the cursor is at {at:?}"));
    }

    let from = screen.cursor().ok();
    let result = sink.move_rel(40, 25);
    sleep(Duration::from_millis(80));
    let to = screen.cursor().ok();
    let moved = match (from, to) {
        (Some((fx, fy)), Some((tx, ty))) => (tx - fx - 40).abs() <= 2 && (ty - fy - 25).abs() <= 2,
        _ => false,
    };
    report.verdict(result.is_ok() && moved, format!("moved by (40, 25): the cursor went from {from:?} to {to:?}"));
    let _ = sink.release_all();
    report.say("");
}

fn ask(question: &str) -> String {
    print!("   {question}");
    let _ = io::stdout().flush();
    let mut answer = String::new();
    let _ = io::stdin().lock().read_line(&mut answer);
    let answer = answer.trim();
    if answer.is_empty() { "no answer".into() } else { answer.into() }
}
