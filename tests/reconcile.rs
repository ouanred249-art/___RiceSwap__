//! Ticket #35: the dispatcher-reconciliation engine as the switch runs it.
//!
//! The unit tests in `src/reconcile.rs` hold the engine's own parts (the QML
//! parse, the dispatch scan, the rewrite, the backups) against ground truth.
//! This file holds the seam that matters to a user: that switching into a
//! profile whose shell registers different names *fixes itself*, that the live
//! config the link installs carries the repair, and that a switch onto a
//! profile whose names already resolve writes nothing at all.
//!
//! The engine is a pure-text step over the *profile's* files, so a switch
//! repairs what is about to be linked rather than what is currently live —
//! which is what these tests read back: the profile tree, and the live path the
//! switch's own symlink resolves to.

mod common;

use common::{Sandbox, profile_toml_with_shell};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

/// The engine's facts as the switch envelope carries them under `report.reconcile`.
fn facts(data: &Value) -> &Value {
    &data["report"]["reconcile"]
}

/// A profile whose configs speak the *previous* shell's vocabulary: `quickshell`
/// names everywhere, while the QML beside them registers under `caelestia` and
/// knows only its own 22. This is the incident, reduced to the two files that
/// matter and the name that can actually be proven.
///
/// The shape is the real one, not a caricature: the appid is declared once, in
/// the base component every registration inherits from.
fn donor_graft(sandbox: &Sandbox, profile: &str) {
    sandbox.write_profile(
        profile,
        &profile_toml_with_shell(
            profile,
            &[],
            &[],
            &[],
            &[".config/hypr", ".config/quickshell/caelestia"],
            ("caelestia", "qs -c caelestia", "pkill qs"),
        ),
    );
    // The base component: one appid declaration for the whole tree.
    sandbox.write_profile_file(
        profile,
        ".config/quickshell/caelestia/components/misc/CustomShortcut.qml",
        "import Quickshell.Hyprland\n\n// qmllint disable unresolved-type\nGlobalShortcut {\n    \
         // qmllint enable unresolved-type\n    appid: \"caelestia\"\n}\n",
    );
    sandbox.write_profile_file(
        profile,
        ".config/quickshell/caelestia/modules/lock/Lock.qml",
        "import QtQuick\n\nScope {\n    GlobalShortcut {\n        name: \"lock\"\n        \
         description: \"Lock the session\"\n        onPressed: {}\n    }\n}\n",
    );
    // The two configs, exactly as the donor spells them: one name the registry
    // can answer (`lock`), one it cannot (`regionScreenshot`).
    sandbox.write_profile_file(
        profile,
        ".config/hypr/hypridle.conf",
        "$lock_cmd = hyprctl dispatch 'hl.dsp.global(\"quickshell:lock\")' & pidof qs quickshell \
         hyprlock || hyprlock\n\nafter_sleep_cmd = hyprctl dispatch \
         'hl.dsp.global(\"quickshell:lockFocus\")'\n",
    );
    sandbox.write_profile_file(
        profile,
        ".config/hypr/hyprland/keybinds.lua",
        concat!(
            "hl.bind(\"SUPER + SHIFT + S\", hl.dsp.global(\"quickshell:regionScreenshot\"), ",
            "{ description = \"Screen snip\" })\n",
            "hl.bind(\"SUPER + R\", hl.dsp.global(\"quickshell:riceswap-toggle\"), ",
            "{ description = \"Toggle RiceSwap\" })\n",
        ),
    );
}

