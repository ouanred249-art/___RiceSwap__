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
use std::process::{Child, Output, Stdio};
use tempfile::TempDir;

/// Executables the backend shells out to, stubbed on `PATH` for every test.
/// `pkexec` and `riceswap-float` are the privilege-flow wrappers of ticket
/// #16: every official package op runs `pkexec pacman`, every AUR helper run
/// goes through the floating-terminal wrapper.
pub const STUB_TOOLS: &[&str] = &[
    "pacman",
    "yay",
    "paru",
    "hyprctl",
    "grim",
    "pkexec",
    "riceswap-float",
];

/// Service commands the fixture manifests run (`start`/`stop`), stubbed on
/// `PATH` beside [`STUB_TOOLS`] so a switch can stop and start services
/// end-to-end. Not probed by `detect`: they are fixtures, not tools.
pub const SERVICE_STUBS: &[&str] = &["pkill", "ags", "waybar"];

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
/// watched `state.json` if it exists, honours a scripted delay, then answers
/// per its scripted mode.
///
/// Scripted behaviour beyond versioning, so the snapshot pipeline and the
/// switch sequence can both run end-to-end:
/// - `pacman -Qo` reports the owning package for any query the
///   `RICESWAP_STUB_OWNERS` fixture maps (by exact name or by basename, so a
///   PATH-resolved path answers too) and exits 1 for anything unowned;
/// - `pacman -Qm` lists the fixture entries marked `aur`;
/// - `grim <path>` writes the screenshot file it was pointed at;
/// - `pkexec` and `riceswap-float` answer `--version` like any other tool,
///   then run the command they were pointed at — `pkexec pacman ...` executes
///   pacman, `riceswap-float <helper> ...` runs the AUR helper — so the
///   fixtures underneath decide the outcome; a scripted failure mode is the
///   polkit denial / dead floating terminal;
/// - a rule in `RICESWAP_STUB_FAIL_DIR/<tool>` fails just the invocations
///   whose arguments contain the pattern (see `fail_on`), leaving
///   `--version` probes answerable — a helper that detects cleanly and then
///   fails inside its real transaction;
/// - `-S` installs: a rule in `RICESWAP_STUB_CONFLICTS`
///   (`<installed> <installer>`) makes the install conflict — exit 2, message
///   naming both packages — while `<installed>` is still in the
///   `RICESWAP_STUB_PACKAGES` state; installed packages accumulate there,
///   and a successful install prints `installing <pkg>` per package — the
///   helper output the switch report carries;
/// - `-R` removes: a package listed in `RICESWAP_STUB_NEEDED` is refused with
///   a dependency error (exit 1, "breaks dependency ... required by ..."),
///   anything else leaves the state file.
const STUB: &str = r##"#!/bin/sh
name=${0##*/}
printf '%s %s\n' "$name" "$*" >> "$RICESWAP_STUB_LOG"
if [ -n "$RICESWAP_STUB_STATE_DIR" ] && [ -f "$HOME/.local/share/riceswap/state.json" ]; then
  cp "$HOME/.local/share/riceswap/state.json" "$RICESWAP_STUB_STATE_DIR/$name.json"
fi
if [ -n "$RICESWAP_STUB_DELAY_DIR" ] && [ -f "$RICESWAP_STUB_DELAY_DIR/$name" ]; then
  while read -r pattern seconds; do
    case " $* " in
      *"$pattern"*) sleep "$seconds"; break ;;
    esac
  done < "$RICESWAP_STUB_DELAY_DIR/$name"
fi
if [ -n "$RICESWAP_STUB_FAIL_DIR" ] && [ -r "$RICESWAP_STUB_FAIL_DIR/$name" ]; then
  while read -r pattern; do
    [ -n "$pattern" ] || continue
    case "$*" in
      *"$pattern"*)
        printf '%s: scripted failure for arguments containing `%s`\n' "$name" "$pattern" >&2
        exit 1 ;;
    esac
  done < "$RICESWAP_STUB_FAIL_DIR/$name"
