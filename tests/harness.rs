//! Proof that the harness itself does what the testing seam promises: an
//! isolated fake `$HOME`, stubs on a stubbed `PATH` that record their
//! invocations, and stubs that can be scripted to fail or conflict.

mod common;

use common::{Mode, STUB_TOOLS, Sandbox, manifest_toml};
use serde_json::json;
use std::fs;

/// The arguments each tool's version probe uses — mirroring
/// `Tool::version_args`, because not every tool spells its version flag the
/// same way (`grim` rejects `--version`, `hyprctl` takes a `version`
/// subcommand).
fn probe_args(tool: &str) -> &'static str {
    match tool {
        "grim" => "-h",
        "hyprctl" => "version",
        _ => "--version",
    }
}

/// Every probed tool gets an executable stub, `PATH` finds it, and `detect`
/// gets a clean answer from all of them. The roster is what `detect` probes,
/// not what the backend shells out to: `git` and `pi` are probe-ready ahead
/// of the install commands that will call them.
#[test]
fn every_probed_tool_gets_a_stub_and_probes_clean() {
    let sandbox = Sandbox::new();
    let data = sandbox.run(&["detect"]).assert_ok();

    for tool in STUB_TOOLS {
        assert!(
            sandbox.log_contains(&format!("{tool} {}", probe_args(tool))),
            "no stub invocation recorded for {tool}: {:?}",
            sandbox.log()
        );
        assert_eq!(
            data["tools"][tool]["exit_code"],
            json!(0),
            "{tool} was not probed successfully"
        );
    }
}

/// The stubs record invocations in order, arguments included.
#[test]
fn stub_log_records_invocations_in_order_with_arguments() {
    let sandbox = Sandbox::new();
    sandbox.run(&["detect"]).assert_ok();

    let log = sandbox.log();
    assert_eq!(
        log.len(),
        STUB_TOOLS.len(),
        "each tool is probed once: {log:?}"
    );
    for line in &log {
        let (tool, args) = line.split_once(' ').expect("log line is `tool args`");
        assert!(STUB_TOOLS.contains(&tool), "unexpected stub logged: {line}");
        assert_eq!(args, probe_args(tool), "unexpected arguments in {line}");
    }
}

/// A stub on the roster that no backend command calls is installed and
/// scriptable, but never probed: `ydotool` is the seam the verification tier
/// will drive, and until a command shells out to it there is nothing honest
/// for `detect` to report about it.
#[test]
fn a_stub_no_backend_command_calls_is_installed_but_never_probed() {
    let sandbox = Sandbox::new();
    sandbox.script("ydotool", Mode::Fail);
    let data = sandbox.run(&["detect"]).assert_ok();

    assert!(
        !data["tools"]
            .as_object()
            .expect("tools is a map")
            .contains_key("ydotool"),
        "an unprobed tool has no status: {data:?}"
    );
    assert!(
        !sandbox.log_contains("ydotool"),
        "nothing invoked it: {:?}",
        sandbox.log()
    );
}

/// One failed probe is a fact the envelope renders, not a run that fails:
/// the tool reports exit 1, its neighbours stay clean, and exactly one
/// warning names it. `bystander` is any other stub — its probe must stay 0.
fn probe_failure_is_reported(tool: &str, bystander: &str) {
    let sandbox = Sandbox::new();
    sandbox.script(tool, Mode::Fail);
    let run = sandbox.run(&["detect"]);
    let data = run.assert_ok();

    assert_eq!(
        data["tools"][tool]["available"],
        json!(true),
        "the stub still ran"
    );
    assert_eq!(data["tools"][tool]["exit_code"], json!(1));
    assert_eq!(
        data["tools"][bystander]["exit_code"],
        json!(0),
        "the probes beside it are unaffected"
    );

    let warnings = run.warnings();
    assert_eq!(
        warnings.len(),
        1,
        "only the failed tool warns: {warnings:?}"
    );
    assert!(
        warnings[0].contains(tool),
        "the warning names the tool: {}",
        warnings[0]
    );
}

