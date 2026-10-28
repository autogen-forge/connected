//! What the window shares with the commands it calls.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use connected_core::Permission;

use crate::demo::Demo;
use crate::model::AppModel;

const PERMISSION_RECHECK: Duration = Duration::from_secs(1);

pub struct AppState {
    pub(crate) model: Mutex<AppModel>,
    permissions: Mutex<(Option<Instant>, Vec<Permission>)>,
    /// The demo has no real input, so it has nothing to ask permission for.
    check_permissions: bool,
    pub(crate) demo: bool,
    /// Keeps the demo's stand-in computers alive.
    _demo: Option<Demo>,
}

impl AppState {
    pub fn new(model: AppModel, demo: Option<Demo>) -> Self {
        Self {
            model: Mutex::new(model),
            permissions: Mutex::new((None, Vec::new())),
            check_permissions: demo.is_none(),
            demo: demo.is_some(),
            _demo: demo,
        }
    }

    /// The permissions the system has and has not given this program, looked up at most once
    /// a second.
    pub(crate) fn permissions(&self) -> Vec<Permission> {
        if !self.check_permissions {
            return Vec::new();
        }
        let Ok(mut cached) = self.permissions.lock() else {
            return Vec::new();
        };
        if cached.0.is_none_or(|at| at.elapsed() >= PERMISSION_RECHECK) {
            *cached = (Some(Instant::now()), connected_platform::permissions());
        }
        cached.1.clone()
    }

    /// Stops sharing or receiving and lets go of the keyboard and mouse. Called when the
    /// program is closing.
    pub fn shutdown(&self) {
        if let Ok(mut model) = self.model.lock() {
            model.stop();
            model.close_pairing();
        }
    }
}
