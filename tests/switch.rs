//! Ticket #14: `plan` and `switch` through the operation surface — the diff
//! between fixture profiles (install/remove/symlink/service sets and blocked
//! paths), the locked switch sequence against stubbed pacman, the install-first
//! conflict fallback, kept-package refusals, service-start warnings, SIGTERM
//! cancellation at a step boundary, idempotent re-runs, and the preservation of
//! a real file the switch is about to replace.

mod common;

use common::{Mode, Run, Sandbox, profile_toml};
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::symlink;
use std::time::{Duration, Instant};

/// The active profile A: a waybar rice with an official package the target
/// no longer wants, a shared package both declare, and an AUR package of its
/// own. Its managed paths (`~/.config/waybar`, `~/.config/hypr`) are symlinked
/// into `$HOME` the way activation leaves them.
fn alpha_manifest() -> String {
    profile_toml(
        "alpha",
        &["oldbar", "shared"],
        &["oldaur"],
        &[("waybar", "waybar", "pkill waybar")],
        &[".config/waybar", ".config/hypr"],
    )
}

/// The target profile B: a different bar, a new official and AUR package, a
/// shared package with A, and one managed path A does not have (kitty) while
/// dropping A's Hyprland dir.
fn beta_manifest() -> String {
    profile_toml(
        "beta",
        &["newbar", "shared"],
        &["newaur"],
        &[("ags", "ags", "pkill ags")],
        &[".config/waybar", ".config/kitty"],
    )
}

/// The switch fixture: both profiles on disk with mirrored files, the live
/// managed layout as activation left it, and `current` pointing at alpha.
fn fixture(sandbox: &Sandbox) {
    fixture_with(sandbox, &alpha_manifest());
}

/// Same fixture with a caller-supplied manifest for alpha, so a test can add
/// the packages its scenario needs (a still-needed refusal, ...).
fn fixture_with(sandbox: &Sandbox, alpha: &str) {
    sandbox.write_profile("alpha", alpha);
    sandbox.write_profile("beta", &beta_manifest());
    sandbox.write_profile_file(
        "alpha",
        ".config/waybar/config.jsonc",
        "{ \"rice\": \"alpha\" }\n",
    );
    sandbox.write_profile_file(
        "alpha",
        ".config/hypr/hyprland.conf",
        "exec-once = waybar\n",
    );
    sandbox.write_profile_file(
        "beta",
        ".config/waybar/config.jsonc",
        "{ \"rice\": \"beta\" }\n",
    );
    sandbox.write_profile_file("beta", ".config/kitty/kitty.conf", "font_size 12\n");

    fs::create_dir_all(sandbox.home().join(".config")).expect("create .config");
    symlink(
        sandbox.profile_dir("alpha").join(".config/waybar"),
        sandbox.home().join(".config/waybar"),
    )
    .expect("manage waybar through alpha");
    symlink(
        sandbox.profile_dir("alpha").join(".config/hypr"),
        sandbox.home().join(".config/hypr"),
    )
    .expect("manage hypr through alpha");
    sandbox.activate("alpha");
}

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

/// Sends SIGTERM to the spawned operation — the cancellation the switch must
/// absorb until its next step boundary.
fn send_sigterm(pid: u32) {
    let status = std::process::Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status()
        .expect("run kill");
    assert!(status.success(), "SIGTERM must reach the operation");
}

/// The index of the first NDJSON line before the final envelope satisfying
/// `predicate` — for asserting where a warning landed in the live stream.
fn stream_index(run: &Run, predicate: impl Fn(&Value) -> bool) -> Option<usize> {
    run.lines[..run.lines.len() - 1].iter().position(|line| {
        serde_json::from_str::<Value>(line)
            .map(|value| predicate(&value))
            .unwrap_or(false)
    })
}

/// The stub log reduced to the commands underneath the privilege wrappers of
/// ticket #16: `pkexec pacman -S x` and the pacman it executed both become
/// `pacman -S x`, so consecutive duplicates collapse and the *locked sequence
/// itself* stays the assertion — wrappers are asserted separately in
/// `tests/privilege.rs`.
fn locked_commands(sandbox: &Sandbox) -> Vec<String> {
    let mut commands: Vec<String> = Vec::new();
    for line in sandbox.log() {
        let trimmed = line.trim_end();
        let stripped = trimmed.strip_prefix("pkexec ").unwrap_or(trimmed);
        let stripped = stripped.strip_prefix("riceswap-float ").unwrap_or(stripped);
        if commands
            .last()
            .is_some_and(|last| last.as_str() == stripped)
        {
            continue;
        }
        commands.push(stripped.to_string());
    }
    commands
}

/// Class: pre-flight plan. Between two fixture profiles `plan` returns the
/// correct install/remove/symlink/service sets, and a real file at a target
/// path shows up in `blocked_paths`.
#[test]
fn plan_between_fixture_profiles_reports_the_diff_and_blocked_paths() {
    let sandbox = Sandbox::new();
    fixture(&sandbox);

    let run = sandbox.run(&["plan", "beta"]);
    let data = run.assert_ok();

    assert_eq!(data.get("stub"), None, "plan is a real diff now: {data:?}");
    assert_eq!(data["target"], json!("beta"));
    assert_eq!(
        data["package_diff"],
        json!({
            "install": { "official": ["newbar"], "aur": ["newaur"] },
            "remove": { "official": ["oldbar"], "aur": ["oldaur"] },
        }),
        "installs are B minus A, removals are A's unique packages"
    );
    assert_eq!(
        data["service_changes"],
        json!({ "stop": ["waybar"], "start": ["ags"] })
    );
    assert_eq!(
        data["symlink_changes"],
        json!({
            "link": [".config/kitty", ".config/waybar"],
            "unlink": [".config/hypr"],
        }),
        "B's paths to link, A's paths B no longer manages to unlink"
    );
    assert_eq!(
        data["blocked_paths"],
        json!([]),
        "the live managed paths are symlinks, not real files"
    );

    // A real file where beta wants a symlink is flagged, and the plan says which
    // side of the identical/backed-up split it falls on.
    sandbox.write_home(".config/kitty", "my own kitty config\n");
    let data = sandbox.run(&["plan", "beta"]).assert_ok();
    assert_eq!(data["blocked_paths"], json!([".config/kitty"]));
    assert_eq!(data["backed_up_paths"], json!([".config/kitty"]));
    assert_eq!(data["identical_paths"], json!([]));

    // A missing target refuses through the failed envelope, naming it.
    let message = sandbox.run(&["plan", "ghost"]).assert_failed();
    assert!(
        message.contains("ghost"),
        "the refusal must name the profile: {message}"
    );

    // A first switch has no active manifest, so the package diff names every
    // declared package. `install_missing` is that set against what the machine
    // actually has — the honest count, and what a profile carrying a real
    // rice's dependency list is really asking for.
    sandbox.mark_installed("newbar");
    let data = sandbox.run(&["plan", "beta"]).assert_ok();
    assert_eq!(
        data["package_diff"]["install"],
        json!({ "official": ["newbar"], "aur": ["newaur"] }),
        "the diff stays manifest-to-manifest: {data}"
    );
    assert_eq!(
        data["install_missing"],
        json!({ "official": [], "aur": ["newaur"] }),
        "only the package the machine lacks is reported missing: {data}"
    );

    for line in sandbox.log() {
        assert!(
            !line.contains("hyprctl"),
            "plan must not touch Hyprland: {line}"
        );
    }
}

