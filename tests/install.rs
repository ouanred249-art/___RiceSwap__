//! Ticket #37: `riceswap install <git-url | local-path>` — acquisition,
//! identity, materialization, and the re-run that resumes a half-finished
//! install.
//!
//! Every test here drives the compiled binary as a subprocess against the
//! sandbox: an isolated fake `$HOME`, a stubbed `PATH` whose `git` really
//! clones and really answers `rev-parse`, and nothing else. There is no network
//! anywhere in this file — the `git` on `PATH` is the stub, always — and no test
//! reads anything inside the crate.
//!
//! The end-to-end case is the one that matters: a donor-graft rice whose
//! Hyprland configs dispatch the *previous* shell's shortcut names, installed
//! from a local directory. It asserts that the profile is built, that the
//! manifest records where the rice came from and which shell it is, that the
//! reconciliation engine repaired the dispatches *before* the first switch read
//! them, and that the switch itself ran.

mod common;

use common::{Mode, Sandbox};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

/// The commit the `git` stub answers `rev-parse HEAD` with, and the directory
/// the URL happy path expects to find it under.
const SHA: &str = "1a2b3c4d5e6f70819a2b3c4d5e6f70819a2b3c4d";

/// The repository the URL cases install from. Nothing connects to it: the `git`
/// stub answers every invocation, and only the argument list is ever read.
const URL: &str = "https://example.invalid/dotfiles/dotfiles.git";

// ------------------------------------------------------------------ fixtures

/// Writes a rice tree at `tree`: one Quickshell shell named `shell`, with the
/// marker file that makes the directory a shell at all.
///
/// `shell.qml` is the marker, so it is written for every shell this builder
/// makes — including the ones a refusal is about.
fn quickshell(tree: &Path, shell: &str) -> PathBuf {
    let dir = tree.join(".config").join("quickshell").join(shell);
    fs::create_dir_all(&dir).expect("create quickshell dir");
    fs::write(dir.join("shell.qml"), "import QtQuick\n\nShellRoot {}\n")
        .expect("write the shell marker");
    dir
}

/// A donor graft, reduced to the two files that matter.
///
/// The shape is the real one: the shell registers under its own appid, in the
/// base component every registration inherits from, and knows exactly one name.
/// The configs beside them dispatch the *donor's* vocabulary — `quickshell:*`,
/// Quickshell's default appid — with one name the new shell can answer
/// (`lock`) and one it cannot (`regionScreenshot`). That is the whole
/// reconciliation problem in two files: the first is a provable exact-name move,
/// the second is a proposal the engine must leave alone.
fn donor_graft(tree: &Path, shell: &str, appid: &str) {
    let qml = quickshell(tree, shell);
    fs::create_dir_all(qml.join("components").join("misc")).expect("create qml dir");
    fs::write(
        qml.join("components")
            .join("misc")
            .join("CustomShortcut.qml"),
        format!("import Quickshell.Hyprland\n\nGlobalShortcut {{\n    appid: \"{appid}\"\n}}\n"),
    )
    .expect("write the appid declaration");
    fs::create_dir_all(qml.join("modules").join("lock")).expect("create qml dir");
    fs::write(
        qml.join("modules").join("lock").join("Lock.qml"),
        "import QtQuick\n\nScope {\n    GlobalShortcut {\n        name: \"lock\"\n        onPressed: {}\n    }\n}\n",
    )
    .expect("write the registration");

    let hypr = tree.join(".config").join("hypr");
    fs::create_dir_all(&hypr).expect("create hypr dir");
    fs::write(
        hypr.join("hyprland.conf"),
        "monitor=,preferred,auto,1\n\nsource = ~/.config/hypr/hyprland/keybinds.lua\n",
    )
    .expect("write hyprland.conf");
    fs::write(
        hypr.join("hypridle.conf"),
        "$lock_cmd = hyprctl dispatch 'hl.dsp.global(\"quickshell:lock\")' & hyprlock\n",
    )
    .expect("write hypridle.conf");
    fs::create_dir_all(hypr.join("hyprland")).expect("create hypr dir");
    fs::write(
        hypr.join("hyprland").join("keybinds.lua"),
        concat!(
            "hl.bind(\"SUPER + SHIFT + S\", hl.dsp.global(\"quickshell:regionScreenshot\"), ",
            "{ description = \"Screen snip\" })\n",
            "hl.bind(\"SUPER + R\", hl.dsp.global(\"quickshell:riceswap-toggle\"), ",
            "{ description = \"Toggle RiceSwap\" })\n",
        ),
    )
    .expect("write keybinds.lua");
}

/// The wallpapers a rice bundles, in the directory the built-in caelestia recipe
/// itself declares as that shell's wallpaper path.
fn wallpapers(tree: &Path) {
    let dir = tree.join("Wallpapers");
    fs::create_dir_all(&dir).expect("create wallpaper dir");
    fs::write(dir.join("dawn.png"), common::image_fixture()).expect("write a wallpaper");
}

