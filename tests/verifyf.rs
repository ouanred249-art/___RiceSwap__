//! Ticket #40: Tier F — recipe-driven probes on the live desktop.
//!
//! The sandbox has no desktop, which is exactly the point. A probe script is
//! executed for real: `ydotool` is invoked with the keycodes a recipe declared
//! and `grim` is asked for the screenshots the tier compares. What a test
//! scripts is the *answers* — a screen that changes or a screen that does not,
//! a command that answers or one that does not — and what it asserts is the
//! tier's own reading of them.
//!
//! Two seams carry that, both in `tests/common/mod.rs`:
//!
//! * the `grim` stub writes a call counter into its own bytes, so two calls
//!   differ. The tier's observation floor is "the encoding of that region is not
//!   what it was", and a stub whose screenshots never varied would make every
//!   probe fail for a reason that is the stub's;
//! * `screen_never_changes` / `screen_changes_after` hold those bytes still for
//!   a while, which is how the "never settled" and "settled on the re-run" cases
//!   are asked for.
//!
//! What these tests hold to, in order:
//!
//! * a declared probe runs its steps in order and answers: `verified-full`,
//!   with a `tier: "F"` row under the id the recipe gave it;
//! * a probe that never sees its expected outcome is `probe-failed:<id>`, and
//!   the old desktop goes back through the same rollback #39 established;
//! * no `ydotool` means the whole tier does not run: `verified-core`, the
//!   Tier C rows unchanged, and a warning that says which tool was missing;
//! * a probe naming the lock capability is refused with evidence, and the other
//!   probes beside it still run;
//! * a probe that only answers on its re-run passes, because one sample of a
//!   flaky thing is not a verdict;
//! * a recipe with no probes still gets the built-in floor;
//! * the byte floor is honest about being a byte floor;
//! * and the whole thing is still not a step: ten steps, unchanged messages.

mod common;

use common::{Mode, Sandbox, profile_toml_with_shell};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::symlink;

/// The verdict and checks, as `data.report.verification` carries them.
fn verdict(data: &Value) -> &Value {
    &data["report"]["verification"]
}

/// One check's `id` → the whole check object.
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

/// Whether any Tier F row exists at all — the shape a degraded tier must never
/// produce, since a row means something was read.
fn has_tier(verification: &Value, tier: &str) -> bool {
    verification["checks"]
        .as_array()
        .expect("a list")
        .iter()
        .any(|check| check["tier"] == json!(tier))
}