/// Class: the switch sequence. Against stubbed pacman the switch executes the
/// locked order — install B while A's packages are still present, remove
/// A-unique with plain `pacman -R`, only then stop A's services, flip
/// symlinks, reload Hyprland, start B's services — verified through the stub
/// log, and the report names what happened. The package ops lead because they
/// authenticate through the still-running old desktop: its polkit agent is
/// what answers a `pkexec` prompt, and a switch that stopped the shell first
/// can only be denied.
#[test]
fn switch_runs_the_locked_sequence_in_order() {
    let sandbox = Sandbox::new();
    fixture(&sandbox);
    sandbox.clear_log();

    let run = sandbox.run(&["switch", "beta"]);
    let data = run.assert_ok();

    assert_eq!(data["target"], json!("beta"));
    assert_eq!(data["completed_steps"], json!(10));
    assert_eq!(data["resume_hint"], json!("switch to `beta` to restore"));
    assert_eq!(data["report"]["installed"], json!(["newbar", "newaur"]));
    assert_eq!(data["report"]["removed"], json!(["oldbar", "oldaur"]));
    assert_eq!(data["report"]["kept"], json!([]));
    assert_eq!(data["report"]["conflict_removed"], json!([]));
    assert_eq!(
        data["report"]["linked"],
        json!([".config/kitty", ".config/waybar"])
    );
    assert_eq!(data["report"]["unlinked"], json!([".config/hypr"]));
    assert_eq!(data["report"]["services_stopped"], json!(["waybar"]));
    assert_eq!(data["report"]["services_started"], json!(["ags"]));
    assert_eq!(data["report"]["reloaded"], json!(true));
    assert!(run.warnings().is_empty(), "{:?}", run.warnings());

    let sequence: Vec<String> = locked_commands(&sandbox)
        .into_iter()
        .filter(|line| {
            // `pacman -Qq` (what is already installed) and `pacman -Qi`
            // (still-needed pre-filter) are read-only queries; like the probes
            // they are not part of the locked sequence.
            !(line.ends_with("--version")
                || line.ends_with("-h")
                || line.ends_with(" version")
                || line == "pacman -Qq"
                || line.starts_with("pacman -Qi "))
        })
        .collect();
    assert_eq!(
        sequence,
        [
            "pacman -S --noconfirm newbar",
            "yay -S --noconfirm newaur",
            "pacman -R --noconfirm oldbar oldaur",
            "pkill waybar",
            "hyprctl reload",
            "ags",
        ],
        "the locked sequence: batched installs, one batched plain -R while the \
         old desktop can still authenticate, then stop A's services, hyprctl \
         reload, start B's services"
    );
    assert!(
        sandbox
            .log()
            .iter()
            .any(|line| line.trim_end() == "yay -S --noconfirm newaur"),
        "the AUR helper itself ran (inside its wrapper): {:?}",
        sandbox.log()
    );

    assert_eq!(
        sandbox.current_target(),
        Some(sandbox.profile_dir("beta")),
        "current points at the target profile"
    );
    assert_eq!(sandbox.state()["active_profile"], json!("beta"));
    assert_eq!(
        fs::read_link(sandbox.home().join(".config/waybar")).ok(),
        Some(sandbox.profile_dir("beta").join(".config/waybar")),
        "the managed path now points into beta"
    );
    assert_eq!(
        fs::read_link(sandbox.home().join(".config/kitty")).ok(),
        Some(sandbox.profile_dir("beta").join(".config/kitty")),
        "beta's new managed path is linked"
    );
    assert!(
        fs::symlink_metadata(sandbox.home().join(".config/hypr")).is_err(),
        "alpha-only managed path is unlinked"
    );
    assert!(
        fs::read_to_string(sandbox.home().join(".config/waybar/config.jsonc"))
            .expect("read through the link")
            .contains("beta"),
        "the link resolves into the target profile"
    );
}

/// Class: real directories at managed paths. The fixture above symlinks the
/// managed paths the way an activated profile leaves them, which is the only
/// shape the old `remove_file` could clear. A machine that has never switched —
/// or whose config is a plain directory — is the other case, and it used to
/// report a successful switch that changed nothing.
#[test]
fn a_real_directory_at_a_managed_path_is_replaced_by_the_symlink() {
    let sandbox = Sandbox::new();
    fixture(&sandbox);

    // Put the live tree back the way a fresh machine has it: real directories,
    // not links. This is the state the original code silently no-opped on.
    fs::remove_file(sandbox.home().join(".config/waybar")).expect("drop the waybar link");
    fs::remove_file(sandbox.home().join(".config/hypr")).expect("drop the hypr link");
    fs::create_dir_all(sandbox.home().join(".config/waybar")).expect("real waybar dir");
    fs::write(
        sandbox.home().join(".config/waybar/config.jsonc"),
        "{ \"rice\": \"live\" }\n",
    )
    .expect("real waybar config");
    fs::create_dir_all(sandbox.home().join(".config/hypr")).expect("real hypr dir");
    fs::write(
        sandbox.home().join(".config/hypr/hyprland.conf"),
        "exec-once = waybar\n",
    )
    .expect("real hypr config");
    sandbox.clear_log();

    let data = sandbox.run(&["switch", "beta"]).assert_ok();

    assert_eq!(
        fs::read_link(sandbox.home().join(".config/waybar")).ok(),
        Some(sandbox.profile_dir("beta").join(".config/waybar")),
        "the real directory is replaced by a link into the target profile"
    );
    assert!(
        data["report"]["linked"]
            .as_array()
            .expect("linked is a list")
            .contains(&json!(".config/waybar")),
        "a path that really linked is reported as linked"
    );
    assert_eq!(
        fs::read_to_string(sandbox.home().join(".config/waybar/config.jsonc"))
            .expect("read through the new link"),
        "{ \"rice\": \"beta\" }\n",
        "the link resolves into the target profile, not the old directory"
    );
    assert!(
        fs::symlink_metadata(sandbox.home().join(".config/hypr")).is_err(),
        "an unlinked real directory is cleared, not left behind"
    );
}

/// A real *file* at a managed path is user data, so it is never destroyed: a
/// file the profile does not already hold byte for byte is preserved under the
/// profile's `backups/` before the symlink goes in.
#[test]
fn a_real_file_is_backed_up_before_a_managed_path_replaces_it() {
    let sandbox = Sandbox::new();
    fixture(&sandbox);

    fs::remove_file(sandbox.home().join(".config/waybar")).expect("drop the waybar link");
    fs::write(sandbox.home().join(".config/waybar"), "not a directory\n")
        .expect("a real file where a config dir belongs");

    let data = sandbox.run(&["switch", "beta"]).assert_ok();

    assert_eq!(
        data["report"]["backed_up"],
        json!([".config/waybar"]),
        "the report names what it preserved: {data}"
    );
    assert_eq!(
        fs::read_to_string(sandbox.profile_dir("beta").join("backups/.config/waybar"))
            .expect("the user's file is kept under the profile"),
        "not a directory\n",
        "the backup holds the bytes that were there before the switch"
    );
    assert!(
        fs::symlink_metadata(sandbox.home().join(".config/waybar"))
            .expect("the path is linked now")
            .file_type()
            .is_symlink(),
        "the managed path is the profile's own copy again"
    );
    assert_eq!(sandbox.current_target(), Some(sandbox.profile_dir("beta")));
}

/// A file the profile already holds byte for byte carries no information the
/// profile does not have, so it is linked with no backup and no report entry.
/// This is the common case for a shell rc the snapshot copied straight back.
#[test]
fn a_real_file_identical_to_the_profile_links_without_a_backup() {
    let sandbox = Sandbox::new();
    sandbox.write_profile(
        "prompt",
        &profile_toml("prompt", &[], &[], &[], &[".config/starship.toml"]),
    );
    sandbox.write_profile_file("prompt", ".config/starship.toml", "add_newline = true\n");
    // The live file is the same bytes, but a real file rather than a link.
    sandbox.write_home(".config/starship.toml", "add_newline = true\n");

    let plan = sandbox.run(&["plan", "prompt"]).assert_ok();
    assert_eq!(
        plan["identical_paths"],
        json!([".config/starship.toml"]),
        "plan separates the identical from the ones it would keep: {plan}"
    );
    assert_eq!(plan["backed_up_paths"], json!([]), "{plan}");

    let data = sandbox.run(&["switch", "prompt"]).assert_ok();
    assert_eq!(data["report"]["backed_up"], json!([]), "{data}");
    assert!(
        !sandbox.profile_dir("prompt").join("backups").exists(),
        "nothing differed, so nothing was copied"
    );
}

