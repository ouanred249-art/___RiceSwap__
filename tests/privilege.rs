//! Ticket #16: the privilege flow through the operation surface — official
//! package ops escalated with `pkexec pacman` (a polkit denial is a clean
//! `ok: false` envelope), the runtime-detected AUR helper with its
//! `--aur-helper` override and its "no usable helper" error naming the
//! packages, and every helper invocation spawned inside the floating-terminal
//! wrapper with its output and errors flowing into the switch report.

mod common;

use common::{Mode, Sandbox, profile_toml};
use serde_json::json;

/// The switch fixture for the privilege flow: an active `alpha` and a target
/// `beta`, so one switch exercises install (official + AUR) and removal
/// (plain `-R`) paths alike.
fn fixture(sandbox: &Sandbox) {
    sandbox.write_profile(
        "alpha",
        &profile_toml(
            "alpha",
            &["oldbar", "shared"],
            &["oldaur"],
            &[("waybar", "waybar", "pkill waybar")],
            &[".config/waybar"],
        ),
    );
    sandbox.write_profile(
        "beta",
        &profile_toml(
            "beta",
            &["newbar", "shared"],
            &["newaur"],
            &[("ags", "ags", "pkill ags")],
            &[".config/kitty"],
        ),
    );
    sandbox.activate("alpha");
}

/// Whether the stub log holds a line equal to `expected` (after trimming the
/// newline the stub logged with).
fn log_has(sandbox: &Sandbox, expected: &str) -> bool {
    sandbox.log().iter().any(|line| line.trim_end() == expected)
}

/// Class: official privilege. Every official package op — install and plain
/// `-R` removal alike — runs `pkexec pacman`, and the escalated command really
/// executes pacman underneath.
#[test]
fn official_package_ops_run_through_pkexec_pacman() {
    let sandbox = Sandbox::new();
    fixture(&sandbox);
    sandbox.clear_log();

    let run = sandbox.run(&["switch", "beta"]);
    let data = run.assert_ok();

    assert!(
        log_has(&sandbox, "pkexec pacman -S --noconfirm newbar"),
        "the official install is escalated: {:?}",
        sandbox.log()
    );
    assert!(
        log_has(&sandbox, "pkexec pacman -R --noconfirm oldbar"),
        "removals are escalated too: {:?}",
        sandbox.log()
    );
    assert!(
        log_has(&sandbox, "pkexec pacman -R --noconfirm oldaur"),
        "AUR packages are still removed with plain pacman -R under pkexec: {:?}",
        sandbox.log()
    );
    // The wrapper really ran pacman: the underlying command is in the log.
    assert!(
        log_has(&sandbox, "pacman -S --noconfirm newbar"),
        "pkexec executes pacman underneath: {:?}",
        sandbox.log()
    );

    assert_eq!(data["report"]["installed"], json!(["newbar", "newaur"]));
    assert_eq!(data["report"]["removed"], json!(["oldbar", "oldaur"]));
    assert!(run.warnings().is_empty(), "{:?}", run.warnings());
}

