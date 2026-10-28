//! What the window can ask for. There is one entry point, [`dispatch`], taking a command's
//! name and arguments as JSON; the window and the development server both go through it.
//!
//! Every command answers with a fresh [`Snapshot`], so the window can draw the result of
//! what it just did without asking again.

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::snapshot::snapshot;
use crate::state::AppState;

pub fn dispatch(state: &AppState, command: &str, args: Value) -> Result<Value, String> {
    match command {
        "snapshot" => {}
        "start" => with_model(state, |model| model.start()),
        "stop" => with_model(state, |model| model.stop()),
        "set_peer_address" => {
            let device: String = arg(&args, "device")?;
            let address: String = arg(&args, "address")?;
            with_model(state, |model| model.set_peer_address(&device, &address));
        }
        "place_device" => {
            let device: String = arg(&args, "device")?;
            let (x, y) = position(&args)?;
            with_model(state, |model| model.place_device(&device, x, y));
        }
        "move_this_computer" => {
            let (x, y) = position(&args)?;
            with_model(state, |model| model.move_this_computer(x, y));
        }
        "move_group" => {
            let devices: Vec<String> = arg(&args, "devices")?;
            let local: bool = arg(&args, "local")?;
            let (dx, dy): (i32, i32) = (arg(&args, "dx")?, arg(&args, "dy")?);
            with_model(state, |model| model.move_group(&devices, local, dx, dy));
        }
        "remove_from_layout" => {
            let device: String = arg(&args, "device")?;
            with_model(state, |model| model.remove_from_layout(&device));
        }
        "forget_device" => {
            let name: String = arg(&args, "name")?;
            with_model(state, |model| model.forget_device(&name));
        }
        "open_pairing" => with_model(state, |model| model.open_pairing()),
        "pair_by_waiting" => with_model(state, |model| model.pair_by_waiting()),
        "pair_by_connecting" => {
            let address: String = arg(&args, "address")?;
            with_model(state, |model| model.pair_by_connecting(&address));
        }
        "answer_pairing" => {
            let matches: bool = arg(&args, "matches")?;
            with_model(state, |model| model.answer_pairing(matches));
        }
        "close_pairing" => with_model(state, |model| model.close_pairing()),
        "dismiss_notice" => with_model(state, |model| model.notice = None),
        "request_permissions" => connected_platform::request_permissions(),
        "open_privacy_settings" => {
            let pane: Option<String> = args.get("pane").and_then(|value| value.as_str()).map(str::to_owned);
            open_privacy_settings(pane.as_deref());
        }
        other => return Err(format!("unknown command '{other}'")),
    }
    serde_json::to_value(snapshot(state)).map_err(|err| err.to_string())
}

/// Runs `action` on the model with it locked. The lock is let go before the snapshot is made.
fn with_model(state: &AppState, action: impl FnOnce(&mut crate::model::AppModel)) {
    let mut model = state.model.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    action(&mut model);
}

fn arg<T: DeserializeOwned>(args: &Value, name: &str) -> Result<T, String> {
    let value = args.get(name).ok_or_else(|| format!("missing argument '{name}'"))?;
    serde_json::from_value(value.clone()).map_err(|err| format!("bad argument '{name}': {err}"))
}

/// A position on the shared desktop, in pixels.
fn position(args: &Value) -> Result<(i32, i32), String> {
    Ok((arg(args, "x")?, arg(args, "y")?))
}

/// The part of the privacy settings address that picks the page: `input_monitoring` is the
/// Input Monitoring page, and anything else, `accessibility` included, is the Accessibility one.
fn privacy_anchor(pane: Option<&str>) -> &'static str {
    match pane {
        Some("input_monitoring") => "Privacy_ListenEvent",
        _ => "Privacy_Accessibility",
    }
}