/// A nested real file keeps its shape under `backups/`, so the restore path is
/// the same path the user had.
#[test]
fn a_nested_real_file_is_backed_up_at_a_nested_path() {
    let sandbox = Sandbox::new();
    sandbox.write_profile(
        "prompt",
        &profile_toml(
            "prompt",
            &[],
            &[],
            &[],
            &[".config/starship.toml", ".config/kitty"],
        ),
    );
    sandbox.write_profile_file("prompt", ".config/starship.toml", "from the profile\n");
    sandbox.write_profile_file("prompt", ".config/kitty/kitty.conf", "font 12\n");
    sandbox.write_home(".config/starship.toml", "my own prompt\n");

    let data = sandbox.run(&["switch", "prompt"]).assert_ok();

    assert_eq!(
        data["report"]["backed_up"],
        json!([".config/starship.toml"])
    );
    assert_eq!(
        fs::read_to_string(
            sandbox
                .profile_dir("prompt")
                .join("backups/.config/starship.toml")
        )
        .expect("nested backup"),
        "my own prompt\n"
    );
}

/// Switching twice over the same path must not overwrite the first backup:
/// the first switch's copy is the one the user would go back to.
#[test]
fn a_second_switch_keeps_the_first_backup() {
    let sandbox = Sandbox::new();
    fixture(&sandbox);
    let backups = sandbox.profile_dir("beta").join("backups/.config/waybar");

    for content in ["first\n", "second\n"] {
        fs::remove_file(sandbox.home().join(".config/waybar")).ok();
        fs::write(sandbox.home().join(".config/waybar"), content).expect("a real file");
        sandbox.run(&["switch", "beta"]).assert_ok();
    }

    let kept: Vec<String> = fs::read_dir(backups.parent().expect("the backups dir"))
        .expect("backups exist")
        .map(|entry| {
            entry
                .expect("a directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(kept.len(), 2, "both copies are kept: {kept:?}");
    assert_eq!(
        fs::read_to_string(&backups).expect("the first backup is intact"),
        "first\n",
        "the earlier backup is the one the user had first"
    );
}

/// A nested managed path — a profile that owns `.config/quickshell/caelestia`
/// rather than all of `.config/quickshell` — links even when the parent holds
/// unrelated siblings that must survive.
#[test]
fn a_nested_managed_path_links_without_disturbing_its_siblings() {
    let sandbox = Sandbox::new();
    sandbox.write_profile(
        "nested",
        &profile_toml("nested", &[], &[], &[], &[".config/quickshell/caelestia"]),
    );
    sandbox.write_profile_file(
        "nested",
        ".config/quickshell/caelestia/shell.qml",
        "Shell { }\n",
    );
    // A sibling shell the profile does not claim.
    fs::create_dir_all(sandbox.home().join(".config/quickshell/ii")).expect("sibling dir");
    fs::write(
        sandbox.home().join(".config/quickshell/ii/shell.qml"),
        "Sibling { }\n",
    )
    .expect("sibling config");
    fs::create_dir_all(sandbox.home().join(".config/quickshell/caelestia"))
        .expect("a real dir where the nested profile belongs");
    fs::write(
        sandbox
            .home()
            .join(".config/quickshell/caelestia/shell.qml"),
        "Live { }\n",
    )
    .expect("live nested config");

    sandbox.run(&["switch", "nested"]).assert_ok();

    assert_eq!(
        fs::read_link(sandbox.home().join(".config/quickshell/caelestia")).ok(),
        Some(
            sandbox
                .profile_dir("nested")
                .join(".config/quickshell/caelestia")
        ),
        "the nested path is linked into the profile"
    );
    assert_eq!(
        fs::read_to_string(sandbox.home().join(".config/quickshell/ii/shell.qml"))
            .expect("sibling survives"),
        "Sibling { }\n",
        "a path the profile does not manage is never touched"
    );
    assert_eq!(
        fs::read_to_string(
            sandbox
                .home()
                .join(".config/quickshell/caelestia/shell.qml")
        )
        .expect("read through the link"),
        "Shell { }\n",
        "the nested link resolves into the profile"
    );
}

#[test]
fn a_declared_package_conflict_removes_the_conflicting_package_and_retries() {
    let sandbox = Sandbox::new();
    fixture(&sandbox);
    sandbox.declare_conflict("oldbar", "newbar");
    sandbox.clear_log();

    let run = sandbox.run(&["switch", "beta"]);
    let data = run.assert_ok();

    assert_eq!(data["completed_steps"], json!(10));
    assert_eq!(data["report"]["conflict_removed"], json!(["oldbar"]));
    assert_eq!(data["report"]["installed"], json!(["newbar", "newaur"]));
    assert_eq!(
        data["report"]["removed"],
        json!(["oldbar", "oldaur"]),
        "the conflicting package counts as removed, exactly once"
    );
    assert!(run.warnings().is_empty(), "{:?}", run.warnings());

    let log = locked_commands(&sandbox);
    let installs: Vec<usize> = log
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains("-S") && line.contains("newbar"))
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        installs.len(),
        2,
        "install attempted, then retried: {log:?}"
    );
    let removal = log
        .iter()
        .position(|line| line.contains("-R") && line.contains("oldbar"))
        .expect("the conflicting A-package is removed");
    assert!(
        installs[0] < removal && removal < installs[1],
        "install-first: install, remove the conflict, retry: {log:?}"
    );

    let installed = sandbox.installed_packages();
    assert!(
        installed.iter().any(|p| p == "newbar") && installed.iter().any(|p| p == "newaur"),
        "the target's packages ended up installed: {installed:?}"
    );
    assert!(
        !installed.iter().any(|p| p == "oldbar" || p == "oldaur"),
        "A's unique packages are gone: {installed:?}"
    );
    assert_eq!(sandbox.current_target(), Some(sandbox.profile_dir("beta")));
    assert_eq!(sandbox.state()["active_profile"], json!("beta"));
}

/// Class: a declined removal is trivia, not a verdict. A removal that fails
/// outright — the shape a polkit denial has, and exactly how one real switch
/// aborted with a bare desktop — keeps the package, warns, and the switch
/// finishes all ten steps with the new desktop up. A stray installed package
/// must never cost the user their shell.
#[test]
fn a_denied_removal_is_kept_and_the_switch_finishes() {
    let sandbox = Sandbox::new();
    fixture(&sandbox);
    // Only removals through pkexec fail; installs authenticate fine.
    sandbox.fail_on("pkexec", "-R");
    sandbox.clear_log();

    let run = sandbox.run(&["switch", "beta"]);
    let data = run.assert_ok();

    assert_eq!(data["completed_steps"], json!(10));
    assert_eq!(
        data["report"]["kept"],
        json!(["oldbar", "oldaur"]),
        "a package pacman would not remove is reported as kept"
    );
    assert_eq!(data["report"]["removed"], json!([]));
    assert!(
        run.warnings()
            .iter()
            .all(|warning| warning.contains("kept:")),
        "the decline surfaces as kept-warnings, never an abort: {:?}",
        run.warnings()
    );
    assert!(
        sandbox.log_contains("hyprctl reload"),
        "the switch went on to finish the desktop: {:?}",
        sandbox.log()
    );
    assert_eq!(data["report"]["shell_started"], json!(null));
    assert_eq!(data["report"]["services_started"], json!(["ags"]));
    assert_eq!(sandbox.current_target(), Some(sandbox.profile_dir("beta")));
}

/// Class: kept packages. A "still needed" refusal is logged as kept, the
/// switch continues, and removals never use `-Rs`/`-Rdd`.
#[test]
fn a_still_needed_refusal_is_kept_and_removals_are_plain_r() {
    let sandbox = Sandbox::new();
    let alpha = profile_toml(
        "alpha",
        &["legacydep", "oldbar", "shared"],
        &["oldaur"],
        &[("waybar", "waybar", "pkill waybar")],
        &[".config/waybar", ".config/hypr"],
    );
    fixture_with(&sandbox, &alpha);
    sandbox.declare_still_needed("legacydep");
    sandbox.clear_log();

    let run = sandbox.run(&["switch", "beta"]);
    let data = run.assert_ok();

    assert_eq!(data["completed_steps"], json!(10));
    assert_eq!(data["report"]["kept"], json!(["legacydep"]));
    assert_eq!(
        data["report"]["removed"],
        json!(["oldbar", "oldaur"]),
        "the kept package is never reported as removed"
    );
    let warnings = run.warnings();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("kept: legacydep (still needed)")),
        "the refusal reaches the envelope as a kept note: {warnings:?}"
    );

    // The refusal did not stop the switch.
    assert!(
        sandbox.log_contains("hyprctl reload"),
        "{:?}",
        sandbox.log()
    );
    assert!(
        sandbox.log().iter().any(|line| line.trim_end() == "ags"),
        "B's service still started: {:?}",
        sandbox.log()
    );
    assert_eq!(sandbox.current_target(), Some(sandbox.profile_dir("beta")));

    for line in sandbox.log() {
        assert!(
            !line.contains("-Rs") && !line.contains("-Rdd"),
            "removal is plain pacman -R, never -Rs/-Rdd: {line}"
        );
    }
    assert!(
        sandbox
            .log()
            .iter()
            .any(|line| line.contains("pacman -R --noconfirm")),
        "removals go through pacman -R: {:?}",
        sandbox.log()
    );
}

