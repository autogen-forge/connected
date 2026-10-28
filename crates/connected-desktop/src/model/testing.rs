//! Helpers shared by the tests of the model and of the screens.

use std::time::{Duration, Instant};

use connected_core::{InputSink, ScreenInfo};
use connected_platform::mock::{ChannelSource, FixedScreen, SharedRecordingSink};
use connected_app::{PairingSession, ServerBackends};

use super::*;

pub fn mock_backends() -> Backends {
    Backends {
        server: Arc::new(|| {
            let (source, _input, _capture) = ChannelSource::new();
            Ok(ServerBackends {
                source: Box::new(source),
                cursor_sink: Box::new(SharedRecordingSink::default()),
                screen: Box::new(FixedScreen::new(1920, 1080)),
            })
        }),
        client: Arc::new(|| {
            Ok((
                Box::new(SharedRecordingSink::default()) as Box<dyn InputSink>,
                Box::new(FixedScreen::new(2560, 1440)) as Box<dyn ScreenInfo>,
            ))
        }),
        local_screen: Arc::new(|| Some((vec![Rect::new(0, 0, 1920, 1080)], vec![100]))),
    }
}


pub fn model(name: &str) -> AppModel {
    let mut model = AppModel::in_memory(name, mock_backends()).unwrap();
    model.settings.listen = "127.0.0.1:0".into();
    model
}


pub fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}


/// Pairs the two models through the same flow the screen uses.
pub fn pair(waiting: &mut AppModel, connecting: &mut AppModel) {
    waiting.open_pairing();
    waiting.settings.listen = "127.0.0.1:0".into();
    let session = PairingSession::listen(waiting.node.clone(), "127.0.0.1:0").unwrap();
    waiting.pairing.as_mut().unwrap().session = Some(session);
    let addr = loop {
        waiting.tick();
        if let Some(PairingFlow { stage: PairingStage::Waiting { addr }, .. }) = &waiting.pairing {
            break addr.clone();
        }
        std::thread::sleep(Duration::from_millis(5));
    };

    connecting.open_pairing();
    let session = PairingSession::connect(connecting.node.clone(), addr);
    connecting.pairing.as_mut().unwrap().session = Some(session);

    let mut answered = [false, false];
    wait_until("both sides to finish pairing", || {
        for (index, model) in [&mut *waiting, &mut *connecting].into_iter().enumerate() {
            model.tick();
            if matches!(model.pairing.as_ref().map(|flow| &flow.stage), Some(PairingStage::Confirming { .. }))
                && !answered[index]
            {
                model.answer_pairing(true);
                answered[index] = true;
            }
        }
        [&*waiting, &*connecting].iter().all(|model| {
            matches!(model.pairing.as_ref().map(|flow| &flow.stage), Some(PairingStage::Done(_)))
        })
    });
}

