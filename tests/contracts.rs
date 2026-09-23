//! The two documents the QML panel reads: the NDJSON envelope on stdout and
//! `state.json` in the data directory. Both shapes are frozen.

mod common;

use common::{Mode, STUB_TOOLS, Sandbox, manifest_toml};
use serde_json::{Value, json};
use std::fs;
use std::time::{Duration, Instant};

/// Polls the stub log until `needle` shows up or the deadline passes.
fn wait_for_log(sandbox: &Sandbox, needle: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if sandbox.log_contains(needle) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

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
        vec!["wallpaper-import", "/nonexistent/riceswap-contract.png"],
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

/// The step checklist the panel's switch view renders: `state.json` carries
/// only the step *number*, so the labels beside it are the `switch`
/// operation's streamed step messages — pinned here as the stream half of
/// the frozen contract (ticket #19's ProgressView).
#[test]
fn switch_streams_the_step_messages_the_panel_renders() {
    let sandbox = Sandbox::new();
    sandbox.write_profile("demo", &manifest_toml("demo"));

    let run = sandbox.run(&["switch", "demo"]);
    run.assert_ok();

    let progress = run.progress();
    let messages: Vec<&str> = progress
        .iter()
        .map(|line| {
            line["message"]
                .as_str()
                .expect("every progress line carries a message")
        })
        .collect();
    assert_eq!(
        messages,
        [
            "verifying profile `demo`",
            "computing the switch plan",
            "activating profile `demo`",
            "stopping old services",
            "linking managed config paths",
            "applying package changes",
            "reloading Hyprland",
            "starting new services",
        ],
        "the panel's checklist rows are exactly these messages, in order"
    );
    for (index, line) in progress.iter().enumerate() {
        assert_eq!(line["operation"], json!("switch"));
        assert_eq!(
            line["step"],
            json!(index + 1),
            "state.json's step counts these messages one-by-one"
        );
    }
}

/// A panel reopened mid-switch resumes from `state.json`: while `switch`
/// runs, the document names the operation, its target, and the step it is
/// on — the three fields the ProgressView renders (ticket #19).
#[test]
fn state_json_names_the_running_switch_and_its_step_while_it_runs() {
    let sandbox = Sandbox::new();
    sandbox.write_profile("demo", &manifest_toml("demo"));
    // The first -S stub sleeps inside its transaction, holding the switch
    // on the package step while the test reads state.json mid-flight.
    sandbox.delay_on("pacman", "waybar", 2);

    let mut child = sandbox.spawn(&["switch", "demo"]);
    assert!(
        wait_for_log(&sandbox, "pacman -S", Duration::from_secs(10)),
        "the package step never started: {:?}",
        sandbox.log()
    );

    let seen = sandbox.state();
    let operation = &seen["operation"];
    assert_eq!(operation["name"], json!("switch"));
    assert_eq!(operation["target"], json!("demo"));
    assert_eq!(
        operation["step"],
        json!(6),
        "the sixth streamed step — the package step — is the visible one"
    );
    assert!(
        operation["started_at"].as_u64().is_some_and(|at| at > 0),
        "started_at is Unix epoch seconds"
    );

    let status = child.wait().expect("wait for the switch");
    assert!(
        status.success(),
        "the operation exits 0 on success, got {status}"
    );
    assert_eq!(
        sandbox.state()["operation"],
        json!(null),
        "the finished switch clears the running operation"
    );
}

/// The document is rewritten atomically: no reader ever sees a partial file.
/// The state directory also hosts the shared layers `init` creates, so only
/// temp-file leftovers count as a violation.
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
        .filter(|name| name.ends_with(".tmp"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp files were left behind: {leftovers:?}"
    );
}
