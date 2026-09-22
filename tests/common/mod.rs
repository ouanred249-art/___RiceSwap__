//! Integration harness for the RiceSwap operation surface.
//!
//! Every test drives the compiled binary as a subprocess against a sandboxed
//! `$HOME` and a stubbed `PATH`, then asserts on the streamed NDJSON and on what
//! the stubs recorded. Nothing reaches inside the crate.
//!
//! Each test binary uses a subset of these helpers, so unused ones are expected.

#![allow(dead_code)]

use assert_cmd::Command;
use serde_json::Value;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Output;
use tempfile::TempDir;

/// Executables the backend shells out to, stubbed on `PATH` for every test.
pub const STUB_TOOLS: &[&str] = &["pacman", "yay", "paru", "hyprctl", "grim"];

/// How a stub executable answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Print a version and exit 0.
    Ok,
    /// Exit 1 with an error on stderr.
    Fail,
    /// Exit 2 with a conflict on stderr.
    Conflict,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Mode::Ok => "ok",
            Mode::Fail => "fail",
            Mode::Conflict => "conflict",
        }
    }
}

/// The stub every tool name is given. It logs its invocation, snapshots the
/// watched `state.json` if it exists, then answers per its scripted mode.
const STUB: &str = r#"#!/bin/sh
name=${0##*/}
printf '%s %s\n' "$name" "$*" >> "$RICESWAP_STUB_LOG"
if [ -n "$RICESWAP_STUB_STATE_DIR" ] && [ -f "$HOME/.local/share/riceswap/state.json" ]; then
  cp "$HOME/.local/share/riceswap/state.json" "$RICESWAP_STUB_STATE_DIR/$name.json"
fi
mode=ok
if [ -r "$RICESWAP_STUB_MODE_DIR/$name" ]; then read -r mode < "$RICESWAP_STUB_MODE_DIR/$name"; fi
case "$mode" in
  ok) printf '%s 1.0.0-stub\n' "$name"; exit 0 ;;
  conflict) printf '%s: stub conflict: conflicting package stub-conflict\n' "$name" >&2; exit 2 ;;
  *) printf '%s: stub failure (%s)\n' "$name" "$mode" >&2; exit 1 ;;
esac
"#;

/// A fake `$HOME`, a stub `PATH`, and the record of what the stubs saw.
pub struct Sandbox {
    _root: TempDir,
    home: PathBuf,
    bin: PathBuf,
    modes: PathBuf,
    states: PathBuf,
    stub_log: PathBuf,
}

impl Sandbox {
    pub fn new() -> Sandbox {
        let root = TempDir::new().expect("create sandbox root");
        let home = root.path().join("home");
        let bin = root.path().join("bin");
        let modes = root.path().join("modes");
        let states = root.path().join("state-snapshots");
        let stub_log = root.path().join("stub-invocations.log");
        for dir in [&home, &bin, &modes, &states] {
            fs::create_dir_all(dir).expect("create sandbox directory");
        }
        for tool in STUB_TOOLS {
            write_stub(&bin, tool);
        }
        fs::write(&stub_log, "").expect("create stub log");
        Sandbox {
            _root: root,
            home,
            bin,
            modes,
            states,
            stub_log,
        }
    }

    /// Scripts a stub's next answer. Sticky until scripted again.
    pub fn script(&self, tool: &str, mode: Mode) {
        assert!(STUB_TOOLS.contains(&tool), "unknown stub tool {tool}");
        fs::write(self.modes.join(tool), format!("{}\n", mode.as_str())).expect("script stub mode");
    }

    /// Runs the binary with `args` under the sandbox.
    pub fn run<S: AsRef<OsStr>>(&self, args: &[S]) -> Run {
        let mut command = Command::cargo_bin("riceswap").expect("riceswap binary is built");
        let output = command
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", self.path_env())
            .env("RICESWAP_STUB_LOG", &self.stub_log)
            .env("RICESWAP_STUB_MODE_DIR", &self.modes)
            .env("RICESWAP_STUB_STATE_DIR", &self.states)
            .args(args)
            .output()
            .expect("run riceswap");
        Run::new(output)
    }