/// A profile that already matches its own shell: the guard. Its configs dispatch
/// `quickshell:*` and its QML registers those names under Quickshell's default
/// appid, so there is nothing to derive, nothing dead, and nothing to write.
fn already_good(sandbox: &Sandbox, profile: &str) {
    sandbox.write_profile(
        profile,
        &profile_toml_with_shell(
            profile,
            &[],
            &[],
            &[],
            &[".config/hypr", ".config/quickshell/ii"],
            ("ii", "qs -c ii", "pkill qs"),
        ),
    );
    // No `appid:` anywhere: Quickshell's own default holds. Both dispatched
    // names are registered, which is the whole point — the configs speak this
    // shell's own vocabulary.
    sandbox.write_profile_file(
        profile,
        ".config/quickshell/ii/modules/common/panels/lock/LockScreen.qml",
        "import QtQuick\n\nScope {\n    GlobalShortcut {\n        name: \"lock\"\n        onPressed: {}\n    \
         }\n}\n",
    );
    sandbox.write_profile_file(
        profile,
        ".config/quickshell/ii/modules/ii/regionSelector/RegionSelector.qml",
        "import QtQuick\n\nScope {\n    GlobalShortcut {\n        name: \"regionScreenshot\"\n        onPressed: {}\n    \
         }\n}\n",
    );
    sandbox.write_profile_file(
        profile,
        ".config/hypr/hypridle.conf",
        "$lock_cmd = hyprctl dispatch 'hl.dsp.global(\"quickshell:lock\")' & hyprlock\n",
    );
    sandbox.write_profile_file(
        profile,
        ".config/hypr/hyprland/keybinds.lua",
        "hl.bind(\"SUPER + SHIFT + S\", hl.dsp.global(\"quickshell:regionScreenshot\"))\n",
    );
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

/// Every file under a directory with its bytes, so a test can assert that
/// *nothing* moved rather than only what it expected to change.
fn stamp(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut entries = Vec::new();
    fn walk(root: &Path, directory: &Path, into: &mut Vec<(String, Vec<u8>)>) {
        let Ok(read) = fs::read_dir(directory) else {
            return;
        };
        for entry in read.flatten() {
            let path = entry.path();
            let name = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            if path.is_dir() {
                into.push((format!("{name}/"), b"<dir>".to_vec()));
                walk(root, &path, into);
            } else {
                into.push((name, fs::read(&path).unwrap_or_default()));
            }
        }
    }
    walk(root, root, &mut entries);
    entries.sort();
    entries
}

/// A switch onto a profile captured while another shell ran repairs the binds
/// that profile's own shell cannot answer, and the repair is in the file the
/// link installs — not in some copy of it.
#[test]
fn switching_into_a_donor_graft_repairs_the_names_the_new_shell_registers() {
    let sandbox = Sandbox::new();
    donor_graft(&sandbox, "grafted");
    let live = sandbox.home().join(".config/hypr");
    fs::create_dir_all(live.join("hyprland")).expect("live config");
    fs::write(
        live.join("hypridle.conf"),
        "$lock_cmd = hyprctl dispatch 'hl.dsp.global(\"quickshell:lock\")'\n",
    )
    .expect("live hypridle");

    let run = sandbox.run(&["switch", "grafted"]);
    let data = run.assert_ok();

    // The envelope says what the engine found, in the shape the report uses.
    let reconciled = facts(&data);
    assert_eq!(reconciled["shell"], json!("caelestia"));
    assert_eq!(reconciled["appid"], json!("caelestia"));
    assert_eq!(reconciled["registered"], json!(1));
    assert_eq!(reconciled["scanned_files"], json!(2));
    assert_eq!(
        reconciled["files_written"],
        json!([".config/hypr/hypridle.conf"])
    );
    assert_eq!(reconciled["backups"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        reconciled["dead_names"],
        json!(["quickshell:lockFocus", "quickshell:regionScreenshot"]),
        "what the engine could not safely fix, and said so"
    );

    // The proven move is applied, and it is applied to the file the switch
    // links — so the very reload this switch performs picks it up.
    let profile_idle = read(
        &sandbox
            .profile_dir("grafted")
            .join(".config/hypr/hypridle.conf"),
    );
    assert!(
        profile_idle.contains(r#"hl.dsp.global("caelestia:lock")"#),
        "lock is registered by this shell, so it moves: {profile_idle}"
    );
    assert_eq!(
        read(&live.join("hypridle.conf")),
        profile_idle,
        "the live path the switch installed carries the repair"
    );

    // The names with no counterpart are left exactly as they were, and
    // RiceSwap's own hotkey — a different live shell's namespace — is untouched.
    let keybinds = read(
        &sandbox
            .profile_dir("grafted")
            .join(".config/hypr/hyprland/keybinds.lua"),
    );
    assert!(keybinds.contains(r#"hl.dsp.global("quickshell:regionScreenshot")"#));
    assert!(keybinds.contains(r#"hl.dsp.global("quickshell:riceswap-toggle")"#));
    assert!(
        !reconciled["dead_names"]
            .as_array()
            .expect("dead_names")
            .iter()
            .any(|dead| dead.as_str().is_some_and(|dead| dead.contains("riceswap"))),
        "the panel's own hotkey is never even reported: {reconciled:?}"
    );

    // The proposal shape the recipe tier consumes: one entry per dead name,
    // carrying the lines that dispatch it, because the tier resolves a name per
    // line (shortcut, exec or drop) and "dead somewhere in this file" is not a
    // question it can act on.
    assert_eq!(
        reconciled["proposals"],
        json!([
            {
                "dispatched": "quickshell:lock",
                "name": "lock",
                "target": "caelestia:lock",
                "sites": [{ "file": ".config/hypr/hypridle.conf", "line": 1 }],
            },
            {
                "dispatched": "quickshell:lockFocus",
                "name": "lockFocus",
                "target": Value::Null,
                "sites": [{ "file": ".config/hypr/hypridle.conf", "line": 3 }],
            },
            {
                "dispatched": "quickshell:regionScreenshot",
                "name": "regionScreenshot",
                "target": Value::Null,
                "sites": [{ "file": ".config/hypr/hyprland/keybinds.lua", "line": 1 }],
            },
        ]),
        "per dead name, the applied move or null, and the line it is dispatched on"
    );

    // The pre-rewrite bytes are recoverable, content-addressed.
    let backup = reconciled["backups"][0]
        .as_str()
        .expect("a backup path")
        .to_string();
    let kept = read(&sandbox.profile_dir("grafted").join(&backup));
    assert!(
        kept.contains(r#"hl.dsp.global("quickshell:lock")"#),
        "the backup holds the bytes as they were: {kept}"
    );
    assert!(backup.starts_with("backups/"), "{backup}");

    // The user is told, inline and in the envelope, what was done and what was
    // not — and the switch itself completed as it always does.
    let warnings = run.warnings();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains(".config/hypr/hypridle.conf")),
        "the repair is announced: {warnings:?}"
    );
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("quickshell:regionScreenshot")),
        "the unfixable name is announced: {warnings:?}"
    );
    assert_eq!(
        data["completed_steps"],
        json!(10),
        "the switch is still ten steps"
    );
    assert!(data["report"]["reloaded"].eq(&json!(true)));
    assert_eq!(sandbox.state()["active_profile"], json!("grafted"));
}

/// The unfixable names are not quietly dropped on the second switch either: the
/// proposal is still there, and the second run still writes nothing.
#[test]
fn a_second_switch_over_a_repaired_profile_writes_nothing_and_still_reports() {
    let sandbox = Sandbox::new();
    donor_graft(&sandbox, "grafted");

    let first = sandbox.run(&["switch", "grafted"]).assert_ok();
    let profile = sandbox.profile_dir("grafted");
    let after_first = stamp(&profile);

    let second = sandbox.run(&["switch", "grafted"]).assert_ok();

    assert_eq!(
        facts(&first)["files_written"],
        json!([".config/hypr/hypridle.conf"]),
        "the first switch has real work to do"
    );
    let reconciled = facts(&second);
    assert_eq!(
        reconciled["files_written"],
        json!([]),
        "the second switch writes no file: {reconciled:?}"
    );
    assert_eq!(
        reconciled["backups"],
        json!([]),
        "and takes no second backup: {reconciled:?}"
    );
    assert_eq!(
        reconciled["dead_names"],
        facts(&first)["dead_names"],
        "the names it could not fix are still named, so a recipe tier can see them"
    );
    assert_eq!(
        after_first,
        stamp(&profile),
        "not one byte of the profile moved on the second switch"
    );
    assert_eq!(second["completed_steps"], json!(10));
}

/// The regression guard, end to end: a profile whose dispatches are its own
/// shell's live vocabulary is a no-op. ii's configs dispatch `quickshell:*`
/// and ii registers those names, so blanket-rewriting the namespace — the
/// mistake that would break the user's own rice — is exactly what must not
/// happen.
#[test]
fn switching_onto_a_profile_that_matches_its_shell_writes_nothing() {
    let sandbox = Sandbox::new();
    already_good(&sandbox, "ii-tweaked");
    let profile = sandbox.profile_dir("ii-tweaked");
    let before = stamp(&profile);

    let data = sandbox.run(&["switch", "ii-tweaked"]).assert_ok();
    let reconciled = facts(&data);

    assert_eq!(reconciled["shell"], json!("ii"));
    assert_eq!(reconciled["appid"], json!("quickshell"));
    assert_eq!(reconciled["registered"], json!(2));
    assert_eq!(reconciled["files_written"], json!([]));
    assert_eq!(reconciled["backups"], json!([]));
    assert_eq!(reconciled["dead_names"], json!([]));
    assert_eq!(reconciled["proposals"], json!([]));
    assert_eq!(
        before,
        stamp(&profile),
        "the whole profile tree is byte-for-byte what it was"
    );
    assert!(
        !profile.join("backups").exists(),
        "a clean profile does not even get a backups directory"
    );
    assert!(
        !sandbox
            .run(&["switch", "ii-tweaked"])
            .warnings()
            .iter()
            .any(|warning| warning.contains("dispatcher")),
        "a no-op pass says nothing out loud"
    );
}

/// A profile naming no shell has no registry to derive and no opinion about
/// the shell that is running — so the pass is skipped, says why in the
/// envelope, and touches nothing. A theme tweak must never be told it has dead
/// keybinds.
#[test]
fn a_profile_with_no_shell_is_skipped_rather_than_guessed_at() {
    let sandbox = Sandbox::new();
    sandbox.write_profile(
        "plain",
        &common::profile_toml(
            "plain",
            &[],
            &[],
            &[("qs", "qs -c $qsConfig", "pkill qs")],
            &[".config/hypr"],
        ),
    );
    sandbox.write_profile_file(
        "plain",
        ".config/hypr/hypridle.conf",
        "$lock_cmd = hyprctl dispatch 'hl.dsp.global(\"quickshell:lock\")'\n",
    );
    let before = stamp(&sandbox.profile_dir("plain"));

    let data = sandbox.run(&["switch", "plain"]).assert_ok();
    let reconciled = facts(&data);

    assert_eq!(reconciled["shell"], json!(null));
    assert_eq!(reconciled["files_written"], json!([]));
    assert!(
        reconciled["skipped"]
            .as_str()
            .is_some_and(|why| why.contains("names no shell")),
        "the envelope says why: {reconciled:?}"
    );
    assert_eq!(before, stamp(&sandbox.profile_dir("plain")));
    assert_eq!(data["report"]["shell_started"], Value::Null);
}

/// The engine reads and reports; it never shells out and never touches anything
/// outside the profile it was handed. Switching a profile with dispatches in it
/// adds no stub traffic beyond the locked switch sequence.
#[test]
fn reconciliation_shells_out_to_nothing() {
    let sandbox = Sandbox::new();
    donor_graft(&sandbox, "grafted");
    sandbox.clear_log();

    sandbox.run(&["switch", "grafted"]).assert_ok();

    let traffic = sandbox.log();
    for line in &traffic {
        assert!(
            !line.contains("hyprctl globalshortcuts"),
            "the engine derives statically and never live-probes: {traffic:?}"
        );
    }
    assert!(
        !traffic.iter().any(|line| line.starts_with("grep")),
        "no shelling out to read QML either: {traffic:?}"
    );
    // The only Hyprland calls are the version probe the pre-flight has always
    // made and the reload it has always made. Nothing here reads the running
    // session's shortcuts: the registry is derived from the profile's own QML,
    // so no foreign shell is ever booted to ask it what it registers.
    assert_eq!(
        traffic
            .iter()
            .filter(|line| line.starts_with("hyprctl"))
            .cloned()
            .collect::<Vec<String>>(),
        ["hyprctl reload", "hyprctl version"],
        "the engine is pure text: {traffic:?}"
    );
}
