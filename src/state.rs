//! `~/.local/share/riceswap/state.json`.
//!
//! Frozen contract: the QML panel watches this file to render progress, to
//! survive a reopen mid-switch, and to keep the active-profile badge truthful.
//! Its shape is therefore pinned:
//!
//! ```json
//! {
//!   "initialized": true,
//!   "active_profile": null,
//!   "operation": { "name": "switch", "target": "demo", "started_at": 1758000000, "step": 3 },
//!   "last_result": { "ok": true, "warnings": [] }
//! }
//! ```
//!
//! Every key is always present; `null` means idle. `started_at` is Unix epoch
//! seconds. The file is rewritten after every state change, via a temp file and
//! a rename so the watcher never reads a half-written document.

use serde::Serialize;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// The operation currently running.
#[derive(Debug, Serialize)]
pub struct OperationState {
    pub name: String,
    pub target: Option<String>,
    pub started_at: u64,
    pub step: u32,
}

/// The outcome of the last finished operation.
#[derive(Debug, Serialize)]
pub struct LastResult {
    pub ok: bool,
    pub warnings: Vec<String>,
}

/// The whole document.
#[derive(Debug, Serialize)]
pub struct State {
    pub initialized: bool,
    pub active_profile: Option<String>,
    pub operation: Option<OperationState>,
    pub last_result: Option<LastResult>,
}

/// Reads, mutates, and rewrites `state.json`.
pub struct StateStore {
    path: PathBuf,
    state: State,
}

impl StateStore {
    /// Loads the document at `path`, falling back to a fresh one when the file
    /// is missing or unreadable — a corrupt state file must not stop the backend.
    pub fn load(path: PathBuf) -> StateStore {
        let state = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .map(|value| State {
                initialized: value["initialized"].as_bool().unwrap_or(false),
                active_profile: value["active_profile"].as_str().map(str::to_string),
                operation: None,
                last_result: None,
            })
            .unwrap_or(State {
                initialized: false,
                active_profile: None,
                operation: None,
                last_result: None,
            });
        StateStore { path, state }
    }

    /// Records the operation that is starting.
    pub fn begin(&mut self, name: &str, target: Option<String>) {
        self.state.operation = Some(OperationState {
            name: name.to_string(),
            target,
            started_at: now(),
            step: 0,
        });
    }

    /// Advances the visible step, so a panel reopened mid-operation still
    /// renders where the backend is.
    pub fn set_step(&mut self, step: u32) {
        if let Some(operation) = &mut self.state.operation {
            operation.step = step;
        }
    }

    /// Records the outcome and clears the running operation.
    pub fn finish(&mut self, ok: bool, warnings: Vec<String>) {
        self.state.operation = None;
        self.state.last_result = Some(LastResult { ok, warnings });
    }

    /// The bootstrap flag `init` owns.
    pub fn set_initialized(&mut self, initialized: bool) {
        self.state.initialized = initialized;
    }

    /// Rewrites the document. Failures are swallowed: the envelope on stdout is
    /// the authoritative result, and a read-only `$HOME` must not become a panic.
    pub fn write(&self) {
        let Some(parent) = self.path.parent() else {
            return;
        };
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
        let Ok(rendered) = serde_json::to_string_pretty(&self.state) else {
            return;
        };
        let temporary = self.path.with_extension("json.tmp");
        if std::fs::write(&temporary, rendered.as_bytes()).is_err() {
            return;
        }
        let _ = std::fs::rename(&temporary, &self.path);
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}