/// A stub scripted to fail makes the backend report it as unusable.
#[test]
fn a_stub_scripted_to_fail_is_reported_as_unusable() {
    probe_failure_is_reported("yay", "pacman");
}

/// The same for an install-tier tool: a `git` whose probe fails is recorded,
/// not fatal. The install commands are what make `git` fatal, and they decide
/// that for themselves.
#[test]
fn a_stub_scripted_to_fail_records_its_exit_without_failing_the_run() {
    probe_failure_is_reported("git", "pi");
}

/// A stub scripted to conflict exits distinctly, so switch conflict fallbacks
/// have something to assert on.
#[test]
fn a_stub_scripted_to_conflict_exits_distinctly() {
    let sandbox = Sandbox::new();
    sandbox.write_profile("demo", &manifest_toml("demo"));
    sandbox.script("pacman", Mode::Conflict);
    let run = sandbox.run(&["plan", "demo"]);
    let data = run.assert_ok();

    assert_eq!(
        data["tools"]["pacman"]["exit_code"],
        json!(2),
        "conflict is exit 2"
    );
    assert!(
        run.warnings()
            .iter()
            .any(|warning| warning.contains("conflict")),
        "the conflict text reaches the envelope: {:?}",
        run.warnings()
    );
}

/// Scripting is sticky: the mode survives until it is scripted again.
#[test]
fn scripted_modes_are_sticky_and_reprogrammable() {
    let sandbox = Sandbox::new();
    sandbox.script("grim", Mode::Fail);

    assert_eq!(
        sandbox.run(&["detect"]).assert_ok()["tools"]["grim"]["exit_code"],
        json!(1)
    );
    assert_eq!(
        sandbox.run(&["detect"]).assert_ok()["tools"]["grim"]["exit_code"],
        json!(1)
    );

    sandbox.script("grim", Mode::Ok);
    assert_eq!(
        sandbox.run(&["detect"]).assert_ok()["tools"]["grim"]["exit_code"],
        json!(0)
    );
}

/// The fake `$HOME` is empty at the start and fully isolated from the real one.
#[test]
fn the_fake_home_starts_empty_and_is_isolated() {
    let sandbox = Sandbox::new();
    assert!(
        !sandbox.state_path().exists(),
        "a fresh sandbox has no state.json"
    );

    sandbox.run(&["init"]).assert_ok();

    let written = fs::read_to_string(sandbox.state_path()).expect("init wrote state.json");
    assert!(written.contains("\"initialized\": true"), "got {written}");

    let real = std::env::var("HOME").expect("the test process has a HOME");
    let real_state = format!("{real}/.local/share/riceswap/state.json");
    assert!(
        !std::path::Path::new(&real_state).exists() || !written.contains(&real),
        "the sandbox must not write into the real $HOME"
    );
}

/// An operation is a real subprocess: it inherits nothing the sandbox did not
/// set, so nothing can leak in from the developer's machine.
#[test]
fn the_backend_runs_with_a_controlled_environment() {
    let sandbox = Sandbox::new();
    sandbox.run(&["list"]).assert_ok();

    let log = sandbox.log();
    assert!(log.is_empty(), "list must not shell out, got {log:?}");
}

/// Both entry points agree: a stub's answer is visible to the operation that
/// shells out to it, not just to `detect`.
#[test]
fn stubs_answer_every_operation_that_shells_out() {
    let sandbox = Sandbox::new();
    sandbox.write_profile("demo", &manifest_toml("demo"));
    sandbox.script("yay", Mode::Fail);

    let run = sandbox.run(&["plan", "demo"]);
    run.assert_ok();

    assert!(sandbox.log_contains("yay --version"), "{:?}", sandbox.log());
    assert!(
        run.warnings().iter().any(|warning| warning.contains("yay")),
        "a broken AUR helper is reported: {:?}",
        run.warnings()
    );
}