/// Class: polkit denial. A denied `pkexec` surfaces as a clean `ok: false`
/// envelope with the recovery contract (`completed_steps` + `resume_hint`),
/// never a crash — and switching back restores, because no auto-rollback
/// exists but every step is re-runnable.
#[test]
fn a_polkit_denial_surfaces_as_a_clean_failed_envelope() {
    let sandbox = Sandbox::new();
    fixture(&sandbox);
    sandbox.script("pkexec", Mode::Fail);
    sandbox.clear_log();

    let run = sandbox.run(&["switch", "beta"]);
    let envelope = run.envelope();
    run.assert_no_panic();

    assert_eq!(
        envelope["ok"],
        json!(false),
        "a denial is a failed envelope: {}",
        run.stdout
    );
    let error = envelope["data"]["error"]
        .as_str()
        .expect("the failure names the denial")
        .to_string();
    assert!(
        error.contains("pkexec") && error.contains("newbar"),
        "the error names the denied command and package: {error}"
    );
    assert_eq!(
        envelope["data"]["completed_steps"],
        json!(5),
        "verify, plan, flip, stop, link completed; the package step did not"
    );
    assert_eq!(
        envelope["data"]["resume_hint"],
        json!("switch to `beta` to restore")
    );

    // The failure model: configs already point at B; nothing rolls back.
    assert_eq!(sandbox.current_target(), Some(sandbox.profile_dir("beta")));
    assert_eq!(sandbox.state()["active_profile"], json!("beta"));
    assert_eq!(
        sandbox.state()["last_result"]["ok"],
        json!(false),
        "state.json records the failed result"
    );

    // Recovery is a re-switch of the profile you want — here, back to alpha.
    sandbox.script("pkexec", Mode::Ok);
    let restored = sandbox.run(&["switch", "alpha"]).assert_ok();
    assert_eq!(restored["completed_steps"], json!(10));
    assert_eq!(sandbox.current_target(), Some(sandbox.profile_dir("alpha")));
    assert_eq!(sandbox.state()["active_profile"], json!("alpha"));
}

/// Class: AUR helper detection. `yay` before `paru`, decided at runtime by
/// what answers its probe on PATH — an unusable `yay` falls through to `paru`.
#[test]
fn the_aur_helper_is_detected_at_runtime_yay_before_paru() {
    // Both present: yay wins.
    let sandbox = Sandbox::new();
    fixture(&sandbox);
    sandbox.clear_log();
    sandbox.run(&["switch", "beta"]).assert_ok();
    assert!(
        log_has(&sandbox, "riceswap-float yay -S --noconfirm newaur"),
        "yay is preferred: {:?}",
        sandbox.log()
    );
    assert!(
        !sandbox.log().iter().any(|line| line.contains("paru -S")),
        "paru is never reached while yay answers: {:?}",
        sandbox.log()
    );

    // yay present but broken: paru is the helper.
    let sandbox = Sandbox::new();
    fixture(&sandbox);
    sandbox.script("yay", Mode::Fail);
    sandbox.clear_log();
    sandbox.run(&["switch", "beta"]).assert_ok();
    assert!(
        log_has(&sandbox, "riceswap-float paru -S --noconfirm newaur"),
        "paru is the fallback when yay does not answer: {:?}",
        sandbox.log()
    );
    assert!(
        !sandbox.log().iter().any(|line| line.contains("yay -S")),
        "a broken yay is never invoked for a transaction: {:?}",
        sandbox.log()
    );
}

/// Class: AUR helper override. `--aur-helper <name>` pins the helper regardless
/// of what else is on PATH — the flag the testing decision locks in.
#[test]
fn the_aur_helper_override_flag_pins_the_helper() {
    let sandbox = Sandbox::new();
    fixture(&sandbox);
    sandbox.clear_log();

    let run = sandbox.run(&["switch", "beta", "--aur-helper", "paru"]);
    let data = run.assert_ok();

    assert!(
        log_has(&sandbox, "riceswap-float paru -S --noconfirm newaur"),
        "the override decides the helper: {:?}",
        sandbox.log()
    );
    assert!(
        !sandbox.log().iter().any(|line| line.contains("yay -S")),
        "an overridden yay is bypassed: {:?}",
        sandbox.log()
    );
    assert_eq!(data["report"]["installed"], json!(["newbar", "newaur"]));
}

