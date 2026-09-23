//! Ticket #14: `plan` and `switch` through the operation surface — the diff
//! between fixture profiles (install/remove/symlink/service sets and blocked
//! paths), the locked switch sequence against stubbed pacman, the install-first
//! conflict fallback, kept-package refusals, service-start warnings, SIGTERM
//! cancellation at a step boundary, idempotent re-runs, and the refusal to
//! ever clobber a real file.

mod common;

use common::{Mode, Run, Sandbox, profile_toml};
use serde_json::json;
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

    // A real file where beta wants a symlink is flagged, never planned over.
    sandbox.write_home(".config/kitty", "my own kitty config\n");
    let data = sandbox.run(&["plan", "beta"]).assert_ok();
    assert_eq!(data["blocked_paths"], json!([".config/kitty"]));

    // A missing target refuses through the failed envelope, naming it.
    let message = sandbox.run(&["plan", "ghost"]).assert_failed();
    assert!(
        message.contains("ghost"),
        "the refusal must name the profile: {message}"
    );

    for line in sandbox.log() {
        assert!(
            !line.contains("hyprctl"),
            "plan must not touch Hyprland: {line}"
        );
    }
}

/// Class: the switch sequence. Against stubbed pacman the switch executes the
/// locked order — stop A's services, install B while A's packages are still
/// present, remove A-unique with plain `pacman -R`, reload Hyprland, start
/// B's services — verified through the stub log, and the report names what
/// happened.
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

    let sequence: Vec<String> = sandbox
        .log()
        .iter()
        .map(|line| line.trim_end().to_string())
        .filter(|line| !line.contains("--version"))
        .collect();
    assert_eq!(
        sequence,
        [
            "pkill waybar",
            "pacman -S --noconfirm newbar",
            "yay -S --noconfirm newaur",
            "pacman -R --noconfirm oldbar",
            "pacman -R --noconfirm oldaur",
            "hyprctl reload",
            "ags",
        ],
        "the locked sequence: stop A's services, install-first, plain -R, \
         hyprctl reload, start B's services"
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

/// Class: conflict fallback. A declared pacman conflict on an install removes
/// just the conflicting A-package, retries the install, and the switch
/// completes.
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

    let log = sandbox.log();
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
    let data = &envelope["data"];
    assert_eq!(
        data["completed_steps"],
        json!(7),
        "steps 1-7 finished; nothing after the package step ran: {data}"
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
        !sandbox.log_contains("hyprctl reload"),
        "no step starts after cancellation: {log:?}"
    );
    assert!(
        !log.iter().any(|line| line.trim_end() == "ags"),
        "services are not started after cancellation: {log:?}"
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

/// Class: blocked paths. A real file at a target path is flagged by `plan`
/// and `switch` refuses before touching anything — no flip, no symlinks, no
/// pacman, no services, and the file itself untouched.
#[test]
fn a_real_file_at_a_target_path_blocks_plan_and_switch_refuses_before_touching() {
    let sandbox = Sandbox::new();
    fixture(&sandbox);
    let kitty = sandbox.write_home(".config/kitty", "my own kitty config\n");

    let plan = sandbox.run(&["plan", "beta"]).assert_ok();
    assert_eq!(plan["blocked_paths"], json!([".config/kitty"]));
    sandbox.clear_log();
    let before = sandbox.home_tree();

    let run = sandbox.run(&["switch", "beta"]);
    let envelope = run.envelope();
    assert_eq!(envelope["ok"], json!(false), "{}", run.stdout);
    let data = &envelope["data"];
    let message = data["error"]
        .as_str()
        .expect("failed envelope carries an error")
        .to_string();
    assert!(
        message.contains(".config/kitty"),
        "the refusal must name the blocked path: {message}"
    );
    assert!(
        message.contains("snapshot"),
        "the refusal points at the adoption path: {message}"
    );
    assert_eq!(data["completed_steps"], json!(2), "{data}");
    assert_eq!(data["resume_hint"], json!("switch to `beta` to restore"));

    assert_eq!(
        sandbox.current_target(),
        Some(sandbox.profile_dir("alpha")),
        "a refused switch never flips `current`"
    );
    assert_eq!(sandbox.state()["active_profile"], json!("alpha"));
    assert_eq!(
        sandbox.home_tree(),
        before,
        "a refused switch changes nothing under the fake $HOME"
    );
    let metadata = fs::symlink_metadata(&kitty).expect("the real file is still there");
    assert!(
        !metadata.file_type().is_symlink(),
        "real files are never clobbered"
    );
    assert_eq!(
        fs::read_to_string(&kitty).expect("read the real file"),
        "my own kitty config\n"
    );
    assert!(
        sandbox.log().is_empty(),
        "a refused switch never reaches pacman or the services: {:?}",
        sandbox.log()
    );
    run.assert_no_panic();
}
