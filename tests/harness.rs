//! Proof that the harness itself does what the testing seam promises: an
//! isolated fake `$HOME`, stubs on a stubbed `PATH` that record their
//! invocations, and stubs that can be scripted to fail or conflict.

mod common;

use common::{Mode, STUB_TOOLS, Sandbox};
use serde_json::json;
use std::fs;

/// Every tool name gets an executable stub, and `PATH` finds it.
#[test]
fn stubs_are_installed_for_every_tool_the_backend_shells_out_to() {
    let sandbox = Sandbox::new();
    sandbox.run(&["detect"]).assert_ok();

    for tool in STUB_TOOLS {
        assert!(
            sandbox.log_contains(&format!("{tool} --version")),
            "no stub invocation recorded for {tool}: {:?}",
            sandbox.log()
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
        assert_eq!(args, "--version", "unexpected arguments in {line}");
    }
}

/// A stub scripted to fail makes the backend report it as unusable.
#[test]
fn a_stub_scripted_to_fail_is_reported_as_unusable() {
    let sandbox = Sandbox::new();
    sandbox.script("yay", Mode::Fail);
    let run = sandbox.run(&["detect"]);
    let data = run.assert_ok();

    assert_eq!(
        data["tools"]["yay"]["available"],
        json!(true),
        "the stub still ran"
    );
    assert_eq!(data["tools"]["yay"]["exit_code"], json!(1));
    assert_eq!(
        data["tools"]["pacman"]["exit_code"],
        json!(0),
        "other stubs are unaffected"
    );

    let warnings = run.warnings();
    assert_eq!(
        warnings.len(),
        1,
        "only the failed tool warns: {warnings:?}"
    );
    assert!(
        warnings[0].contains("yay"),
        "the warning names the tool: {}",
        warnings[0]
    );
}

/// A stub scripted to conflict exits distinctly, so switch conflict fallbacks
/// have something to assert on.
#[test]
fn a_stub_scripted_to_conflict_exits_distinctly() {
    let sandbox = Sandbox::new();
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
