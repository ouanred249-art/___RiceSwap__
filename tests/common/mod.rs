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
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Child, Output, Stdio};
use tempfile::TempDir;

/// Executables the backend shells out to, stubbed on `PATH` for every test.
/// `pkexec` and `riceswap-float` are the privilege-flow wrappers of ticket
/// #16: every official package op runs `pkexec pacman`, every AUR helper run
/// goes through the floating-terminal wrapper. `git` and `pi` are the
/// acquisition and research tools of the installer (ticket #34): probed like
/// every other tool, so a machine missing either is visible before any real
/// work starts.
pub const STUB_TOOLS: &[&str] = &[
    "pacman",
    "yay",
    "paru",
    "hyprctl",
    "grim",
    "pkexec",
    "riceswap-float",
    "git",
    "pi",
];

/// Tools the sandbox provides but `detect` does not probe: not `Tool`
/// variants, so they carry no version contract. `sudo` is the switch's
/// fallback path when a `pkexec` transaction is refused; a test scripts its
/// mode to make that path succeed (`Ok`, the default) or fail (`Denied`).
/// `ydotool` is the synthetic-input tool the functional verification tier
/// (#40) drives, so it has graduated from a seam to a tool the switch itself
/// runs: every probe's `key` and `type` steps go through it, and `ydotool help`
/// is how the tier decides it can drive this session at all. It stays off the
/// `detect` roster — `tests/harness.rs` asserts that — because it has no
/// version contract of its own to probe against.
pub const FALLBACK_STUBS: &[&str] = &["sudo", "ydotool"];

/// Service commands the fixture manifests run (`start`/`stop`), stubbed on
/// `PATH` beside [`STUB_TOOLS`] so a switch can stop and start services
/// end-to-end. Not probed by `detect`: they are fixtures, not tools. `qs` is
/// here because a desktop shell *is* a service, and the shell-swap tests need
/// to see it run.
pub const SERVICE_STUBS: &[&str] = &["pkill", "ags", "waybar", "qs"];

