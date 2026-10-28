//! In-memory backends for tests.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use connected_core::platform::{PlatformError, Result};
use connected_core::{InputEvent, InputSink, InputSource, Os, Rect, ScreenInfo};
use connected_proto::{ButtonState, MouseButton};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SinkCall {
    MoveAbs(i32, i32),
    MoveRel(i32, i32),
    Button(MouseButton, ButtonState),
    Wheel(i32, i32),
    Key(u16, ButtonState),
    ReleaseAll,
}

/// A sink that records what it was asked to do.
#[derive(Debug, Default)]
pub struct RecordingSink {
    pub calls: Vec<SinkCall>,
}

impl InputSink for RecordingSink {
    fn move_abs(&mut self, x: i32, y: i32) -> Result<()> {
        self.calls.push(SinkCall::MoveAbs(x, y));
        Ok(())
    }

    fn move_rel(&mut self, dx: i32, dy: i32) -> Result<()> {
        self.calls.push(SinkCall::MoveRel(dx, dy));
        Ok(())
    }

    fn button(&mut self, button: MouseButton, state: ButtonState) -> Result<()> {
        self.calls.push(SinkCall::Button(button, state));
        Ok(())
    }

    fn wheel(&mut self, dx: i32, dy: i32) -> Result<()> {
        self.calls.push(SinkCall::Wheel(dx, dy));
        Ok(())
    }

    fn key(&mut self, usage: u16, state: ButtonState) -> Result<()> {
        self.calls.push(SinkCall::Key(usage, state));
        Ok(())
    }

    fn release_all(&mut self) -> Result<()> {
        self.calls.push(SinkCall::ReleaseAll);
        Ok(())
    }
}

/// A sink whose record can still be read after the sink was handed to something else.
#[derive(Debug, Clone, Default)]
pub struct SharedRecordingSink {
    calls: Arc<Mutex<Vec<SinkCall>>>,
}

impl SharedRecordingSink {
    pub fn calls(&self) -> Vec<SinkCall> {
        self.calls.lock().map(|calls| calls.clone()).unwrap_or_default()
    }

    fn record(&self, call: SinkCall) -> Result<()> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(call);
        }
        Ok(())
    }
}

impl InputSink for SharedRecordingSink {
    fn move_abs(&mut self, x: i32, y: i32) -> Result<()> {
        self.record(SinkCall::MoveAbs(x, y))
    }

    fn move_rel(&mut self, dx: i32, dy: i32) -> Result<()> {
        self.record(SinkCall::MoveRel(dx, dy))
    }

    fn button(&mut self, button: MouseButton, state: ButtonState) -> Result<()> {
        self.record(SinkCall::Button(button, state))
    }

    fn wheel(&mut self, dx: i32, dy: i32) -> Result<()> {
        self.record(SinkCall::Wheel(dx, dy))
    }

    fn key(&mut self, usage: u16, state: ButtonState) -> Result<()> {
        self.record(SinkCall::Key(usage, state))
    }

    fn release_all(&mut self) -> Result<()> {
        self.record(SinkCall::ReleaseAll)
    }
}

/// A source the test feeds through a channel. Whether capture is on can be read through
/// the flag that [`ChannelSource::new`] returns. If nobody holds the sending end any more,
/// it simply stays quiet, as a keyboard and mouse nobody touches would.
pub struct ChannelSource {
    events: Receiver<InputEvent>,
    capture: Arc<AtomicBool>,
}

impl ChannelSource {
    pub fn new() -> (Self, Sender<InputEvent>, Arc<AtomicBool>) {
        let (sender, events) = mpsc::channel();
        let capture = Arc::new(AtomicBool::new(false));
        (Self { events, capture: Arc::clone(&capture) }, sender, capture)
    }
}

