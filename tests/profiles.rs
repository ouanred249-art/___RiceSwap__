//! The profile store through the operation surface: the locked manifest schema,
//! forward-only `manifest_version` migration, `list`/`info` envelope data, and
//! the `current` symlink ⇄ `state.json` agreement. Fixtures are written by hand;
//! nothing here reaches inside the crate.

mod common;

use common::{Sandbox, manifest_toml};
use serde_json::json;

/// Class: profile store reads. A fixture profile with the locked manifest
/// schema parses, and `info` reports exactly what the manifest says.
#[test]
fn a_fixture_profile_manifest_parses_through_info() {
    let sandbox = Sandbox::new();
    sandbox.write_profile("demo", &manifest_toml("demo"));
    let run = sandbox.run(&["info", "demo"]);
    let data = run.assert_ok();

    assert_eq!(data["name"], json!("demo"));
    let manifest = &data["manifest"];
    let object = manifest.as_object().expect("manifest is an object");
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "files",
            "manifest_version",
            "packages",
            "profile",
            "rice_info",
            "services",
        ],
        "the locked manifest schema, every key present"
    );

    assert_eq!(manifest["manifest_version"], json!(1));
    assert_eq!(manifest["profile"]["name"], json!("demo rice"));
    assert_eq!(
        manifest["profile"]["description"],
        json!("fixture rice for demo")
    );
    assert_eq!(
        manifest["profile"]["created_at"],
        json!("2026-09-21T10:30:00Z")
    );
    assert_eq!(
        manifest["profile"]["updated_at"],
        json!("2026-09-22T08:00:00Z")
    );
    assert_eq!(manifest["profile"]["screenshot"], json!("screenshot.png"));
    assert_eq!(
        manifest["profile"]["source_url"],
        json!("https://example.invalid/demo")
    );
    assert_eq!(manifest["profile"]["source_commit"], json!("abc123"));

    assert_eq!(manifest["packages"]["official"], json!(["waybar", "kitty"]));
    assert_eq!(manifest["packages"]["aur"], json!(["ags"]));

    assert_eq!(manifest["services"].as_array().expect("services").len(), 1);
    assert_eq!(manifest["services"][0]["name"], json!("ags"));
    assert_eq!(manifest["services"][0]["start"], json!("ags"));
    assert_eq!(manifest["services"][0]["stop"], json!("pkill ags"));

    assert_eq!(manifest["files"].as_array().expect("files").len(), 2);
    assert_eq!(manifest["files"][0]["path"], json!(".config/hypr"));
    assert_eq!(manifest["files"][0]["optional"], json!(false));
    assert_eq!(manifest["files"][1]["path"], json!(".zshrc"));
    assert_eq!(manifest["files"][1]["optional"], json!(true));

    assert_eq!(manifest["rice_info"]["bar"], json!("ags"));
    assert_eq!(manifest["rice_info"]["terminal"], json!("kitty"));
    assert_eq!(manifest["rice_info"]["colors"], json!("matugen"));

    assert!(
        sandbox.log().is_empty(),
        "a profile store read must not consult external tools: {:?}",
        sandbox.log()
    );
}

/// Class: manifest validation. A required field is missing: the envelope says
/// which file and which field, and the invocation still exits cleanly.
#[test]
fn a_manifest_missing_a_required_field_fails_with_a_clear_error() {
    let sandbox = Sandbox::new();
    let broken = manifest_toml("demo").replace("description = \"fixture rice for demo\"\n", "");
    sandbox.write_profile("demo", &broken);

    let run = sandbox.run(&["info", "demo"]);
    let message = run.assert_failed();
    assert!(
        message.contains("profile.toml"),
        "the error must name the manifest file: {message}"
    );
    assert!(
        message.contains("description"),
        "the error must name the missing field: {message}"
    );
    run.assert_no_panic();
    assert!(
        sandbox.log().is_empty(),
        "a failed manifest read must not consult external tools"
    );
}