    /// Every invocation the stubs recorded, in order.
    pub fn log(&self) -> Vec<String> {
        let log = fs::read_to_string(&self.stub_log).expect("read stub log");
        log.lines().map(str::to_string).collect()
    }

    /// Whether any stub recorded an invocation containing `needle`.
    pub fn log_contains(&self, needle: &str) -> bool {
        self.log().iter().any(|line| line.contains(needle))
    }

    /// The `state.json` the backend wrote inside the fake `$HOME`.
    pub fn state(&self) -> Value {
        let raw = fs::read_to_string(self.state_path())
            .unwrap_or_else(|error| panic!("read {}: {error}", self.state_path().display()));
        serde_json::from_str(&raw).expect("state.json is valid JSON")
    }

    /// Where the frozen `state.json` contract lives.
    pub fn state_path(&self) -> PathBuf {
        self.home
            .join(".local")
            .join("share")
            .join("riceswap")
            .join("state.json")
    }

    /// The `state.json` a stub captured mid-invocation, if it captured one.
    pub fn state_seen_by(&self, tool: &str) -> Value {
        let path = self.states.join(format!("{tool}.json"));
        let raw = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        serde_json::from_str(&raw).expect("captured state.json is valid JSON")
    }

    fn path_env(&self) -> String {
        let inherited = std::env::var("PATH").unwrap_or_default();
        format!("{}:{inherited}", self.bin.display())
    }
}

/// The profile store's helpers. Fixtures are built by hand here, exactly the way
/// the spec's testing seam describes: profiles are files on disk, and the
/// `current` symlink is flipped by writing a symlink.
impl Sandbox {
    /// The fake `$HOME`, for building fixtures beside the store.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// `~/.local/share/riceswap`.
    pub fn data_dir(&self) -> PathBuf {
        self.home.join(".local").join("share").join("riceswap")
    }

    /// `~/.local/share/riceswap/profiles`.
    pub fn profiles_dir(&self) -> PathBuf {
        self.data_dir().join("profiles")
    }

    /// `~/.local/share/riceswap/profiles/<name>`.
    pub fn profile_dir(&self, name: &str) -> PathBuf {
        self.profiles_dir().join(name)
    }

    /// `~/.local/share/riceswap/current`.
    pub fn current_link(&self) -> PathBuf {
        self.data_dir().join("current")
    }

    /// Writes a profile directory with `manifest` as its `profile.toml`.
    pub fn write_profile(&self, name: &str, manifest: &str) -> PathBuf {
        let path = self.profile_dir(name).join("profile.toml");
        fs::create_dir_all(self.profile_dir(name)).expect("create profile directory");
        fs::write(&path, manifest).expect("write profile.toml");
        path
    }

    /// Points `current` at `profiles/<name>`, the way a flip does.
    pub fn activate(&self, name: &str) -> PathBuf {
        self.point_current_at(Path::new("profiles").join(name))
    }

    /// Points `current` at an arbitrary target, for dangling and outside cases.
    pub fn point_current_at(&self, target: impl AsRef<Path>) -> PathBuf {
        let link = self.current_link();
        fs::create_dir_all(self.data_dir()).expect("create data directory");
        let _ = fs::remove_file(&link);
        symlink(target.as_ref(), &link).expect("create current symlink");
        link
    }

    /// What `current` points at, read independently of the backend.
    pub fn current_target(&self) -> Option<PathBuf> {
        fs::read_link(self.current_link()).ok()
    }
}

