//! Ticket #15: `delete` and `diff` — profile housekeeping. `delete` removes a
//! profile's directory from the store, refusing the active profile unless
//! forced, and refuses a missing profile through a failed envelope. `diff`
//! reports the package and config delta between two profiles as JSON, sharing
//! its computation with `plan` so the two never diverge.

mod common;

use common::{Sandbox, profile_toml};
use serde_json::json;

/// Two fixture profiles with deliberately different package and file sets.
fn two_profiles(sandbox: &Sandbox) {
    sandbox.write_profile(
        "alpha",
        &profile_toml(
            "alpha",
            &["oldbar", "shared"],
            &["oldaur"],
            &[],
            &[".config/waybar", ".config/hypr"],
        ),
    );
    sandbox.write_profile(
        "beta",
        &profile_toml(
            "beta",
            &["newbar", "shared"],
            &["newaur"],
            &[],
            &[".config/kitty", ".config/waybar"],
        ),
    );
}

/// `delete` on a non-active, present profile removes its directory: the store
/// no longer lists it.
#[test]
fn delete_removes_a_present_inactive_profile_from_the_store() {
    let sandbox = Sandbox::new();
    two_profiles(&sandbox);
    sandbox.activate("alpha");

    let data = sandbox.run(&["delete", "beta"]).assert_ok();
    assert_eq!(data["name"], json!("beta"));
    assert_eq!(data["force"], json!(false));
    assert_eq!(data["deleted"], json!(true));
    assert!(
        !sandbox.profile_dir("beta").exists(),
        "the profile directory is gone"
    );

    let listed = sandbox.run(&["list"]).assert_ok();
    let names: Vec<String> = listed["profiles"]
        .as_array()
        .expect("profiles is a list")
        .iter()
        .map(|p| p["name"].as_str().expect("name is a string").to_string())
        .collect();
    assert_eq!(names, vec!["alpha"], "beta no longer lists: {names:?}");

    // delete is filesystem-only: no external tools consulted.
    assert!(
        sandbox.log().is_empty(),
        "delete must not consult external tools: {:?}",
        sandbox.log()
    );
}

/// `delete` on the active profile refuses without `--force`; the force flag
/// completes it and clears the active record.
#[test]
fn delete_refuses_the_active_profile_unless_forced() {
    let sandbox = Sandbox::new();
    two_profiles(&sandbox);
    sandbox.activate("alpha");

    let message = sandbox.run(&["delete", "alpha"]).assert_failed();
    assert!(
        message.contains("active"),
        "the refusal must say the profile is active: {message}"
    );
    assert!(
        sandbox.profile_dir("alpha").is_dir(),
        "a refused delete leaves the profile in place"
    );

    let data = sandbox.run(&["delete", "alpha", "--force"]).assert_ok();
    assert_eq!(data["force"], json!(true));
    assert_eq!(data["deleted"], json!(true));
    assert!(!sandbox.profile_dir("alpha").exists());
    assert_eq!(
        sandbox.state()["active_profile"],
        json!(null),
        "state.json no longer names the deleted active profile"
    );
}

/// `delete` on a profile that is not in the store is a clean failure, not a
/// crash: the envelope names the profile and exits with the failure code.
#[test]
fn delete_of_a_missing_profile_is_a_clean_failure() {
    let sandbox = Sandbox::new();
    two_profiles(&sandbox);

    let run = sandbox.run(&["delete", "ghost"]);
    let message = run.assert_failed();
    assert!(
        message.contains("ghost"),
        "the error must name the profile: {message}"
    );
    run.assert_no_panic();
}