fi
mode=ok
if [ -r "$RICESWAP_STUB_MODE_DIR/$name" ]; then read -r mode < "$RICESWAP_STUB_MODE_DIR/$name"; fi
if [ "$mode" = "ok" ]; then
  # The privilege wrappers run the command they were pointed at — pkexec
  # executing pacman, the floating-terminal wrapper running the AUR helper —
  # but still answer their own version probe.
  case "$name" in
    pkexec|riceswap-float)
      if [ "$1" != "--version" ]; then
        wrapped=$1
        shift
        exec "$wrapped" "$@"
      fi
      ;;
  esac
  if [ "$name" = "pacman" ] && [ "$1" = "-Qo" ]; then
    query=$2
    base=${query##*/}
    if [ -r "$RICESWAP_STUB_OWNERS" ]; then
      while read -r binary package origin; do
        if [ -n "$binary" ] && { [ "$query" = "$binary" ] || [ "$base" = "$binary" ]; }; then
          printf '%s is owned by %s 1.0.0-1\n' "$query" "$package"
          exit 0
        fi
      done < "$RICESWAP_STUB_OWNERS"
    fi
    printf "error: no possible owner found for '%s'\n" "$query" >&2
    exit 1
  fi
  if [ "$name" = "pacman" ] && [ "$1" = "-Qm" ]; then
    if [ -r "$RICESWAP_STUB_OWNERS" ]; then
      while read -r binary package origin; do
        if [ "$origin" = "aur" ]; then printf '%s 1.0.0-1\n' "$package"; fi
      done < "$RICESWAP_STUB_OWNERS"
    fi
    exit 0
  fi
  if [ "$name" = "grim" ] && [ "$1" != "--version" ]; then
    for argument in "$@"; do destination=$argument; done
    printf 'stub screenshot' > "$destination"
    exit 0
  fi
  case "$1" in
    -S*|-R*)
      packages=""
      for argument in "$@"; do
        case "$argument" in -*) ;; *) packages="$packages $argument" ;; esac
      done
      if [ "${1#-R}" != "$1" ]; then
        for package in $packages; do
          if [ -n "$RICESWAP_STUB_NEEDED" ] && [ -f "$RICESWAP_STUB_NEEDED" ] \
            && grep -Fxq "$package" "$RICESWAP_STUB_NEEDED"; then
            printf 'error: failed to prepare transaction (could not satisfy dependencies)\n' >&2
            printf "removing %s breaks dependency '%s' required by dependent\n" "$package" "$package" >&2
            exit 1
          fi
        done
        if [ -n "$RICESWAP_STUB_PACKAGES" ] && [ -f "$RICESWAP_STUB_PACKAGES" ]; then
          for package in $packages; do
            grep -Fxv "$package" "$RICESWAP_STUB_PACKAGES" > "$RICESWAP_STUB_PACKAGES.tmp" || true
            mv "$RICESWAP_STUB_PACKAGES.tmp" "$RICESWAP_STUB_PACKAGES"
          done
        fi
        exit 0
      fi
      if [ -n "$RICESWAP_STUB_CONFLICTS" ] && [ -f "$RICESWAP_STUB_CONFLICTS" ]; then
        for package in $packages; do
          while read -r blocked installer; do
            if [ -n "$installer" ] && [ "$installer" = "$package" ] \
              && [ -n "$RICESWAP_STUB_PACKAGES" ] && [ -f "$RICESWAP_STUB_PACKAGES" ] \
              && grep -Fxq "$blocked" "$RICESWAP_STUB_PACKAGES"; then
              printf 'error: failed to prepare transaction (conflicting dependencies)\n' >&2
              printf ':: %s and %s are in conflict\n' "$blocked" "$installer" >&2
              exit 2
            fi
          done < "$RICESWAP_STUB_CONFLICTS"
        done
      fi
      for package in $packages; do printf 'installing %s\n' "$package"; done
      if [ -n "$RICESWAP_STUB_PACKAGES" ]; then
        for package in $packages; do
          if [ ! -f "$RICESWAP_STUB_PACKAGES" ] || ! grep -Fxq "$package" "$RICESWAP_STUB_PACKAGES"; then
            printf '%s\n' "$package" >> "$RICESWAP_STUB_PACKAGES"
          fi
        done
      fi
      exit 0
      ;;
  esac
fi
case "$mode" in
  ok) printf '%s 1.0.0-stub\n' "$name"; exit 0 ;;
  conflict) printf '%s: stub conflict: conflicting package stub-conflict\n' "$name" >&2; exit 2 ;;
  *) printf '%s: stub failure (%s)\n' "$name" "$mode" >&2; exit 1 ;;
esac
"##;

/// A fake `$HOME`, a stub `PATH`, and the record of what the stubs saw.
pub struct Sandbox {
    _root: TempDir,
    home: PathBuf,
    bin: PathBuf,
    modes: PathBuf,
    states: PathBuf,
    stub_log: PathBuf,
    owners: PathBuf,
    conflicts: PathBuf,
    needed: PathBuf,
    packages: PathBuf,
    delays: PathBuf,
    fails: PathBuf,
}