/// How a stub executable answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Print a version and exit 0.
    Ok,
    /// Exit 1 with an error on stderr.
    Fail,
    /// Exit 2 with a conflict on stderr.
    Conflict,
    /// Exit 1 with pkexec's own refusal words — the answer a dead polkit
    /// agent gives every password, `Not authorized`. The `--version` probe
    /// still answers, so the tool is detected and then refused in its real
    /// transaction, which is exactly how the live incident went.
    Denied,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Mode::Ok => "ok",
            Mode::Fail => "fail",
            Mode::Conflict => "conflict",
            Mode::Denied => "denied",
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
/// - `pacman -T <names…>` is the dependency-satisfaction query `plan`'s
///   `install_missing` is computed against: it prints the names nothing on
///   the machine satisfies and exits 127 when there are any. A name is
///   satisfied by an installed package of that name, or by an installed
///   package the `RICESWAP_STUB_PROVIDES` fixture (`<provider> <provided>`)
///   says provides it — the way `matugen-bin` satisfies `matugen`;
/// - `grim <path>` writes the screenshot file it was pointed at (its version
///   probe is `grim -h`, which falls through to the generic answer);
/// - `pkexec`, `sudo` and `riceswap-float` answer `--version` like any other
///   tool, then run the command they were pointed at — `pkexec pacman ...`
///   executes pacman, `sudo pacman ...` executes pacman, `riceswap-float
///   <helper> ...` runs the AUR helper — so the fixtures underneath decide
///   the outcome; a scripted `Denied` mode on `pkexec` is the dead polkit
///   agent (real refusal words), a scripted failure elsewhere the dead
///   floating terminal;
/// - `git` is the acquisition seam, and the only stub that has to *do*
///   something rather than answer a version: `git clone <url> <dest>` copies
///   the tree named by `RICESWAP_STUB_GIT_SOURCE` into the destination and
///   leaves a `.git` behind (a clone is a checkout), and `git -C <dir>
///   rev-parse HEAD` prints the sha in `RICESWAP_STUB_GIT_SHA` when `<dir>`
///   carries that `.git` and fails the way git does when it does not — a clone
///   that copied nothing would leave the install nothing to read, and a
///   rev-parse that answered for any directory would put a commit in the
///   manifest of a tree that has none;
/// - `pi` is the research seam (ticket #38). Its version probe answers like
///   any other tool; `pi auth check …` answers from
///   `RICESWAP_STUB_PI_AUTH`; and every other invocation is a *research run*,
///   which appends its working directory and its flags — everything before the
///   `--` that ends the brief — to `RICESWAP_STUB_PI_LOG`, one line per
///   invocation, which is how a test counts invocations and asserts that the
///   repair pass ran without `--tools`. The run then writes the stream named by
///   `RICESWAP_STUB_PI_STREAM` to stdout and exits with the code in
///   `RICESWAP_STUB_PI_EXIT`. The stream is whatever JSONL the test wants: a
///   good answer, a refusal inside a well-formed stream, prose, or nothing at
///   all. No test in this file reaches a network, and the real `pi` is never on
///   the sandbox `PATH`;
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
if [ "$name" = "pi" ]; then
  # The research seam (ticket #38). Three kinds of invocation, three answers:
  # the version probe, the advisory auth check, and the research run itself.
  # A scripted mode still wins, so `pi` can be made unusable the way every other
  # tool can — which is how the "no pi here" path is exercised.
  pimode=ok
  if [ -r "$RICESWAP_STUB_MODE_DIR/$name" ]; then read -r pimode < "$RICESWAP_STUB_MODE_DIR/$name"; fi
  case "$1" in
    --version)
      if [ "$pimode" = "ok" ]; then printf '0.87.1\n'; exit 0; fi ;;
    auth)
      if [ "$pimode" = "ok" ]; then
        [ -r "$RICESWAP_STUB_PI_AUTH" ] && cat "$RICESWAP_STUB_PI_AUTH"
        exit 0
      fi ;;
  esac
  if [ "$pimode" != "ok" ]; then
    printf '%s: stub failure (%s)\n' "$name" "$pimode" >&2
    exit 1
  fi
  # The record a research-tier test reads: the directory the run happened in,
  # and the flags it was given. The brief is deliberately not recorded — it is
  # a paragraph long, and a log line a test cannot read is a log line nobody
  # reads.
  flags=
  for argument in "$@"; do
    [ "$argument" = "--" ] && break
    flags="$flags $argument"
  done
  printf 'cwd=%s flags:%s\n' "$PWD" "$flags" >> "$RICESWAP_STUB_PI_LOG"
  # The part of the stream a run emits before it stalls: printed first, so a
  # test can script a stream that opens and then goes quiet — the mid-stream arm
  # of the budget, as distinct from a run that never says anything.
  [ -r "$RICESWAP_STUB_PI_PREAMBLE" ] && cat "$RICESWAP_STUB_PI_PREAMBLE"
  if [ -n "$RICESWAP_STUB_DELAY_DIR" ] && [ -f "$RICESWAP_STUB_DELAY_DIR/pi" ]; then
    while read -r pattern seconds; do
      case "$flags" in
        *"$pattern"*) sleep "$seconds"; break ;;
      esac
    done < "$RICESWAP_STUB_DELAY_DIR/pi"
  fi
  [ -r "$RICESWAP_STUB_PI_STREAM" ] && cat "$RICESWAP_STUB_PI_STREAM"
  # A second delay rule, in its own file, hangs a run *after* its whole stream
  # is written: the process prints everything and then never exits, which is the
  # shape a consumer has to finish on `agent_settled` rather than on EOF.
  if [ -n "$RICESWAP_STUB_DELAY_DIR" ] && [ -f "$RICESWAP_STUB_DELAY_DIR/pi-settle" ]; then
    while read -r pattern seconds; do
      case "$flags" in
        *"$pattern"*) sleep "$seconds"; break ;;
      esac
    done < "$RICESWAP_STUB_DELAY_DIR/pi-settle"
  fi
  [ -r "$RICESWAP_STUB_PI_STDERR" ] && cat "$RICESWAP_STUB_PI_STDERR" >&2
  piexit=0
  [ -r "$RICESWAP_STUB_PI_EXIT" ] && read -r piexit < "$RICESWAP_STUB_PI_EXIT"
  exit "$piexit"
