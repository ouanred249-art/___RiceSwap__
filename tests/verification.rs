//! Ticket #39: Tier C verification and the rollback floor.
//!
//! The sandbox has no compositor, and that is the point: every test here decides
//! what the live registry *answers* by scripting the `hyprctl` stub, so a verdict
//! can be produced and inspected without a desktop anywhere. The default stub
//! answers `globalshortcuts` with a version banner that carries no registration
//! in it, which the tier reads as "no live registry here" — the same honest
//! degradation a machine without `hyprctl` gets, and the reason the ordinary
//! switch tests in `tests/switch.rs` keep passing untouched.
//!
//! What these tests hold to, in order:
//!
//! * a switch onto a profile the live session fully backs is `verified-core`,
//!   with the two executed checks visible and the frozen key set exact;
//! * a dispatched name the live session registers nothing for is
//!   `invariant-dead-names`, and the old profile goes back;
//! * a shell that will not start is `shell-not-alive`, and the old profile goes
//!   back;
//! * no compositor is a *skip*, not a failure, and the verdict stays
//!   `verified-core`;
//! * a recipe pinned to a namespace the live session does not use is
//!   `recipe-drift`, not a dead bind;
//! * a rollback that cannot finish says so and names the manual step;
//! * packages are enumerated and never reverted;
//! * and an install's envelope carries all of it, because install calls switch.

mod common;

use common::{Mode, Sandbox, profile_toml_with_shell};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

/// The verdict and checks, as `data.report.verification` carries them.
fn verdict(data: &Value) -> &Value {
    &data["report"]["verification"]
}

/// One check's `id` → the whole check object, so a test can assert on the check
/// it cares about without indexing a position that another check could move.
fn check<'a>(verification: &'a Value, id: &str) -> &'a Value {
    verification["checks"]
        .as_array()
        .expect("checks is a list")
        .iter()
        .find(|check| check["id"] == json!(id))
        .unwrap_or_else(|| panic!("no `{id}` check in {verification}"))
}

/// The evidence a check carries, as prose.
fn evidence(verification: &Value, id: &str) -> String {
    check(verification, id)["evidence"]
        .as_str()
        .unwrap_or_else(|| panic!("`{id}` carries no evidence"))
        .to_string()
}

/// A profile the verification tier has something to say about: a shell with a
/// QML tree the engine can derive a registry from, a Hyprland config that
/// dispatches, and a service of its own.
///
/// The shape is the real one from `tests/reconcile.rs` rather than a caricature:
/// the appid is declared once in the base component every registration inherits
/// from, and the config dispatches the *previous* shell's vocabulary, which is
/// what lets the engine's exact-name move do its work before verification looks.
fn rice(sandbox: &Sandbox, name: &str, dead: Option<&str>) {
    rice_with_packages(sandbox, name, &[], dead)
}

/// [`rice`], for the one case that needs a package diff to have happened: the
/// profile the switch is *leaving* declares the package the failed switch
/// removes, and the one it activates declares the package that switch installs.
fn rice_with_packages(sandbox: &Sandbox, name: &str, official: &[&str], dead: Option<&str>) {
    let service = ("qs", "qs -c $qsConfig", "pkill qs");
    sandbox.write_profile(
        name,
        &profile_toml_with_shell(
            name,
            official,
            &[],
            &[service],
            &[".config/hypr", &format!(".config/quickshell/{name}")],
            (name, &format!("qs -c {name}"), "pkill qs"),
        ),
    );
    sandbox.write_profile_file(
        name,
        &format!(".config/quickshell/{name}/components/misc/CustomShortcut.qml"),
        "import Quickshell.Hyprland\n\n// qmllint disable unresolved-type\nGlobalShortcut {\n    \
         // qmllint enable unresolved-type\n    appid: \"caelestia\"\n}\n",
    );
    sandbox.write_profile_file(
        name,
        &format!(".config/quickshell/{name}/modules/lock/Lock.qml"),
        "import QtQuick\n\nScope {\n    GlobalShortcut {\n        name: \"lock\"\n        \
         description: \"Lock the session\"\n        onPressed: {}\n    }\n}\n",
    );
    // The bind the engine can prove: this shell registers `lock`, so
    // `quickshell:lock` moves to `caelestia:lock` before verification reads it.
    let mut configs = String::from(
        "$lock_cmd = hyprctl dispatch 'hl.dsp.global(\"quickshell:lock\")' & hyprlock\n",
    );
    if let Some(dead) = dead {
        configs.push_str(&format!(
            "\nhl.bind(\"SUPER + SHIFT + S\", hl.dsp.global(\"{dead}\"), {{ description = \"Dead\" }})\n"
        ));
    }
    sandbox.write_profile_file(name, ".config/hypr/hyprland/keybinds.lua", configs);
}