/// Class: service failures. A service that fails to start is a warning
/// carrying the captured stderr — never a failed switch.
#[test]
fn a_failing_service_start_warns_without_failing_the_switch() {
    let sandbox = Sandbox::new();
    fixture(&sandbox);
    sandbox.script("ags", Mode::Fail);

    let run = sandbox.run(&["switch", "beta"]);
    let data = run.assert_ok();

    let warnings = run.warnings();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("`ags`") && warning.contains("stub failure")),
        "the warning carries the captured stderr: {warnings:?}"
    );
    assert_eq!(
        data["report"]["services_started"],
        json!([]),
        "a failed start is not reported as started"
    );
    assert_eq!(data["completed_steps"], json!(10));
    assert_eq!(data["report"]["services_stopped"], json!(["waybar"]));
    assert!(
        sandbox.log_contains("hyprctl reload"),
        "{:?}",
        sandbox.log()
    );
    assert_eq!(sandbox.current_target(), Some(sandbox.profile_dir("beta")));
    assert_eq!(sandbox.state()["active_profile"], json!("beta"));
}

/// Class: cancellation. SIGTERM mid-switch is absorbed until the next step
/// boundary: the package transaction finishes, no later step starts, the
/// envelope carries `completed_steps` + `resume_hint`, and re-running the
/// switch completes the recovery.
#[test]
fn sigterm_stops_at_the_next_step_boundary_and_a_reshwitch_recovers() {
    let sandbox = Sandbox::new();
    fixture(&sandbox);
    // The -S stub sleeps inside its transaction, giving the test a wide
    // window to signal mid-step.
    sandbox.delay_on("pacman", "-S", 2);
    sandbox.clear_log();

    let mut child = sandbox.spawn(&["switch", "beta"]);
    let stdout = child.stdout.take().expect("piped stdout");
    let mut reader = BufReader::new(stdout);
    let mut lines: Vec<String> = Vec::new();
    let mut buffer = String::new();

    // Read progress until the package step announces itself.
    loop {
        buffer.clear();
        let read = reader.read_line(&mut buffer).expect("read progress");
        assert!(
            read > 0,
            "the operation exited before the package step: {lines:?}"
        );
        lines.push(buffer.trim_end().to_string());
        if lines
            .last()
            .is_some_and(|line| line.contains("applying package changes"))
        {
            break;
        }
    }
    assert!(
        wait_for_log(&sandbox, "pacman -S", Duration::from_secs(10)),
        "the -S transaction never started: {:?}",
        sandbox.log()
    );
    send_sigterm(child.id());

    // Drain the rest of the NDJSON stream.
    loop {
        buffer.clear();
        let read = reader.read_line(&mut buffer).expect("read the rest");
        if read == 0 {
            break;
        }
        lines.push(buffer.trim_end().to_string());
    }
    let status = child.wait().expect("wait for the operation");
    assert!(
        status.success(),
        "the backend exits 0 on cancellation, got {status}"
    );
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("piped stderr")
        .read_to_string(&mut stderr)
        .expect("read stderr");
    let run = Run::from_parts(lines, stderr);

    let envelope = run.envelope();
    assert_eq!(
        envelope["ok"],
        json!(false),
        "a cancelled switch is not a success: {}",
        run.stdout
    );
    // The stopped state a reopened panel reads: no operation is running
    // anymore, and the stop is recorded as the failure it was.
    let stopped = sandbox.state();
    assert_eq!(
        stopped["operation"],
        json!(null),
        "the stopped switch clears the running operation"
    );
    assert_eq!(
        stopped["last_result"]["ok"],
        json!(false),
        "state.json reports the stop honestly"
    );
    let data = &envelope["data"];
    assert_eq!(
        data["completed_steps"],
        json!(4),
        "verify, plan and both package steps finished; the switch stopped \
         before the flip and touched nothing else: {data}"
    );
    assert_eq!(data["resume_hint"], json!("switch to `beta` to restore"));
    assert!(
        data["error"]
            .as_str()
            .is_some_and(|error| error.contains("cancelled")),
        "the envelope says it was cancelled: {data}"
    );
    run.assert_no_panic();

    // Never mid-transaction: the package step ran to completion, and no
    // later step started.
    let log = sandbox.log();
    assert!(
        log.iter()
            .any(|line| line.contains("pacman -R --noconfirm")),
        "the package step finished its removals: {log:?}"
    );
    assert!(
        !log.iter().any(|line| line.contains("pkill waybar")),
        "the cancellation came before the flip, so nothing was stopped: {log:?}"
    );
    assert!(
        !sandbox.log_contains("hyprctl reload"),
        "no step starts after cancellation: {log:?}"
    );
    assert!(
        !log.iter().any(|line| line.trim_end() == "ags"),
        "services are not started after cancellation: {log:?}"
    );
    assert_eq!(
        sandbox.current_target(),
        Some(sandbox.profile_dir("alpha")),
        "cancelling after the package steps leaves `current` on the old profile"
    );

    // Re-switch is the recovery: the same switch completes the sequence.
    let recovery = sandbox.run(&["switch", "beta"]);
    let data = recovery.assert_ok();
    assert_eq!(data["completed_steps"], json!(10));
    assert!(
        sandbox.log_contains("hyprctl reload"),
        "the recovered switch reloaded and finished: {:?}",
        sandbox.log()
    );
    assert_eq!(sandbox.current_target(), Some(sandbox.profile_dir("beta")));
    assert_eq!(sandbox.state()["active_profile"], json!("beta"));
    assert_eq!(
        fs::read_link(sandbox.home().join(".config/waybar")).ok(),
        Some(sandbox.profile_dir("beta").join(".config/waybar")),
        "the recovered switch left the managed paths correct"
    );
}

/// Class: idempotency. Running `switch` twice in a row ends in the same
/// correct state — the second run changes nothing under the fake `$HOME`.
#[test]
fn switch_is_idempotent_a_second_run_ends_in_the_same_state() {
    let sandbox = Sandbox::new();
    fixture(&sandbox);

    let first = sandbox.run(&["switch", "beta"]);
    let first_data = first.assert_ok();
    assert_eq!(first_data["completed_steps"], json!(10));
    let after_first = sandbox.home_tree();

    let second = sandbox.run(&["switch", "beta"]);
    let second_data = second.assert_ok();
    assert_eq!(second_data["completed_steps"], json!(10));
    assert_eq!(
        second_data["report"]["installed"],
        json!([]),
        "A equals B the second time: nothing to install"
    );
    assert_eq!(second_data["report"]["removed"], json!([]));
    assert_eq!(second_data["report"]["unlinked"], json!([]));
    assert!(
        second.warnings().is_empty(),
        "a clean repeat is warning-free: {:?}",
        second.warnings()
    );
    assert_eq!(
        sandbox.home_tree(),
        after_first,
        "a second switch changes nothing under the fake $HOME"
    );

    assert_eq!(
        fs::read_link(sandbox.home().join(".config/waybar")).ok(),
        Some(sandbox.profile_dir("beta").join(".config/waybar"))
    );
    assert_eq!(
        fs::read_link(sandbox.home().join(".config/kitty")).ok(),
        Some(sandbox.profile_dir("beta").join(".config/kitty"))
    );
    assert_eq!(sandbox.current_target(), Some(sandbox.profile_dir("beta")));
    assert_eq!(sandbox.state()["active_profile"], json!("beta"));
}