/// The leaving desktop: another shell, its own config, and a live tree.
fn old_desktop(sandbox: &Sandbox) {
    let service = ("qs", "qs -c $qsConfig", "pkill qs");
    sandbox.write_profile(
        "ii",
        &profile_toml_with_shell(
            "ii",
            &[],
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

/// The arriving desktop: a shell whose QML the engine can derive a registry
/// from, and a Hyprland config dispatching the previous shell's vocabulary —
/// which is the shape the whole verification tier was designed against.
fn rice(sandbox: &Sandbox) {
    rice_with(sandbox, "");
}

/// [`rice`], with a `adapt.toml` in the human tier. A probe script is a claim
/// about *this* machine, so it is written where a machine-specific answer
/// belongs: beside the profile, in the tier that speaks for it.
fn rice_with(sandbox: &Sandbox, adapt: &str) {
    let service = ("qs", "qs -c $qsConfig", "pkill qs");
    sandbox.write_profile(
        "caelestia",
        &profile_toml_with_shell(
            "caelestia",
            &[],
            &[],
            &[service],
            &[".config/hypr", ".config/quickshell/caelestia"],
            ("caelestia", "qs -c caelestia", "pkill qs"),
        ),
    );
    sandbox.write_profile_file(
        "caelestia",
        ".config/quickshell/caelestia/components/misc/CustomShortcut.qml",
        "import Quickshell.Hyprland\n\n// qmllint disable unresolved-type\nGlobalShortcut {\n    \
         // qmllint enable unresolved-type\n    appid: \"caelestia\"\n}\n",
    );
    sandbox.write_profile_file(
        "caelestia",
        ".config/quickshell/caelestia/modules/lock/Lock.qml",
        "import QtQuick\n\nScope {\n    GlobalShortcut {\n        name: \"lock\"\n        \
         description: \"Lock the session\"\n        onPressed: {}\n    }\n}\n",
    );
    sandbox.write_profile_file(
        "caelestia",
        ".config/hypr/hyprland/keybinds.lua",
        "$lock_cmd = hyprctl dispatch 'hl.dsp.global(\"quickshell:lock\")' & hyprlock\n",
    );
    if !adapt.is_empty() {
        sandbox.write_profile_file("caelestia", "adapt.toml", adapt);
    }
}

/// A registry the live session is claimed to hold: the rice's own name, plus
/// the panel RiceSwap itself runs (which belongs to no profile's registry and is
/// in the keep set instead).
fn live_caelestia() -> Vec<&'static str> {
    vec!["caelestia:lock", "quickshell:riceswap-toggle"]
}

/// The #35-session script, written down: tap Super, the launcher region opens.
/// Declared in the human tier, driven by the switch, read back out of `checks`.
const LAUNCHER_OPENS: &str = r#"
[[probe]]
id = "launcher-opens"
steps = [
  { do = "key", keycodes = "125:1 125:0" },
  { do = "shot", region = "800x600+0+0", expect = "change" },
]
"#;

/// A declared probe that the session answers: `verified-full`, and the row is
/// the recipe's own id under `tier: "F"`, with the evidence naming the steps in
/// the order they ran.
#[test]
fn a_declared_probe_that_answers_makes_the_verdict_verified_full() {
    let sandbox = Sandbox::new();
    old_desktop(&sandbox);
    rice_with(&sandbox, LAUNCHER_OPENS);
    sandbox.live_registry(&live_caelestia());
    sandbox.clear_log();

    let data = sandbox.run(&["switch", "caelestia"]).assert_ok();

    let verification = verdict(&data);
    assert_eq!(
        verification["verdict"],
        json!("verified-full"),
        "Tier C passed and the functional tier's only probe answered: {verification}"
    );
    let row = check(verification, "launcher-opens");
    assert_eq!(row["tier"], json!("F"));
    assert_eq!(row["ok"], json!(true));
    // The frozen check object carries nothing else, at this tier either.
    let mut keys: Vec<&str> = row
        .as_object()
        .expect("a check is an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["evidence", "id", "ok", "tier"], "{row}");

    // The evidence says what ran, in the order it ran: the synthetic input
    // first, then the region it opened.
    let evidence = evidence(verification, "launcher-opens");
    assert!(
        evidence.contains("`ydotool key 125:1 125:0` sent"),
        "the input step is named: {evidence}"
    );
    assert!(
        evidence.contains("the `800x600+0+0` region changed"),
        "and so is the region it was supposed to change: {evidence}"
    );
    let sent = evidence
        .find("ydotool key")
        .expect("the input is in the evidence");
    let looked = evidence
        .find("region changed")
        .expect("the observation is in the evidence");
    assert!(sent < looked, "steps are reported as they ran: {evidence}");

    // The tools really were driven, in that order, against the live session. The
    // region is captured *twice* — once for the `before` and once for the look
    // that saw the change — and the input belongs between the two, which is
    // what makes the pair a pair.
    let log = sandbox.log();
    let before = log
        .iter()
        .position(|line| line.contains("grim -g 800x600+0+0"))
        .expect("the region's `before` was captured");
    let key = log
        .iter()
        .position(|line| line.contains("ydotool key 125:1 125:0"))
        .expect("the keycodes were sent");
    let after = log
        .iter()
        .rposition(|line| line.contains("grim -g 800x600+0+0"))
        .expect("the region was captured again");
    assert!(
        before < key && key < after,
        "the `before`, then the input, then the look:\n{log:?}"
    );
    assert!(
        sandbox.shots() >= 2,
        "a `before` and an `after`, at least: {}",
        sandbox.shots()
    );
    assert_eq!(
        sandbox.current_target(),
        Some(sandbox.profile_dir("caelestia"))
    );
}

/// A declared probe that never sees its expected outcome is the same failure as
/// a dead bind: `probe-failed:<id>`, the frozen payload, and the old desktop
/// back through the rollback #39 already established. Nothing about the
/// recovery is a second mechanism.
#[test]
fn a_probe_that_never_settles_fails_and_puts_the_old_desktop_back() {
    let sandbox = Sandbox::new();
    old_desktop(&sandbox);
    rice_with(&sandbox, LAUNCHER_OPENS);
    sandbox.live_registry(&live_caelestia());
    // The screen never changes, so the launcher never opens.
    sandbox.screen_never_changes();
    sandbox.clear_log();

    let run = sandbox.run(&["switch", "caelestia"]);
    let error = run.assert_failed();
    let data = &run.envelope()["data"];

    assert_eq!(data["phase"], json!("verify"));
    assert_eq!(
        data["reason_code"],
        json!("probe-failed:launcher-opens"),
        "the reason code is the frozen one, carrying the probe's own id"
    );
    assert!(error.contains("probe-failed:launcher-opens"), "{error}");
    assert_eq!(verdict(data)["verdict"], json!("fail"));
    assert_eq!(
        check(verdict(data), "shell-alive")["ok"],
        json!(true),
        "the core passed; the functional tier is what rejected the desktop"
    );
    let evidence = evidence(verdict(data), "launcher-opens");
    assert!(
        evidence.contains("never changed"),
        "and the row says what it kept waiting for: {evidence}"
    );
    assert!(
        evidence.contains("re-run"),
        "and that it was re-run once before counting: {evidence}"
    );

    // The same rollback, in the same order, off the same evidence.
    assert_eq!(
        sandbox.current_target(),
        Some(sandbox.profile_dir("ii")),
        "current points back at the profile that was there before"
    );
    assert_eq!(sandbox.state()["active_profile"], json!("ii"));
    assert!(sandbox.log_contains("pkill qs"), "{:?}", sandbox.log());
    assert!(sandbox.log_contains("qs -c ii"), "{:?}", sandbox.log());
}

/// The degrade rule, one tier wider than #39's. A `ydotool` that cannot even
/// print its help means this session cannot be driven at all, so the tier does
/// not run — `verified-core`, the Tier C rows byte for byte as they were, and
/// one warning naming what was missing. The load-bearing half is the *absence*:
/// no `tier: "F"` row, because a check that never ran is not a check that
/// passed and must not look like one.
#[test]
fn no_ydotool_means_the_whole_functional_tier_does_not_run() {
    let sandbox = Sandbox::new();
    old_desktop(&sandbox);
    rice_with(&sandbox, LAUNCHER_OPENS);
    sandbox.live_registry(&live_caelestia());
    sandbox.script("ydotool", Mode::Fail);
    sandbox.clear_log();

    let run = sandbox.run(&["switch", "caelestia"]);
    let data = run.assert_ok();

    assert_eq!(
        verdict(&data)["verdict"],
        json!("verified-core"),
        "a session that cannot be driven is the honest degraded pass: {}",
        verdict(&data)
    );
    assert!(
        !has_tier(verdict(&data), "F"),
        "no phantom row: a tier that did not run contributes no check: {}",
        verdict(&data)
    );
    assert_eq!(check(verdict(&data), "shell-alive")["ok"], json!(true));
    assert_eq!(check(verdict(&data), "invariant-live")["ok"], json!(true));
    assert!(
        !sandbox.log_contains("ydotool key"),
        "and nothing was typed at the session: {:?}",
        sandbox.log()
    );
    let warnings = run.warnings();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("ydotool") && warning.contains("did not run")),
        "the downgrade is loud and names the tool: {warnings:?}"
    );
}

/// The same rule from the other tool: no `grim`, no tier. The two are separate
/// questions because they are separate absences, and a machine missing only one
/// of them is the case a reader will meet first.
#[test]
fn no_grim_means_the_whole_functional_tier_does_not_run() {
    let sandbox = Sandbox::new();
    old_desktop(&sandbox);
    rice_with(&sandbox, LAUNCHER_OPENS);
    sandbox.live_registry(&live_caelestia());
    sandbox.script("grim", Mode::Fail);
    sandbox.clear_log();

    let run = sandbox.run(&["switch", "caelestia"]);
    let data = run.assert_ok();

    assert_eq!(verdict(&data)["verdict"], json!("verified-core"));
    assert!(
        !has_tier(verdict(&data), "F"),
        "no row for a tier that could not run: {}",
        verdict(&data)
    );
    assert!(
        run.warnings()
            .iter()
            .any(|warning| warning.contains("grim") && warning.contains("did not run")),
        "{:?}",
        run.warnings()
    );
    assert_eq!(
        sandbox.current_target(),
        Some(sandbox.profile_dir("caelestia"))
    );
}

/// Lock is invariant-only in production. A probe that names the lock capability
/// is refused *before it runs* — the evidence says so, the rest of the tier
/// carries on, and the verdict degrades rather than rolling a working desktop
/// back over a probe RiceSwap declined to run. The other probe in the same
/// recipe is the proof that a refusal is not a cancellation.
#[test]
fn a_probe_naming_the_lock_capability_is_refused_with_evidence() {
    let sandbox = Sandbox::new();
    old_desktop(&sandbox);
    rice_with(
        &sandbox,
        concat!(
            "[[probe]]\nid = \"lock-the-session\"\nsteps = [\n",
            "  { do = \"key\", keycodes = \"125:1 125:0\" },\n",
            "  { do = \"shot\", expect = \"change\" },\n",
            "]\n",
            "[[probe]]\nid = \"launcher-opens\"\nsteps = [\n",
            "  { do = \"key\", keycodes = \"125:1 125:0\" },\n",
            "  { do = \"shot\", expect = \"change\" },\n",
            "]\n",
        ),
    );
    sandbox.live_registry(&live_caelestia());
    sandbox.clear_log();

    let data = sandbox.run(&["switch", "caelestia"]).assert_ok();

    // Refused, named, and nothing was dispatched to find out.
    let refused = check(verdict(&data), "lock-the-session");
    assert_eq!(refused["tier"], json!("F"));
    assert_eq!(
        refused["ok"],
        json!(true),
        "a refusal is not a failure: it is not evidence of a broken desktop, and the \
         frozen key set has no `skipped` to say it in but the evidence"
    );
    let evidence = evidence(verdict(&data), "lock-the-session");
    assert!(evidence.contains("refused"), "{evidence}");
    assert!(evidence.contains("`lock`"), "the name is named: {evidence}");
    assert!(
        evidence.contains("never drives a session lock"),
        "and so is the rule: {evidence}"
    );

    // The probe beside it ran, and the verdict degrades rather than failing:
    // a probe the tier declined to run is not a probe the desktop failed.
    let ran = check(verdict(&data), "launcher-opens");
    assert_eq!(ran["ok"], json!(true));
    assert_eq!(
        verdict(&data)["verdict"],
        json!("verified-core"),
        "a refused probe is not a proved desktop: {}",
        verdict(&data)
    );
    assert_eq!(
        sandbox.current_target(),
        Some(sandbox.profile_dir("caelestia"))
    );
}

/// The same refusal by the *command* rather than by the id, because a probe can
/// name a lock without calling itself one: a `hyprctl dispatch` at
/// `caelestia:lock` is the shape the real configs use, and it is refused on the
/// strength of the name in it.
#[test]
fn a_probe_whose_command_dispatches_the_lock_is_refused_too() {
    let sandbox = Sandbox::new();
    old_desktop(&sandbox);
    rice_with(
        &sandbox,
        concat!(
            "[[probe]]\nid = \"cycle\"\nsteps = [\n",
            "  { do = \"ipc\", cmd = \"hyprctl dispatch 'hl.dsp.global(\\\"caelestia:lock\\\")'\", \
             expect = \"ok\" },\n",
            "]\n",
        ),
    );
    sandbox.live_registry(&live_caelestia());
    sandbox.clear_log();

    let data = sandbox.run(&["switch", "caelestia"]).assert_ok();

    let evidence = evidence(verdict(&data), "cycle");
    assert!(evidence.contains("refused"), "{evidence}");
    assert!(
        !sandbox.log_contains("hl.dsp.global"),
        "the dispatch never ran: {:?}",
        sandbox.log()
    );
    assert_eq!(verdict(&data)["verdict"], json!("verified-core"));
}

/// A probe that answers only on its re-run passes, because the policy is one
/// retry before a probe contributes to the verdict: a launcher that needed a
/// second look is a launcher that opened, and a probe that failed the first
/// time and passed the second is exactly the flakiness the retry exists for.
/// The evidence says which of the two runs is being reported, so a reader can
/// see the flake rather than infer it.
#[test]
fn a_probe_that_answers_only_on_its_re_run_passes() {
    let sandbox = Sandbox::new();
    old_desktop(&sandbox);
    // A one-second settle, so the test is a second of waiting rather than ten.
    rice_with(
        &sandbox,
        concat!(
            "[[probe]]\nid = \"launcher-opens\"\nsettle_seconds = 1\nsteps = [\n",
            "  { do = \"key\", keycodes = \"125:1 125:0\" },\n",
            "  { do = \"shot\", expect = \"change\" },\n",
            "]\n",
        ),
    );
    sandbox.live_registry(&live_caelestia());
    // The screen is frozen long enough to cover a whole first attempt — its
    // `before` plus every look its one-second settle allows — and starts
    // changing for the re-run.
    sandbox.screen_changes_after(6);
    sandbox.clear_log();

    let data = sandbox.run(&["switch", "caelestia"]).assert_ok();

    let verification = verdict(&data);
    assert_eq!(
        verification["verdict"],
        json!("verified-full"),
        "the re-run answered, so the probe did not contribute a failure: {verification}"
    );
    let evidence = evidence(verification, "launcher-opens");
    assert!(
        evidence.contains("re-run"),
        "the flake is in the evidence: {evidence}"
    );
    assert!(
        evidence.contains("did not answer"),
        "and so is what the first run did: {evidence}"
    );
    assert_eq!(
        sandbox.current_target(),
        Some(sandbox.profile_dir("caelestia")),
        "a flaky probe does not roll a working desktop back"
    );
}

/// The built-in floor: a recipe with no `[[probe]]` at all still gets a
/// functional check, because a machine that can be driven should not be asked
/// nothing. This is the one probe every shell earns without a recipe, and its
/// id says so in `checks`.
#[test]
fn a_recipe_with_no_probes_still_gets_the_built_in_floor() {
    let sandbox = Sandbox::new();
    old_desktop(&sandbox);
    rice(&sandbox);
    sandbox.live_registry(&live_caelestia());
    sandbox.clear_log();

    let data = sandbox.run(&["switch", "caelestia"]).assert_ok();

    let verification = verdict(&data);
    assert_eq!(
        verification["verdict"],
        json!("verified-full"),
        "the floor ran and answered with no recipe present: {verification}"
    );
    let floor = check(verification, "floor-super-opens-something");
    assert_eq!(floor["tier"], json!("F"));
    assert_eq!(floor["ok"], json!(true));
    let evidence = evidence(verification, "floor-super-opens-something");
    assert!(
        evidence.contains("`ydotool key 125:1 125:0` sent"),
        "the floor is the #35 session's first move, verbatim: {evidence}"
    );
    assert!(
        evidence.contains("the whole screen changed"),
        "and it watches the whole screen rather than guessing a region: {evidence}"
    );
}

/// The screenshot primitive is a *byte* comparison and the report says so, in
/// the row every reader of a pass will read. This is the honesty the ticket
/// bought: the claim is sized to the primitive, and a reader who knows a ticking
/// clock would satisfy it is told, rather than having to guess what "changed"
/// was measured against.
#[test]
fn the_screenshot_evidence_says_it_compared_bytes_and_not_images() {
    let sandbox = Sandbox::new();
    old_desktop(&sandbox);
    rice_with(&sandbox, LAUNCHER_OPENS);
    sandbox.live_registry(&live_caelestia());
    sandbox.clear_log();

    let data = sandbox.run(&["switch", "caelestia"]).assert_ok();
    let evidence = evidence(verdict(&data), "launcher-opens");

    assert!(
        evidence.contains("a byte comparison, not an image diff"),
        "the row names the floor it measured: {evidence}"
    );
    assert!(
        evidence.contains(" B before, ") && evidence.contains(" B after"),
        "and it reports the two sizes, so `changed` is checkable: {evidence}"
    );
}

/// A declared probe is a script, and a script runs in the order it was written:
/// type, then observe, then observe again. The stub log is the only place the
/// order is visible, and it is where a wrong order would show.
#[test]
fn a_probe_runs_its_steps_in_the_order_it_declares_them() {
    let sandbox = Sandbox::new();
    old_desktop(&sandbox);
    rice_with(
        &sandbox,
        concat!(
            "[[probe]]\nid = \"launcher-then-wallpaper\"\nsteps = [\n",
            "  { do = \"key\", keycodes = \"125:1 125:0\" },\n",
            "  { do = \"type\", text = \">wal\" },\n",
            "  { do = \"key\", keycodes = \"28:1 28:0\" },\n",
            "  { do = \"shot\", expect = \"change\" },\n",
            "]\n",
        ),
    );
    sandbox.live_registry(&live_caelestia());
    sandbox.clear_log();

    let data = sandbox.run(&["switch", "caelestia"]).assert_ok();

    assert_eq!(verdict(&data)["verdict"], json!("verified-full"));
    // The last `grim` is the look that saw the change, and it is the one the
    // ordering claim is about: the `before` is deliberately taken first.
    let log = sandbox.log();
    let at = |needle: &str| {
        log.iter()
            .position(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("`{needle}` never ran:\n{log:?}"))
    };
    let (super_tap, typed, enter, shot) = (
        at("ydotool key 125:1 125:0"),
        at("ydotool type"),
        at("ydotool key 28:1 28:0"),
        log.iter()
            .rposition(|line| line.starts_with("grim "))
            .expect("the region was looked at"),
    );
    assert!(
        super_tap < typed && typed < enter && enter < shot,
        "the #35 sequence, in the order it was declared:\n{log:?}"
    );
    // And the text is passed as an argument, not interpreted by a shell: the
    // `>wal` is a query a launcher types, not a redirect.
    let typed_line = &log[typed];
    assert!(
        typed_line.contains("-- >wal"),
        "the text is one argument, after `--`: {typed_line}"
    );
}

/// A `shot` step that declares `same` is the other half of the observation, and
/// a probe that expects a region *not* to move is as real a check as one that
/// expects it to. It is also the case that would be silently wrong if `expect`
/// were read as a truthiness test, so it is asked for explicitly.
#[test]
fn a_step_can_expect_a_region_to_stay_the_same() {
    let sandbox = Sandbox::new();
    old_desktop(&sandbox);
    rice_with(
        &sandbox,
        concat!(
            "[[probe]]\nid = \"no-wallpaper-change\"\nsettle_seconds = 1\nsteps = [\n",
            "  { do = \"shot\", region = \"1920x1080+0+0\", expect = \"same\" },\n",
            "]\n",
        ),
    );
    sandbox.live_registry(&live_caelestia());
    // The default stub *changes* its bytes every call, so a `same` expectation
    // can only pass on a screen held still.
    sandbox.screen_never_changes();
    sandbox.clear_log();

    let data = sandbox.run(&["switch", "caelestia"]).assert_ok();

    let verification = verdict(&data);
    assert_eq!(
        verification["verdict"],
        json!("verified-full"),
        "a region that did not change is the outcome this probe asked for: {verification}"
    );
    let evidence = evidence(verification, "no-wallpaper-change");
    assert!(
        evidence.contains("the `1920x1080+0+0` region stayed the same"),
        "{evidence}"
    );
}

/// An `ipc` step is the precise half of the observation vocabulary: where a
/// screenshot can only say "something moved", a command's own answer can say
/// what. Both primitives are used by probes on the same desktop, and the
/// `verified-full` is earned by the two together.
#[test]
fn an_ipc_step_reads_a_command_answer_and_both_primitives_are_used() {
    let sandbox = Sandbox::new();
    old_desktop(&sandbox);
    rice_with(
        &sandbox,
        concat!(
            "[[probe]]\nid = \"launcher-opens\"\nsettle_seconds = 1\nsteps = [\n",
            "  { do = \"key\", keycodes = \"125:1 125:0\" },\n",
            "  { do = \"shot\", expect = \"change\" },\n",
            "]\n",
            "[[probe]]\nid = \"wallpaper-applied\"\nsettle_seconds = 1\nsteps = [\n",
            "  { do = \"ipc\", cmd = \"echo wallpaper is set\", expect = \"is set\" },\n",
            "]\n",
        ),
    );
    sandbox.live_registry(&live_caelestia());
    sandbox.screen_changes_after(1);
    sandbox.clear_log();

    let data = sandbox.run(&["switch", "caelestia"]).assert_ok();

    let verification = verdict(&data);
    assert_eq!(
        verification["verdict"],
        json!("verified-full"),
        "the screenshot probe and the ipc probe both answered: {verification}"
    );
    assert_eq!(check(verification, "launcher-opens")["ok"], json!(true));
    let ipc = evidence(verification, "wallpaper-applied");
    assert!(
        ipc.contains("`echo wallpaper is set` answered with `wallpaper is set`"),
        "the ipc row quotes the answer it matched: {ipc}"
    );
}

/// The degrade rule and the schema's tolerance meet in the ordinary case: a
/// recipe with no `[[probe]]` on a machine that cannot be driven is
/// `verified-core` with the Tier C rows alone, and the floor is not even
/// attempted — no row, no keystroke, no screenshot.
#[test]
fn a_degraded_machine_with_a_probe_declared_keeps_the_core_rows_exactly() {
    let sandbox = Sandbox::new();
    old_desktop(&sandbox);
    rice_with(&sandbox, LAUNCHER_OPENS);
    sandbox.live_registry(&live_caelestia());
    sandbox.script("ydotool", Mode::Fail);

    let data = sandbox.run(&["switch", "caelestia"]).assert_ok();
    let verification = verdict(&data);

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
        "the frozen verdict object is unchanged by a tier that did not run: {verification}"
    );
    let tiers: Vec<&str> = verification["checks"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|check| check["tier"].as_str().expect("a tier"))
        .collect();
    assert!(
        tiers.iter().all(|tier| *tier == "C"),
        "every row is a Tier C row, byte for byte as #39 left it: {tiers:?}"
    );
    assert_eq!(sandbox.shots(), 0, "and nothing was screenshotted");
}