/// The rice the switch leaves: a different shell, its own config, and a live
/// tree the way a finished switch leaves one.
fn old_desktop(sandbox: &Sandbox) {
    old_desktop_with_packages(sandbox, &[])
}

/// [`old_desktop`], for the one case that needs the leaving profile to have
/// declared a package the failed switch removes.
fn old_desktop_with_packages(sandbox: &Sandbox, official: &[&str]) {
    let service = ("qs", "qs -c $qsConfig", "pkill qs");
    sandbox.write_profile(
        "ii",
        &profile_toml_with_shell(
            "ii",
            official,
            &[],
            &[service],
            &[".config/hypr", ".config/quickshell/ii"],
            ("ii", "qs -c ii", "pkill qs"),
        ),
    );
    sandbox.write_profile_file(
        "ii",
        ".config/hypr/hyprland/execs.lua",
        "hl.exec_cmd(\"qs -c ii\")\n",
    );
    sandbox.write_profile_file("ii", ".config/quickshell/ii/README", "ii\n");
    for relative in [".config/hypr", ".config/quickshell/ii"] {
        let link = sandbox.home().join(relative);
        fs::create_dir_all(link.parent().expect("a link has a parent"))
            .expect("create the live parent");
        symlink(sandbox.profile_dir("ii").join(relative), &link)
            .expect("manage the path through ii");
    }
    sandbox.activate("ii");
}

/// A registry the live session is claimed to hold: the rice's own name, plus
/// the panel RiceSwap itself runs (which belongs to no profile's registry and is
/// in the keep set instead).
fn live_caelestia() -> Vec<&'static str> {
    vec!["caelestia:lock", "quickshell:riceswap-toggle"]
}

/// A switch onto a profile the running session fully backs: `verified-core`,
/// both checks run, and the verdict object exactly the shape #30 froze.
#[test]
fn a_switch_the_live_session_backs_is_verified_core() {
    let sandbox = Sandbox::new();
    old_desktop(&sandbox);
    rice(&sandbox, "caelestia", None);
    sandbox.live_registry(&live_caelestia());
    sandbox.clear_log();

    let data = sandbox.run(&["switch", "caelestia"]).assert_ok();

    let verification = verdict(&data);
    assert_eq!(verification["verdict"], json!("verified-core"));
    let mut keys: Vec<&str> = verification
        .as_object()
        .expect("the verdict is an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["checks", "verdict"],
        "the frozen verdict object carries nothing else: {verification}"
    );
    assert_eq!(data["completed_steps"], json!(10), "still ten steps");
    assert_eq!(sandbox.state()["active_profile"], json!("caelestia"));

    for check in verification["checks"].as_array().expect("a list") {
        let mut keys: Vec<&str> = check
            .as_object()
            .expect("a check is an object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["evidence", "id", "ok", "tier"],
            "the frozen check object carries nothing else: {check}"
        );
        assert_eq!(check["tier"], json!("C"));
        assert_eq!(check["ok"], json!(true), "every check passed: {check}");
    }
    assert_eq!(check(verification, "shell-alive")["ok"], json!(true));
    assert_eq!(check(verification, "invariant-live")["ok"], json!(true));
    assert_eq!(
        evidence(verification, "invariant-live"),
        "every dispatched name resolves against the live session: 1 dispatched, 2 registered \
         live, 1 kept",
        "the evidence says how many names were measured against how many registrations"
    );
}