/// Class: blocked paths. A real file at a target path is reported by `plan` and
/// preserved by `switch` — the bytes survive under the profile's `backups/`, the
/// switch carries on to completion, and the report names what it kept. This is
/// what makes a switch possible at all on a machine that has never switched: a
/// real `$HOME` always has a `.bashrc`.
#[test]
fn a_real_file_at_a_target_path_is_preserved_and_the_switch_completes() {
    let sandbox = Sandbox::new();
    fixture(&sandbox);
    let kitty = sandbox.write_home(".config/kitty", "my own kitty config\n");

    let plan = sandbox.run(&["plan", "beta"]).assert_ok();
    assert_eq!(plan["blocked_paths"], json!([".config/kitty"]));
    assert_eq!(
        plan["backed_up_paths"],
        json!([".config/kitty"]),
        "plan says which files it would keep: {plan}"
    );
    assert_eq!(plan["identical_paths"], json!([]), "{plan}");

    let run = sandbox.run(&["switch", "beta"]);
    let data = run.assert_ok();
    run.assert_no_panic();

    assert_eq!(
        data["report"]["backed_up"],
        json!([".config/kitty"]),
        "the report names the preserved path: {data}"
    );
    assert_eq!(
        fs::read_to_string(sandbox.profile_dir("beta").join("backups/.config/kitty"))
            .expect("the real file's bytes are kept"),
        "my own kitty config\n",
        "real files are never lost"
    );
    assert!(
        fs::symlink_metadata(&kitty)
            .expect("the path is linked now")
            .file_type()
            .is_symlink()
    );
    assert_eq!(sandbox.current_target(), Some(sandbox.profile_dir("beta")));
    assert_eq!(sandbox.state()["active_profile"], json!("beta"));
    assert!(
        !kitty.to_string_lossy().is_empty(),
        "the switch ran to the end, not a refusal"
    );
}

/// Class: live warnings (ticket #19). Warnings reach the panel as they
/// happen — streamed as their own NDJSON lines at the step that produced
/// them, before the final envelope closes the stream — while the envelope
/// keeps carrying the same warnings, its frozen shape unchanged.
#[test]
fn warnings_stream_inline_as_they_arrive_and_still_close_the_envelope() {
    let sandbox = Sandbox::new();
    let alpha = profile_toml(
        "alpha",
        &["legacydep", "oldbar", "shared"],
        &["oldaur"],
        &[("waybar", "waybar", "pkill waybar")],
        &[".config/waybar", ".config/hypr"],
    );
    fixture_with(&sandbox, &alpha);
    sandbox.declare_still_needed("legacydep");
    sandbox.script("ags", Mode::Fail);
    sandbox.clear_log();

    let run = sandbox.run(&["switch", "beta"]);
    run.assert_ok();

    // The envelope's frozen warnings still carry both notes.
    let envelope_warnings = run.warnings();
    assert!(
        envelope_warnings
            .iter()
            .any(|warning| warning.contains("kept: legacydep (still needed)")),
        "the kept note reaches the envelope: {envelope_warnings:?}"
    );
    assert!(
        envelope_warnings
            .iter()
            .any(|warning| warning.contains("`ags`") && warning.contains("stub failure")),
        "the service note reaches the envelope: {envelope_warnings:?}"
    );

    // Both notes also streamed as warning lines, in arrival order, each
    // naming the operation and the step it happened on.
    let streamed = run.streamed_warnings();
    assert_eq!(
        streamed.len(),
        2,
        "one streamed line per warning: {streamed:?}"
    );
    assert_eq!(streamed[0]["operation"], json!("switch"));
    assert_eq!(
        streamed[0]["step"],
        json!(3),
        "the kept note arrives on the package step: {streamed:?}"
    );
    assert!(
        streamed[0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("kept: legacydep (still needed)")),
        "{streamed:?}"
    );
    assert_eq!(streamed[1]["operation"], json!("switch"));
    assert_eq!(
        streamed[1]["step"],
        json!(8),
        "the service note arrives on the start-services step: {streamed:?}"
    );
    assert!(
        streamed[1]["message"]
            .as_str()
            .is_some_and(|message| message.contains("`ags`")),
        "{streamed:?}"
    );

    // Inline, not batched at the end: the kept note sits between the
    // package step and the reload, the service note after the start step
    // but still before the envelope.
    let between = |earlier: &str, later: usize, note: &str| {
        let step = stream_index(&run, |value| {
            value["progress"]["message"].as_str() == Some(earlier)
        })
        .unwrap_or_else(|| panic!("no progress line {earlier:?} in {}", run.stdout));
        let index = stream_index(&run, |value| {
            value["warning"]["message"]
                .as_str()
                .is_some_and(|message| message.contains(note))
        })
        .unwrap_or_else(|| panic!("no streamed warning containing {note:?} in {}", run.stdout));
        assert!(
            step < index && index < later,
            "{note:?} should sit between {earlier:?} (line {step}) and line {later}: {}",
            run.lines.join("\n")
        );
    };
    between(
        "applying package changes",
        stream_index(&run, |value| {
            value["progress"]["message"].as_str() == Some("reloading Hyprland")
        })
        .expect("the reload step streamed"),
        "kept: legacydep",
    );
    between("starting new services", run.lines.len() - 1, "`ags`");
}

/// Class: package scope. A first switch has no active profile, so the
/// manifest-to-manifest diff reports every declared package as an install.
/// Installing the ones the machine already has is not merely wasted: a
/// 22-package batch that is already on disk is a doomed transaction and wasted
/// work. The machine is the other half of the question.
#[test]
fn a_first_switch_installs_only_the_packages_the_machine_lacks() {
    let sandbox = Sandbox::new();
    fixture(&sandbox);
    // Every package both fixtures declare is already on this machine.
    for package in ["oldbar", "shared", "newbar", "oldaur", "newaur"] {
        sandbox.mark_installed(package);
    }
    // No active profile: this is the first switch, where the raw diff would
    // call all of beta's packages an install.
    let _ = fs::remove_file(sandbox.data_dir().join("current"));
    sandbox.clear_log();

    let data = sandbox.run(&["switch", "beta"]).assert_ok();

    assert_eq!(
        data["report"]["installed"],
        json!([]),
        "nothing needs installing, so nothing is reported installed: {}",
        data["report"]
    );
    for line in sandbox.log() {
        assert!(
            !line.contains(" -S --noconfirm"),
            "an already-installed package must not be reinstalled — that is a password prompt per package: {line}"
        );
        // `pkexec --version` is a tool probe, not a transaction: polkit does
        // not authenticate it. Only a real package op must be absent.
        assert!(
            !(line.starts_with("pkexec ") && !line.contains("--version")),
            "no password prompt at all when there is nothing to install: {line}"
        );
    }
}

/// Class: the confirmation is read-only (ticket #19). The plan behind the
/// confirm view probes tools and computes a diff; it changes nothing under
/// `$HOME` and never reaches a transaction — so Cancel, which only pops the
/// view, aborts with the system untouched.
#[test]
fn the_confirmation_plan_is_read_only_so_cancel_aborts_before_anything_runs() {
    let sandbox = Sandbox::new();
    fixture(&sandbox);
    sandbox.clear_log();
    let before = sandbox.home_tree();

    let run = sandbox.run(&["plan", "beta"]);
    let data = run.assert_ok();

    // Everything the confirm view renders comes from this payload: the
    // summary counts and the expandable sections alike.
    assert!(data["package_diff"]["install"].is_object());
    assert!(data["package_diff"]["remove"].is_object());
    assert!(data["service_changes"]["stop"].is_array());
    assert!(data["service_changes"]["start"].is_array());
    assert!(data["symlink_changes"]["link"].is_array());
    assert!(data["symlink_changes"]["unlink"].is_array());
    assert!(data["blocked_paths"].is_array());

    assert_eq!(
        sandbox.home_tree(),
        before,
        "the plan changed nothing under the fake $HOME"
    );
    for line in sandbox.log() {
        assert!(
            line == "pacman -Qq" || line.ends_with("--version"),
            "plan only probes tools and asks pacman what is installed, never transacts: {line}"
        );
        assert!(
            !line.starts_with("pkexec "),
            "a preview must never ask for a password: {line}"
        );
    }
}