impl Sandbox {
    pub fn new() -> Sandbox {
        let root = TempDir::new().expect("create sandbox root");
        let home = root.path().join("home");
        let bin = root.path().join("bin");
        let modes = root.path().join("modes");
        let states = root.path().join("state-snapshots");
        let stub_log = root.path().join("stub-invocations.log");
        let owners = root.path().join("package-owners.txt");
        let conflicts = root.path().join("package-conflicts.txt");
        let needed = root.path().join("still-needed.txt");
        let packages = root.path().join("installed-packages.txt");
        let delays = root.path().join("delays");
        let fails = root.path().join("fail-on");
        for dir in [&home, &bin, &modes, &states, &delays, &fails] {
            fs::create_dir_all(dir).expect("create sandbox directory");
        }
        for tool in STUB_TOOLS.iter().chain(SERVICE_STUBS) {
            write_stub(&bin, tool);
        }
        fs::write(&stub_log, "").expect("create stub log");
        fs::write(&owners, "").expect("create package owner fixture");
        fs::write(&conflicts, "").expect("create package conflict fixture");
        fs::write(&needed, "").expect("create still-needed fixture");
        fs::write(&packages, "").expect("create installed-package state");
        Sandbox {
            _root: root,
            home,
            bin,
            modes,
            states,
            stub_log,
            owners,
            conflicts,
            needed,
            packages,
            delays,
            fails,
        }
    }

    /// Scripts a stub's next answer. Sticky until scripted again.
    pub fn script(&self, tool: &str, mode: Mode) {
        assert!(
            STUB_TOOLS.contains(&tool) || SERVICE_STUBS.contains(&tool),
            "unknown stub tool {tool}"
        );
        fs::write(self.modes.join(tool), format!("{}\n", mode.as_str())).expect("script stub mode");
    }

    /// Declares a pacman conflict: installing `installer` fails while
    /// `installed` is still installed — the install-first fallback fixture.
    /// `installed` enters the stub's installed-package state with the rule.
    pub fn declare_conflict(&self, installed: &str, installer: &str) {
        let mut conflicts = fs::read_to_string(&self.conflicts).expect("read conflict fixture");
        conflicts.push_str(&format!("{installed} {installer}\n"));
        fs::write(&self.conflicts, conflicts).expect("write conflict fixture");
        self.mark_installed(installed);
    }

    /// Declares a package pacman must refuse to remove: a plain `-R` answers
    /// with the "still needed" dependency error instead of removing it.
    pub fn declare_still_needed(&self, package: &str) {
        let mut needed = fs::read_to_string(&self.needed).expect("read still-needed fixture");
        needed.push_str(&format!("{package}\n"));
        fs::write(&self.needed, needed).expect("write still-needed fixture");
    }

    /// Records `package` as installed in the stub's state, so a conflict rule
    /// that names it actually fires.
    pub fn mark_installed(&self, package: &str) {
        if !self.installed_packages().iter().any(|p| p == package) {
            let mut state = fs::read_to_string(&self.packages).expect("read package state");
            state.push_str(&format!("{package}\n"));
            fs::write(&self.packages, state).expect("write package state");
        }
    }

