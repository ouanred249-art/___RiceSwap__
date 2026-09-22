//! The two documents the QML panel reads: the NDJSON envelope on stdout and
//! `state.json` in the data directory. Both shapes are frozen.

mod common;

use common::{Mode, STUB_TOOLS, Sandbox, manifest_toml};
use serde_json::{Value, json};
use std::fs;

/// Every invocation, successful or not, ends in exactly one envelope line.
#[test]
fn every_operation_ends_in_exactly_one_envelope() {
    let sandbox = Sandbox::new();
    let invocations: Vec<Vec<&str>> = vec![
        vec!["detect"],
        vec!["snapshot", "demo"],
        vec!["plan", "demo"],
        vec!["switch", "demo"],
        vec!["list"],
        vec!["info", "demo"],
        vec!["delete", "demo"],
        vec!["diff", "a", "b"],
        vec!["wallpaper-import", "/tmp/x.png"],
        vec!["init"],
        vec!["nonsense"],
    ];
    for args in invocations {
        let run = sandbox.run(&args);
        let envelope = run.envelope();
        assert!(
            envelope["ok"].is_boolean(),
            "{args:?} envelope has no ok flag"
        );
        assert!(
            envelope["warnings"].is_array(),
            "{args:?} warnings is not a list"
        );
        assert!(envelope["data"].is_object() || envelope["data"].is_null());
    }
}

/// A subprocess gets a clean stdin and no terminal; the envelope must still be
/// the last thing on stdout.
#[test]
fn the_envelope_is_the_last_stdout_line() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["switch", "demo"]);

    let last = run.lines.last().expect("some output");
    let envelope: Value = serde_json::from_str(last).expect("final line is JSON");
    assert!(
        envelope.get("ok").is_some(),
        "final line is not the envelope: {last}"
    );
    assert!(
        run.lines.iter().all(|line| !line.trim().is_empty()),
        "no blank lines in the NDJSON stream"
    );
}

/// `state.json` is written for every state change, with every key present.
#[test]
fn state_json_keeps_its_frozen_shape() {
    let sandbox = Sandbox::new();
    sandbox.write_profile("demo", &manifest_toml("demo"));
    sandbox.run(&["switch", "demo"]).assert_ok();

    let state = sandbox.state();
    let object = state.as_object().expect("state.json is an object");
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["active_profile", "initialized", "last_result", "operation"],
        "state.json keys are frozen"
    );
    assert!(state["initialized"].is_boolean());
    assert!(state["active_profile"].is_null() || state["active_profile"].is_string());
    assert_eq!(
        state["last_result"],
        json!({ "ok": true, "warnings": [] }),
        "a finished operation records its result"
    );
    assert!(
        state["operation"].is_null(),
        "an idle backend has no running operation"
    );
}

/// The running operation is visible in `state.json` *while* it runs — this is
/// what lets a panel reopened mid-switch render progress.
#[test]
fn state_json_shows_the_running_operation_to_external_processes() {
    let sandbox = Sandbox::new();
    sandbox.run(&["detect"]).assert_ok();

    let seen = sandbox.state_seen_by("pacman");
    assert_eq!(seen["operation"]["name"], json!("detect"));
    assert_eq!(seen["operation"]["target"], json!(null));
    assert!(
        seen["operation"]["started_at"]
            .as_u64()
            .is_some_and(|at| at > 0),
        "started_at is Unix epoch seconds"
    );
    assert!(
        seen["operation"]["step"]
            .as_u64()
            .is_some_and(|step| step >= 1),
        "the step advances as progress streams"
    );
}

/// A backend whose external tools are all broken still reports honestly.
#[test]
fn state_json_records_warnings_from_a_degraded_environment() {
    let sandbox = Sandbox::new();
    for tool in STUB_TOOLS {
        sandbox.script(tool, Mode::Fail);
    }
    let run = sandbox.run(&["detect"]);
    let envelope = run.envelope();

    let warnings = run.warnings();
    assert_eq!(
        warnings.len(),
        STUB_TOOLS.len(),
        "one warning per unusable tool"
    );
    assert!(
        warnings.iter().any(|warning| warning.contains("pacman")),
        "{warnings:?}"
    );
    assert_eq!(
        sandbox.state()["last_result"],
        json!({ "ok": envelope["ok"], "warnings": warnings })
    );
}

/// The document is rewritten atomically: no reader ever sees a partial file.
#[test]
fn state_json_is_rewritten_without_leaving_temp_files() {
    let sandbox = Sandbox::new();
    sandbox.run(&["init"]).assert_ok();

    let dir = sandbox
        .state_path()
        .parent()
        .expect("state directory")
        .to_path_buf();
    let leftovers: Vec<String> = fs::read_dir(&dir)
        .expect("read state directory")
        .map(|entry| {
            entry
                .expect("dir entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name != "state.json")
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp files were left behind: {leftovers:?}"
    );
}