/// Class: the shell swap. A desktop shell is a service like any other, and a
/// service diff cannot see that two profiles hold *different* shells — both
/// record a `qs` service whose command is the identical string
/// `qs -c $qsConfig`, so the names match, the strings match, and the diff
/// concludes nothing happened. It did not stop the old shell. The `[shell]`
/// table is what says `ii` and `caelestia` differ.
///
/// The fixture is the real one: identical `qs` service in both profiles,
/// different `[shell]` names.
fn shell_fixture(sandbox: &Sandbox) {
    let service = ("qs", "qs -c $qsConfig", "pkill qs");
    sandbox.write_profile(
        "ii",
        &common::profile_toml_with_shell(
            "ii",
            &[],
            &[],
            &[service],
            &[".config/quickshell/ii"],
            ("ii", "qs -c ii", "pkill qs"),
        ),
    );
    sandbox.write_profile(
        "caelestia",
        &common::profile_toml_with_shell(
            "caelestia",
            &[],
            &[],
            &[service],
            &[".config/quickshell/caelestia"],
            ("caelestia", "qs -c caelestia", "pkill qs"),
        ),
    );
    sandbox.activate("ii");
}

#[test]
fn switching_between_two_shells_stops_the_old_one_and_starts_the_new_one() {
    let sandbox = Sandbox::new();
    shell_fixture(&sandbox);
    sandbox.clear_log();

    let data = sandbox.run(&["switch", "caelestia"]).assert_ok();

    // The report names the shell by name, both ways. This is the assertion the
    // whole feature exists for: a service diff reports nothing here.
    assert_eq!(data["report"]["shell_stopped"], json!("ii"));
    assert_eq!(data["report"]["shell_started"], json!("caelestia"));

    // The stop runs the *leaving* profile's command and the start runs the
    // *entering* profile's, each with its shell spelled out rather than read
    // from an environment variable that says `ii`.
    assert!(
        sandbox.log_contains("pkill qs"),
        "the old shell is stopped: {:?}",
        sandbox.log()
    );
    assert!(
        sandbox.log_contains("qs -c caelestia"),
        "the new shell is started by name, not via $qsConfig: {:?}",
        sandbox.log()
    );
    assert!(
        !sandbox.log_contains("qs -c $qsConfig"),
        "the switch never runs the env-dependent command: {:?}",
        sandbox.log()
    );
}

#[test]
fn switching_to_a_profile_with_the_same_shell_restarts_nothing() {
    let sandbox = Sandbox::new();
    shell_fixture(&sandbox);
    // A second profile wearing the same shell: a settings change, not a
    // desktop change. The shell is already right, so restarting it would drop
    // the user's session for nothing.
    sandbox.write_profile(
        "ii-tweaked",
        &common::profile_toml_with_shell(
            "ii-tweaked",
            &[],
            &[],
            &[("qs", "qs -c $qsConfig", "pkill qs")],
            &[".config/quickshell/ii"],
            ("ii", "qs -c ii", "pkill qs"),
        ),
    );
    sandbox.clear_log();

    let data = sandbox.run(&["switch", "ii-tweaked"]).assert_ok();

    assert_eq!(data["report"]["shell_stopped"], Value::Null);
    assert_eq!(data["report"]["shell_started"], Value::Null);
    assert!(
        !sandbox.log_contains("pkill qs"),
        "the same shell is left running: {:?}",
        sandbox.log()
    );
}

#[test]
fn a_profile_with_no_shell_leaves_the_running_one_alone() {
    let sandbox = Sandbox::new();
    shell_fixture(&sandbox);
    // A profile that names no shell has no opinion about the one running —
    // a theme tweak, a keybind change. Ending the desktop because such a
    // profile was activated would be a switch that breaks the machine.
    sandbox.write_profile(
        "plain",
        &common::profile_toml(
            "plain",
            &[],
            &[],
            &[("qs", "qs -c $qsConfig", "pkill qs")],
            &[],
        ),
    );
    sandbox.clear_log();

    let data = sandbox.run(&["switch", "plain"]).assert_ok();

    assert_eq!(data["report"]["shell_stopped"], Value::Null);
    assert_eq!(data["report"]["shell_started"], Value::Null);
    assert!(
        !sandbox.log_contains("pkill qs"),
        "no shell is named, so none is ended: {:?}",
        sandbox.log()
    );
}

#[test]
fn plan_reports_the_shell_change_before_anything_runs() {
    let sandbox = Sandbox::new();
    shell_fixture(&sandbox);
    sandbox.clear_log();

    let data = sandbox.run(&["plan", "caelestia"]).assert_ok();

    // The service diff alone says nothing changed. The shell diff is the only
    // thing in the payload that tells the panel a desktop is about to end, so
    // it has to be there before the user commits.
    assert_eq!(data["service_changes"]["stop"], json!([]));
    assert_eq!(data["service_changes"]["start"], json!([]));
    assert_eq!(data["shell_change"]["stop"], json!("ii"));
    assert_eq!(data["shell_change"]["start"], json!("caelestia"));
    assert!(
        !sandbox.log_contains("pkill qs"),
        "the preview stops nothing: {:?}",
        sandbox.log()
    );
}

#[test]
fn the_shell_starts_after_the_reload_so_it_reads_the_new_config() {
    let sandbox = Sandbox::new();
    shell_fixture(&sandbox);
    sandbox.clear_log();
    let log_path = sandbox.log_path();

    sandbox.run(&["switch", "caelestia"]).assert_ok();
    let log = fs::read_to_string(&log_path).unwrap_or_default();
    let reload = log
        .lines()
        .position(|line| line.contains("hyprctl reload"))
        .expect("the reload runs");
    let start = log
        .lines()
        .position(|line| line.contains("qs -c caelestia"))
        .expect("the shell starts");

    // A shell that comes up before the reload reads the old config and keeps
    // it — the exact half-switched state that started this.
    assert!(
        start > reload,
        "the shell must start after the reload, not before:\n{log}"
    );
}

#[test]
fn a_shell_that_will_not_start_warns_without_failing_the_switch() {
    let sandbox = Sandbox::new();
    shell_fixture(&sandbox);
    sandbox.fail_on("qs", "-c caelestia");
    sandbox.clear_log();

    let run = sandbox.run(&["switch", "caelestia"]);
    let data = run.assert_ok();

    // Everything else succeeded and the config is correct for the next login,
    // so failing here would report a broken profile for a shell not yet up.
    assert_eq!(data["completed_steps"], json!(10));
    assert_eq!(data["report"]["shell_started"], Value::Null);
    assert!(
        run.warnings().iter().any(|w| w.contains("caelestia")),
        "the failure names the shell: {:?}",
        run.warnings()
    );
    assert_eq!(
        sandbox.current_target(),
        Some(sandbox.profile_dir("caelestia"))
    );
}