impl InputSource for ChannelSource {
    fn next_event_timeout(&mut self, timeout: Duration) -> Result<Option<InputEvent>> {
        match self.events.recv_timeout(timeout) {
            Ok(event) => Ok(Some(event)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => {
                std::thread::sleep(timeout);
                Ok(None)
            }
        }
    }

    fn set_capture(&mut self, capture: bool) -> Result<()> {
        self.capture.store(capture, Ordering::SeqCst);
        Ok(())
    }
}

/// A screen of fixed size whose cursor position the test sets.
#[derive(Debug, Clone)]
pub struct FixedScreen {
    size: (i32, i32),
    displays: Vec<Rect>,
    scales: Vec<u16>,
    os: Os,
    cursor: Arc<Mutex<(i32, i32)>>,
}

impl FixedScreen {
    pub fn new(width: i32, height: i32) -> Self {
        Self {
            size: (width, height),
            displays: vec![Rect::new(0, 0, width, height)],
            scales: vec![100],
            os: Os::current(),
            cursor: Arc::new(Mutex::new((width / 2, height / 2))),
        }
    }

    /// A computer with several displays, the first being the main one.
    pub fn with_displays(displays: Vec<Rect>) -> Self {
        let main = displays.first().copied().unwrap_or(Rect::new(0, 0, 1, 1));
        Self {
            size: (main.width, main.height),
            cursor: Arc::new(Mutex::new((main.x + main.width / 2, main.y + main.height / 2))),
            scales: vec![100; displays.len()],
            displays,
            os: Os::current(),
        }
    }

    /// Says how much each display is magnified, in percent.
    pub fn with_scales(mut self, scales: Vec<u16>) -> Self {
        self.scales = scales;
        self
    }

    /// Says which system this computer runs.
    pub fn with_os(mut self, os: Os) -> Self {
        self.os = os;
        self
    }

    pub fn set_cursor(&self, x: i32, y: i32) {
        if let Ok(mut cursor) = self.cursor.lock() {
            *cursor = (x, y);
        }
    }
}

impl ScreenInfo for FixedScreen {
    fn size(&self) -> Result<(i32, i32)> {
        Ok(self.size)
    }

    fn displays(&self) -> Result<Vec<Rect>> {
        Ok(self.displays.clone())
    }

    fn scales(&self) -> Result<Vec<u16>> {
        Ok(self.scales.clone())
    }

    fn os(&self) -> Os {
        self.os
    }

    fn cursor(&self) -> Result<(i32, i32)> {
        self.cursor
            .lock()
            .map(|cursor| *cursor)
            .map_err(|_| PlatformError::Other("cursor lock poisoned".into()))
    }
}

/// A source that replays a fixed script of events, then reports an error.
#[derive(Debug, Default)]
pub struct ScriptedSource {
    events: VecDeque<InputEvent>,
    pub capture: bool,
}

impl ScriptedSource {
    pub fn new(events: impl IntoIterator<Item = InputEvent>) -> Self {
        Self { events: events.into_iter().collect(), capture: false }
    }
}

impl InputSource for ScriptedSource {
    fn next_event_timeout(&mut self, _timeout: Duration) -> Result<Option<InputEvent>> {
        self.events
            .pop_front()
            .map(Some)
            .ok_or_else(|| PlatformError::Other("script exhausted".into()))
    }

    fn set_capture(&mut self, capture: bool) -> Result<()> {
        self.capture = capture;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sink_records_calls_in_order() {
        let mut sink = RecordingSink::default();
        sink.move_abs(1, 2).unwrap();
        sink.key(0x04, ButtonState::Down).unwrap();
        sink.release_all().unwrap();
        assert_eq!(
            sink.calls,
            vec![
                SinkCall::MoveAbs(1, 2),
                SinkCall::Key(0x04, ButtonState::Down),
                SinkCall::ReleaseAll
            ]
        );
    }

    #[test]
    fn source_replays_then_errors() {
        let ev = InputEvent::Wheel { dx: 0, dy: 1 };
        let mut src = ScriptedSource::new([ev]);
        assert_eq!(src.next_event().unwrap(), ev);
        assert!(src.next_event().is_err());
    }
}