/// A complete donor-graft rice in the sandbox, outside `$HOME` — the directory
/// a user hands `install` while authoring a rice.
fn local_rice(sandbox: &Sandbox, shell: &str) -> PathBuf {
    let tree = sandbox.outside_home(&format!("rice-{shell}"));
    donor_graft(&tree, shell, shell);
    wallpapers(&tree);
    tree
}

/// A tree that carries a Hyprland config and no Quickshell shell at all.
fn not_a_rice(sandbox: &Sandbox) -> PathBuf {
    let tree = sandbox.outside_home("waybar-rice");
    let hypr = tree.join(".config").join("hypr");
    fs::create_dir_all(&hypr).expect("create hypr dir");
    fs::write(hypr.join("hyprland.conf"), "monitor=,preferred,auto,1\n").expect("write hypr");
    fs::create_dir_all(tree.join(".config").join("waybar")).expect("create waybar dir");
    fs::write(
        tree.join(".config").join("waybar").join("config.jsonc"),
        "{}\n",
    )
    .expect("write waybar");
    tree
}

/// The manifest an install wrote, read straight off disk so the test asserts on
/// the bytes rather than on what the loader makes of them.
fn manifest_of(sandbox: &Sandbox, name: &str) -> toml::Value {
    toml::from_str(&sandbox.profile_manifest(name)).expect("the manifest is valid TOML")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

/// The install's own facts out of the envelope.
fn source_of(data: &Value) -> &Value {
    &data["source"]
}

// ---------------------------------------------------------- the happy path

/// The end-to-end case: a local rice installs into a profile, the engine
/// repairs the donor's dispatch names before the first switch reads them, and
/// the switch runs.
#[test]
fn a_local_rice_installs_as_a_profile_reconciles_its_dispatches_and_switches() {
    let sandbox = Sandbox::new();
    let tree = local_rice(&sandbox, "demo");

    let run = sandbox.run(&["install", &tree.display().to_string()]);
    let data = run.assert_ok();

    // ---- what was written.
    assert_eq!(
        data["profile"],
        json!("demo"),
        "the profile is the shell's name"
    );
    assert_eq!(data["shell"], json!("demo"));
    assert_eq!(data["manifest_written"], json!(true));
    assert_eq!(data["resumed"], json!(false), "nothing to resume here");
    assert_eq!(data["identity"]["candidates"], json!(["demo"]));
    assert_eq!(data["identity"]["hypr"], json!(true));

    // The manifest records where the rice came from — as a canonical `file://`
    // path, not as the string the command happened to be given — and no
    // commit, because the directory is not a checkout.
    let manifest = manifest_of(&sandbox, "demo");
    let expected_url = format!(
        "file://{}",
        tree.canonicalize()
            .expect("canonical fixture path")
            .display()
    );
    assert_eq!(
        manifest["profile"]["source_url"].as_str(),
        Some(expected_url.as_str()),
    );
    assert_eq!(
        manifest["profile"].get("source_commit"),
        None,
        "a plain directory has no commit to record"
    );
    assert_eq!(manifest["profile"]["name"].as_str(), Some("demo"));
    assert_eq!(
        manifest["profile"]["description"].as_str(),
        Some(format!("installed from {expected_url}").as_str()),
        "the description says what it was installed from"
    );
    assert_eq!(
        manifest["profile"]["screenshot"].as_str(),
        Some(""),
        "an acquired tree is not running, so there is nothing to photograph"
    );

    // The `[shell]` table is what lets a switch treat two profiles as two
    // shells, so an install has to write it even though nothing was running.
    assert_eq!(manifest["shell"]["name"].as_str(), Some("demo"));
    assert_eq!(manifest["shell"]["start"].as_str(), Some("qs -c demo"));
    assert_eq!(manifest["shell"]["stop"].as_str(), Some("pkill qs"));

    // The snapshot machinery ran: the detected config dirs are mirrored into the
    // profile and recorded as `[[files]]`, and the captured Hyprland config is
    // connected to the shared hardware layer.
    let files: Vec<&str> = manifest["files"]
        .as_array()
        .expect("files is a list")
        .iter()
        .filter_map(|entry| entry["path"].as_str())
        .collect();
    assert!(files.contains(&".config/hypr"), "mirrored: {files:?}");
    assert!(
        files.contains(&".config/quickshell/demo"),
        "mirrored: {files:?}"
    );
    let captured = read(
        &sandbox
            .profile_dir("demo")
            .join(".config/hypr/hyprland.conf"),
    );
    assert!(
        captured.contains("source = ~/.config/hypr/riceswap/hardware.conf"),
        "the captured config is on the shared hardware layer: {captured}"
    );

    // The wallpapers the rice bundles were imported — by copy, because the tree
    // is read-only to an install.
    let imported = data["wallpapers"]
        .as_array()
        .expect("wallpapers is a list")
        .iter()
        .map(|path| path.as_str().expect("a path").to_string())
        .collect::<Vec<String>>();
    assert_eq!(
        imported,
        vec![
            sandbox
                .wallpapers_dir()
                .join("dawn.png")
                .display()
                .to_string()
        ]
    );
    assert_eq!(
        fs::read(&imported[0]).expect("imported bytes"),
        common::image_fixture()
    );
    assert!(
        tree.join("Wallpapers/dawn.png").is_file(),
        "the source keeps its own wallpapers: an install copies, it never takes"
    );

    // The engine's own report, in the shape the switch envelope carries.
    let reconciled = &data["reconcile"];
    assert_eq!(reconciled["shell"], json!("demo"));
    assert_eq!(
        reconciled["appid"],
        json!("demo"),
        "the tree declares its appid"
    );
    assert_eq!(
        reconciled["registered"],
        json!(1),
        "one provable registration"
    );
    assert_eq!(
        reconciled["dead_names"],
        json!(["quickshell:regionScreenshot"])
    );
    assert!(
        reconciled["backups"]
            .as_array()
            .is_some_and(|backups| !backups.is_empty()),
        "a rewrite keeps the pre-rewrite bytes: {:?}",
        reconciled["backups"]
    );
    assert_eq!(
        reconciled["proposals"][0]["target"],
        json!("demo:lock"),
        "the applied move is reported as a proposal with its target"
    );

    // The engine ran before the switch read anything: the donor's provable
    // exact-name move landed, and the unprovable proposal stayed untouched.
    let hypridle = read(
        &sandbox
            .profile_dir("demo")
            .join(".config/hypr/hypridle.conf"),
    );
    assert!(
        hypridle.contains("demo:lock"),
        "the exact-name move applied: {hypridle}"
    );
    let keybinds = read(
        &sandbox
            .profile_dir("demo")
            .join(".config/hypr/hyprland/keybinds.lua"),
    );
    assert!(
        keybinds.contains("quickshell:regionScreenshot"),
        "the proposal was left for the declarative tiers: {keybinds}"
    );
    assert!(
        keybinds.contains("quickshell:riceswap-toggle"),
        "the foreign guard survived the whole pipeline: {keybinds}"
    );

    // The switch ran as the install's own step, and its facts ride under
    // `switch`: the locked sequence completed, the compositor reloaded, the
    // shell this profile owns was started, and `current` names it.
    let switched = &data["switch"];
    assert_eq!(switched["target"], json!("demo"));
    assert_eq!(
        switched["completed_steps"],
        json!(10),
        "the locked switch sequence"
    );
    assert_eq!(switched["report"]["reloaded"], json!(true));
    assert_eq!(switched["report"]["shell_started"], json!("demo"));
    assert!(
        sandbox.log_contains("hyprctl reload"),
        "the switch reloaded"
    );
    assert!(sandbox.log_contains("qs -c demo"), "the shell was started");
    assert_eq!(
        sandbox.current_target(),
        Some(sandbox.profile_dir("demo")),
        "`current` points at the profile that was just installed"
    );
    // The managed path the reload read is the profile's own file, reached
    // through the link the switch makes — the repair the engine wrote is what
    // the compositor loaded, not a copy of it.
    let live = sandbox.home().join(".config/hypr");
    assert!(
        fs::symlink_metadata(&live).is_ok_and(|meta| meta.file_type().is_symlink()),
        "the switch links the profile's own config dir into place: {:?}",
        fs::read_link(&live)
    );
    assert_eq!(
        fs::canonicalize(&live).expect("the link resolves"),
        fs::canonicalize(sandbox.profile_dir("demo").join(".config/hypr")).expect("in the store")
    );
    assert!(
        read(&live.join("hypridle.conf")).contains("demo:lock"),
        "and the live config carries the repair"
    );
    assert_eq!(source_of(&data)["url"], json!(expected_url));
    assert_eq!(
        source_of(&data)["cache"],
        json!(null),
        "a local path is not cached"
    );

    // `resumed: false` above was the claim; the journal closed on success.
    assert_eq!(
        sandbox.state()["operation"],
        Value::Null,
        "the claim closed"
    );
    assert_eq!(sandbox.state()["active_profile"], json!("demo"));
    run.assert_no_panic();
}

// ------------------------------------------------------------- the refusals

/// A tree with no Quickshell shell is refused even though it is a perfectly
/// good Hyprland desktop: that refusal is the gate keeping a rice this tool
/// cannot verify out of the store, and it says what it did find.
#[test]
fn a_tree_without_a_quickshell_shell_is_refused() {
    let sandbox = Sandbox::new();
    let tree = not_a_rice(&sandbox);

    let run = sandbox.run(&["install", &tree.display().to_string()]);
    let message = run.assert_failed();

    assert!(message.contains("not a quickshell rice"), "got {message}");
    assert!(
        message.contains("shell.qml"),
        "the refusal names the marker it looked for: {message}"
    );
    let envelope = run.envelope();
    assert_eq!(
        envelope["data"]["reason_code"],
        json!("not-a-quickshell-rice")
    );
    assert_eq!(envelope["data"]["phase"], json!("identify"));
    assert_eq!(envelope["data"]["hypr_config"], json!(true));
    assert!(
        envelope["data"]["next"]
            .as_str()
            .is_some_and(|next| !next.is_empty()),
        "the refusal says what to do about it"
    );
    assert!(
        !sandbox.profiles_dir().exists(),
        "a refusal creates no profile"
    );
    assert!(!sandbox.data_dir().join("sources").exists(), "and no cache");
    run.assert_no_panic();
}

/// A Quickshell config with no `shell.qml` is somebody's settings folder, not a
/// shell — so a tree carrying only that is refused, and the refusal names the
/// near miss rather than shrugging.
#[test]
fn a_quickshell_config_without_the_marker_is_not_a_shell() {
    let sandbox = Sandbox::new();
    let tree = sandbox.outside_home("half-rice");
    let dir = tree.join(".config").join("quickshell").join("settings");
    fs::create_dir_all(&dir).expect("create quickshell dir");
    fs::write(dir.join("colors.qml"), "import QtQuick\n").expect("write a component");

    let run = sandbox.run(&["install", &tree.display().to_string()]);
    let message = run.assert_failed();

    assert!(message.contains("not a quickshell rice"), "got {message}");
    assert!(
        message.contains("settings"),
        "the near miss is named: {message}"
    );
}

/// Two shells and no `--shell` is a refusal: guessing installs the wrong
/// desktop, so the candidates are the refusal's content.
#[test]
fn two_shells_without_a_shell_flag_is_refused_with_the_candidates() {
    let sandbox = Sandbox::new();
    let tree = sandbox.outside_home("two-shells");
    quickshell(&tree, "alpha");
    quickshell(&tree, "beta");
    fs::create_dir_all(tree.join(".config").join("hypr")).expect("create hypr dir");

    let run = sandbox.run(&["install", &tree.display().to_string()]);
    let message = run.assert_failed();

    assert!(
        message.contains("more than one quickshell shell"),
        "got {message}"
    );
    assert!(
        message.contains("alpha, beta"),
        "both candidates: {message}"
    );
    assert!(
        message.contains("--shell"),
        "the override is offered: {message}"
    );
    assert_eq!(
        run.envelope()["data"]["reason_code"],
        json!("ambiguous-identity")
    );
    assert_eq!(
        run.envelope()["data"]["candidates"],
        json!(["alpha", "beta"])
    );
    assert!(!sandbox.profile_dir("alpha").exists());
    assert!(!sandbox.profile_dir("beta").exists());
}

/// `--shell` picks between them, and the profile takes the chosen shell's name.
#[test]
fn the_shell_flag_chooses_which_shell_to_install() {
    let sandbox = Sandbox::new();
    let tree = sandbox.outside_home("two-shells");
    quickshell(&tree, "alpha");
    quickshell(&tree, "beta");
    fs::create_dir_all(tree.join(".config").join("hypr")).expect("create hypr dir");

    let data = sandbox
        .run(&["install", &tree.display().to_string(), "--shell", "beta"])
        .assert_ok();

    assert_eq!(data["profile"], json!("beta"));
    assert_eq!(data["identity"]["candidates"], json!(["alpha", "beta"]));
    assert_eq!(data["identity"]["selected"], json!("beta"));
    assert_eq!(
        manifest_of(&sandbox, "beta")["shell"]["name"].as_str(),
        Some("beta")
    );
    assert_eq!(
        manifest_of(&sandbox, "beta")["shell"]["start"].as_str(),
        Some("qs -c beta")
    );
    assert!(
        !sandbox.profile_dir("alpha").exists(),
        "only the chosen shell is installed"
    );
}

/// `--shell` naming a shell the tree does not carry is an error, and it says
/// which ones it does carry.
#[test]
fn the_shell_flag_naming_no_candidate_is_refused() {
    let sandbox = Sandbox::new();
    let tree = local_rice(&sandbox, "demo");

    let run = sandbox.run(&[
        "install",
        &tree.display().to_string(),
        "--shell",
        "caelestia",
    ]);
    let message = run.assert_failed();

    assert!(
        message.contains("caelestia"),
        "the offender is named: {message}"
    );
    assert!(
        message.contains("demo"),
        "the candidates are named: {message}"
    );
    assert!(!sandbox.profile_dir("caelestia").exists());
    assert!(!sandbox.profile_dir("demo").exists());
}

/// A profile that is already there is not something a plain re-run may clear:
/// installing is not updating, and the profile may be one the user lives in.
/// A *finished* install also closed its journal claim, which is what makes the
/// second identical run a refusal rather than a resume.
#[test]
fn a_second_identical_install_of_a_local_rice_is_refused() {
    let sandbox = Sandbox::new();
    let tree = local_rice(&sandbox, "demo");
    sandbox
        .run(&["install", &tree.display().to_string()])
        .assert_ok();
    let manifest_before = sandbox.profile_manifest("demo");

    let run = sandbox.run(&["install", &tree.display().to_string()]);
    let message = run.assert_failed();

    assert!(message.contains("already exists"), "got {message}");
    assert!(
        message.contains("riceswap delete demo --force"),
        "the refusal says how to proceed anyway: {message}"
    );
    assert_eq!(
        run.envelope()["data"]["reason_code"],
        json!("profile-exists")
    );
    // Refused before anything was written: the live profile is byte-for-byte
    // what the first install left, and the desktop never moved.
    assert_eq!(sandbox.profile_manifest("demo"), manifest_before);
    assert_eq!(sandbox.current_target(), Some(sandbox.profile_dir("demo")));
    assert_eq!(
        sandbox.state()["last_result"]["ok"],
        json!(false),
        "the refused run is recorded as a failure"
    );

    // Refuse again — and this is the regression: a refusal must not leave a
    // "did not finish" claim behind that the *next* run reads as permission to
    // re-make and clear a profile that is complete. Before the fix, this third
    // invocation resumed and wiped the profile; now it refuses identically and
    // the profile is still untouched.
    let third = sandbox.run(&["install", &tree.display().to_string()]);
    assert!(
        third.assert_failed().contains("already exists"),
        "a refusal never plants a claim that resumes the profile"
    );
    assert_eq!(sandbox.profile_manifest("demo"), manifest_before);
    assert_eq!(sandbox.current_target(), Some(sandbox.profile_dir("demo")));
}

/// A local rice is read where it stands and never copied into the cache: the
/// user's directory is the source of truth, so edits made after a first install
/// are what a second one sees.
#[test]
fn a_local_rice_is_read_where_it_stands_and_never_copied_into_the_cache() {
    let sandbox = Sandbox::new();
    let tree = local_rice(&sandbox, "demo");
    sandbox
        .run(&["install", &tree.display().to_string()])
        .assert_ok();
    sandbox.run(&["delete", "demo", "--force"]).assert_ok();

    fs::write(
        tree.join(".config/hypr/hypridle.conf"),
        "$lock_cmd = hyprctl dispatch 'hl.dsp.global(\"demo:lock\")' & hyprlock\n",
    )
    .expect("edit the rice in place");
    sandbox
        .run(&["install", &tree.display().to_string()])
        .assert_ok();

    assert!(
        read(
            &sandbox
                .profile_dir("demo")
                .join(".config/hypr/hypridle.conf")
        )
        .contains("demo:lock"),
        "the second install read the directory as it now stands"
    );
    assert!(
        !sandbox.data_dir().join("sources").exists(),
        "a local path is never copied into the acquisition cache"
    );
}

/// What a dead install's state.json looks like: a named operation with a
/// target, and — for an install — the source it claimed. `None` writes the
/// journal without the source field, which is how a switch's record reads.
///
/// The document is created when it is not there yet, because a process that
/// died during an install may well have died before anything wrote one: the
/// frozen shape is written whole either way, so what the loader reads is the
/// same document in both cases.
fn journal(sandbox: &Sandbox, name: &str, target: &str, source: Option<&str>) {
    let mut document: Value = fs::read_to_string(sandbox.state_path())
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_else(|| {
            json!({
                "initialized": true,
                "active_profile": null,
                "operation": null,
                "last_result": null,
            })
        });
    document["operation"] = json!({
        "name": name,
        "target": target,
        "started_at": 1_759_000_000,
        "step": 3,
        "source": source,
    });
    if source.is_none() {
        document["operation"]
            .as_object_mut()
            .expect("object")
            .remove("source");
    }
    if let Some(parent) = sandbox.state_path().parent() {
        fs::create_dir_all(parent).expect("create the state directory");
    }
    fs::write(sandbox.state_path(), document.to_string()).expect("write the journal");
}

/// A profile an install started and never finished is re-made by the plain
/// re-invocation — no flag, no cleaning up by hand.
#[test]
fn a_half_finished_install_is_resumed_by_a_plain_re_run() {
    let sandbox = Sandbox::new();
    let tree = local_rice(&sandbox, "demo");
    let url = format!(
        "file://{}",
        tree.canonicalize().expect("canonical fixture").display()
    );

    // What an install that died mid-way leaves: the profile directory it had
    // already cleared and half-written, and the journal entry saying so.
    fs::create_dir_all(sandbox.profile_dir("demo")).expect("half-made profile");
    fs::write(
        sandbox.profile_dir("demo").join("half-written.lua"),
        "-- a file the dead install had copied\n",
    )
    .expect("half-written file");
    journal(&sandbox, "install", "demo", Some(&url));

    let run = sandbox.run(&["install", &tree.display().to_string()]);
    let data = run.assert_ok();

    assert_eq!(data["resumed"], json!(true), "the re-run is a resume");
    assert!(
        !sandbox
            .profile_dir("demo")
            .join("half-written.lua")
            .exists(),
        "the resume re-makes the profile rather than continuing inside it"
    );
    assert!(sandbox.profile_dir("demo").join("profile.toml").is_file());
    assert_eq!(
        data["switch"]["target"],
        json!("demo"),
        "and finishes the switch"
    );
    assert_eq!(sandbox.current_target(), Some(sandbox.profile_dir("demo")));
    assert!(
        run.warnings()
            .iter()
            .any(|warning| warning.contains("resuming a half-finished install")),
        "the resume says itself out loud: {:?}",
        run.warnings()
    );
}

/// Only an install that claimed *this* profile from *this* source may re-make it.
/// A switch of the same profile is not that: switching touches a profile the
/// user already has, and it must never make the profile look re-installable.
#[test]
fn a_switch_in_the_journal_does_not_make_a_profile_resumable() {
    let sandbox = Sandbox::new();
    let tree = local_rice(&sandbox, "demo");
    let url = format!(
        "file://{}",
        tree.canonicalize().expect("canonical fixture").display()
    );

    // A profile the user already has, with a manifest worth checking against.
    sandbox.write_profile(
        "demo",
        &common::profile_toml("demo", &[], &[], &[], &[".config/hypr"]),
    );
    let before = sandbox.profile_manifest("demo");
    // A switch of that very profile is what the journal holds — even naming the
    // same source, which no switch ever does.
    journal(&sandbox, "switch", "demo", Some(&url));

    let run = sandbox.run(&["install", &tree.display().to_string()]);
    let message = run.assert_failed();

    assert!(message.contains("already exists"), "got {message}");
    assert_eq!(
        run.envelope()["data"]["reason_code"],
        json!("profile-exists")
    );
    assert_eq!(
        sandbox.profile_manifest("demo"),
        before,
        "the existing profile is byte-for-byte untouched"
    );
    assert_eq!(
        sandbox.current_target(),
        None,
        "and nothing was switched to"
    );
}

/// The same gate, from the other side: an install of a *different* source never
/// inherits the permission to clear another source's half-made profile, and the
/// refusal says so.
#[test]
fn a_half_finished_install_of_another_source_is_not_resumed() {
    let sandbox = Sandbox::new();
    let tree = local_rice(&sandbox, "demo");

    fs::create_dir_all(sandbox.profile_dir("demo")).expect("half-made profile");
    journal(
        &sandbox,
        "install",
        "demo",
        Some("file:///somewhere/else/rice"),
    );

    let message = sandbox
        .run(&["install", &tree.display().to_string()])
        .assert_failed();

    assert!(message.contains("already exists"), "got {message}");
    assert!(
        message.contains("file:///somewhere/else/rice"),
        "the refusal names the source that left it: {message}"
    );
}

/// An install that fails leaves its journal entry behind on purpose: the steps
/// are idempotent and re-running the command is how a user resumes.
#[test]
fn a_failed_install_stays_marked_as_unfinished() {
    let sandbox = Sandbox::new();
    let rice = profile_with_aur(&sandbox);
    // The rice names an AUR package and no helper can answer for it: the
    // switch refuses after the profile has already been written, which is the
    // half-failed install a plain re-run has to be able to finish.
    sandbox.script("yay", Mode::Fail);
    sandbox.script("paru", Mode::Fail);

    let run = sandbox.run(&["install", &rice.display().to_string()]);
    let message = run.assert_failed();
    assert!(
        message.contains("AUR"),
        "the switch's own failure rides through: {message}"
    );
    assert_eq!(run.envelope()["data"]["phase"], json!("switch"));
    assert!(
        run.envelope()["data"]["switch"]["completed_steps"].is_number(),
        "the switch's own report rides through: {:?}",
        run.envelope()["data"]["switch"]
    );
    assert!(
        sandbox.profile_dir("demo").join("profile.toml").is_file(),
        "the profile was written before the switch failed"
    );

    let state = sandbox.state();
    let operation = &state["operation"];
    assert_eq!(operation["name"], json!("install"), "the marker survives");
    assert_eq!(operation["target"], json!("demo"));
    assert_eq!(
        operation["source"].as_str(),
        Some(
            format!(
                "file://{}",
                rice.canonicalize().expect("canonical").display()
            )
            .as_str()
        ),
        "and names the source it was installing"
    );
    assert_eq!(state["last_result"]["ok"], json!(false));
    assert!(
        run.envelope()["data"]["resume_hint"]
            .as_str()
            .is_some_and(|hint| hint.contains("re-run")),
        "the failure says how to resume: {:?}",
        run.envelope()["data"]
    );

    // And the plain re-invocation is the resume: no flag, no cleaning up.
    sandbox.script("yay", Mode::Ok);
    sandbox.script("paru", Mode::Ok);
    let data = sandbox
        .run(&["install", &rice.display().to_string()])
        .assert_ok();
    assert_eq!(data["resumed"], json!(true));
    assert_eq!(
        sandbox.state()["operation"],
        Value::Null,
        "the claim closed"
    );
}

/// A rice whose Hyprland config names a binary the machine would have to fetch
/// from the AUR — the one dependency shape that makes a first switch fail on
/// its own, so the install's failure path can be driven end to end.
///
/// The reference is a plain `exec` rather than an `exec-once`, so it contributes
/// a package and not a service the sandbox has no binary to start. The ownership
/// is declared in the pacman fixtures, which is how the detection scan resolves
/// a referenced binary to an AUR package at all.
fn profile_with_aur(sandbox: &Sandbox) -> PathBuf {
    let tree = sandbox.outside_home("aur-rice");
    quickshell(&tree, "demo");
    let hypr = tree.join(".config").join("hypr");
    fs::create_dir_all(&hypr).expect("create hypr dir");
    fs::write(hypr.join("hyprland.conf"), "monitor=,preferred,auto,1\n")
        .expect("write hyprland.conf");
    fs::write(hypr.join("packages.conf"), "exec = yay-curses\n").expect("write the dep line");
    sandbox.own("yay-curses", "yay-curses", true);
    tree
}

// --------------------------------------------------------------- the URL path

/// A URL install clones the repository in full into the cache, under the repo's
/// slug and the commit the clone reported, and records both facts.
#[test]
fn a_url_install_clones_the_whole_repository_into_the_cache() {
    let sandbox = Sandbox::new();
    let rice = local_rice(&sandbox, "demo");
    sandbox.clone_from(&rice);
    sandbox.clear_log();

    let run = sandbox.run(&["install", URL]);
    let data = run.assert_ok();

    // The clone is a plain `git clone <url> <dest>`: no `--depth`, no
    // `--single-branch`, no `--branch`. Dotfiles are tiny and the commit pin
    // and every later update need the history.
    let clone = sandbox
        .log()
        .into_iter()
        .find(|line| line.starts_with("git clone "))
        .expect("git was asked to clone");
    let staging = sandbox
        .data_dir()
        .join("sources")
        .join("dotfiles")
        .join("clone.staged");
    assert_eq!(
        clone,
        format!("git clone {URL} {}", staging.display()),
        "a full clone of the default branch, staged beside its final home"
    );
    assert!(
        !clone.contains("--depth"),
        "the clone is not shallow: {clone}"
    );
    assert!(
        !clone.contains("--branch"),
        "it is the default branch: {clone}"
    );
    assert!(
        sandbox.log_contains(&format!("git -C {} rev-parse HEAD", staging.display())),
        "the commit is read off the clone itself: {:?}",
        sandbox.log()
    );

    // The cache is per repo, per commit — the sha the clone reported, which the
    // stub answers for, is the directory name.
    let cached = sandbox
        .data_dir()
        .join("sources")
        .join("dotfiles")
        .join(SHA);
    assert!(
        cached.join(".config/quickshell/demo/shell.qml").is_file(),
        "the clone is under sources/<slug>/<commit-sha>/"
    );

    let source = source_of(&data);
    assert_eq!(source["url"], json!(URL));
    assert_eq!(source["commit"], json!(SHA));
    assert_eq!(source["cache"]["slug"], json!("dotfiles"));
    assert_eq!(source["cache"]["commit"], json!(SHA));
    assert_eq!(source["cache"]["path"], json!(cached.display().to_string()));
    assert_eq!(
        source["cache"]["reused"],
        json!(false),
        "the first clone is not a reuse"
    );

    // The profile holds copies: it was built from the cache, and nothing about
    // it points back at the cache.
    assert_eq!(data["profile"], json!("demo"));
    let manifest = manifest_of(&sandbox, "demo");
    assert_eq!(manifest["profile"]["source_url"].as_str(), Some(URL));
    assert_eq!(manifest["profile"]["source_commit"].as_str(), Some(SHA));
    assert!(
        sandbox
            .profile_dir("demo")
            .join(".config/hypr/hypridle.conf")
            .is_file()
    );
    assert!(
        read(
            &sandbox
                .profile_dir("demo")
                .join(".config/hypr/hypridle.conf")
        )
        .contains("demo:lock"),
        "the install read the clone, and the engine repaired it there"
    );
    run.assert_no_panic();
}

/// The second install of a URL is a cache read, not a clone.
///
/// This is the reuse rule stated as a test, and it is deliberately boring: the
/// cache is keyed by the URL, so the second install of that URL installs the
/// clone this tool already has. It does not go and ask the remote whether that
/// clone is current — asking is a network round-trip, and this operation is
/// specified to work offline. Re-fetching a moved default branch is *updating*,
/// which is a different operation and not this one, so the answer here is the
/// same tree twice.
#[test]
fn a_second_install_of_the_same_url_reads_the_cache_instead_of_cloning_again() {
    let sandbox = Sandbox::new();
    let rice = local_rice(&sandbox, "demo");
    sandbox.clone_from(&rice);
    let first = sandbox.run(&["install", URL]).assert_ok();
    sandbox.run(&["delete", "demo", "--force"]).assert_ok();
    sandbox.clear_log();

    let data = sandbox.run(&["install", URL]).assert_ok();

    assert_eq!(
        source_of(&data)["cache"]["reused"],
        json!(true),
        "the second install reused the clone"
    );
    assert_eq!(
        source_of(&data)["cache"]["path"],
        source_of(&first)["cache"]["path"],
        "the very same cache entry"
    );
    assert!(
        !sandbox.log_contains("git clone"),
        "nothing was cloned again: {:?}",
        sandbox.log()
    );
    assert!(
        !sandbox.log_contains("rev-parse"),
        "and nothing asked git about a commit it had already recorded: {:?}",
        sandbox.log()
    );
    // One entry per repo, keyed by the commit the clone reported.
    let repo = sandbox.data_dir().join("sources").join("dotfiles");
    let mut entries = fs::read_dir(&repo)
        .expect("the cache dir")
        .flatten()
        .map(|entry| entry.file_name())
        .collect::<Vec<_>>();
    entries.sort();
    assert_eq!(
        entries,
        vec![
            std::ffi::OsStr::new("1a2b3c4d5e6f70819a2b3c4d5e6f70819a2b3c4d"),
            std::ffi::OsStr::new("head.json")
        ],
        "no second clone directory was created: {repo:?}"
    );
    // And the profile was built again from that cache entry, copies and all.
    assert_eq!(data["profile"], json!("demo"));
    assert_eq!(
        manifest_of(&sandbox, "demo")["profile"]["source_commit"].as_str(),
        Some(SHA)
    );
}

/// No `git` at all: refused with the frozen reason code, before a directory
/// exists anywhere.
#[test]
fn a_url_install_without_a_usable_git_creates_nothing() {
    let sandbox = Sandbox::new();
    sandbox.script("git", Mode::Fail);

    let run = sandbox.run(&["install", URL]);
    let message = run.assert_failed();

    assert!(
        message.contains("git"),
        "the refusal names the tool: {message}"
    );
    assert_eq!(run.envelope()["data"]["reason_code"], json!("git-missing"));
    assert!(
        !sandbox.data_dir().join("sources").exists(),
        "not even an empty cache directory is left behind"
    );
    assert!(!sandbox.profiles_dir().exists());
}

/// A `git` that clones cleanly on its version probe and then fails the clone is
/// a different refusal — and it unwinds its own staging, so no half-clone is
/// left under `sources/` for a later run to mistake for a cache entry.
#[test]
fn a_failed_clone_leaves_no_cache_entry_behind() {
    let sandbox = Sandbox::new();
    sandbox.clone_from(&local_rice(&sandbox, "demo"));
    sandbox.fail_on("git", "clone");

    let run = sandbox.run(&["install", URL]);
    let message = run.assert_failed();

    assert!(message.contains("clone"), "got {message}");
    assert_eq!(run.envelope()["data"]["reason_code"], json!("clone-failed"));
    assert!(
        !sandbox.data_dir().join("sources").exists(),
        "the staged clone was unwound: {:?}",
        sandbox.data_dir().join("sources")
    );
    assert!(!sandbox.profiles_dir().exists());
    assert_eq!(sandbox.state()["last_result"]["ok"], json!(false));
}

/// A source that is not there at all is refused before anything is read, and the
/// message says what install takes.
#[test]
fn a_source_that_is_not_a_directory_or_a_url_is_refused() {
    let sandbox = Sandbox::new();
    let missing = sandbox.outside_home("no-such-rice").display().to_string();

    let message = sandbox.run(&["install", &missing]).assert_failed();

    assert!(message.contains("git URL"), "got {message}");
    assert!(!sandbox.profiles_dir().exists());
}

/// The `install` operation is a first-class one in the CLI surface: it needs a
/// source, it takes `--shell`, and it rejects flags it does not know rather
/// than ignoring them.
#[test]
fn install_takes_one_source_and_the_shell_flag() {
    let sandbox = Sandbox::new();

    let message = sandbox.run::<&str>(&["install"]).assert_failed();
    assert!(message.contains("<source>"), "got {message}");

    let message = sandbox.run(&["install", "a", "b"]).assert_failed();
    assert!(message.contains("one argument"), "got {message}");

    let message = sandbox
        .run(&["install", "/tmp", "--shells", "demo"])
        .assert_failed();
    assert!(message.contains("--shells"), "got {message}");

    let message = sandbox.run(&["install", "/tmp", "--shell"]).assert_failed();
    assert!(message.contains("--shell"), "got {message}");
}