/// A name the configs dispatch and no live shell registers is the invariant the
/// engine could not fix, caught against the session itself. The verdict is
/// `fail`, the payload is #29's, and the old desktop is back.
#[test]
fn a_dead_dispatched_name_fails_the_invariant_and_puts_the_old_desktop_back() {
    let sandbox = Sandbox::new();
    old_desktop(&sandbox);
    rice(&sandbox, "caelestia", Some("ghost:name"));
    // The live session knows the one name the engine could prove a move for, and
    // has never heard of `ghost:name`.
    sandbox.live_registry(&live_caelestia());
    sandbox.clear_log();

    let run = sandbox.run(&["switch", "caelestia"]);
    let error = run.assert_failed();
    let data = &run.envelope()["data"];

    assert_eq!(data["phase"], json!("verify"));
    assert_eq!(data["reason_code"], json!("invariant-dead-names"));
    assert!(error.contains("invariant-dead-names"), "{error}");
    assert_eq!(verdict(data)["verdict"], json!("fail"));
    assert_eq!(check(verdict(data), "shell-alive")["ok"], json!(true));
    assert_eq!(check(verdict(data), "invariant-live")["ok"], json!(false));

    // The payload is the frozen one: four facts, no more, no fewer.
    let facts = &data["facts"];
    let mut keys: Vec<&str> = facts
        .as_object()
        .expect("facts is an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["dead_names", "proposals", "registry_n", "shell"],
        "the frozen facts object: {facts}"
    );
    assert_eq!(facts["shell"], json!("caelestia"));
    assert_eq!(facts["registry_n"], json!(2));
    assert_eq!(facts["dead_names"], json!(["ghost:name"]));
    assert_eq!(
        facts["proposals"],
        json!([
            {
                "dispatched": "ghost:name",
                "name": "name",
                "target": null,
                "sites": [{ "file": ".config/hypr/hyprland/keybinds.lua", "line": 3 }],
            },
            {
                // The engine's whole vocabulary, not just the surviving miss: the
                // move it proved for `lock` is part of the same research brief,
                // and the sites are what a recipe resolves against.
                "dispatched": "quickshell:lock",
                "name": "lock",
                "target": "caelestia:lock",
                "sites": [{ "file": ".config/hypr/hyprland/keybinds.lua", "line": 1 }],
            },
        ]),
        "the proposal vocabulary rides verbatim: it is the next research brief"
    );
    assert!(
        data["next"]
            .as_str()
            .is_some_and(|next| next.contains("switched back to `ii`")),
        "the recovery action is named: {data}"
    );

    // The rollback, in the order it is allowed to happen: the symlink first.
    assert_eq!(
        sandbox.current_target(),
        Some(sandbox.profile_dir("ii")),
        "current points back at the profile that was there before"
    );
    assert_eq!(sandbox.state()["active_profile"], json!("ii"));
    let log = sandbox.log();
    let stop = log
        .iter()
        .position(|line| line.contains("pkill qs"))
        .expect("the shell this switch started is stopped");
    let restart = log
        .iter()
        .position(|line| line.contains("qs -c ii"))
        .expect("the previous shell is started again");
    let reload = log
        .iter()
        .rposition(|line| line.contains("hyprctl reload"))
        .expect("the compositor is reloaded");
    assert!(
        stop < reload && reload < restart,
        "the old desktop comes up against the config the reload installed:\n{log:?}"
    );
    assert_eq!(
        fs::read_link(sandbox.home().join(".config/hypr")).ok(),
        Some(sandbox.profile_dir("ii").join(".config/hypr")),
        "the managed path points back into the profile that owns it"
    );
    assert!(
        fs::symlink_metadata(sandbox.home().join(".config/quickshell/caelestia")).is_err(),
        "and the path only the failed profile claimed is cleared"
    );
}