/// Class: manifest validation. A manifest that is not a RiceSwap manifest at
/// all — no version marker — is rejected with guidance, not parsed by luck.
#[test]
fn a_manifest_without_a_version_is_not_recognized() {
    let sandbox = Sandbox::new();
    let versionless = manifest_toml("demo").replace("manifest_version = 1\n", "");
    sandbox.write_profile("demo", &versionless);

    let message = sandbox.run(&["info", "demo"]).assert_failed();
    assert!(
        message.contains("manifest_version"),
        "the error must name the missing marker: {message}"
    );
    assert!(
        message.contains("profile.toml"),
        "the error must name the manifest file: {message}"
    );
}

/// Class: manifest validation. Malformed TOML reaches the envelope as a
/// message, never as a crash.
#[test]
fn a_malformed_manifest_fails_without_panicking() {
    let sandbox = Sandbox::new();
    sandbox.write_profile("demo", "manifest_version = 1\nthis is not = toml [[[");
    let run = sandbox.run(&["info", "demo"]);

    let message = run.assert_failed();
    assert!(
        message.contains("profile.toml"),
        "the error must name the manifest file: {message}"
    );
    run.assert_no_panic();
}

/// Class: migration. An older `manifest_version` migrates forward-only at
/// load: the envelope reports the current version with the content intact.
#[test]
fn an_older_manifest_version_migrates_forward_on_load() {
    let sandbox = Sandbox::new();
    let old = manifest_toml("demo").replace("manifest_version = 1", "manifest_version = 0");
    sandbox.write_profile("demo", &old);

    let data = sandbox.run(&["info", "demo"]).assert_ok();
    assert_eq!(
        data["manifest"]["manifest_version"],
        json!(1),
        "the load result is reported at the current manifest version"
    );
    assert_eq!(
        data["manifest"]["profile"]["name"],
        json!("demo rice"),
        "migration keeps the content"
    );

    let listed = sandbox.run(&["list"]).assert_ok();
    assert_eq!(
        listed["profiles"][0]["manifest"]["manifest_version"],
        json!(1),
        "list reports migrated manifests too"
    );
}

/// Class: migration. A `manifest_version` from the future refuses with
/// guidance instead of guessing — migrations are forward-only.
#[test]
fn a_future_manifest_version_refuses_with_guidance() {
    let sandbox = Sandbox::new();
    let future = manifest_toml("demo").replace("manifest_version = 1", "manifest_version = 99");
    sandbox.write_profile("demo", &future);
    let run = sandbox.run(&["info", "demo"]);

    let message = run.assert_failed();
    assert!(
        message.contains("manifest_version = 99"),
        "the error must name the version it met: {message}"
    );
    assert!(
        message.contains("update RiceSwap"),
        "the error must say how to proceed: {message}"
    );
    run.assert_no_panic();
}

/// Class: profile store reads. `list` returns every profile in name order with
/// its full manifest, matching what the store holds on disk.
#[test]
fn list_returns_every_profile_with_its_manifest() {
    let sandbox = Sandbox::new();
    sandbox.write_profile("beta", &manifest_toml("beta"));
    sandbox.write_profile("alpha", &manifest_toml("alpha"));

    let run = sandbox.run(&["list"]);
    let data = run.assert_ok();

    let profiles = data["profiles"].as_array().expect("profiles is a list");
    assert_eq!(profiles.len(), 2, "one entry per profile: {profiles:?}");
    assert_eq!(profiles[0]["name"], json!("alpha"), "name order");
    assert_eq!(profiles[1]["name"], json!("beta"));
    assert_eq!(
        profiles[0]["manifest"]["profile"]["name"],
        json!("alpha rice")
    );
    assert_eq!(
        profiles[0]["manifest"]["packages"]["official"],
        json!(["waybar", "kitty"])
    );
    assert_eq!(profiles[1]["manifest"]["rice_info"]["bar"], json!("ags"));
    assert_eq!(
        data["active_profile"],
        json!(null),
        "no current symlink yet"
    );
    assert!(
        sandbox.log().is_empty(),
        "a profile store read must not consult external tools: {:?}",
        sandbox.log()
    );
}