    /// Every package the stub state holds as installed, in file order.
    pub fn installed_packages(&self) -> Vec<String> {
        fs::read_to_string(&self.packages)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Makes `tool` sleep `seconds` whenever its arguments contain `pattern`
    /// — the window a test sends SIGTERM into mid-step.
    pub fn delay_on(&self, tool: &str, pattern: &str, seconds: u64) {
        assert!(
            STUB_TOOLS.contains(&tool) || SERVICE_STUBS.contains(&tool),
            "unknown stub tool {tool}"
        );
        let path = self.delays.join(tool);
        let mut delays = fs::read_to_string(&path).unwrap_or_default();
        delays.push_str(&format!("{pattern} {seconds}\n"));
        fs::write(&path, delays).expect("write delay fixture");
    }

    /// Makes `tool` fail (exit 1, message on stderr) any invocation whose
    /// arguments contain `pattern`, while `--version` probes keep answering —
    /// so a tool can be *detected* and then fail inside its real transaction.
    /// Sticky until cleared by re-scripting the tool's mode.
    pub fn fail_on(&self, tool: &str, pattern: &str) {
        assert!(
            STUB_TOOLS.contains(&tool) || SERVICE_STUBS.contains(&tool),
            "unknown stub tool {tool}"
        );
        let path = self.fails.join(tool);
        let mut fails = fs::read_to_string(&path).unwrap_or_default();
        fails.push_str(&format!("{pattern}\n"));
        fs::write(&path, fails).expect("write fail-on fixture");
    }

    /// Truncates the invocation log, so a test can assert on one operation's
    /// stub traffic alone.
    pub fn clear_log(&self) {
        fs::write(&self.stub_log, "").expect("clear stub log");
    }

    /// Records that `pacman -Qo` resolves `binary` to `package`, and (when
    /// `aur`) that `pacman -Qm` lists it as foreign — the fixture the owner
    /// stub answers the binary-reference scan from.
    pub fn own(&self, binary: &str, package: &str, aur: bool) {
        let origin = if aur { "aur" } else { "official" };
        let mut owners = fs::read_to_string(&self.owners).expect("read owner fixture");
        owners.push_str(&format!("{binary} {package} {origin}\n"));
        fs::write(&self.owners, owners).expect("write owner fixture");
    }

    /// The configured subprocess: clean environment, sandboxed `$HOME` and
    /// `PATH`, stub fixtures wired in. `run` and `spawn` both start here, so
    /// a spawned operation sees exactly what a blocking one sees.
    fn command(&self) -> Command {
        let mut command = Command::cargo_bin("riceswap").expect("riceswap binary is built");
        command
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", self.path_env())
            .env("RICESWAP_STUB_LOG", &self.stub_log)
            .env("RICESWAP_STUB_MODE_DIR", &self.modes)
            .env("RICESWAP_STUB_STATE_DIR", &self.states)
            .env("RICESWAP_STUB_OWNERS", &self.owners)
            .env("RICESWAP_STUB_CONFLICTS", &self.conflicts)
            .env("RICESWAP_STUB_NEEDED", &self.needed)
            .env("RICESWAP_STUB_PACKAGES", &self.packages)
            .env("RICESWAP_STUB_DELAY_DIR", &self.delays)
            .env("RICESWAP_STUB_FAIL_DIR", &self.fails);
        command
    }

    /// Runs the binary with `args` under the sandbox, to completion.
    pub fn run<S: AsRef<OsStr>>(&self, args: &[S]) -> Run {
        let output = self.command().args(args).output().expect("run riceswap");
        Run::new(output)
    }

    /// Spawns the binary with `args` under the sandbox, stdout and stderr
    /// piped — for tests that must signal the operation mid-switch and keep
    /// reading its NDJSON stream.
    ///
    /// Uses `std::process::Command` directly (not `assert_cmd::Command`)
    /// because `assert_cmd` doesn't expose `.stdout()` / `.spawn()`.
    pub fn spawn<S: AsRef<OsStr>>(&self, args: &[S]) -> Child {
        let bin = assert_cmd::cargo::cargo_bin("riceswap");
        std::process::Command::new(bin)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", self.path_env())
            .env("RICESWAP_STUB_LOG", &self.stub_log)
            .env("RICESWAP_STUB_MODE_DIR", &self.modes)
            .env("RICESWAP_STUB_STATE_DIR", &self.states)
            .env("RICESWAP_STUB_OWNERS", &self.owners)
            .env("RICESWAP_STUB_CONFLICTS", &self.conflicts)
            .env("RICESWAP_STUB_NEEDED", &self.needed)
            .env("RICESWAP_STUB_PACKAGES", &self.packages)
            .env("RICESWAP_STUB_DELAY_DIR", &self.delays)
            .env("RICESWAP_STUB_FAIL_DIR", &self.fails)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn riceswap")
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

    /// Writes a file inside the fake `$HOME` (creating its directories) and
    /// returns its path — fixtures for `hyprland.conf`, images, anything.
    pub fn write_home(&self, relative: &str, contents: impl AsRef<[u8]>) -> PathBuf {
        let path = self.home.join(relative);
        fs::create_dir_all(path.parent().expect("fixture has a parent"))
            .expect("create fixture dir");
        fs::write(&path, contents.as_ref()).expect("write fixture");
        path
    }

    /// Every path under the fake `$HOME` with its content, in a stable order:
    /// files as their bytes, directories as a marker, symlinks as their target.
    /// `state.json` is excluded — it records progress and legitimately changes
    /// on every run.
    pub fn home_tree(&self) -> Vec<(String, Vec<u8>)> {
        let mut entries = Vec::new();
        collect_tree(&self.home, &self.home, &mut entries);
        entries.sort();
        entries
    }

    /// `~/.local/share/riceswap/wallpapers`.
    pub fn wallpapers_dir(&self) -> PathBuf {
        self.data_dir().join("wallpapers")
    }

    /// `~/.local/share/riceswap/hardware.conf`, the shared hardware file.
    pub fn hardware_file(&self) -> PathBuf {
        self.data_dir().join("hardware.conf")
    }

    /// `~/.config/hypr/hyprland.conf`, the live Hyprland config.
    pub fn hyprland_config(&self) -> PathBuf {
        self.home.join(".config").join("hypr").join("hyprland.conf")
    }

    /// `~/.config/hypr/riceswap/hardware.conf`, the permanent symlink.
    pub fn hardware_link(&self) -> PathBuf {
        self.home
            .join(".config")
            .join("hypr")
            .join("riceswap")
            .join("hardware.conf")
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

    /// Writes a file inside a profile's mirrored `$HOME` layout, so switch
    /// tests have real content for the managed symlinks to point at.
    pub fn write_profile_file(
        &self,
        profile: &str,
        relative: &str,
        contents: impl AsRef<[u8]>,
    ) -> PathBuf {
        let path = self.profile_dir(profile).join(relative);
        fs::create_dir_all(path.parent().expect("profile file has a parent"))
            .expect("create profile subdirectory");
        fs::write(&path, contents.as_ref()).expect("write profile file");
        path
    }

    /// The raw `profile.toml` text a profile holds on disk, for asserting on
    /// exactly what `snapshot` wrote — before the loader normalizes anything.
    pub fn profile_manifest(&self, name: &str) -> String {
        let path = self.profile_dir(name).join("profile.toml");
        fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
    }

    /// Points `current` at `profiles/<name>`, the way a flip does.
    pub fn activate(&self, name: &str) -> PathBuf {
        self.point_current_at(self.profile_dir(name))
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

/// A real image fixture: one of the bundled default wallpapers, so an import
/// test moves actual PNG bytes around.
pub fn image_fixture() -> &'static [u8] {
    include_bytes!("../../assets/wallpapers/default-dawn.png")
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

/// A manifest with the packages, services, and managed files a test's
/// profiles declare — the switch/plan fixture builder: two calls with
/// different arguments are two profiles with a computable diff.
pub fn profile_toml(
    name: &str,
    official: &[&str],
    aur: &[&str],
    services: &[(&str, &str, &str)],
    files: &[&str],
) -> String {
    fn list(items: &[&str]) -> String {
        let quoted: Vec<String> = items.iter().map(|item| format!("\"{item}\"")).collect();
        format!("[{}]", quoted.join(", "))
    }

    let mut manifest = format!(
        "manifest_version = 1\n\
         \n\
         [profile]\n\
         name = \"{name}\"\n\
         description = \"fixture rice for {name}\"\n\
         created_at = \"2026-09-21T10:30:00Z\"\n\
         updated_at = \"2026-09-22T08:00:00Z\"\n\
         screenshot = \"\"\n\
         \n\
         [packages]\n\
         official = {official}\n\
         aur = {aur}\n",
        official = list(official),
        aur = list(aur),
    );
    for (service, start, stop) in services {
        manifest.push_str(&format!(
            "\n[[services]]\nname = \"{service}\"\nstart = \"{start}\"\nstop = \"{stop}\"\n"
        ));
    }
    for file in files {
        manifest.push_str(&format!("\n[[files]]\npath = \"{file}\"\n"));
    }
    manifest
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

    /// A run assembled from a stream read line-by-line while the subprocess
    /// was alive — the shape a signalled, mid-switch operation comes back in.
    /// The caller has already asserted the exit status.
    pub fn from_parts(lines: Vec<String>, stderr: String) -> Run {
        let stdout = lines.join("\n");
        Run {
            lines,
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

/// Recursively collects `directory`'s entries relative to `root`: files as
/// their bytes, directories as a marker, symlinks as their target. Unreadable
/// entries are skipped — [`Sandbox::state`] reports state problems on its own.
fn collect_tree(root: &Path, directory: &Path, entries: &mut Vec<(String, Vec<u8>)>) {
    let Ok(read) = fs::read_dir(directory) else {
        return;
    };
    for entry in read.flatten() {
        let path = entry.path();
        let name = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.is_symlink() {
            let target = fs::read_link(&path)
                .map(|target| format!("-> {}", target.display()))
                .unwrap_or_else(|error| format!("-> unreadable: {error}"));
            entries.push((name, target.into_bytes()));
        } else if metadata.is_dir() {
            entries.push((format!("{name}/"), b"<dir>".to_vec()));
            collect_tree(root, &path, entries);
        } else if name != ".local/share/riceswap/state.json" {
            entries.push((name, fs::read(&path).unwrap_or_default()));
        }
    }
}