/// A shell that dies inside its start window is the one failure the switch used
/// to report as a warning and carry on from. The warning is still the warning;
/// what changed is that the operation now knows the desktop is not there.
#[test]
fn a_shell_that_dies_inside_the_grace_window_fails_and_puts_the_old_desktop_back() {
    let sandbox = Sandbox::new();
    old_desktop(&sandbox);
    rice(&sandbox, "caelestia", None);
    sandbox.live_registry(&live_caelestia());
    sandbox.fail_on("qs", "-c caelestia");
    sandbox.clear_log();

    let run = sandbox.run(&["switch", "caelestia"]);
    run.assert_failed();
    let data = &run.envelope()["data"];

    assert_eq!(data["reason_code"], json!("shell-not-alive"));
    assert_eq!(verdict(data)["verdict"], json!("fail"));
    assert_eq!(check(verdict(data), "shell-alive")["ok"], json!(false));
    assert_eq!(
        check(verdict(data), "invariant-live")["ok"],
        json!(true),
        "the registry is fine; the desktop behind it is not"
    );
    assert_eq!(
        data["report"]["shell_started"],
        Value::Null,
        "the switch's own report is unchanged and still honest"
    );
    assert!(
        run.warnings()
            .iter()
            .any(|warning| warning.contains("failed to start")),
        "and the step's own warning was not swallowed by the failure: {:?}",
        run.warnings()
    );
    assert_eq!(sandbox.current_target(), Some(sandbox.profile_dir("ii")));
    assert!(sandbox.log_contains("qs -c ii"));
}

/// No compositor is not a dead bind. A machine without `hyprctl`, and a sandbox
/// whose stub answers a version banner instead of a registry, both get the
/// honest degraded pass: the check is recorded as skipped and the verdict stays
/// `verified-core`. This is what keeps every switch test in this repository
/// green without a compositor anywhere.
#[test]
fn a_session_without_a_compositor_degrades_to_a_skipped_check() {
    let sandbox = Sandbox::new();
    old_desktop(&sandbox);
    rice(&sandbox, "caelestia", None);

    // The two shapes of "no live registry", one after the other.
    sandbox.script("hyprctl", Mode::Fail);
    let failed = sandbox.run(&["switch", "caelestia"]).assert_ok();
    let by_failure = verdict(&failed);
    assert_eq!(by_failure["verdict"], json!("verified-core"));
    assert_eq!(check(by_failure, "invariant-live")["ok"], json!(true));
    assert!(
        evidence(by_failure, "invariant-live").contains("skipped"),
        "the skip is in the evidence, where the frozen key set can carry it: {}",
        evidence(by_failure, "invariant-live")
    );
    assert!(
        evidence(by_failure, "invariant-live").contains("exited non-zero"),
        "and it says what happened: {}",
        evidence(by_failure, "invariant-live")
    );

    // A second, honest "nothing is registered here": the stub's generic answer.
    // An unreadable answer is an unknown registry, not an empty one — reading it
    // as empty would declare every dispatch dead and roll a working desktop back
    // on a machine that simply has no shell running.
    sandbox.script("hyprctl", Mode::Ok);
    sandbox.live_registry_answers("hyprctl 1.0.0-stub\n");
    let second = sandbox.run(&["switch", "caelestia"]).assert_ok();
    let by_answer = verdict(&second);
    assert_eq!(by_answer["verdict"], json!("verified-core"));
    assert_eq!(check(by_answer, "shell-alive")["ok"], json!(true));
    assert!(
        evidence(by_answer, "invariant-live").contains("no `appid:name` registration"),
        "{}",
        evidence(by_answer, "invariant-live")
    );
    assert_eq!(
        sandbox.current_target(),
        Some(sandbox.profile_dir("caelestia"))
    );
}