/// Class: profile store reads. One broken manifest is a warning, not a blank
/// profile list — the GUI still renders what is readable.
#[test]
fn list_warns_about_a_broken_manifest_without_blanking_the_store() {
    let sandbox = Sandbox::new();
    sandbox.write_profile("good", &manifest_toml("good"));
    sandbox.write_profile("bad", "this is not a manifest");

    let run = sandbox.run(&["list"]);
    let data = run.assert_ok();

    let profiles = data["profiles"].as_array().expect("profiles is a list");
    assert_eq!(
        profiles.len(),
        1,
        "the readable profile survives: {profiles:?}"
    );
    assert_eq!(profiles[0]["name"], json!("good"));

    let warnings = run.warnings();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("`bad`") && warning.contains("profile.toml")),
        "the broken profile must be named in a warning: {warnings:?}"
    );
}

/// Class: activation. The `current` symlink flipped by hand is what `list`
/// reports and what `state.json` records — all three agree.
#[test]
fn a_hand_flipped_current_symlink_lands_in_state_json() {
    let sandbox = Sandbox::new();
    sandbox.write_profile("demo", &manifest_toml("demo"));
    sandbox.activate("demo");

    let data = sandbox.run(&["list"]).assert_ok();

    assert_eq!(data["active_profile"], json!("demo"));
    assert_eq!(
        sandbox.state()["active_profile"],
        json!("demo"),
        "state.json agrees with the current symlink"
    );
    assert_eq!(
        sandbox.current_target(),
        Some(sandbox.profile_dir("demo")),
        "reading the symlink needs no backend at all"
    );
}

/// Class: activation. A `current` symlink that leads nowhere reports no active
/// profile rather than a name the GUI would badge and then be lied to about.
#[test]
fn a_dangling_current_symlink_reports_no_active_profile() {
    let sandbox = Sandbox::new();
    sandbox.write_profile("demo", &manifest_toml("demo"));
    sandbox.point_current_at("profiles/ghost");

    let run = sandbox.run(&["list"]);
    let data = run.assert_ok();

    assert_eq!(data["active_profile"], json!(null));
    assert_eq!(sandbox.state()["active_profile"], json!(null));
    let warnings = run.warnings();
    assert!(
        warnings.iter().any(|warning| warning.contains("ghost")),
        "the warning must name the profile the symlink points at: {warnings:?}"
    );
}

/// Class: activation. `switch` flips the `current` symlink — the flip is a
/// real primitive — and `state.json` follows it to the new profile.
#[test]
fn switch_flips_the_current_symlink_and_state_agrees() {
    let sandbox = Sandbox::new();
    sandbox.write_profile("demo", &manifest_toml("demo"));
    sandbox.write_profile("other", &manifest_toml("other"));

    sandbox.run(&["switch", "demo"]).assert_ok();
    assert_eq!(
        sandbox.current_target(),
        Some(sandbox.profile_dir("demo")),
        "switch points current at the target profile"
    );
    assert_eq!(sandbox.state()["active_profile"], json!("demo"));

    sandbox.run(&["switch", "other"]).assert_ok();
    assert_eq!(sandbox.current_target(), Some(sandbox.profile_dir("other")));
    assert_eq!(
        sandbox.state()["active_profile"],
        json!("other"),
        "state.json tracks every flip"
    );
}

/// Class: activation. Switching to a profile that is not in the store fails
/// with a message and flips nothing.
#[test]
fn switch_refuses_a_profile_that_is_not_in_the_store() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["switch", "ghost"]);

    let message = run.assert_failed();
    assert!(
        message.contains("ghost"),
        "the error must name the profile: {message}"
    );
    run.assert_no_panic();
    assert!(
        sandbox.current_target().is_none(),
        "a refused switch must not flip the current symlink"
    );
    assert_eq!(sandbox.state()["active_profile"], json!(null));
}

/// Class: frozen contract. `state.json` is rewritten even when the operation
/// fails, and the document keeps its exact shape on that path too — this is the
/// GUI's contract, failure included.
#[test]
fn state_json_is_rewritten_after_a_failed_operation() {
    let sandbox = Sandbox::new();
    sandbox.write_profile("demo", &manifest_toml("demo"));
    sandbox.run(&["info", "demo"]).assert_ok();
    sandbox.run(&["info", "ghost"]).assert_failed();

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
        json!({ "ok": false, "warnings": [] }),
        "the failure reaches the frozen document"
    );
    assert!(
        state["operation"].is_null(),
        "an idle backend has no running operation"
    );
}
