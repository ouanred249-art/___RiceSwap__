//! One harness-proving test per operation class: invoke the operation, parse the
//! envelope, assert on what the stubs recorded.

mod common;

use common::{STUB_TOOLS, Sandbox, manifest_toml};
use serde_json::json;

/// Class: pre-flight detection. On a fake `$HOME` with nothing in it, detect
/// proposes nothing and reports the environment honestly: every candidate
/// list is empty, and every tool the snapshot pipeline shells out to was
/// probed.
#[test]
fn detect_probes_every_tool_and_reports_the_candidate_environment() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["detect"]);
    let data = run.assert_ok();

    assert_eq!(
        data.get("stub"),
        None,
        "detection is a real scan now, not a stub: {data:?}"
    );
    assert_eq!(data["config_dirs"], json!([]));
    assert_eq!(
        data["packages"],
        json!({ "official": [], "aur": [] }),
        "packages are reported as the official/AUR split"
    );
    assert_eq!(data["assets"], json!([]));
    assert_eq!(data["wallpapers"], json!([]));
    for tool in STUB_TOOLS {
        assert_eq!(
            data["tools"][tool]["exit_code"],
            json!(0),
            "{tool} was not probed"
        );
    }
    for tool in STUB_TOOLS {
        let probe = match *tool {
            "grim" => "-h",
            "hyprctl" => "version",
            _ => "--version",
        };
        assert!(
            sandbox.log_contains(&format!("{tool} {probe}")),
            "stub log is missing {tool}: {:?}",
            sandbox.log()
        );
    }
}

/// Class: pre-flight plan.
#[test]
fn plan_reports_the_switch_preflight_shape() {
    let sandbox = Sandbox::new();
    sandbox.write_profile("demo", &manifest_toml("demo"));
    let run = sandbox.run(&["plan", "demo"]);
    let data = run.assert_ok();

    assert_eq!(data["target"], json!("demo"));
    assert_eq!(data.get("stub"), None, "plan is a real diff now");
    // With no active profile, plan shows installing everything from demo
    assert!(data["package_diff"]["install"]["official"].is_array());
    assert!(data["package_diff"]["install"]["aur"].is_array());
    assert_eq!(data["package_diff"]["remove"]["official"], json!([]));
    assert_eq!(data["package_diff"]["remove"]["aur"], json!([]));
    assert_eq!(data["blocked_paths"], json!([]));
    for tool in ["pacman", "yay", "paru"] {
        assert!(
            sandbox.log_contains(&format!("{tool} --version")),
            "{tool} was not probed"
        );
    }
    assert!(
        !sandbox.log_contains("hyprctl"),
        "plan must not touch Hyprland"
    );
}

/// Class: profile write. A snapshot on an empty fake `$HOME` still writes a
/// complete profile: the directory, the manifest against the locked schema,
/// and the `grim` screenshot — alongside the probes it reports.
#[test]
fn snapshot_takes_a_profile_name_and_probes_the_package_managers() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["snapshot", "demo"]);
    let data = run.assert_ok();

    assert_eq!(data["profile"], json!("demo"));
    assert_eq!(data["forked"], json!(false), "nothing was active");
    assert_eq!(data["manifest_written"], json!(true));
    assert_eq!(data["checked_paths"], json!([]));
    assert_eq!(
        data["checked_packages"],
        json!({ "official": [], "aur": [] })
    );
    assert!(
        sandbox.profile_dir("demo").join("profile.toml").is_file(),
        "the manifest lands in the profile directory"
    );
    assert!(
        sandbox.profile_dir("demo").join("screenshot.png").is_file(),
        "grim captured the profile screenshot"
    );
    for tool in ["pacman", "yay", "paru"] {
        assert!(
            sandbox.log_contains(&format!("{tool} --version")),
            "{tool} was not probed"
        );
    }
    assert!(sandbox.log_contains("grim -h"), "grim was not probed");
}

