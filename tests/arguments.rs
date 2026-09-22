//! Bad invocations are results, not crashes: `ok: false`, a useful message in
//! place of `data`, exit 0, and no panic on stderr.

mod common;

use common::Sandbox;
use serde_json::json;

#[test]
fn unknown_operation_reports_the_full_surface() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["frobnicate"]);
    let message = run.assert_failed();

    assert!(
        message.contains("frobnicate"),
        "message must name the offender: {message}"
    );
    for operation in [
        "detect",
        "snapshot",
        "plan",
        "switch",
        "list",
        "info",
        "delete",
        "diff",
        "wallpaper-import",
        "init",
    ] {
        assert!(
            message.contains(operation),
            "message must list `{operation}`: {message}"
        );
    }
    run.assert_no_panic();
    assert!(
        sandbox.log().is_empty(),
        "a bad invocation must not shell out"
    );
}

#[test]
fn missing_operation_reports_the_surface() {
    let sandbox = Sandbox::new();
    let run = sandbox.run::<&str>(&[]);
    let message = run.assert_failed();

    assert!(
        message.contains("detect"),
        "message must list the surface: {message}"
    );
    run.assert_no_panic();
}

#[test]
fn a_failed_envelope_still_carries_a_warnings_list() {
    let sandbox = Sandbox::new();
    let envelope = sandbox.run(&["frobnicate"]).envelope();

    assert_eq!(envelope["warnings"], json!([]), "warnings is always a list");
    assert!(
        envelope["data"].is_object(),
        "the message goes in place of data"
    );
}

#[test]
fn operations_that_need_a_name_reject_a_missing_one() {
    let sandbox = Sandbox::new();
    for operation in [
        "snapshot",
        "plan",
        "switch",
        "info",
        "delete",
        "wallpaper-import",
    ] {
        let run = sandbox.run(&[operation]);
        let message = run.assert_failed();
        assert!(
            message.contains(operation),
            "`{operation}` must explain its usage, got {message}"
        );
        run.assert_no_panic();
    }
}

#[test]
fn diff_needs_exactly_two_profiles() {
    let sandbox = Sandbox::new();

    let message = sandbox.run(&["diff"]).assert_failed();
    assert!(message.contains("diff"), "got {message}");

    let message = sandbox.run(&["diff", "alpha"]).assert_failed();
    assert!(message.contains("diff"), "got {message}");

    let message = sandbox
        .run(&["diff", "alpha", "beta", "gamma"])
        .assert_failed();
    assert!(
        message.contains("gamma"),
        "message must name the extra argument: {message}"
    );
}

#[test]
fn no_argument_operations_reject_positionals() {
    let sandbox = Sandbox::new();
    for operation in ["detect", "list", "init"] {
        let run = sandbox.run(&[operation, "surplus"]);
        let message = run.assert_failed();
        assert!(
            message.contains("surplus"),
            "`{operation}` must name the surplus argument, got {message}"
        );
        run.assert_no_panic();
    }
}

#[test]
fn unknown_flags_are_rejected_rather_than_ignored() {
    let sandbox = Sandbox::new();

    let message = sandbox.run(&["list", "--json"]).assert_failed();
    assert!(message.contains("--json"), "got {message}");

    let message = sandbox.run(&["switch", "--force", "demo"]).assert_failed();
    assert!(message.contains("--force"), "got {message}");

    let message = sandbox
        .run(&["delete", "--forceful", "demo"])
        .assert_failed();
    assert!(message.contains("--forceful"), "got {message}");
}

#[test]
fn empty_positionals_are_rejected() {
    let sandbox = Sandbox::new();
    let message = sandbox.run(&["snapshot", ""]).assert_failed();
    assert!(message.contains("snapshot"), "got {message}");
}