#[test]
fn a_shell_that_never_exits_does_not_hang_the_switch() {
    let sandbox = Sandbox::new();
    fixture(&sandbox);
    // The real manifests start `qs -c ii` — the desktop itself, which never
    // exits. A switch that waits for such a start to finish waits forever: it
    // has already killed the session at step 4, so a hang at step 10 is a dead
    // desktop and a terminal wedged on one switch line, with no report and no
    // way back. Beta's shell here is the honest shape of that command: a
    // process that simply stays alive.
    sandbox.write_profile(
        "beta",
        &common::profile_toml_with_shell(
            "beta",
            &["newbar", "shared"],
            &["newaur"],
            &[("ags", "ags", "pkill ags")],
            &[".config/waybar", ".config/kitty"],
            ("neverends", "sleep 30", "pkill neverends"),
        ),
    );
    sandbox.clear_log();

    let mut child = sandbox.spawn(&["switch", "beta"]);
    let stdout = child.stdout.take().expect("piped stdout");
    let (lines_tx, lines_rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut buffer = String::new();
        while reader.read_line(&mut buffer).unwrap_or(0) > 0 {
            let line = buffer.trim_end().to_string();
            buffer.clear();
            if lines_tx.send(line).is_err() {
                break;
            }
        }
    });

    // The watchdog the suite lacked: every earlier fixture stub exited
    // instantly, so `.output()` looked safe in tests and wedged on the real
    // machine. This switch must close its own envelope in seconds, while the
    // daemon it started is still sleeping.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut lines: Vec<String> = Vec::new();
    loop {
        match lines_rx.recv_timeout(Duration::from_millis(250)) {
            Ok(line) => lines.push(line),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!(
                        "the switch never finished — it is waiting for a start that never exits. Lines so far: {lines:?}"
                    );
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = child.wait().expect("wait for the switch");
    assert!(status.success(), "the switch exited {status}");
    let run = Run::from_parts(lines, String::new());
    let data = run.envelope();
    assert_eq!(data["ok"], json!(true), "{}", run.stdout);
    assert_eq!(
        data["data"]["completed_steps"],
        json!(10),
        "the steps after the daemon start still ran: {}",
        run.stdout
    );
    // Survival through the grace window is reported as started — the desktop
    // came up and stayed up, which is exactly what happened.
    assert_eq!(data["data"]["report"]["shell_started"], json!("neverends"));
}

/// Class: the way back. A path the new profile no longer manages used to be
/// deleted outright — and on the return leg the old profile's *own* files are
/// exactly the ones the new profile stopped claiming. Deleting them makes the
/// round trip the tool exists to support lossy: you can go out, but coming
/// home finds the config gone. The path is moved into the leaving profile's
/// `backups/` instead, and the report says so.
fn round_trip_fixture(sandbox: &Sandbox) {
    sandbox.write_profile(
        "out",
        &common::profile_toml_with_shell(
            "out",
            &[],
            &[],
            &[("qs", "qs -c $qsConfig", "pkill qs")],
            &[".config/quickshell/out"],
            ("out", "qs -c out", "pkill qs"),
        ),
    );
    sandbox.write_profile(
        "home",
        &common::profile_toml_with_shell(
            "home",
            &[],
            &[],
            &[("qs", "qs -c $qsConfig", "pkill qs")],
            &[".config/quickshell/ii"],
            ("ii", "qs -c ii", "pkill qs"),
        ),
    );
    // Real content for the managed symlinks to point at — a manifest naming a
    // path with nothing behind it tests the wiring, not the preservation.
    sandbox.write_profile_file("out", ".config/quickshell/out/shell.qml", "out shell\n");
    sandbox.write_profile_file("home", ".config/quickshell/ii/shell.qml", "home shell\n");
    sandbox.activate("out");
    // Activation in the real store leaves the managed paths linked; the
    // fixture mirrors that, or nothing at `~/.config/quickshell/out` exists to
    // take back.
    let link = sandbox.home().join(".config/quickshell/out");
    fs::create_dir_all(link.parent().expect("a parent")).expect("create the parent");
    symlink(
        sandbox.profile_dir("out").join(".config/quickshell/out"),
        &link,
    )
    .expect("link the active profile's config");
}

#[test]
fn a_path_the_new_profile_does_not_manage_is_preserved_not_deleted() {
    let sandbox = Sandbox::new();
    round_trip_fixture(&sandbox);
    sandbox.clear_log();

    // `plan` is the side that names the paths; `switch` is the side that acts
    // on them. Both are asserted, so the promise is checked from both ends.
    let planned = sandbox.run(&["plan", "home"]).assert_ok();
    let data = sandbox.run(&["switch", "home"]).assert_ok();

    // The path the leaving profile owned and the arriving one does not.
    assert_eq!(
        planned["symlink_changes"]["unlink"],
        json!([".config/quickshell/out"]),
        "the outgoing shell config is among the paths the switch takes back"
    );
    assert_eq!(
        data["report"]["unlinked"],
        json!([".config/quickshell/out"]),
        "and the switch reports acting on it"
    );
    assert_eq!(
        data["report"]["preserved"],
        json!([".config/quickshell/out"]),
        "and reports preserving it rather than dropping it"
    );

    // Its bytes are still there — inside the leaving profile's backups.
    let backups = sandbox.profile_dir("out").join("backups");
    assert!(
        backups.join(".config/quickshell/out").exists(),
        "the path is preserved under the leaving profile, not deleted: {}",
        backups.display()
    );
    // The profile's own copy is untouched: this is a pointer being moved, not
    // the config being consumed.
    assert!(
        sandbox
            .profile_dir("out")
            .join(".config/quickshell/out/shell.qml")
            .exists(),
        "the leaving profile still holds its own config"
    );
}

#[test]
fn a_switch_out_and_back_leaves_both_profiles_whole() {
    let sandbox = Sandbox::new();
    round_trip_fixture(&sandbox);
    let out_before = sandbox
        .profile_dir("out")
        .join(".config/quickshell/out/shell.qml")
        .exists();
    let home_before = sandbox
        .profile_dir("home")
        .join(".config/quickshell/ii/shell.qml")
        .exists();
    assert!(out_before && home_before, "the fixture ships both configs");

    // Out to the other shell, then home again. The promise is that the second
    // leg works from a machine the first leg did not damage.
    sandbox.run(&["switch", "home"]).assert_ok();
    let returned = sandbox.run(&["switch", "out"]).assert_ok();

    assert_eq!(returned["completed_steps"], json!(10));
    assert_eq!(sandbox.current_target(), Some(sandbox.profile_dir("out")));
    assert_eq!(returned["report"]["shell_started"], json!("out"));
    assert!(
        sandbox
            .profile_dir("out")
            .join(".config/quickshell/out/shell.qml")
            .exists(),
        "the first profile's config survived the trip and the return"
    );
    assert!(
        sandbox
            .profile_dir("home")
            .join(".config/quickshell/ii/shell.qml")
            .exists(),
        "so did the second's"
    );
    assert!(
        sandbox.home().join(".config/quickshell/out").exists(),
        "and the path is live again, linked to the profile that owns it"
    );
}

#[test]
fn a_second_take_back_of_the_same_path_keeps_both_generations() {
    let sandbox = Sandbox::new();
    round_trip_fixture(&sandbox);

    // Out, home, and home again by way of a third profile: the same path is
    // taken back twice. Neither generation may overwrite the other.
    sandbox.write_profile(
        "third",
        &common::profile_toml_with_shell(
            "third",
            &[],
            &[],
            &[("qs", "qs -c $qsConfig", "pkill qs")],
            &[".config/quickshell/ii"],
            ("ii", "qs -c ii", "pkill qs"),
        ),
    );
    sandbox.run(&["switch", "home"]).assert_ok();
    sandbox.run(&["switch", "out"]).assert_ok();
    sandbox.run(&["switch", "home"]).assert_ok();
    sandbox.run(&["switch", "third"]).assert_ok();

    let backups = sandbox
        .profile_dir("out")
        .join("backups/.config/quickshell");
    let generations: Vec<_> = fs::read_dir(&backups)
        .expect("the backups directory")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        generations.len() >= 2,
        "both take-backs are kept: {generations:?}"
    );
}