/// A recipe pinned to a namespace the live session does not use is a different
/// failure from a dead bind, and gets its own reason code: the bind's name is
/// registered, just not where the recipe said it would be. The fix is the
/// recipe; a codebook rewrite would be the wrong answer.
#[test]
fn a_recipe_pinned_to_the_wrong_namespace_is_recipe_drift() {
    let sandbox = Sandbox::new();
    old_desktop(&sandbox);
    rice(&sandbox, "caelestia", None);
    // The built-in caelestia recipe pins `caelestia`; this session answers under
    // another namespace entirely, and it does register the same *names*.
    sandbox.live_registry(&["other:lock", "quickshell:riceswap-toggle"]);

    let run = sandbox.run(&["switch", "caelestia"]);
    run.assert_failed();
    let data = &run.envelope()["data"];

    assert_eq!(data["reason_code"], json!("recipe-drift"));
    assert_eq!(data["facts"]["dead_names"], json!(["caelestia:lock"]));
    assert!(
        evidence(verdict(data), "invariant-live").contains("pins this shell's namespace"),
        "the evidence says what drifted: {}",
        evidence(verdict(data), "invariant-live")
    );
    assert_eq!(
        sandbox.current_target(),
        Some(sandbox.profile_dir("ii")),
        "drift is still a failed verification, so the old desktop comes back"
    );
}

/// A declared IPC probe is a claim about the shell, and the running desktop is
/// the only thing that can settle it. A probe that does not answer fails.
#[test]
fn a_declared_ipc_probe_that_does_not_answer_fails() {
    let sandbox = Sandbox::new();
    old_desktop(&sandbox);
    rice(&sandbox, "caelestia", None);
    sandbox.live_registry(&live_caelestia());
    // The human tier, which is the one that speaks for a profile's own shell.
    sandbox.write_profile_file(
        "caelestia",
        "adapt.toml",
        "[ipc]\nprobe = \"qs -c caelestia ipc call panel state\"\n",
    );
    sandbox.fail_on("qs", "ipc call panel state");
    sandbox.clear_log();

    let run = sandbox.run(&["switch", "caelestia"]);
    run.assert_failed();
    let data = &run.envelope()["data"];

    assert_eq!(data["reason_code"], json!("probe-failed:ipc-liveness"));
    assert_eq!(check(verdict(data), "ipc-liveness")["ok"], json!(false));
    assert!(
        run.warnings()
            .iter()
            .any(|warning| warning.contains("ipc call panel state"))
            || evidence(verdict(data), "ipc-liveness").contains("did not answer"),
        "the probe that failed is named: {:?}",
        run.warnings()
    );
    assert_eq!(sandbox.current_target(), Some(sandbox.profile_dir("ii")));

    // And a probe that answers adds its check and *passes*, rather than being
    // reported as unrun: the id is in `checks` either way, and only the evidence
    // differs. Without the declaration there is no check at all, which is how a
    // consumer tells "not run" from "passed".
    let quiet = Sandbox::new();
    old_desktop(&quiet);
    rice(&quiet, "caelestia", None);
    quiet.live_registry(&live_caelestia());
    let plain = quiet.run(&["switch", "caelestia"]).assert_ok();
    assert!(
        verdict(&plain)["checks"]
            .as_array()
            .expect("a list")
            .iter()
            .all(|check| check["id"] != json!("ipc-liveness")),
        "a shell with no declared probe has no `ipc-liveness` check: {}",
        verdict(&plain)
    );
}