/// Opens the system settings at the page where the given permission is given.
fn open_privacy_settings(pane: Option<&str>) {
    #[cfg(target_os = "macos")]
    {
        let url = format!(
            "x-apple.systempreferences:com.apple.preference.security?{}",
            privacy_anchor(pane)
        );
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(not(target_os = "macos"))]
    let _ = pane;
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::model::testing::{model, pair, wait_until};
    use crate::snapshot::as_json;

    fn state() -> AppState {
        AppState::new(model("desk"), None)
    }

    /// A desk that is paired with a laptop.
    fn state_with_a_laptop() -> AppState {
        let (mut desk, mut laptop) = (model("desk"), model("laptop"));
        pair(&mut desk, &mut laptop);
        desk.close_pairing();
        AppState::new(desk, None)
    }

    fn call(state: &AppState, command: &str, args: Value) -> Value {
        dispatch(state, command, args).unwrap_or_else(|err| panic!("{command} failed: {err}"))
    }

    #[test]
    fn each_permission_has_its_own_page_in_the_system_settings() {
        assert_eq!(privacy_anchor(Some("accessibility")), "Privacy_Accessibility");
        assert_eq!(privacy_anchor(Some("input_monitoring")), "Privacy_ListenEvent");
        assert_eq!(privacy_anchor(None), "Privacy_Accessibility");
        assert_eq!(privacy_anchor(Some("../../etc")), "Privacy_Accessibility", "only known pages are opened");
    }

    #[test]
    fn the_first_snapshot_describes_an_idle_computer() {
        let snapshot = call(&state(), "snapshot", json!({}));
        assert_eq!(snapshot["this_computer"], "desk");
        assert_eq!(snapshot["running"], false);
        assert_eq!(snapshot["devices"], json!([]));
        assert_eq!(
            snapshot["screens"],
            json!([{
                "name": "desk", "x": 0, "y": 0, "width": 1920, "height": 1080, "local": true,
                "displays": [{ "x": 0, "y": 0, "width": 1920, "height": 1080 }],
                "scales": [100],
                "os": connected_core::Os::current(),
                "online": true,
            }])
        );
        assert_eq!(snapshot["mesh"], Value::Null);
        assert_eq!(snapshot["pairing"], Value::Null);
        assert_eq!(snapshot["mesh_port"], 24900);
        assert_eq!(snapshot["pairing_port"], 24901);
        assert!(snapshot["fingerprint"].as_str().unwrap().len() == 64);
    }

    #[test]
    fn starting_and_stopping_shows_up_in_the_snapshot() {
        let state = state();
        let started = call(&state, "start", json!({}));
        assert_eq!(started["running"], true);
        assert_eq!(started["mesh"]["active_screen"], "desk");
        assert_eq!(started["mesh"]["controlled_by"], Value::Null);
        assert!(started["mesh"]["listening_on"].as_str().unwrap().contains(':'));

        let stopped = call(&state, "stop", json!({}));
        assert_eq!(stopped["running"], false);
        assert_eq!(stopped["mesh"], Value::Null);
    }

    #[test]
    fn a_devices_address_is_set_and_reported() {
        let state = state_with_a_laptop();
        let snapshot = call(&state, "set_peer_address", json!({ "device": "laptop", "address": "10.0.0.5" }));
        assert_eq!(snapshot["devices"][0]["address"], "10.0.0.5:24900");

        let snapshot = call(&state, "set_peer_address", json!({ "device": "laptop", "address": "" }));
        assert_eq!(snapshot["devices"][0]["address"], Value::Null);
    }

    #[test]
    fn a_failure_to_start_leaves_a_notice_that_can_be_dismissed() {
        let backends = crate::model::Backends {
            mesh: std::sync::Arc::new(|| Err(connected_app::AppError::Config("no permission".into()))),
            local_screen: std::sync::Arc::new(|| None),
        };
        let mut model = crate::model::AppModel::in_memory("desk", backends).unwrap();
        model.settings.listen = "127.0.0.1:0".into();
        let state = AppState::new(model, None);
        let snapshot = call(&state, "start", json!({}));
        assert_eq!(snapshot["running"], false);
        assert!(snapshot["notice"].as_str().unwrap().contains("no permission"));

        let snapshot = call(&state, "dismiss_notice", json!({}));
        assert_eq!(snapshot["notice"], Value::Null);
    }

    fn screen_of<'a>(snapshot: &'a Value, name: &str) -> Option<&'a Value> {
        snapshot["screens"].as_array()?.iter().find(|screen| screen["name"] == name)
    }

    #[test]
    fn a_paired_device_is_listed_and_can_be_placed_moved_and_removed() {
        let state = state_with_a_laptop();
        let snapshot = call(&state, "snapshot", json!({}));
        assert_eq!(snapshot["devices"][0]["name"], "laptop");
        assert_eq!(snapshot["devices"][0]["online"], false);
        assert_eq!(
            snapshot["devices"][0]["displays"],
            json!([{ "x": 0, "y": 0, "width": 1920, "height": 1080 }])
        );
        assert!(screen_of(&snapshot, "laptop").is_none(), "not on the layout until placed");

        let snapshot = call(&state, "place_device", json!({ "device": "laptop", "x": 1920, "y": 0 }));
        let laptop = screen_of(&snapshot, "laptop").unwrap();
        assert_eq!((laptop["x"].as_i64(), laptop["y"].as_i64()), (Some(1920), Some(0)));
        assert_eq!(laptop["local"], false);

        let snapshot = call(&state, "place_device", json!({ "device": "laptop", "x": -1920, "y": 200 }));
        let laptop = screen_of(&snapshot, "laptop").unwrap();
        assert_eq!((laptop["x"].as_i64(), laptop["y"].as_i64()), (Some(-1920), Some(200)));

        let snapshot = call(&state, "remove_from_layout", json!({ "device": "laptop" }));
        assert!(screen_of(&snapshot, "laptop").is_none());
    }

    #[test]
    fn a_screen_cannot_be_put_on_top_of_another() {
        let state = state_with_a_laptop();
        let snapshot = call(&state, "place_device", json!({ "device": "laptop", "x": 500, "y": 500 }));
        assert!(screen_of(&snapshot, "laptop").is_none());
        assert!(snapshot["notice"].as_str().unwrap().contains("desk"));
    }

    #[test]
    fn this_computer_can_be_moved_but_not_onto_a_device() {
        let state = state_with_a_laptop();
        call(&state, "place_device", json!({ "device": "laptop", "x": 1920, "y": 0 }));

        let snapshot = call(&state, "move_this_computer", json!({ "x": 0, "y": 1080 }));
        assert_eq!(screen_of(&snapshot, "desk").unwrap()["y"], 1080);
        assert_eq!(snapshot["notice"], Value::Null);

        let snapshot = call(&state, "move_this_computer", json!({ "x": 1500, "y": 0 }));
        assert_eq!(screen_of(&snapshot, "desk").unwrap()["x"], 0);
        assert!(snapshot["notice"].is_string());
    }

    #[test]
    fn several_computers_can_be_moved_at_once_together_with_this_one() {
        let state = state_with_a_laptop();
        call(&state, "place_device", json!({ "device": "laptop", "x": 1920, "y": 0 }));

        let snapshot = call(
            &state,
            "move_group",
            json!({ "devices": ["laptop"], "local": true, "dx": 0, "dy": 1500 }),
        );
        assert_eq!(snapshot["notice"], Value::Null);
        assert_eq!(screen_of(&snapshot, "desk").unwrap()["y"], 1500);
        assert_eq!(screen_of(&snapshot, "laptop").unwrap()["y"], 1500);
        assert_eq!(screen_of(&snapshot, "laptop").unwrap()["x"], 1920);
    }

    #[test]
    fn a_group_that_would_overlap_a_computer_left_behind_does_not_move() {
        let state = state_with_a_laptop();
        call(&state, "place_device", json!({ "device": "laptop", "x": 1920, "y": 0 }));

        // This computer alone, into the laptop.
        let snapshot = call(
            &state,
            "move_group",
            json!({ "devices": [], "local": true, "dx": 1500, "dy": 0 }),
        );
        assert!(snapshot["notice"].as_str().unwrap().contains("laptop"));
        assert_eq!(screen_of(&snapshot, "desk").unwrap()["x"], 0);
    }

    #[test]
    fn forgetting_a_device_removes_it_everywhere() {
        let state = state_with_a_laptop();
        call(&state, "place_device", json!({ "device": "laptop", "x": 1920, "y": 0 }));

        let snapshot = call(&state, "forget_device", json!({ "name": "laptop" }));
        assert_eq!(snapshot["devices"], json!([]));
        assert!(screen_of(&snapshot, "laptop").is_none());
    }

    #[test]
    fn pairing_walks_through_its_stages() {
        let state = state();
        let snapshot = call(&state, "open_pairing", json!({}));
        assert_eq!(snapshot["pairing"]["stage"], "choosing");

        // Nothing listens on port 1, so this ends in an error that the window can show.
        let snapshot = call(&state, "pair_by_connecting", json!({ "address": "127.0.0.1:1" }));
        assert_eq!(snapshot["pairing"]["stage"], "connecting");

        wait_until("the attempt to fail", || {
            as_json(&state)["pairing"]["stage"] == "done"
        });
        let snapshot = call(&state, "snapshot", json!({}));
        assert_eq!(snapshot["pairing"]["ok"], false);
        assert!(snapshot["pairing"]["message"].is_string());

        let snapshot = call(&state, "close_pairing", json!({}));
        assert_eq!(snapshot["pairing"], Value::Null);
    }

    #[test]
    fn the_snapshot_lists_what_the_activity_tab_shows() {
        let state = state();
        call(&state, "start", json!({}));
        let snapshot = call(&state, "snapshot", json!({}));
        let logs = snapshot["logs"].as_array().unwrap();
        assert!(logs.iter().any(|line| line.as_str().unwrap().contains("started")));
    }

    #[test]
    fn unknown_commands_and_bad_arguments_are_errors_not_panics() {
        let state = state();
        assert!(dispatch(&state, "launch_missiles", json!({})).unwrap_err().contains("unknown command"));
        assert!(dispatch(&state, "set_peer_address", json!({})).unwrap_err().contains("missing argument"));
        assert!(dispatch(&state, "set_peer_address", json!({ "device": 5, "address": "" })).unwrap_err().contains("bad argument"));
        assert!(dispatch(&state, "place_device", json!({ "device": "x", "x": "one", "y": 0 })).is_err());
        assert!(dispatch(&state, "answer_pairing", json!({ "matches": "yes" })).is_err());
    }

    #[test]
    fn a_connected_device_is_reported_online_with_its_screen() {
        // Other computers, as the demo makes them, connected to this one.
        let (state, _demo) = {
            let (model, demo) = crate::demo::build().unwrap();
            (AppState::new(model, Some(demo)), ())
        };
        wait_until("the stand-in computers to connect", || {
            as_json(&state)["mesh"]["peers"].as_array().is_some_and(|peers| peers.len() == 2)
        });
        let snapshot = call(&state, "snapshot", json!({}));
        let online: Vec<_> = snapshot["devices"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|device| device["online"] == true)
            .map(|device| device["name"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(online, vec!["MacBook Air", "Windows PC"]);
        assert_eq!(snapshot["demo"], true);
        let macbook = snapshot["devices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|device| device["name"] == "MacBook Air")
            .unwrap();
        assert_eq!(macbook["screen"], json!([1470, 956]));

        // Each computer says which system it runs, and the screens carry it too.
        let systems: Vec<_> = ["MacBook Air", "Windows PC"]
            .iter()
            .map(|name| screen_of(&snapshot, name).unwrap()["os"].clone())
            .collect();
        assert_eq!(systems, vec![json!("mac"), json!("windows")]);
        let tv = snapshot["devices"].as_array().unwrap().iter().find(|d| d["name"] == "거실 TV").unwrap();
        assert_eq!(tv["os"], "linux", "known from before, though it is switched off");

        // Each monitor's magnification arrives with the displays, and the main display is first.
        let windows = screen_of(&snapshot, "Windows PC").unwrap();
        assert_eq!(windows["scales"], json!([100, 150]));
        assert_eq!(screen_of(&snapshot, "MacBook Air").unwrap()["scales"], json!([200]));
        assert_eq!(screen_of(&snapshot, "Studio Mac").unwrap()["scales"], json!([200]));
    }
}