#[test]
fn a_path_edited_in_place_while_linked_is_preserved_as_edited() {
    let sandbox = Sandbox::new();
    round_trip_fixture(&sandbox);

    // The user replaced the symlink with a real directory and put their own
    // config in it — a legitimate thing to do while a profile is active. What
    // is preserved must be *their* version, not the profile's stale capture.
    let live = sandbox.home().join(".config/quickshell/out");
    let _ = fs::remove_file(&live);
    fs::create_dir_all(&live).expect("make a real directory");
    fs::write(live.join("mine.qml"), "user edit\n").expect("write the edit");

    sandbox.run(&["switch", "home"]).assert_ok();

    let preserved = sandbox
        .profile_dir("out")
        .join("backups/.config/quickshell/out/mine.qml");
    assert_eq!(
        fs::read_to_string(&preserved).unwrap_or_default(),
        "user edit\n",
        "the edit is preserved, not the profile's copy"
    );
}

/// A leaving profile whose manifest names foundation packages — the exact
/// shape the real caelestia manifest had, which raised `pkexec pacman -R
/// glibc` and died on a polkit denial mid-switch.
fn system_package_fixture(sandbox: &Sandbox) {
    let alpha = profile_toml(
        "alpha",
        &["glibc", "coreutils", "oldbar"],
        &["gcc-libs"],
        &[("waybar", "waybar", "pkill waybar")],
        &[".config/waybar", ".config/hypr"],
    );
    fixture_with(sandbox, &alpha);
}

/// Class: removal safety. The system-package floor is raised before pacman is
/// ever asked: a switch away from a profile whose manifest lists foundation
/// packages removes the ordinary ones, keeps the foundation ones without a
/// transaction, completes all ten steps, and names what it kept.
#[test]
fn a_switch_never_raises_a_transaction_for_system_packages() {
    let sandbox = Sandbox::new();
    system_package_fixture(&sandbox);

    let run = sandbox.run(&["switch", "beta"]);
    let data = run.assert_ok();

    assert_eq!(
        data["completed_steps"],
        json!(10),
        "the switch completes instead of dying at a denial: {data}"
    );
    assert_eq!(data["report"]["removed"], json!(["oldbar"]));
    assert_eq!(
        data["report"]["protected"],
        json!(["coreutils", "gcc-libs", "glibc"]),
        "the floor's keeps are named, official and AUR alike"
    );
    assert!(
        run.warnings().iter().any(|warning| {
            warning.contains("glibc")
                && warning.contains("coreutils")
                && warning.contains("gcc-libs")
        }),
        "the decline is warned about, live: {:?}",
        run.warnings()
    );

    let log = sandbox.log();
    for forbidden in ["glibc", "coreutils", "gcc-libs"] {
        assert!(
            !log.iter()
                .any(|line| line.contains("-R") && line.contains(forbidden)),
            "no removal transaction is ever raised for {forbidden}: {log:?}"
        );
    }
    assert!(
        log.iter()
            .any(|line| line.contains("pacman -R --noconfirm oldbar")),
        "the ordinary removal still happens: {log:?}"
    );
}

/// Class: removal safety. The preview gates removals the same way the switch
/// does, so the panel never offers a `glibc` removal the switch will decline,
/// and the declined ones are named in the plan for the panel to show.
#[test]
fn plan_names_the_removals_the_floor_will_decline() {
    let sandbox = Sandbox::new();
    system_package_fixture(&sandbox);

    let data = sandbox.run(&["plan", "beta"]).assert_ok();
    assert_eq!(
        data["package_diff"]["remove"],
        json!({ "official": ["oldbar"], "aur": [] }),
        "the offered removals exclude the floor: {data}"
    );
    assert_eq!(
        data["remove_protected"],
        json!(["coreutils", "gcc-libs", "glibc"]),
        "the declines are named, not hidden"
    );
}

/// Two absent packages against no active profile: one transaction for both.
#[test]
fn several_missing_official_packages_install_in_one_transaction() {
    let sandbox = Sandbox::new();
    fixture(&sandbox);
    // Two beta official packages absent, one shared present — so only two
    // install. No active profile, so the raw diff is "every beta official
    // package", trimmed by the machine to what is actually missing.
    let _ = fs::remove_file(sandbox.data_dir().join("current"));
    sandbox.write_profile(
        "beta",
        &profile_toml(
            "beta",
            &["newbar", "extrabar", "shared"],
            &["newaur"],
            &[("ags", "ags", "pkill ags")],
            &[".config/kitty"],
        ),
    );
    sandbox.mark_installed("shared");
    sandbox.mark_installed("newaur");
    sandbox.clear_log();

    let data = sandbox.run(&["switch", "beta"]).assert_ok();

    assert_eq!(data["completed_steps"], json!(10));
    assert!(
        data["report"]["installed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == "newbar")
            && data["report"]["installed"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value == "extrabar"),
        "both missing packages are reported installed: {}",
        data["report"]["installed"]
    );
    let installs: Vec<_> = locked_commands(&sandbox)
        .into_iter()
        .filter(|line| line.contains("-S --noconfirm") && line.contains("newbar"))
        .collect();
    assert_eq!(
        installs.len(),
        1,
        "one batch for the two-package install, not two prompts: {installs:?}"
    );
    assert!(
        installs[0].contains("extrabar"),
        "the batch names both packages: {}",
        installs[0]
    );
}

/// Several removable packages go in one transaction, not one per package.
#[test]
fn several_removals_run_in_one_transaction() {
    let sandbox = Sandbox::new();
    fixture(&sandbox);
    sandbox.clear_log();

    let run = sandbox.run(&["switch", "beta"]);
    let data = run.assert_ok();

    assert_eq!(data["completed_steps"], json!(10));
    assert_eq!(data["report"]["removed"], json!(["oldbar", "oldaur"]));
    assert!(run.warnings().is_empty(), "{:?}", run.warnings());

    let removes: Vec<_> = locked_commands(&sandbox)
        .into_iter()
        .filter(|line| line.contains("-R --noconfirm"))
        .collect();
    assert_eq!(
        removes.len(),
        1,
        "one batch for the two-package removal, not two prompts: {removes:?}"
    );
    assert!(
        removes[0].contains("oldbar") && removes[0].contains("oldaur"),
        "the batch names both packages: {}",
        removes[0]
    );
}

/// A leaving profile whose removals are *all* still needed by the machine —
/// the shape of the real caelestia manifest, where ~twenty packages each had
/// a dependent. The old code raised one `pkexec` per package after the batch
/// died on the first refusal: twenty password prompts for a switch. The
/// `pacman -Qi` pre-filter must keep every one of them without raising a
/// removal transaction at all.
#[test]
fn every_removal_still_needed_raises_no_transaction() {
    let sandbox = Sandbox::new();
    let alpha = profile_toml(
        "alpha",
        &[
            "oldbar",
            "oldbaz",
            "oldqux",
            "legacydep",
            "fuzzel",
            "cava",
            "songrec",
            "neovim",
        ],
        &["oldaur", "legacyaur"],
        &[],
        &[".config/kitty"],
    );
    fixture_with(&sandbox, &alpha);
    for package in [
        "oldbar",
        "oldbaz",
        "oldqux",
        "legacydep",
        "fuzzel",
        "cava",
        "songrec",
        "neovim",
        "oldaur",
        "legacyaur",
    ] {
        sandbox.declare_still_needed(package);
    }
    sandbox.clear_log();

    let run = sandbox.run(&["switch", "beta"]);
    let data = run.assert_ok();

    assert_eq!(
        data["report"]["removed"],
        json!([]),
        "nothing is removed — every package is still needed: {data}"
    );
    assert_eq!(
        data["report"]["kept"],
        json!([
            "oldbar",
            "oldbaz",
            "oldqux",
            "legacydep",
            "fuzzel",
            "cava",
            "songrec",
            "neovim",
            "oldaur",
            "legacyaur"
        ]),
        "each kept package is named in the report"
    );
    assert!(
        run.warnings()
            .iter()
            .all(|warning| warning.contains("kept:")),
        "the declines surface as kept notes, never as failures: {:?}",
        run.warnings()
    );

    let log = sandbox.log();
    assert!(
        log.iter().all(|line| !line.contains("-R")),
        "no removal transaction of any kind is raised — the whole storm is \
         pre-filtered out: {log:?}"
    );
    assert!(
        log.iter()
            .filter(|line| line.starts_with("pacman -Qi "))
            .count()
            >= 10,
        "the pre-filter actually ran per candidate: {log:?}"
    );
}