/// A complete, valid manifest for `name`: every locked field filled in, so a
/// test can break one thing and leave the rest honest. The display name is
/// distinct from the directory name on purpose — the directory is the id.
pub fn manifest_toml(name: &str) -> String {
    format!(
        r#"manifest_version = 1

[profile]
name = "{name} rice"
description = "fixture rice for {name}"
created_at = "2026-09-21T10:30:00Z"
updated_at = "2026-09-22T08:00:00Z"
screenshot = "screenshot.png"
source_url = "https://example.invalid/{name}"
source_commit = "abc123"

[packages]
official = ["waybar", "kitty"]
aur = ["ags"]

[[services]]
name = "ags"
start = "ags"
stop = "pkill ags"

[[files]]
path = ".config/hypr"

[[files]]
path = ".zshrc"
optional = true

[rice_info]
bar = "ags"
terminal = "kitty"
colors = "matugen"
"#
    )
}

/// One finished invocation.
pub struct Run {
    pub stdout: String,
    pub stderr: String,
    pub lines: Vec<String>,
}

impl Run {
    fn new(output: Output) -> Run {
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(
            output.status.success(),
            "riceswap exited with {}\nstdout:\n{stdout}\nstderr:\n{stderr}",
            output.status
        );
        Run {
            lines: stdout.lines().map(str::to_string).collect(),
            stdout,
            stderr,
        }
    }

    /// The final stdout line, validated as the frozen envelope: every earlier
    /// line is a progress line, and exactly one envelope closes the stream.
    pub fn envelope(&self) -> Value {
        let Some((last, progress)) = self.lines.split_last() else {
            panic!("riceswap wrote no NDJSON\nstderr:\n{}", self.stderr);
        };
        for line in progress {
            let value: Value = serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("progress line {line:?} is not JSON: {error}"));
            assert!(
                value.get("progress").is_some(),
                "only the final line may carry the envelope, found {line:?}"
            );
        }
        let envelope: Value = serde_json::from_str(last)
            .unwrap_or_else(|error| panic!("final line {last:?} is not JSON: {error}"));
        let object = envelope
            .as_object()
            .unwrap_or_else(|| panic!("final line is not an object: {last:?}"));
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["data", "ok", "warnings"],
            "envelope keys are frozen, got {last:?}"
        );
        envelope
    }

    /// Asserts `ok: true` and returns the `data` payload.
    pub fn assert_ok(&self) -> Value {
        let envelope = self.envelope();
        assert_eq!(
            envelope["ok"], true,
            "expected ok envelope, got {}\nstderr:\n{}",
            self.stdout, self.stderr
        );
        envelope["data"].clone()
    }

    /// Asserts `ok: false` and returns the message carried in `data.error`.
    pub fn assert_failed(&self) -> String {
        let envelope = self.envelope();
        assert_eq!(
            envelope["ok"], false,
            "expected a failed envelope, got {}\nstderr:\n{}",
            self.stdout, self.stderr
        );
        envelope["data"]["error"]
            .as_str()
            .unwrap_or_else(|| panic!("failed envelope carries no data.error: {}", self.stdout))
            .to_string()
    }

    /// The progress lines that preceded the envelope.
    pub fn progress(&self) -> Vec<Value> {
        self.lines[..self.lines.len() - 1]
            .iter()
            .map(|line| {
                serde_json::from_str::<Value>(line).expect("progress line is JSON")["progress"]
                    .clone()
            })
            .collect()
    }

    /// The warnings the envelope carried.
    pub fn warnings(&self) -> Vec<String> {
        self.envelope()["warnings"]
            .as_array()
            .expect("warnings is a list")
            .iter()
            .map(|warning| warning.as_str().expect("warning is a string").to_string())
            .collect()
    }

    /// The invocation never unwound into a crash.
    pub fn assert_no_panic(&self) {
        assert!(
            !self.stderr.contains("panicked at"),
            "backend panicked:\n{}",
            self.stderr
        );
    }
}

fn write_stub(bin: &Path, tool: &str) {
    let path = bin.join(tool);
    fs::write(&path, STUB).expect("write stub");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod stub");
}