/// Class: no AUR helper. With AUR packages in the plan and no helper usable on
/// PATH, the switch fails before touching anything, through an envelope error
/// that names both the packages and the helpers it looked for.
#[test]
fn no_usable_aur_helper_fails_with_an_error_naming_the_packages() {
    let sandbox = Sandbox::new();
    fixture(&sandbox);
    sandbox.script("yay", Mode::Fail);
    sandbox.script("paru", Mode::Fail);

    let run = sandbox.run(&["switch", "beta"]);
    let envelope = run.envelope();
    run.assert_no_panic();

    assert_eq!(envelope["ok"], json!(false), "{}", run.stdout);
    let error = envelope["data"]["error"]
        .as_str()
        .expect("the failure carries an error")
        .to_string();
    assert!(
        error.contains("newaur"),
        "the error names the AUR packages left uninstalled: {error}"
    );
    assert!(
        error.contains("yay") && error.contains("paru"),
        "the error says which helpers were looked for: {error}"
    );
    assert_eq!(
        envelope["data"]["completed_steps"],
        json!(2),
        "refused before the flip, like any other pre-flight refusal"
    );
    assert_eq!(
        envelope["data"]["resume_hint"],
        json!("switch to `beta` to restore")
    );

    // Nothing was touched: `current` still points at alpha, and the only stub
    // traffic was the helper probes themselves.
    assert_eq!(sandbox.current_target(), Some(sandbox.profile_dir("alpha")));
    assert_eq!(sandbox.state()["active_profile"], json!("alpha"));
    for line in sandbox.log() {
        assert!(
            line.ends_with("--version"),
            "no package op ran before the refusal: {line}"
        );
    }
}

/// Class: the floating-terminal wrapper. Every AUR helper transaction is
/// spawned inside the wrapper (stub-verifiable in the log), and what the
/// helper prints reaches the switch report.
#[test]
fn aur_helper_invocations_run_in_the_floating_terminal_and_output_reaches_the_report() {
    let sandbox = Sandbox::new();
    fixture(&sandbox);
    sandbox.clear_log();

    let run = sandbox.run(&["switch", "beta"]);
    let data = run.assert_ok();

    assert!(
        log_has(&sandbox, "riceswap-float yay -S --noconfirm newaur"),
        "the helper is spawned inside the floating-terminal wrapper: {:?}",
        sandbox.log()
    );
    assert!(
        log_has(&sandbox, "yay -S --noconfirm newaur"),
        "the wrapper runs the helper itself: {:?}",
        sandbox.log()
    );
    assert!(
        !sandbox
            .log()
            .iter()
            .any(|line| line.contains("yay") && line.contains("-R")),
        "AUR removals never go through the helper: {:?}",
        sandbox.log()
    );
    assert_eq!(
        data["report"]["aur_output"],
        json!(["installing newaur"]),
        "the helper's output flows into the switch report"
    );
}

/// Class: helper errors. A helper that fails inside the wrapper stops the
/// package ops with the failure model's report — `ok: false`, its stderr in
/// the error, `completed_steps` + `resume_hint` for recovery.
#[test]
fn aur_helper_errors_flow_into_the_switch_report() {
    let sandbox = Sandbox::new();
    fixture(&sandbox);
    // The helper detects cleanly (`--version` answers) and then fails its
    // transaction — the shape a real broken helper has.
    sandbox.fail_on("yay", "-S");
    sandbox.clear_log();

    let run = sandbox.run(&["switch", "beta"]);
    let envelope = run.envelope();
    run.assert_no_panic();

    assert_eq!(envelope["ok"], json!(false), "{}", run.stdout);
    let error = envelope["data"]["error"]
        .as_str()
        .expect("the failure carries an error")
        .to_string();
    assert!(
        error.contains("yay") && error.contains("newaur"),
        "the error names the helper and the package: {error}"
    );
    assert!(
        error.contains("scripted failure"),
        "the helper's own stderr reaches the envelope: {error}"
    );
    assert_eq!(envelope["data"]["completed_steps"], json!(5));
    assert_eq!(
        envelope["data"]["resume_hint"],
        json!("switch to `beta` to restore")
    );
    assert!(
        log_has(&sandbox, "pkexec pacman -S --noconfirm newbar"),
        "official installs ran first, before the helper failed: {:?}",
        sandbox.log()
    );
}