/// A declaration that cannot be read costs that entry and nothing else — the
/// same rule a bad `[[resolution]]` has had since #36. The verdict degrades
/// because a tier that was asked to be driven and was not has not been proved,
/// and the warning says which entry is missing.
#[test]
fn a_probe_that_cannot_be_read_costs_itself_and_degrades_the_verdict() {
    let sandbox = Sandbox::new();
    old_desktop(&sandbox);
    rice_with(
        &sandbox,
        concat!(
            "[[probe]]\nid = \"launcher-opens\"\nsteps = [\n",
            "  { do = \"key\", keycodes = \"125:1 125:0\" },\n",
            "  { do = \"shot\", expect = \"change\" },\n",
            "]\n",
            "[[probe]]\nid = \"no-expectation\"\nsteps = [\n",
            "  { do = \"shot\" },\n",
            "]\n",
        ),
    );
    sandbox.live_registry(&live_caelestia());
    sandbox.clear_log();

    let run = sandbox.run(&["switch", "caelestia"]);
    let data = run.assert_ok();

    assert_eq!(
        verdict(&data)["verdict"],
        json!("verified-core"),
        "one probe ran, one could not be read, so nothing is fully proved: {}",
        verdict(&data)
    );
    assert_eq!(check(verdict(&data), "launcher-opens")["ok"], json!(true));
    assert!(
        run.warnings()
            .iter()
            .any(|warning| warning.contains("could not be read")),
        "the dropped entry is loud: {:?}",
        run.warnings()
    );
}

