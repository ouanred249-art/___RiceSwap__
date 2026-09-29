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
//!
//! The `operation` entry carries one more field, `source`, which only `install`
//! fills in. An install's steps are idempotent and it has a fatal half — a
//! missing AUR helper, a profile that cannot be written — so "run the same
//! command again" is how a user resumes it; the marker that tells a *half*
//! install apart from a *finished* one has to name both what was being installed
//! and where it was being installed to. A finished install clears the entry like
//! every other operation, so a plain re-run of a complete install is refused
//! instead of re-materializing a profile that is already live. See
//! [`StateStore::claim`].

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// The operation currently running.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OperationState {
    pub name: String,
    pub target: Option<String>,
    pub started_at: u64,
    pub step: u32,
    /// The source an `install` is installing — the git URL, or the `file://`
    /// form of the local directory. `null` for every other operation, and for
    /// an install that has not resolved its source yet.
    #[serde(default)]
    pub source: Option<String>,
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
    /// The operation that was already on disk when this process loaded the
    /// document — the one that never reached its own `finish`, because the
    /// process died. Read *before* [`StateStore::begin`] overwrites it, and
    /// only `install` has anything to say about one (see
    /// [`StateStore::interrupted`]).
    interrupted: Option<OperationState>,
    /// Whether this operation claimed a profile and must stay marked in
    /// `state.json` when it fails — set by [`StateStore::claim`], consumed by
    /// [`StateStore::finish`].
    holding: bool,
}

impl StateStore {
    /// Loads the document at `path`, falling back to a fresh one when the file
    /// is missing or unreadable — a corrupt state file must not stop the backend.
    ///
    /// The running operation is deliberately *not* carried into the new state: a
    /// backend that starts while another one is running shows no operation of
    /// its own. What is kept is the entry that was already on disk, for the one
    /// operation that has to reason about a predecessor.
    pub fn load(path: PathBuf) -> StateStore {
        let raw = std::fs::read_to_string(&path).ok();
        let parsed = raw
            .as_deref()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok());
        let state = parsed
            .as_ref()
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
        // An entry this build cannot read is no marker: a state file written by
        // a future build, or a hand-edited one, must not make a profile look
        // half-installed.
        let interrupted = parsed.as_ref().and_then(|value| {
            serde_json::from_value::<OperationState>(value["operation"].clone()).ok()
        });
        StateStore {
            path,
            state,
            interrupted,
            holding: false,
        }
    }

    /// The operation this process found already on disk, if one was there.
    pub fn interrupted(&self) -> Option<&OperationState> {
        self.interrupted.as_ref()
    }

    /// Records the operation that is starting.
    pub fn begin(&mut self, name: &str, target: Option<String>) {
        self.state.operation = Some(OperationState {
            name: name.to_string(),
            target,
            started_at: now(),
            step: 0,
            source: None,
        });
    }

    /// Points the running operation at the profile it has just claimed, under
    /// the source it resolved that profile from, and marks the entry as one that
    /// has to survive a failure.
    ///
    /// This is what makes a half-finished install resumable by plain
    /// re-invocation: the next run reads [`StateStore::interrupted`], sees an
    /// `install` of the *same* source that had claimed the *same* profile, and
    /// is allowed to clear and re-make it instead of refusing. A `switch` of the
    /// same profile records nothing here, so it never confers that permission.
    pub fn claim(&mut self, target: &str, source: &str) {
        if let Some(operation) = &mut self.state.operation {
            operation.target = Some(target.to_string());
            operation.source = Some(source.to_string());
        }
        self.holding = true;
    }

    /// Advances the visible step, so a panel reopened mid-operation still
    /// renders where the backend is.
    pub fn set_step(&mut self, step: u32) {
        if let Some(operation) = &mut self.state.operation {
            operation.step = step;
        }
    }

    /// Records the outcome and clears the running operation.
    ///
    /// An operation that claimed a profile and then failed keeps its entry: its
    /// steps are idempotent and the next run of the same command is the resume.
    /// `last_result` still says the operation failed, and the panel that
    /// watched it stream knows it is over — the entry is the record a *later*
    /// invocation reads to tell a half-made profile from a finished one.
    pub fn finish(&mut self, ok: bool, warnings: Vec<String>) {
        if ok || !self.holding {
            self.state.operation = None;
        }
        self.state.last_result = Some(LastResult { ok, warnings });
    }

    /// The bootstrap flag `init` owns.
    pub fn set_initialized(&mut self, initialized: bool) {
        self.state.initialized = initialized;
    }

    /// Re-reads the active profile from the `current` symlink, so the document
    /// can never disagree with the filesystem the GUI badges from.
    pub fn set_active_profile(&mut self, active: Option<String>) {
        self.state.active_profile = active;
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
