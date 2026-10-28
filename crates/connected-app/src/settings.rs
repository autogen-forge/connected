//! What the program remembers between runs.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use connected_core::{Arrangement, Os, Rect};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

const FILE: &str = "settings.json";
const DEFAULT_PORT_ADDRESS: &str = "0.0.0.0:24900";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The address this computer listens on for the others.
    pub listen: String,
    /// Where to reach each paired computer, as `address:port`, by name.
    pub peer_addresses: BTreeMap<String, String>,
    /// Where each paired computer sits relative to this one.
    pub arrangement: Arrangement,
    /// When the arrangement was last changed, in milliseconds; the newest one among the
    /// computers is the one they all use. 0 if it was never changed.
    pub layout_version: u64,
    /// The displays each paired computer had when it last connected, so that its place on
    /// the layout can be drawn to scale while it is switched off.
    pub known_screens: BTreeMap<String, Vec<Rect>>,
    /// How much each display of each paired computer is magnified, as last reported.
    pub known_scales: BTreeMap<String, Vec<u16>>,
    /// The system each paired computer runs, as last reported.
    pub known_os: BTreeMap<String, Os>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            listen: DEFAULT_PORT_ADDRESS.to_owned(),
            peer_addresses: BTreeMap::new(),
            arrangement: Arrangement::default(),
            layout_version: 0,
            known_screens: BTreeMap::new(),
            known_scales: BTreeMap::new(),
            known_os: BTreeMap::new(),
        }
    }
}

impl Settings {
    /// Reads the settings in `dir`. Nothing saved yet means the defaults. A file that cannot
    /// be understood is set aside as `settings.json.bad`, so it is not overwritten, and the
    /// defaults are used; the second value says what happened.
    pub fn load(dir: &Path) -> (Self, Option<String>) {
        let path = dir.join(FILE);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return (Self::default(), None),
            Err(err) => {
                return (Self::default(), Some(format!("cannot read {}: {err}", path.display())));
            }
        };
        match serde_json::from_str(&text) {
            Ok(settings) => (settings, None),
            Err(err) => {
                let aside = path.with_extension("json.bad");
                let moved = fs::rename(&path, &aside).is_ok();
                let note = format!(
                    "{} is not valid ({err}); {}",
                    path.display(),
                    if moved { format!("kept as {}", aside.display()) } else { "left untouched".into() }
                );
                (Self::default(), Some(note))
            }
        }
    }

    /// Writes the settings to `dir`, replacing the old file in one step.
    pub fn save(&self, dir: &Path) -> Result<()> {
        let io_error = |err: std::io::Error| AppError::Config(format!("cannot save settings: {err}"));
        fs::create_dir_all(dir).map_err(io_error)?;
        let text = serde_json::to_string_pretty(self)
            .map_err(|err| AppError::Config(format!("cannot save settings: {err}")))?;
        let path = dir.join(FILE);
        let temp = path.with_extension("json.tmp");
        fs::write(&temp, text).map_err(io_error)?;
        fs::rename(&temp, &path).map_err(io_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use connected_core::Metrics;
    use connected_proto::Edge;
    use std::collections::BTreeMap;

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("connected-settings-{label}-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn nothing_saved_means_defaults() {
        let (settings, note) = Settings::load(&temp_dir("none"));
        assert_eq!(settings, Settings::default());
        assert!(note.is_none());
        assert_eq!(settings.listen, "0.0.0.0:24900");
    }

    #[test]
    fn settings_survive_a_save_and_load() {
        let dir = temp_dir("roundtrip");
        let mut settings = Settings { layout_version: 42, ..Settings::default() };
        settings.peer_addresses.insert("laptop".into(), "10.0.0.5:24900".into());
        let devices = BTreeMap::new();
        let local = [Rect::new(0, 0, 1920, 1080)];
        let metrics = Metrics { this_computer: "desk", local: &local, devices: &devices };
        settings.arrangement.place_beside_this_computer("laptop", Edge::Right, &metrics).unwrap();
        settings.arrangement.place("tablet", 4000, -300, &metrics).unwrap();
        settings.save(&dir).unwrap();

        let (loaded, note) = Settings::load(&dir);
        assert_eq!(loaded, settings);
        assert!(note.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let dir = temp_dir("partial");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(FILE), r#"{ "mode": "receive", "layout_version": 7 }"#).unwrap();

        let (loaded, note) = Settings::load(&dir);
        assert_eq!(loaded.layout_version, 7);
        assert_eq!(loaded.listen, "0.0.0.0:24900");
        assert!(note.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_file_is_set_aside_not_overwritten() {
        let dir = temp_dir("broken");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(FILE), "{ this is not json").unwrap();

        let (loaded, note) = Settings::load(&dir);
        assert_eq!(loaded, Settings::default());
        assert!(note.unwrap().contains("settings.json.bad"));
        assert!(dir.join("settings.json.bad").exists());
        assert!(!dir.join(FILE).exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