/// Driving the desktop is still not a step of the switch. The panel's checklist
/// is frozen at eight messages and `completed_steps` at ten, and the tier reads
/// the result of those ten rather than adding a row of its own.
#[test]
fn driving_the_desktop_is_not_a_step_of_the_switch() {
    let sandbox = Sandbox::new();
    old_desktop(&sandbox);
    rice_with(&sandbox, LAUNCHER_OPENS);
    sandbox.live_registry(&live_caelestia());

    let run = sandbox.run(&["switch", "caelestia"]);
    let data = run.assert_ok();

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
        "the panel's checklist is unchanged: the tier reads the result, it does not add a row \
         to the work"
    );
}

/// #41 will run this on a real machine. What it will *not* do is lock the
/// session, and this holds the production path to that by name: a `lock`
/// capability in the live registry is a fact Tier C reads, and nothing in the
/// switch or the tier dispatches one to find out.
#[test]
fn the_production_path_never_dispatches_a_lock() {
    let sandbox = Sandbox::new();
    old_desktop(&sandbox);
    rice_with(&sandbox, LAUNCHER_OPENS);
    sandbox.live_registry(&live_caelestia());
    sandbox.clear_log();

    let data = sandbox.run(&["switch", "caelestia"]).assert_ok();

    // The lock capability is a *name* the live session registers, and Tier C's
    // invariant is what proves it: the profile dispatches `quickshell:lock`, the
    // engine moved it, and the live registry is asked whether `caelestia:lock`
    // is there. Nothing dispatches one to find out.
    let invariant = evidence(verdict(&data), "invariant-live");
    assert!(
        invariant.contains("1 dispatched, 2 registered live"),
        "the lock capability is a name in the registry, read and not pressed: {invariant}"
    );
    let evidence = evidence(verdict(&data), "launcher-opens");
    assert!(
        !evidence.to_lowercase().contains("lock"),
        "no probe row claims to have touched it: {evidence}"
    );
    assert_eq!(
        sandbox.current_target(),
        Some(sandbox.profile_dir("caelestia"))
    );
}