fi
if [ -n "$RICESWAP_STUB_DELAY_DIR" ] && [ -f "$RICESWAP_STUB_DELAY_DIR/$name" ]; then
  while read -r pattern seconds; do
    case " $* " in
      *"$pattern"*) sleep "$seconds"; break ;;
    esac
  done < "$RICESWAP_STUB_DELAY_DIR/$name"
fi
if [ -n "$RICESWAP_STUB_FAULT_DIR" ] && [ -r "$RICESWAP_STUB_FAULT_DIR/$name" ]; then
  # A fault the fake machine develops *while the operation is running*: the one
  # thing a real filesystem can do that a test otherwise cannot stage up front,
  # because it has to happen between two calls the same process makes. The
  # trigger is the first whitespace-delimited word of the line, matched anywhere
  # in the arguments; the action is the rest of the line, run by `sh`.
  while read -r trigger action; do
    [ -n "$trigger" ] && [ -n "$action" ] || continue
    case " $* " in
      *" $trigger "*) sh -c "$action" ;;
    esac
  done < "$RICESWAP_STUB_FAULT_DIR/$name"
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
    pkexec|sudo|riceswap-float)
      if [ "$1" != "--version" ]; then
        wrapped=$1
        shift
        exec "$wrapped" "$@"
      fi
      ;;
  esac
  if [ "$name" = "git" ]; then
    case "$1" in
      clone)
        destination=
        for argument in "$@"; do destination=$argument; done
        if [ ! -d "$destination" ]; then
          mkdir -p "$destination" || exit 1
        fi
        if [ -r "$RICESWAP_STUB_GIT_SOURCE" ]; then
          read -r origin < "$RICESWAP_STUB_GIT_SOURCE"
          if [ -n "$origin" ] && [ -d "$origin" ]; then
            cp -R "$origin/." "$destination/" || exit 1
          fi
        fi
        # A clone is a checkout: it leaves a `.git` behind, which is what makes
        # the very next `rev-parse` answer for this directory and not for some
        # unrelated one. A clone without it is not a tree the installer could
        # pin a commit to.
        mkdir -p "$destination/.git" || exit 1
        printf 'ref: refs/heads/stub\n' > "$destination/.git/HEAD" || exit 1
        exit 0 ;;
      -C)
        if [ -e "$2/.git" ]; then
          sha=
          if [ -r "$RICESWAP_STUB_GIT_SHA" ]; then read -r sha < "$RICESWAP_STUB_GIT_SHA"; fi
          [ -n "$sha" ] || sha=0000000000000000000000000000000000000000
          printf '%s\n' "$sha"
          exit 0
        fi
        printf 'fatal: not a git repository (or any of the parent directories): .git\n' >&2
        exit 128 ;;
    esac
  fi
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
  if [ "$name" = "pacman" ] && [ "$1" = "-T" ]; then
    shift
    unmet=0
    for wanted in "$@"; do
      satisfied=0
      if [ -f "$RICESWAP_STUB_PACKAGES" ] && grep -Fxq "$wanted" "$RICESWAP_STUB_PACKAGES"; then
        satisfied=1
      elif [ -r "$RICESWAP_STUB_PROVIDES" ]; then
        while read -r provider provided; do
          if [ "$provided" = "$wanted" ] && [ -f "$RICESWAP_STUB_PACKAGES" ] \
            && grep -Fxq "$provider" "$RICESWAP_STUB_PACKAGES"; then
            satisfied=1
          fi
        done < "$RICESWAP_STUB_PROVIDES"
      fi
      if [ "$satisfied" = 0 ]; then printf '%s\n' "$wanted"; unmet=1; fi
    done
    [ "$unmet" = 0 ] && exit 0
    exit 127
  fi
  if [ "$name" = "pacman" ] && [ "$1" = "-Qi" ]; then
    package=$2
    # Simulated `pacman -Qi`: packages listed in STUB_NEEDED are still
    # required, everything else is removable. This is the read-only query
    # `is_still_needed` uses to pre-filter removals without a prompt.
    if [ -n "$RICESWAP_STUB_NEEDED" ] && [ -f "$RICESWAP_STUB_NEEDED" ] \
      && grep -Fxq "$package" "$RICESWAP_STUB_NEEDED"; then
      printf 'Name            : %s\nVersion         : 1.0.0-1\nRequired By     : dependent\n' "$package"
    else
      printf 'Name            : %s\nVersion         : 1.0.0-1\nRequired By     : None\n' "$package"
    fi
    exit 0
  fi
  if [ "$name" = "pacman" ] && [ "$1" = "-Qm" ]; then
    if [ -r "$RICESWAP_STUB_OWNERS" ]; then
      while read -r binary package origin; do
        if [ "$origin" = "aur" ]; then printf '%s 1.0.0-1\n' "$package"; fi
      done < "$RICESWAP_STUB_OWNERS"
    fi
    exit 0
  fi
  if [ "$name" = "grim" ] && [ "$1" != "-h" ]; then
    for argument in "$@"; do destination=$argument; done
    # The screenshot seam for the functional tier (#40). Two calls write
    # different bytes, because a counter rides along in them, and the tier's
    # observation floor is "the encoding of that region is not what it was" —
    # a stub that wrote the same bytes every time would make every floor probe
    # fail for a reason that is the stub's and not the desktop's.
    #
    # `RICESWAP_STUB_GRIM_FROZEN` holds the call count up to which the bytes
    # are held still, which is how a test asks for a screen that does not
    # change (a probe that must fail) or for one that changes only after a
    # while (a probe that must flake and then pass its re-run). `-h` is the
    # version probe and still falls through to the generic answer above.
    shots=0
    if [ -r "$RICESWAP_STUB_SHOTS" ]; then read -r shots < "$RICESWAP_STUB_SHOTS"; fi
    shots=$((shots + 1))
    printf '%s\n' "$shots" > "$RICESWAP_STUB_SHOTS"
    frozen=0
    if [ -r "$RICESWAP_STUB_GRIM_FROZEN" ]; then read -r frozen < "$RICESWAP_STUB_GRIM_FROZEN"; fi
    if [ "$shots" -le "$frozen" ]; then
      printf 'stub screenshot\n' > "$destination"
    else
      printf 'stub screenshot %s\n' "$shots" > "$destination"
    fi
    exit 0
  fi
  if [ "$name" = "hyprctl" ] && [ "$1" = "globalshortcuts" ]; then
    # The live registry read (issue #39). A test that cares about the answer
    # scripts it; a test that does not gets the generic version banner below,
    # which carries no `appid:name` in it and is therefore read as "no registry
    # here" — the degrade the CI sandbox is meant to be in.
    [ -r "$RICESWAP_STUB_GLOBALSHORTCUTS" ] && cat "$RICESWAP_STUB_GLOBALSHORTCUTS"
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
  denied)
    if [ "$1" = "--version" ]; then printf '%s 1.0.0-stub\n' "$name"; exit 0; fi
    # pkexec's refusal lines, the ones a dead polkit agent leaves behind
    # after the user has typed their password into nothing.
    printf '==== AUTHENTICATING FOR org.freedesktop.policykit.exec ====\n' >&2
    printf '==== AUTHENTICATION FAILED ====\n' >&2
    printf 'Error executing command as another user: Not authorized\n\n' >&2
    printf 'This incident has been reported.\n' >&2
    exit 1 ;;
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
    provides: PathBuf,
    delays: PathBuf,
    fails: PathBuf,
    faults: PathBuf,
    git_source: PathBuf,
    git_sha: PathBuf,
    /// What `hyprctl globalshortcuts` answers, in the real command's own
    /// format. The default is an empty file, so the stub falls through to its
    /// generic version banner instead — the "no live registry here" the headless
    /// sandbox is.
    globalshortcuts: PathBuf,
    /// How many screenshots the `grim` stub has written, so two calls can write
    /// different bytes (the functional tier's observation floor) and so a test
    /// can count them.
    shots: PathBuf,
    /// The call count up to which the `grim` stub writes *the same* bytes every
    /// time: the fixture for a screen that does not change, and for one that
    /// only starts changing after a while.
    grim_frozen: PathBuf,
    /// One line per `pi` research run: the directory it ran in and its flags.
    pi_log: PathBuf,
    /// The JSONL stream the `pi` stub writes for a research run.
    pi_stream: PathBuf,
    /// What `pi auth check` answers, when a test wants an answer.
    pi_auth: PathBuf,
    /// The exit code the `pi` stub returns for a research run.
    pi_exit: PathBuf,
    /// What a `pi` research run prints on stderr, for the startup-failure arm.
    pi_stderr: PathBuf,
    /// What a `pi` research run writes to stdout before a scripted stall.
    pi_preamble: PathBuf,
    /// Environment variables a test adds for one operation, on top of the
    /// fixtures: the knobs with no fixture file behind them.
    extra_env: RefCell<BTreeMap<String, String>>,
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
        let provides = root.path().join("package-provides.txt");
        let delays = root.path().join("delays");
        let fails = root.path().join("fail-on");
        let faults = root.path().join("faults");
        let git_source = root.path().join("git-source.txt");
        let git_sha = root.path().join("git-sha.txt");
        let globalshortcuts = root.path().join("hyprctl-globalshortcuts.txt");
        let shots = root.path().join("grim-shots");
        let grim_frozen = root.path().join("grim-frozen");
        let pi_log = root.path().join("pi-invocations.log");
        let pi_stream = root.path().join("pi-stream.jsonl");
        let pi_auth = root.path().join("pi-auth.json");
        let pi_exit = root.path().join("pi-exit");
        let pi_stderr = root.path().join("pi-stderr.txt");
        let pi_preamble = root.path().join("pi-preamble.jsonl");
        for dir in [&home, &bin, &modes, &states, &delays, &fails, &faults] {
            fs::create_dir_all(dir).expect("create sandbox directory");
        }
        for tool in STUB_TOOLS.iter().chain(FALLBACK_STUBS).chain(SERVICE_STUBS) {
            write_stub(&bin, tool);
        }
        fs::write(&stub_log, "").expect("create stub log");
        fs::write(&owners, "").expect("create package owner fixture");
        fs::write(&conflicts, "").expect("create package conflict fixture");
        fs::write(&needed, "").expect("create still-needed fixture");
        fs::write(&packages, "").expect("create installed-package state");
        fs::write(&provides, "").expect("create package provides fixture");
        fs::write(&git_source, "").expect("create git clone-source fixture");
        fs::write(&git_sha, "1a2b3c4d5e6f70819a2b3c4d5e6f70819a2b3c4d\n")
            .expect("create git sha fixture");
        fs::write(&globalshortcuts, "").expect("create the live registry fixture");
        fs::write(&shots, "0\n").expect("create the grim shot counter");
        fs::write(&grim_frozen, "0\n").expect("create the grim frozen fixture");
        fs::write(&pi_log, "").expect("create pi invocation log");
        fs::write(&pi_stream, "").expect("create pi stream fixture");
        fs::write(&pi_auth, "").expect("create pi auth fixture");
        fs::write(&pi_exit, "0\n").expect("create pi exit fixture");
        fs::write(&pi_stderr, "").expect("create pi stderr fixture");
        fs::write(&pi_preamble, "").expect("create pi preamble fixture");
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
            provides,
            delays,
            fails,
            faults,
            git_source,
            git_sha,
            globalshortcuts,
            shots,
            grim_frozen,
            pi_log,
            pi_stream,
            pi_auth,
            pi_exit,
            pi_stderr,
            pi_preamble,
            extra_env: RefCell::new(BTreeMap::new()),
        }
    }

    /// Scripts a stub's next answer. Sticky until scripted again.
    pub fn script(&self, tool: &str, mode: Mode) {
        assert!(
            STUB_TOOLS.contains(&tool)
                || FALLBACK_STUBS.contains(&tool)
                || SERVICE_STUBS.contains(&tool),
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

    /// Declares that the installed package `provider` satisfies the name
    /// `provided` without being named that — `matugen-bin` for `matugen`.
    /// The provider must also be installed (`mark_installed`) to count.
    pub fn provide(&self, provider: &str, provided: &str) {
        let mut provides = fs::read_to_string(&self.provides).expect("read provides fixture");
        provides.push_str(&format!("{provider} {provided}\n"));
        fs::write(&self.provides, provides).expect("write provides fixture");
    }

    /// Every package the stub state holds as installed, in file order.
    pub fn installed_packages(&self) -> Vec<String> {
        fs::read_to_string(&self.packages)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Makes a `pi` research run hang for `seconds` *after* it has written its
    /// whole stream and before it exits.
    ///
    /// This is the arm the `agent_settled` record exists for: a run that has
    /// said everything it is going to say and then lingers has to be finished on
    /// that record, not waited out to the process's exit or the stall budget.
    /// Matched on the flags as `delay_on` does, so a test can scope it to one
    /// kind of run.
    pub fn delay_after(&self, pattern: &str, seconds: u64) {
        let path = self.delays.join("pi-settle");
        let mut delays = fs::read_to_string(&path).unwrap_or_default();
        delays.push_str(&format!("{pattern} {seconds}\n"));
        fs::write(&path, delays).expect("write the settle-delay fixture");
    }

    /// Makes `tool` sleep `seconds` whenever its arguments contain `pattern`
    /// — the window a test sends SIGTERM into mid-step.
    pub fn delay_on(&self, tool: &str, pattern: &str, seconds: u64) {
        assert!(
            STUB_TOOLS.contains(&tool)
                || FALLBACK_STUBS.contains(&tool)
                || SERVICE_STUBS.contains(&tool),
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
            STUB_TOOLS.contains(&tool)
                || FALLBACK_STUBS.contains(&tool)
                || SERVICE_STUBS.contains(&tool),
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

    /// Points the `git` stub's clone at `tree`: the bytes a
    /// `git clone <url> <dest>` produces. Re-pointable, so one sandbox can
    /// serve a repo that changes between two installs.
    pub fn clone_from(&self, tree: &Path) {
        fs::write(&self.git_source, format!("{}\n", tree.display())).expect("write clone source");
    }

    // ------------------------------------------- the live registry (issue #39)

    /// Scripts what `hyprctl globalshortcuts` answers: the registrations the
    /// live session is claimed to have, as `appid:name`.
    ///
    /// Rendered in the real command's own `bind =` lines, because parsing the
    /// real format is half of what the verification tier does and a stub that
    /// answered some private shape would not be testing it.
    pub fn live_registry(&self, entries: &[&str]) {
        let mut answer = String::from("globalshortcuts:\n");
        for entry in entries {
            let (appid, name) = entry.split_once(':').unwrap_or((*entry, ""));
            answer.push_str(&format!(
                "bind = SUPER, F{hash}, global, {appid}:{name}, {name} ({appid})\n",
                hash = name.len(),
            ));
        }
        fs::write(&self.globalshortcuts, answer).expect("write the live registry fixture");
    }

    /// Scripts `hyprctl globalshortcuts` to answer with `answer` verbatim, for
    /// the shapes a rendered registry cannot express: a command that fails, a
    /// session-less answer, a banner with nothing in it.
    pub fn live_registry_answers(&self, answer: &str) {
        fs::write(&self.globalshortcuts, answer).expect("write the live registry fixture");
    }

    // --------------------------------------- the screenshot seam (issue #40)

    /// How many screenshots the `grim` stub has written so far.
    pub fn shots(&self) -> u32 {
        fs::read_to_string(&self.shots)
            .ok()
            .and_then(|count| count.trim().parse().ok())
            .unwrap_or(0)
    }

    /// A screen that never changes: every `grim` call writes the same bytes,
    /// which is the only honest way to ask the functional tier to fail — a
    /// probe that never sees its expected outcome.
    pub fn screen_never_changes(&self) {
        fs::write(&self.grim_frozen, "1000000\n").expect("freeze the fake screen");
    }

    /// A screen that starts changing only after `calls` screenshots. The
    /// functional tier's flake case: the first run of a probe sees nothing, and
    /// the re-run sees something, which is exactly what the one-retry policy
    /// exists for.
    pub fn screen_changes_after(&self, calls: u32) {
        fs::write(&self.grim_frozen, format!("{calls}\n")).expect("freeze the fake screen");
    }

    /// Makes the fake machine do `action` whenever `tool` is invoked with
    /// arguments containing `pattern`.
    ///
    /// This is the one fault a test cannot stage before the operation starts,
    /// because it has to happen *between* two calls the same process makes —
    /// removing the profile a rollback is about to point `current` back at, for
    /// instance, which only exists as a fault once the forward flip is done.
    /// Sticky until scripted again.
    pub fn fault_on(&self, tool: &str, pattern: &str, action: &str) {
        assert!(
            STUB_TOOLS.contains(&tool)
                || FALLBACK_STUBS.contains(&tool)
                || SERVICE_STUBS.contains(&tool),
            "unknown stub tool {tool}"
        );
        let path = self.faults.join(tool);
        let mut faults = fs::read_to_string(&path).unwrap_or_default();
        faults.push_str(&format!("{pattern} {action}\n"));
        fs::write(&path, faults).expect("write the fault fixture");
    }

    /// The sha the `git` stub answers `rev-parse HEAD` with, for the checkouts
    /// it is told about.
    pub fn git_reports_commit(&self, sha: &str) {
        fs::write(&self.git_sha, format!("{sha}\n")).expect("write git sha fixture");
    }

    /// A path beside the fake `$HOME`, for fixtures that must not live inside
    /// it: an acquired rice a user hands the installer in, for instance.
    pub fn outside_home(&self, relative: &str) -> PathBuf {
        self._root.path().join(relative)
    }

    // ---------------------------------------------------- the research seam

    /// Scripts what `pi auth check --json` answers. An empty fixture — the
    /// default — makes the stub print nothing, which the backend reads as
    /// "unreadable, carry on" rather than as a refusal.
    pub fn pi_auth_says(&self, answer: &str) {
        fs::write(&self.pi_auth, answer).expect("write pi auth fixture");
    }

    /// Scripts the JSONL stream a `pi` research run writes to stdout.
    ///
    /// Every line must be a complete record: the stub is a `cat`, not a
    /// simulator of pi's streaming behaviour, because what a test needs to
    /// control is the *content* of the stream, not how it arrives.
    pub fn pi_stream(&self, stream: &str) {
        fs::write(&self.pi_stream, stream).expect("write pi stream fixture");
    }

    /// The exit code a `pi` research run returns — the arm of the contract's
    /// trap that is a startup failure (exit 1, empty stdout, `Error: Model …`
    /// on stderr) rather than a refusal inside the stream.
    pub fn pi_exits(&self, code: i32) {
        fs::write(&self.pi_exit, format!("{code}\n")).expect("write pi exit fixture");
    }

    /// Makes a `pi` research run print `text` on stderr before exiting — the
    /// channel pi's own startup failures use.
    pub fn pi_says(&self, text: &str) {
        fs::write(&self.pi_stderr, text).expect("write pi stderr fixture");
    }

    /// The records a `pi` research run writes to stdout *before* a scripted
    /// stall, so a test can open a stream and then let it go quiet.
    pub fn pi_preamble(&self, records: &str) {
        fs::write(&self.pi_preamble, records).expect("write pi preamble fixture");
    }

    /// Every `pi` research run, in order, as `cwd` plus the flags it was given.
    pub fn pi_runs(&self) -> Vec<String> {
        fs::read_to_string(&self.pi_log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Whether any `pi` research run was given `needle` among its flags.
    pub fn pi_saw(&self, needle: &str) -> bool {
        self.pi_runs().iter().any(|run| run.contains(needle))
    }

    /// Shrinks the research budgets so the timeout path is milliseconds rather
    /// than minutes.
    ///
    /// The variables are the ones the tier documents, which is the point: a
    /// test that wants the first-byte arm shrinks `first_byte` and leaves
    /// `total` alone, and the production defaults are untouched by anything
    /// here.
    pub fn research_budget(&self, first_byte: u64, total: u64) -> &Sandbox {
        self.set_env("RICESWAP_PI_FIRST_BYTE_SECONDS", &first_byte.to_string());
        self.set_env("RICESWAP_PI_TOTAL_SECONDS", &total.to_string());
        self
    }

    /// Sets one extra environment variable for the operations this sandbox runs,
    /// for the knobs a test has to reach that have no fixture file.
    pub fn set_env(&self, name: &str, value: &str) {
        self.extra_env
            .borrow_mut()
            .insert(name.to_string(), value.to_string());
    }

    /// Every fixture variable the stubs read, in one place.
    ///
    /// `run` and `spawn` both start here, so a spawned operation sees exactly
    /// what a blocking one sees — and a fixture added in one place cannot be
    /// forgotten in the other.
    fn envs(&self) -> Vec<(&'static str, &Path)> {
        vec![
            ("RICESWAP_STUB_LOG", &self.stub_log),
            ("RICESWAP_STUB_MODE_DIR", &self.modes),
            ("RICESWAP_STUB_STATE_DIR", &self.states),
            ("RICESWAP_STUB_OWNERS", &self.owners),
            ("RICESWAP_STUB_CONFLICTS", &self.conflicts),
            ("RICESWAP_STUB_NEEDED", &self.needed),
            ("RICESWAP_STUB_PACKAGES", &self.packages),
            ("RICESWAP_STUB_PROVIDES", &self.provides),
            ("RICESWAP_STUB_DELAY_DIR", &self.delays),
            ("RICESWAP_STUB_FAIL_DIR", &self.fails),
            ("RICESWAP_STUB_FAULT_DIR", &self.faults),
            ("RICESWAP_STUB_GIT_SOURCE", &self.git_source),
            ("RICESWAP_STUB_GIT_SHA", &self.git_sha),
            ("RICESWAP_STUB_GLOBALSHORTCUTS", &self.globalshortcuts),
            ("RICESWAP_STUB_SHOTS", &self.shots),
            ("RICESWAP_STUB_GRIM_FROZEN", &self.grim_frozen),
            ("RICESWAP_STUB_PI_LOG", &self.pi_log),
            ("RICESWAP_STUB_PI_STREAM", &self.pi_stream),
            ("RICESWAP_STUB_PI_AUTH", &self.pi_auth),
            ("RICESWAP_STUB_PI_EXIT", &self.pi_exit),
            ("RICESWAP_STUB_PI_STDERR", &self.pi_stderr),
            ("RICESWAP_STUB_PI_PREAMBLE", &self.pi_preamble),
        ]
    }

    /// The configured subprocess: clean environment, sandboxed `$HOME` and
    /// `PATH`, stub fixtures wired in. `run` and `spawn` both start here, so
    /// a spawned operation sees exactly what a blocking one sees.
    fn command(&self) -> Command {
        let mut command = Command::cargo_bin("riceswap").expect("riceswap binary is built");
        command
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", self.path_env());
        for (name, path) in self.envs() {
            command.env(name, path);
        }
        for (name, value) in self.extra_env.borrow().iter() {
            command.env(name, value);
        }
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
        let mut command = std::process::Command::new(bin);
        command
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", self.path_env());
        for (name, path) in self.envs() {
            command.env(name, path);
        }
        for (name, value) in self.extra_env.borrow().iter() {
            command.env(name, value);
        }
        command
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

    /// The stub log's path, for a test that needs the raw text (ordering, say)
    /// rather than a membership answer.
    pub fn log_path(&self) -> PathBuf {
        self.stub_log.clone()
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

/// `profile_toml` plus a `[shell]` table, for the fixtures whose whole point
/// is that the two profiles own different shells.
pub fn profile_toml_with_shell(
    name: &str,
    official: &[&str],
    aur: &[&str],
    services: &[(&str, &str, &str)],
    files: &[&str],
    shell: (&str, &str, &str),
) -> String {
    let (shell_name, start, stop) = shell;
    format!(
        "{}\n[shell]\nname = \"{shell_name}\"\nstart = \"{start}\"\nstop = \"{stop}\"\n",
        profile_toml(name, official, aur, services, files)
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
    /// line is a progress or warning line, and exactly one envelope closes
    /// the stream.
    pub fn envelope(&self) -> Value {
        let Some((last, progress)) = self.lines.split_last() else {
            panic!("riceswap wrote no NDJSON\nstderr:\n{}", self.stderr);
        };
        for line in progress {
            let value: Value = serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("stream line {line:?} is not JSON: {error}"));
            assert!(
                value.get("progress").is_some() || value.get("warning").is_some(),
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
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter_map(|value| value.get("progress").cloned())
            .collect()
    }

    /// The warning lines streamed before the envelope, in arrival order —
    /// the document the panel renders inline while the operation runs.
    pub fn streamed_warnings(&self) -> Vec<Value> {
        if self.lines.is_empty() {
            return Vec::new();
        }
        self.lines[..self.lines.len() - 1]
            .iter()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter_map(|value| value.get("warning").cloned())
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