/// A rollback that cannot finish is its own report. The envelope says the
/// verification failed, says the rollback did not finish either, and names the
/// command that finishes it by hand — a failed rollback is the one case where
/// the `next` field is a manual step rather than a statement of what happened.
#[test]
fn a_rollback_that_cannot_finish_names_the_manual_step() {
    let sandbox = Sandbox::new();
    old_desktop(&sandbox);
    rice(&sandbox, "caelestia", Some("ghost:name"));
    sandbox.live_registry(&live_caelestia());
    // The fault has to land *between* the two flips, which is what a stub-side
    // fault is for: the leaving profile's directory is gone by the time the
    // rollback tries to point `current` back at it, so the flip cannot resolve
    // it. It fires on the arriving shell's start — the last command the switch
    // runs before verification — so nothing after it recreates the directory
    // behind the fault's back.
    sandbox.fault_on(
        "qs",
        "caelestia",
        "rm -rf \"$HOME/.local/share/riceswap/profiles/ii\"",
    );
    sandbox.clear_log();

    let run = sandbox.run(&["switch", "caelestia"]);
    let error = run.assert_failed();
    let data = &run.envelope()["data"];

    assert_eq!(
        data["reason_code"],
        json!("rollback-failed"),
        "the reason a user acts on is the one that leaves them a manual step"
    );
    assert!(error.contains("rollback"), "the error says so: {error}");
    assert_eq!(data["rollback"]["ok"], json!(false));
    let next = data["next"].as_str().expect("a next step").to_string();
    assert!(
        next.contains("riceswap switch ii"),
        "and it names the command that finishes it: {next}"
    );
    assert!(
        next.contains("were not reverted"),
        "and the package boundary is restated where the user reads it: {next}"
    );
    assert_eq!(
        data["facts"]["dead_names"],
        json!(["ghost:name"]),
        "the verification facts survive the rollback failure: they are the evidence"
    );
}

/// The rollback stops at the filesystem and services. A package transaction is
/// not undone — the envelope enumerates what moved and says plainly that it
/// stays moved, which is the #30 boundary and the whole reason a rollback here
/// is safe to run unattended.
#[test]
fn packages_the_failed_switch_moved_are_enumerated_and_not_reverted() {
    let sandbox = Sandbox::new();
    // The leaving profile declared `oldbar`, the arriving one declares `newbar`,
    // and the arriving one is the profile whose desktop does not come up.
    old_desktop_with_packages(&sandbox, &["oldbar"]);
    rice_with_packages(&sandbox, "caelestia", &["newbar"], Some("ghost:name"));
    sandbox.live_registry(&live_caelestia());
    sandbox.clear_log();

    let run = sandbox.run(&["switch", "caelestia"]);
    run.assert_failed();
    let data = &run.envelope()["data"];

    assert_eq!(data["reason_code"], json!("invariant-dead-names"));
    assert_eq!(
        data["report"]["installed"],
        json!(["newbar"]),
        "what the failed switch installed is enumerated in the report"
    );
    assert_eq!(data["report"]["removed"], json!(["oldbar"]));
    assert!(
        data["next"]
            .as_str()
            .is_some_and(|next| next.contains("does not run the opposite transaction")),
        "and `next` says the boundary out loud: {}",
        data["next"]
    );
    assert_eq!(
        sandbox.installed_packages(),
        vec!["newbar".to_string()],
        "the installed package stays installed and the removed one stays removed: no \
         second transaction, in either direction"
    );
    // One install and one removal — each raised once through the privilege
    // wrapper, which the stub log records twice (the `pkexec pacman` line and
    // the `pacman` it executed). A rollback that undid either direction would
    // add a third and fourth of each.
    let log = sandbox.log();
    let transactions: Vec<String> = log
        .iter()
        .filter_map(|line| line.strip_prefix("pkexec "))
        .filter(|line| line.starts_with("pacman -S") || line.starts_with("pacman -R"))
        .map(str::to_string)
        .collect();
    assert_eq!(
        transactions,
        [
            "pacman -S --noconfirm newbar",
            "pacman -R --noconfirm oldbar",
        ],
        "the forward switch's two transactions and no others: {log:?}"
    );
    assert_eq!(
        sandbox.state()["last_result"]["ok"],
        json!(false),
        "and state.json records the operation for what it was"
    );
}