/// `diff` between two identical-content profiles reports empty deltas: the
/// shape is what the test locks. A missing side is a clean failure naming the
/// profile.
#[test]
fn diff_of_identical_profiles_reports_empty_deltas_and_a_missing_side_refuses() {
    let sandbox = Sandbox::new();
    // Both sides declare the same package and path sets, so the delta is
    // empty: the shape is what the test locks.
    let same = profile_toml("alpha", &["waybar"], &[], &[], &[".config/waybar"]);
    let beta = profile_toml("beta", &["waybar"], &[], &[], &[".config/waybar"]);
    sandbox.write_profile("alpha", &same);
    sandbox.write_profile("beta", &beta);

    let data = sandbox.run(&["diff", "alpha", "beta"]).assert_ok();
    assert_eq!(data["a"], json!("alpha"));
    assert_eq!(data["b"], json!("beta"));
    assert_eq!(data["package_delta"]["added"]["official"], json!([]));
    assert_eq!(data["package_delta"]["added"]["aur"], json!([]));
    assert_eq!(data["package_delta"]["removed"]["official"], json!([]));
    assert_eq!(data["package_delta"]["removed"]["aur"], json!([]));
    // Path-level diff: B's full managed set is reported as the link set, and
    // A's paths B no longer manages as the unlink set — the same sets `plan`
    // computes for a switch, where a re-pointed shared link is re-run.
    assert_eq!(data["config_delta"]["added"], json!([".config/waybar"]));
    assert_eq!(data["config_delta"]["removed"], json!([]));

    assert!(
        sandbox.log().is_empty(),
        "diff must not consult external tools: {:?}",
        sandbox.log()
    );

    let message = sandbox.run(&["diff", "alpha", "ghost"]).assert_failed();
    assert!(
        message.contains("ghost"),
        "the missing side is named: {message}"
    );
}

/// Two profiles that genuinely differ: `diff` names every added and removed
/// package and path — the same sets `plan` computes for a switch between
/// them, so the two can never diverge.
#[test]
fn diff_between_different_profiles_names_every_delta_set() {
    let sandbox = Sandbox::new();
    two_profiles(&sandbox);

    let data = sandbox.run(&["diff", "alpha", "beta"]).assert_ok();
    // Package adds are B's packages that A does not declare (`shared` is in
    // both, so it is in neither set); removals are A's packages B drops.
    // The same sets `plan` computes for a switch A→B.
    assert_eq!(
        data["package_delta"]["added"],
        json!({ "official": ["newbar"], "aur": ["newaur"] }),
        "adds are B minus A, official/AUR split"
    );
    assert_eq!(
        data["package_delta"]["removed"],
        json!({ "official": ["oldbar"], "aur": ["oldaur"] }),
        "removals are A minus B"
    );
    // Path-level diff: B's full managed set is `added` (the shared
    // .config/waybar re-point is idempotent and reported for re-runs), and
    // A's path B no longer manages is `removed`.
    assert_eq!(
        data["config_delta"],
        json!({
            "added": [".config/kitty", ".config/waybar"],
            "removed": [".config/hypr"]
        })
    );

    // Activate alpha so `plan beta` uses the real current profile; with
    // alpha active the plan's install/remove/link/unlink sets are the very
    // same sets `diff` reports — they share the same computation.
    sandbox.activate("alpha");
    let plan = sandbox.run(&["plan", "beta"]).assert_ok();
    assert_eq!(
        plan["package_diff"]["install"],
        data["package_delta"]["added"]
    );
    assert_eq!(
        plan["package_diff"]["remove"],
        data["package_delta"]["removed"]
    );
    assert_eq!(
        plan["symlink_changes"]["link"],
        data["config_delta"]["added"]
    );
    assert_eq!(
        plan["symlink_changes"]["unlink"],
        data["config_delta"]["removed"]
    );

    // The reverse direction inverts the sets: what was added is now removed.
    let data = sandbox.run(&["diff", "beta", "alpha"]).assert_ok();
    assert_eq!(
        data["package_delta"]["added"],
        json!({ "official": ["oldbar"], "aur": ["oldaur"] })
    );
    assert_eq!(
        data["package_delta"]["removed"],
        json!({ "official": ["newbar"], "aur": ["newaur"] })
    );
    assert_eq!(
        data["config_delta"],
        json!({
            "added": [".config/hypr", ".config/waybar"],
            "removed": [".config/kitty"]
        })
    );
}