/// Class: the switch sequence.
#[test]
fn switch_streams_step_progress_before_the_envelope() {
    let sandbox = Sandbox::new();
    sandbox.write_profile("demo", &manifest_toml("demo"));
    let run = sandbox.run(&["switch", "demo"]);
    let data = run.assert_ok();

    assert_eq!(data["target"], json!("demo"));
    assert_eq!(
        data["completed_steps"],
        json!(10),
        "switch completes all steps"
    );

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

/// Class: profile removal. A missing profile is a clean failure naming it;
/// the force flag still parses on that path and delete never shelled out.
#[test]
fn delete_takes_a_name_and_a_force_flag() {
    let sandbox = Sandbox::new();

    let message = sandbox.run(&["delete", "demo"]).assert_failed();
    assert!(
        message.contains("demo"),
        "the missing profile is named: {message}"
    );

    let message = sandbox.run(&["delete", "demo", "--force"]).assert_failed();
    assert!(
        message.contains("demo"),
        "force does not guess a profile into existence: {message}"
    );
    assert!(
        sandbox.log().is_empty(),
        "delete must not consult external tools"
    );
}

/// Class: profile comparison. Two fixture profiles differ in packages and
/// managed paths; `diff` reports the adds/removes split by source and the
/// paths each side gains, as pure manifest arithmetic.
#[test]
fn diff_takes_two_profiles_and_reports_the_delta_shape() {
    let sandbox = Sandbox::new();
    let alpha = common::profile_toml(
        "alpha",
        &["oldbar", "shared"],
        &["oldaur"],
        &[],
        &[".config/waybar", ".config/hypr"],
    );
    let beta = common::profile_toml(
        "beta",
        &["newbar", "shared"],
        &["newaur"],
        &[],
        &[".config/kitty", ".config/waybar"],
    );
    sandbox.write_profile("alpha", &alpha);
    sandbox.write_profile("beta", &beta);

    let data = sandbox.run(&["diff", "alpha", "beta"]).assert_ok();
    assert_eq!(data["a"], json!("alpha"));
    assert_eq!(data["b"], json!("beta"));
    assert_eq!(
        data["package_delta"]["added"],
        json!({ "official": ["newbar"], "aur": ["newaur"] })
    );
    assert_eq!(
        data["package_delta"]["removed"],
        json!({ "official": ["oldbar"], "aur": ["oldaur"] })
    );
    // Path-level diff: B's full managed set is `added` (the shared
    // .config/waybar re-point is idempotent), A's dropped path is `removed`.
    assert_eq!(
        data["config_delta"],
        json!({
            "added": [".config/kitty", ".config/waybar"],
            "removed": [".config/hypr"]
        })
    );
    assert!(
        sandbox.log().is_empty(),
        "diff must not consult external tools"
    );
}

/// Class: asset import.
#[test]
fn wallpaper_import_takes_a_path_and_reports_the_shared_layer_target() {
    let sandbox = Sandbox::new();
    let source = sandbox.write_home("forest.png", common::image_fixture());
    let run = sandbox.run(&["wallpaper-import", source.to_str().expect("utf-8 path")]);
    let data = run.assert_ok();

    assert_eq!(data["source"], json!(source.display().to_string()));
    assert_eq!(
        data["imported_to"],
        json!(
            sandbox
                .wallpapers_dir()
                .join("forest.png")
                .display()
                .to_string()
        ),
        "the image lands in the shared wallpapers layer"
    );
    assert!(
        sandbox.wallpapers_dir().join("forest.png").is_file(),
        "the layer holds the imported image"
    );
    assert!(
        sandbox.log().is_empty(),
        "importing an image only moves a file; it must not shell out: {:?}",
        sandbox.log()
    );
    assert!(
        data.get("tools").is_none(),
        "an import reports the move, not a tool probe: {data:?}"
    );
}

/// Class: first-run bootstrap.
#[test]
fn init_bootstraps_and_flips_the_initialized_flag() {
    let sandbox = Sandbox::new();
    let data = sandbox.run(&["init"]).assert_ok();

    assert_eq!(data["initialized"], json!(true));
    assert!(
        data["created"]
            .as_array()
            .is_some_and(|created| !created.is_empty()),
        "a first run reports the layers it created: {data:?}"
    );
    assert!(
        sandbox.log().is_empty(),
        "bootstrapping shared layers is filesystem work; it must not shell out: {:?}",
        sandbox.log()
    );
    assert!(
        data.get("tools").is_none(),
        "a clean bootstrap reports its layers, not a tool probe: {data:?}"
    );

    assert_eq!(
        sandbox.state()["initialized"],
        json!(true),
        "state.json mirrors the bootstrap"
    );
}
