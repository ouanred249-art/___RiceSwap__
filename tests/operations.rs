//! One harness-proving test per operation class: invoke the operation, parse the
//! envelope, assert on what the stubs recorded.

mod common;

use common::{STUB_TOOLS, Sandbox, manifest_toml};
use serde_json::json;

/// Class: pre-flight detection.
#[test]
fn detect_probes_every_tool_and_reports_the_candidate_environment() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["detect"]);
    let data = run.assert_ok();

    assert_eq!(
        data["stub"],
        json!(true),
        "the scaffold marks placeholder data"
    );
    assert_eq!(data["config_dirs"], json!([]));
    assert_eq!(data["packages"], json!([]));
    assert_eq!(data["assets"], json!([]));
    for tool in STUB_TOOLS {
        assert_eq!(
            data["tools"][tool]["exit_code"],
            json!(0),
            "{tool} was not probed"
        );
    }
    for tool in STUB_TOOLS {
        assert!(
            sandbox.log_contains(&format!("{tool} --version")),
            "stub log is missing {tool}: {:?}",
            sandbox.log()
        );
    }
}

/// Class: pre-flight plan.
#[test]
fn plan_reports_the_switch_preflight_shape() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["plan", "demo"]);
    let data = run.assert_ok();

    assert_eq!(data["target"], json!("demo"));
    assert_eq!(data["package_diff"], json!([]));
    assert_eq!(data["service_changes"], json!([]));
    assert_eq!(data["blocked_paths"], json!([]));
    for tool in ["pacman", "yay", "paru"] {
        assert!(
            sandbox.log_contains(&format!("{tool} --version")),
            "{tool} was not probed"
        );
    }
    assert!(
        !sandbox.log_contains("hyprctl --version"),
        "plan must not touch Hyprland"
    );
}

/// Class: profile write.
#[test]
fn snapshot_takes_a_profile_name_and_probes_the_package_managers() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["snapshot", "demo"]);
    let data = run.assert_ok();

    assert_eq!(data["profile"], json!("demo"));
    assert_eq!(data["manifest_written"], json!(false));
    assert_eq!(data["checked_paths"], json!([]));
    assert_eq!(data["checked_packages"], json!([]));
    for tool in ["pacman", "yay", "paru"] {
        assert!(
            sandbox.log_contains(&format!("{tool} --version")),
            "{tool} was not probed"
        );
    }
}

/// Class: the switch sequence.
#[test]
fn switch_streams_step_progress_before_the_envelope() {
    let sandbox = Sandbox::new();
    sandbox.write_profile("demo", &manifest_toml("demo"));
    let run = sandbox.run(&["switch", "demo"]);
    let data = run.assert_ok();

    assert_eq!(data["target"], json!("demo"));
    assert_eq!(data["completed_steps"], json!(0));

    let progress = run.progress();
    assert!(
        progress.len() >= 3,
        "expected streamed progress, got {progress:?}"
    );
    for (index, line) in progress.iter().enumerate() {
        assert_eq!(line["operation"], json!("switch"));
        assert_eq!(line["step"], json!(index as u64 + 1));
        assert!(
            line["message"]
                .as_str()
                .is_some_and(|message| !message.is_empty())
        );
    }
    assert!(
        sandbox.log_contains("pacman --version"),
        "switch must consult pacman"
    );
}

/// Class: profile store reads.
#[test]
fn list_returns_the_profile_store_without_shelling_out() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["list"]);
    let data = run.assert_ok();

    assert_eq!(data["profiles"], json!([]));
    assert_eq!(data["active_profile"], json!(null));
    assert!(
        sandbox.log().is_empty(),
        "a profile store read must not consult external tools: {:?}",
        sandbox.log()
    );
}

/// Class: profile store reads.
#[test]
fn info_returns_one_profile_manifest() {
    let sandbox = Sandbox::new();
    sandbox.write_profile("demo", &manifest_toml("demo"));
    let run = sandbox.run(&["info", "demo"]);
    let data = run.assert_ok();

    assert_eq!(data["name"], json!("demo"));
    assert_eq!(
        data["manifest"]["manifest_version"],
        json!(1),
        "the envelope carries the parsed manifest"
    );
    assert_eq!(data["manifest"]["profile"]["name"], json!("demo rice"));
    assert!(
        sandbox.log().is_empty(),
        "info must not consult external tools"
    );

    let message = sandbox.run(&["info", "ghost"]).assert_failed();
    assert!(
        message.contains("ghost"),
        "a missing profile is a clear error: {message}"
    );
}

/// Class: profile removal.
#[test]
fn delete_takes_a_name_and_a_force_flag() {
    let sandbox = Sandbox::new();

    let data = sandbox.run(&["delete", "demo"]).assert_ok();
    assert_eq!(data["name"], json!("demo"));
    assert_eq!(data["force"], json!(false));
    assert_eq!(data["deleted"], json!(false));

    let data = sandbox.run(&["delete", "demo", "--force"]).assert_ok();
    assert_eq!(data["force"], json!(true));
    assert!(
        sandbox.log().is_empty(),
        "delete must not consult external tools"
    );
}

/// Class: profile comparison.
#[test]
fn diff_takes_two_profiles_and_reports_the_delta_shape() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["diff", "alpha", "beta"]);
    let data = run.assert_ok();

    assert_eq!(data["a"], json!("alpha"));
    assert_eq!(data["b"], json!("beta"));
    assert_eq!(data["package_delta"], json!([]));
    assert_eq!(data["config_delta"], json!([]));
    assert!(
        sandbox.log().is_empty(),
        "diff must not consult external tools"
    );
}

/// Class: asset import.
#[test]
fn wallpaper_import_takes_a_path_and_reports_the_shared_layer_target() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["wallpaper-import", "/tmp/forest.png"]);
    let data = run.assert_ok();

    assert_eq!(data["source"], json!("/tmp/forest.png"));
    assert_eq!(data["imported_to"], json!(null));
    assert!(
        sandbox.log_contains("hyprctl --version"),
        "wallpaper import consults Hyprland"
    );
}

/// Class: first-run bootstrap.
#[test]
fn init_bootstraps_and_flips_the_initialized_flag() {
    let sandbox = Sandbox::new();
    let data = sandbox.run(&["init"]).assert_ok();

    assert_eq!(data["initialized"], json!(true));
    assert_eq!(data["created"], json!([]));
    assert!(
        sandbox.log_contains("hyprctl --version"),
        "bootstrap extracts hardware info"
    );

    assert_eq!(
        sandbox.state()["initialized"],
        json!(true),
        "state.json mirrors the bootstrap"
    );
}