/// Install calls the ordinary switch, so it inherits the whole tier — and its
/// envelope merge must not swallow any of it. The install payload nests the
/// switch's own `data` under `switch`, so the verdict lives at
/// `switch.report.verification`, and a switch that failed verification fails the
/// install with the reason code intact.
#[test]
fn the_verdict_and_its_failure_survive_the_install_envelope() {
    let sandbox = Sandbox::new();
    let tree = rice_tree(&sandbox);

    // A pass first: the verdict rides the success payload.
    let data = sandbox
        .run(&["install", &tree.display().to_string()])
        .assert_ok();
    let switched = &data["switch"];
    assert_eq!(verdict(switched)["verdict"], json!("verified-core"));
    assert_eq!(
        check(verdict(switched), "shell-alive")["ok"],
        json!(true),
        "an install's own shell check ran and is reported: {}",
        verdict(switched)
    );

    // And a failure: `install` merges its own `error`, `phase` and `resume_hint`
    // at the top level, and the switch's payload — verdict, reason code, facts —
    // has to come through beside them untouched.
    let again = Sandbox::new();
    let failing = rice_tree(&again);
    again.script("qs", Mode::Fail);
    let run = again.run(&["install", &failing.display().to_string()]);
    run.assert_failed();
    let data = &run.envelope()["data"];
    assert_eq!(data["phase"], json!("switch"), "the install's own phase");
    assert_eq!(data["switch"]["phase"], json!("verify"));
    assert_eq!(data["switch"]["reason_code"], json!("shell-not-alive"));
    assert_eq!(verdict(&data["switch"])["verdict"], json!("fail"));
    assert!(
        data["switch"]["facts"]["dead_names"].is_array(),
        "the facts payload is not clobbered by the merge: {}",
        data["switch"]
    );
    assert!(
        data["resume_hint"]
            .as_str()
            .is_some_and(|hint| hint.contains("re-run")),
        "and the install's own resume hint is still the one that resumes: {}",
        data["resume_hint"]
    );
}

/// A rice outside `$HOME` for the installer to acquire: a Quickshell config
/// whose shell registers what its Hyprland config dispatches, so the install's
/// own reconcile pass and the switch's verification both have real work.
fn rice_tree(sandbox: &Sandbox) -> std::path::PathBuf {
    let tree = sandbox.outside_home("rice-tree");
    fs::create_dir_all(tree.join(".config/hypr/hyprland")).expect("create the rice tree");
    fs::create_dir_all(tree.join(".config/quickshell/demo/components")).expect("create QML");
    fs::write(
        tree.join(".config/quickshell/demo/components/Shortcut.qml"),
        "import Quickshell.Hyprland\n\nGlobalShortcut {\n    name: \"lock\"\n    \
         onPressed: {}\n}\n",
    )
    .expect("write the registration");
    fs::write(
        tree.join(".config/hypr/hyprland/keybinds.lua"),
        "hl.bind(\"SUPER + L\", hl.dsp.global(\"quickshell:lock\"))\n",
    )
    .expect("write the bind");
    fs::write(
        tree.join(".config/quickshell/demo/shell.qml"),
        "import Quickshell\n\nShellRoot {\n    // marks this tree as a quickshell rice\n}\n",
    )
    .expect("write the marker");
    tree
}

/// The two documents the panel reads stay exactly as they were: the step
/// messages a switch streams are the checklist, and verification is not one of
/// them. It runs after the tenth step, inside the same envelope, and moves
/// nothing.
#[test]
fn verification_is_not_a_step_of_the_switch() {
    let sandbox = Sandbox::new();
    old_desktop(&sandbox);
    rice(&sandbox, "caelestia", Some("ghost:name"));
    sandbox.live_registry(&live_caelestia());

    let run = sandbox.run(&["switch", "caelestia"]);
    run.assert_failed();
    let data = &run.envelope()["data"];

    assert_eq!(data["completed_steps"], json!(10));
    let progress = run.progress();
    let messages: Vec<&str> = progress
        .iter()
        .map(|line| line["message"].as_str().expect("a message"))
        .collect();
    assert_eq!(
        messages,
        [
            "verifying profile `caelestia`",
            "computing the switch plan",
            "applying package changes",
            "activating profile `caelestia`",
            "stopping old services",
            "linking managed config paths",
            "reloading Hyprland",
            "starting new services",
        ],
        "the panel's checklist is unchanged: the tier reads the result, it does not \
         add a row to the work"
    );
    let profile = &sandbox.profile_dir("caelestia");
    assert!(
        Path::new(&sandbox.current_link()).is_symlink(),
        "and the store is a store"
    );
    assert!(profile.join("profile.toml").is_file());
}
