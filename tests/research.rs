//! Ticket #38: the research tier — one `pi` run for a rice no recipe covers,
//! and the user-tier recipe that run produces.
//!
//! Every test here drives the compiled binary as a subprocess against the
//! sandbox: an isolated fake `$HOME`, a stubbed `PATH` whose `pi` is the seam,
//! and nothing else. **No test in this file reaches a network, and the real
//! `pi` is never on the sandbox `PATH`** — the stub records what it was asked
//! to do, prints the JSONL stream the fixture names, and exits with the code
//! the fixture names. That is the whole contract the research tier has to
//! survive, including the arm the contract warns about most: a provider that
//! refuses at request time still exits 0.
//!
//! The rice every test installs is the same shape, and it is the shape that
//! *asks* to be researched: a Quickshell shell whose shortcut name is built at
//! runtime (so the static parse records uncertainty and proves nothing), beside
//! configs that dispatch names nothing in the tree answers for. A shell the
//! engine can already read is the other shape, and there `pi` must never run.

mod common;

use common::{Mode, Sandbox};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

// ------------------------------------------------------------------ fixtures

/// A `riceswap.research.v1` answer, in the shape the brief asks for.
///
/// It answers three of the engine's dead names and admits to one gap, which is
/// the shape a real answer has: a codebook, a package nothing in the configs
/// names, and the questions it could not close.
fn answer_document(appid: &str) -> String {
    json!({
        "schema": "riceswap.research.v1",
        "shell": "demo",
        "appid": appid,
        "env": { "DEMO_WALLPAPERS": "~/Pictures/Wallpapers" },
        "dirs": [{ "path": "~/Pictures/Wallpapers" }],
        "foreign": ["quickshell:riceswap-toggle"],
        "resolutions": [
            { "dispatched": "quickshell:regionScreenshot", "to": "screenshotClip" },
            { "dispatched": "quickshell:overviewClipboardToggle", "drop": true },
            { "dispatched": "quickshell:barToggle", "drop": true }
        ],
        "packages": { "official": ["quickshell", "matugen"], "aur": [] },
        "dispatcher_namespaces": { "screenshotClip": appid },
        "start_cmd": ["qs", "-c", "demo"],
        "stop_cmd": ["pkill", "qs"],
        "keybind_namespace": "SUPER",
        "wallpaper_dir": "~/Pictures/Wallpapers",
        "capabilities": ["a launcher", "a screenshot shortcut"],
        "confidence": 0.72,
        "evidence": [{ "file": "README.md", "quote": "requires quickshell" }],
        "unknowns": ["where the launcher keeps its own config"]
    })
    .to_string()
}

/// A well-formed stream whose last assistant message *stopped*, carrying
/// `document` as its text — the shape a successful `--mode json` run has.
fn stopped_stream(document: &str) -> String {
    stream_of(document, "stop", None)
}

/// The same stream with a refusal at the end of it: exit 0, every record
/// well-formed, and the answer saying it failed. The contract's own trap, and
/// the one a test has to prove is not mistaken for success.
fn refused_stream(message: &str) -> String {
    stream_of("", "error", Some(message))
}

/// A full JSONL record set around one assistant message: the session header
/// pi emits even with `--no-session`, a start, the message, and the settle.
fn stream_of(document: &str, stop: &str, error_message: Option<&str>) -> String {
    let mut message = json!({
        "role": "assistant",
        "stopReason": stop,
        "content": [{ "type": "text", "text": document }],
        "usage": { "input": 1200, "output": 340, "totalTokens": 1540, "cost": { "total": 0.02 } }
    });
    if let Some(message_text) = error_message {
        message["errorMessage"] = json!(message_text);
    }
    let records = [
        json!({ "type": "session", "version": 3, "id": "3f0b", "timestamp": "2026-09-29T00:00:00Z", "cwd": "/tmp" }),
        json!({ "type": "agent_start" }),
        json!({ "type": "turn_start" }),
        json!({ "type": "message_start", "message": { "role": "user" } }),
        json!({ "type": "message_end", "message": message }),
        json!({ "type": "agent_end", "willRetry": false }),
        json!({ "type": "agent_settled" }),
    ];
    records
        .iter()
        .map(|record| record.to_string())
        .collect::<Vec<String>>()
        .join("\n")
        + "\n"
}

/// A shell the static parse cannot read: its shortcut's name is built at
/// runtime, so the registry is empty and the parse says so.
///
/// This is the honest "unknown rice" — a real quickshell shell that composes
/// its registration names, which #35's static parse refuses to guess at. The
/// configs beside it dispatch three names the tree answers for none of, which
/// is the engine's proposal vocabulary the brief is built from.
fn unreadable_shell(tree: &Path, shell: &str, appid: &str) {
    let qml = tree.join(".config").join("quickshell").join(shell);
    fs::create_dir_all(&qml).expect("create the quickshell dir");
    fs::write(qml.join("shell.qml"), "import QtQuick\n\nShellRoot {}\n")
        .expect("write the shell marker");
    fs::create_dir_all(qml.join("components").join("misc")).expect("create a qml dir");
    fs::write(
        qml.join("components")
            .join("misc")
            .join("CustomShortcut.qml"),
        format!("import Quickshell.Hyprland\n\nGlobalShortcut {{\n    appid: \"{appid}\"\n}}\n"),
    )
    .expect("write the appid declaration");
    fs::write(
        qml.join("components").join("misc").join("Launchers.qml"),
        "import QtQuick\n\nScope {\n    property string suffix: \"Clip\"\n    GlobalShortcut {\n        name: screenshotName\n    }\n}\n",
    )
    .expect("write a registration built at runtime");
    // One name the static parse *can* prove, so the answer's `to` target has a
    // real counterpart to point at. The recipe tier refuses a target the
    // derived registry does not hold, so an answer naming an unproved one would
    // be reported and left alone — and that is worth its own test, further down.
    fs::write(
        qml.join("components").join("misc").join("Snips.qml"),
        "import QtQuick\n\nScope {\n    GlobalShortcut {\n        name: \"screenshotClip\"\n    }\n}\n",
    )
    .expect("write the one provable registration");

    let hypr = tree.join(".config").join("hypr");
    fs::create_dir_all(&hypr).expect("create the hypr dir");
    fs::write(
        hypr.join("hyprland.conf"),
        "monitor=,preferred,auto,1\n\nsource = ~/.config/hypr/hyprland/keybinds.lua\n",
    )
    .expect("write hyprland.conf");
    fs::create_dir_all(hypr.join("hyprland")).expect("create the hyprland dir");
    fs::write(
        hypr.join("hyprland").join("keybinds.lua"),
        concat!(
            "hl.bind(\"SUPER + SHIFT + S\", hl.dsp.global(\"quickshell:regionScreenshot\"), ",
            "{ description = \"Screen snip\" })\n",
            "hl.bind(\"SUPER + SHIFT + C\", hl.dsp.global(\"quickshell:overviewClipboardToggle\"), ",
            "{ description = \"Clipboard\" })\n",
            "hl.bind(\"SUPER + B\", hl.dsp.global(\"quickshell:barToggle\"), ",
            "{ description = \"Bar\" })\n",
        ),
    )
    .expect("write keybinds.lua");
}

/// A shell the static parse can read: one literal registration, no findings.
/// The static engine has a registry for it, so there is nothing to ask about.
fn readable_shell(tree: &Path, shell: &str, appid: &str) {
    let qml = tree.join(".config").join("quickshell").join(shell);
    fs::create_dir_all(&qml).expect("create the quickshell dir");
    fs::write(qml.join("shell.qml"), "import QtQuick\n\nShellRoot {}\n")
        .expect("write the shell marker");
    fs::write(
        qml.join("Lock.qml"),
        format!(
            "import QtQuick\n\nScope {{\n    GlobalShortcut {{\n        appid: \"{appid}\"\n        \
             name: \"lock\"\n    }}\n}}\n"
        ),
    )
    .expect("write the one provable registration");
    let hypr = tree.join(".config").join("hypr");
    fs::create_dir_all(&hypr).expect("create the hypr dir");
    fs::write(
        hypr.join("hyprland.conf"),
        "monitor=,preferred,auto,1\n\nsource = ~/.config/hypr/hyprland/keybinds.lua\n",
    )
    .expect("write hyprland.conf");
    fs::create_dir_all(hypr.join("hyprland")).expect("create the hyprland dir");
    fs::write(
        hypr.join("hyprland").join("keybinds.lua"),
        concat!(
            "hl.bind(\"SUPER + L\", hl.dsp.global(\"quickshell:lock\"), ",
            "{ description = \"Lock\" })\n",
            "hl.bind(\"SUPER + R\", hl.dsp.global(\"quickshell:riceswap-toggle\"), ",
            "{ description = \"Toggle RiceSwap\" })\n",
        ),
    )
    .expect("write keybinds.lua");
}

/// A rice whose shell the static parse cannot read, outside `$HOME`.
fn unknown_rice(sandbox: &Sandbox, shell: &str) -> PathBuf {
    let tree = sandbox.outside_home(&format!("rice-{shell}"));
    unreadable_shell(&tree, shell, shell);
    tree
}

fn install(sandbox: &Sandbox, tree: &Path) -> common::Run {
    sandbox.run(&["install", &tree.display().to_string()])
}

/// `<data-dir>/recipes/<shell>.toml`, the user tier's file.
fn recipe_file(sandbox: &Sandbox, shell: &str) -> PathBuf {
    sandbox
        .data_dir()
        .join("recipes")
        .join(format!("{shell}.toml"))
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

/// The research half of the install envelope.
fn research_of(data: &Value) -> &Value {
    &data["research"]
}

// ------------------------------------------------------------- the happy path

/// The whole tier, end to end: an unknown shell, a `pi` that answers, a recipe
/// on disk in the schema the adapt pass reads, the packages the answer named
/// merged into the manifest, and the dead names the answer resolved — all in
/// the *same* install, because the write-back has to land before the adapt pass
/// loads the layers.
#[test]
fn an_unknown_shell_is_researched_and_the_answer_becomes_a_user_tier_recipe() {
    let sandbox = Sandbox::new();
    sandbox.pi_stream(&stopped_stream(&answer_document("demo")));
    let tree = unknown_rice(&sandbox, "demo");

    let run = install(&sandbox, &tree);
    let data = run.assert_ok();

    // ---- pi was asked, exactly as the contract says to ask.
    let runs = sandbox.pi_runs();
    assert_eq!(runs.len(), 1, "one run, one answer: {runs:?}");
    let asked = &runs[0];
    for flag in [
        "--offline",
        "-p",
        "--mode json",
        "--no-session",
        "--no-context-files",
        "--no-approve",
        "--tools read,grep,find,ls",
        "--thinking off",
    ] {
        assert!(asked.contains(flag), "`{flag}` is missing from: {asked}");
    }
    assert!(
        asked.contains("--model "),
        "the model is always explicit: {asked}"
    );
    assert!(
        asked.starts_with(&format!("cwd={}", tree.display())),
        "the run happens in the acquired tree: {asked}"
    );

    // ---- the recipe is on disk, in the schema the adapt pass parses.
    let recipe_path = recipe_file(&sandbox, "demo");
    let written = read(&recipe_path);
    let document: toml::Value = toml::from_str(&written)
        .unwrap_or_else(|error| panic!("the written recipe is TOML: {error}\n{written}"));
    assert_eq!(document["schema_version"].as_integer(), Some(1));
    assert_eq!(document["shell"]["name"].as_str(), Some("demo"));
    assert_eq!(document["shell"]["appid"].as_str(), Some("demo"));
    assert_eq!(
        document["env"]["DEMO_WALLPAPERS"].as_str(),
        Some("~/Pictures/Wallpapers")
    );
    assert_eq!(
        document["dirs"][0]["path"].as_str(),
        Some("~/Pictures/Wallpapers")
    );
    let resolutions: Vec<&str> = document["resolution"]
        .as_array()
        .expect("resolutions are an array of tables")
        .iter()
        .map(|entry| entry["dispatched"].as_str().expect("a dispatched name"))
        .collect();
    assert_eq!(
        resolutions,
        vec![
            "quickshell:regionScreenshot",
            "quickshell:overviewClipboardToggle",
            "quickshell:barToggle"
        ],
        "the answer's codebook, in the schema's own table name"
    );

    // The round trip itself is the backend's own: it parses the rendered
    // document with the very `Recipe` the adapt pass will use before the bytes
    // land, reads them back afterwards, and removes the file if either read
    // fails. What is proved *here* is that the file survived the whole install
    // untouched — the adapt pass, the switch and the manifest write all ran
    // after it, and none of them rewrote a codebook it had already read.
    assert_eq!(
        read(&recipe_path),
        written,
        "the recipe is not touched again after it is written"
    );

    // ---- the user tier was read by the adapt pass, not just written. The
    // `drop = true` entries removed their statements: nothing is dead any more.
    let reconcile = &data["reconcile"];
    assert_eq!(
        reconcile["dead_names"],
        json!([]),
        "every dead name the answer spoke for is resolved, so none is left dead"
    );
    assert_eq!(
        reconcile["layers"],
        json!(["user"]),
        "the recipe was read as the user tier"
    );
    let applied: Vec<(&str, &str)> = reconcile["resolutions"]
        .as_array()
        .expect("resolutions are reported")
        .iter()
        .map(|entry| {
            (
                entry["dispatched"].as_str().expect("a dispatched name"),
                entry["kind"].as_str().expect("a kind"),
            )
        })
        .collect();
    assert_eq!(
        applied,
        vec![
            ("quickshell:barToggle", "drop"),
            ("quickshell:overviewClipboardToggle", "drop"),
            ("quickshell:regionScreenshot", "to"),
        ],
        "all three landed, in name order, each with the kind the answer declared"
    );
    assert_eq!(
        reconcile["resolutions"][0]["layer"],
        json!("user"),
        "and the tier that decided them was the user tier"
    );
    assert_eq!(
        reconcile["resolutions"][2]["to"],
        json!("demo:screenshotClip"),
        "the `to` target is namespace-qualified by the derived registry, which is what proved it"
    );
    let keybinds = read(
        &sandbox
            .profile_dir("demo")
            .join(".config/hypr/hyprland/keybinds.lua"),
    );
    assert!(
        !keybinds.contains("overviewClipboardToggle") && !keybinds.contains("barToggle"),
        "the dropped statements are gone: {keybinds}"
    );

    // ---- the packages the answer named reached the manifest, and so the
    // switch that followed.
    let manifest: toml::Value =
        toml::from_str(&sandbox.profile_manifest("demo")).expect("the manifest is TOML");
    let official: Vec<&str> = manifest["packages"]["official"]
        .as_array()
        .expect("official packages")
        .iter()
        .map(|name| name.as_str().expect("a package name"))
        .collect();
    assert!(
        official.contains(&"quickshell") && official.contains(&"matugen"),
        "the answer's packages are in the manifest: {official:?}"
    );
    assert_eq!(data["checked_packages"]["official"], json!(official));

    // ---- the envelope says what happened, in the research object.
    let research = research_of(&data);
    assert_eq!(research["asked"], json!(true));
    assert_eq!(research["invocations"], json!(1));
    assert_eq!(research["repaired"], json!(false));
    assert_eq!(research["model"], json!("anthropic/claude-sonnet-4-5"));
    assert_eq!(research["confidence"], json!(0.72));
    assert_eq!(
        research["unknowns"],
        json!(["where the launcher keeps its own config"]),
        "the answer's gaps are reported, not swallowed"
    );
    assert_eq!(research["recipe"]["written"], json!(true));
    assert_eq!(
        research["recipe"]["path"],
        json!(recipe_path.display().to_string())
    );
    assert_eq!(research["usage"]["cost"]["total"], json!(0.02));
    assert_eq!(
        research["survey"]["dead"].as_array().map(Vec::len),
        Some(3),
        "the brief was built from the dead names the engine found"
    );
    // The survey is what the trigger read, and it says why the tier ran.
    assert!(
        !research["survey"]["uncertain"]
            .as_array()
            .expect("findings")
            .is_empty(),
        "the parse reported uncertainty: {}",
        research["survey"]
    );
    // Silence is not the contract: a research run that produced a file says so
    // in the stream, not only in the envelope.
    let progress: Vec<String> = run
        .progress()
        .iter()
        .map(|line| line["message"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(
        progress
            .iter()
            .any(|message| message.contains("user-tier recipe")),
        "the write is announced while the install still runs: {progress:?}"
    );
}

// ---------------------------------------------------------------- the trigger

/// A shell a built-in recipe covers is answered by that recipe: `pi` is never
/// started, so the whole run is offline and deterministic.
#[test]
fn a_shell_a_built_in_recipe_covers_is_never_researched() {
    let sandbox = Sandbox::new();
    sandbox.pi_stream(&stopped_stream(&answer_document("caelestia")));
    let tree = sandbox.outside_home("rice-caelestia");
    unreadable_shell(&tree, "caelestia", "caelestia");

    let data = install(&sandbox, &tree).assert_ok();

    assert!(
        sandbox.pi_runs().is_empty(),
        "a built-in recipe needs no model: {:?}",
        sandbox.pi_runs()
    );
    let research = research_of(&data);
    assert_eq!(research["asked"], json!(false));
    assert_eq!(research["invocations"], json!(0));
    assert!(
        research["skipped"]
            .as_str()
            .is_some_and(|reason| reason.contains("built-in recipe")),
        "and the envelope says why: {research}"
    );
    assert!(
        !recipe_file(&sandbox, "caelestia").exists(),
        "no user-tier recipe is written for a shell a built-in already covers"
    );
    // The floor still worked: the built-in codebook is the codebook.
    assert_eq!(data["reconcile"]["layers"], json!(["builtin"]));
}

/// The other skip: a shell whose QML the engine can read has a registry, and a
/// brief about a registry the engine already has is a worse question than no
/// question at all.
#[test]
fn a_shell_the_static_parse_already_reads_is_never_researched() {
    let sandbox = Sandbox::new();
    sandbox.pi_stream(&stopped_stream(&answer_document("plain")));
    let tree = sandbox.outside_home("rice-plain");
    readable_shell(&tree, "plain", "plain");

    let data = install(&sandbox, &tree).assert_ok();

    assert!(
        sandbox.pi_runs().is_empty(),
        "nothing to ask about: {:?}",
        sandbox.pi_runs()
    );
    let research = research_of(&data);
    assert_eq!(research["asked"], json!(false));
    assert!(
        research["skipped"]
            .as_str()
            .is_some_and(|reason| reason.contains("low-confidence")),
        "{research}"
    );
    assert_eq!(research["survey"]["registered"], json!(1));
}

// ----------------------------------------------------------------- the floor

/// Exit 0 with a refusal inside the stream is a failure, and it must leave
/// nothing behind. This is the contract's sharpest trap: the process succeeded,
/// the JSONL is well formed, and the answer is a 403.
#[test]
fn an_exit_zero_carrying_a_provider_refusal_is_a_failure_and_writes_nothing() {
    let sandbox = Sandbox::new();
    sandbox.pi_stream(&refused_stream(
        "403: {\"type\":\"FreeTierError\",\"message\":\"OpenCode's free tier can only be used \
         from within OpenCode\"}",
    ));
    let tree = unknown_rice(&sandbox, "demo");

    let run = install(&sandbox, &tree);
    let data = run.assert_ok();

    assert!(
        !recipe_file(&sandbox, "demo").exists(),
        "a refusal is never written, not even as a partial recipe"
    );
    let research = research_of(&data);
    assert_eq!(research["asked"], json!(true));
    assert_eq!(
        research["recipe"]["written"],
        json!(false),
        "there is no recipe: {research}"
    );
    assert_eq!(
        research["recipe"]["path"],
        json!(recipe_file(&sandbox, "demo").display().to_string()),
        "and the file it would have written is named anyway"
    );
    let failure = research["failure"]
        .as_str()
        .unwrap_or_else(|| panic!("the failure is named: {research}"));
    assert!(failure.contains("stopReason `error`"), "{failure}");
    assert!(failure.contains("FreeTierError"), "{failure}");

    // The floor: the install happened anyway, on the engine and the
    // declarations alone.
    assert_eq!(data["profile"], json!("demo"));
    assert_eq!(data["manifest_written"], json!(true));
    assert_eq!(
        data["reconcile"]["dead_names"].as_array().map(Vec::len),
        Some(3),
        "the engine's own proposals are what the user is left with"
    );
    assert!(
        run.warnings()
            .iter()
            .any(|warning| warning.contains("produced no recipe")),
        "the downgrade is said out loud: {:?}",
        run.warnings()
    );
}

/// A malformed answer costs exactly one repair pass — a second invocation, with
/// no tools — and then the floor, after which the chain walks to its second
/// model. The model is pinned to something that is *not* the fallback, so the
/// invocation count pins the whole chain: a run, its repair, and the fallback's
/// own run. Three, and no more, whatever the model does.
#[test]
fn a_malformed_answer_costs_one_repair_pass_and_then_the_floor() {
    let sandbox = Sandbox::new();
    sandbox.set_env("RICESWAP_PI_MODEL", "kios/grok-4.6-free");
    // Prose with no object in it: the shape a repair pass exists for.
    sandbox.pi_stream(&stopped_stream(
        "I looked through the repo and the launcher is written in Python.",
    ));
    let tree = unknown_rice(&sandbox, "demo");

    let data = install(&sandbox, &tree).assert_ok();

    let runs = sandbox.pi_runs();
    assert_eq!(
        runs.len(),
        3,
        "one run, its repair, and the fallback model's own run: {runs:?}"
    );
    assert!(
        runs[0].contains("--tools read,grep,find,ls") && runs[0].contains("--model kios/grok"),
        "the first run is the configured model, with the allowlist: {}",
        runs[0]
    );
    assert!(
        runs[1].contains("--no-tools"),
        "the repair pass is invoked WITHOUT the read/grep/find/ls allowlist: {}",
        runs[1]
    );
    assert!(
        !runs[1].contains("--tools read"),
        "and it replaces the allowlist rather than adding to it: {}",
        runs[1]
    );
    assert!(
        runs[2].contains("--tools read,grep,find,ls")
            && runs[2].contains("--model anthropic/claude-sonnet-4-5"),
        "the chain's second model gets the full allowlist back: {}",
        runs[2]
    );
    assert!(
        !recipe_file(&sandbox, "demo").exists(),
        "a bad parse is never cached, and never written"
    );
    let research = research_of(&data);
    assert_eq!(research["repaired"], json!(true));
    assert_eq!(research["invocations"], json!(3));
    assert!(
        research["failure"]
            .as_str()
            .is_some_and(|reason| reason.contains("no JSON object")),
        "{research}"
    );
}

/// A startup failure is the other arm of the trap: exit 1, no stdout, the
/// model's name on stderr. The retry chain walks to the fallback and then stops.
#[test]
fn a_startup_failure_is_named_by_its_own_stderr_and_never_writes() {
    let sandbox = Sandbox::new();
    sandbox.pi_stream("");
    sandbox.pi_exits(1);
    sandbox.pi_says(
        "Error: Model \"nosuchprovider/nosuchmodel\" not found. Use --list-models to see \
         available models.\n",
    );
    let tree = unknown_rice(&sandbox, "demo");

    let data = install(&sandbox, &tree).assert_ok();

    assert!(!recipe_file(&sandbox, "demo").exists());
    let failure = research_of(&data)["failure"]
        .as_str()
        .expect("the failure is named")
        .to_string();
    assert!(failure.contains("exit 1"), "{failure}");
    assert!(failure.contains("not found"), "{failure}");
    assert_eq!(data["manifest_written"], json!(true));
}

/// No `pi` on the machine at all: the tier is off, said plainly, and the
/// install is untouched by it.
#[test]
fn a_machine_without_a_usable_pi_installs_on_the_engine_alone() {
    let sandbox = Sandbox::new();
    sandbox.script("pi", Mode::Fail);
    let tree = unknown_rice(&sandbox, "demo");

    let run = install(&sandbox, &tree);
    let data = run.assert_ok();

    assert!(
        sandbox.pi_runs().is_empty(),
        "a pi that cannot even be probed is not run: {:?}",
        sandbox.pi_runs()
    );
    assert!(!recipe_file(&sandbox, "demo").exists());
    let research = research_of(&data);
    assert_eq!(research["invocations"], json!(0));
    assert!(
        research["failure"]
            .as_str()
            .is_some_and(|reason| reason.contains("pi is not usable")),
        "{research}"
    );
    assert!(
        run.warnings()
            .iter()
            .any(|warning| warning.contains("research tier is off")),
        "{:?}",
        run.warnings()
    );
    assert_eq!(
        data["reconcile"]["dead_names"].as_array().map(Vec::len),
        Some(3)
    );
}

/// `pi` that reports no credentials is degraded the same way, with the hint the
/// contract's failure matrix asks for.
#[test]
fn pi_reporting_no_credentials_degrades_with_a_hint() {
    let sandbox = Sandbox::new();
    sandbox.pi_auth_says(
        r#"{"status":"not_ready","provider":"openai","reason":"credentials_not_configured"}"#,
    );
    let tree = unknown_rice(&sandbox, "demo");

    let run = install(&sandbox, &tree);
    let data = run.assert_ok();

    assert!(sandbox.pi_runs().is_empty(), "no credentials, no run");
    assert!(!recipe_file(&sandbox, "demo").exists());
    let failure = research_of(&data)["failure"]
        .as_str()
        .expect("named")
        .to_string();
    assert!(failure.contains("openai"), "{failure}");
    assert!(
        failure.contains("pi auth"),
        "the hint is the contract's: {failure}"
    );
    assert!(
        run.warnings()
            .iter()
            .any(|warning| warning.contains("pi auth"))
    );
    assert_eq!(data["manifest_written"], json!(true));
}

// ------------------------------------------------------------------ the clock

/// A hung run is killed and the install continues. The budgets are shrunk to a
/// second so the test is about the mechanism, not about waiting: the run
/// produces no first byte at all, which is the contract's F5 and the two free
/// providers it measured producing nothing for ninety seconds.
#[test]
fn a_hung_run_is_terminated_and_the_install_continues() {
    let sandbox = Sandbox::new();
    sandbox.pi_stream(&stopped_stream(&answer_document("demo")));
    sandbox.delay_on("pi", "--offline", 30);
    sandbox.research_budget(1, 1);
    sandbox.set_env("RICESWAP_PI_STALL_SECONDS", "1");
    let tree = unknown_rice(&sandbox, "demo");

    let run = install(&sandbox, &tree);
    let data = run.assert_ok();

    assert!(
        !recipe_file(&sandbox, "demo").exists(),
        "a killed run writes nothing"
    );
    let failure = research_of(&data)["failure"]
        .as_str()
        .expect("named")
        .to_string();
    assert!(failure.contains("terminated"), "{failure}");
    assert!(
        failure.contains('1'),
        "the budget that ran out is named: {failure}"
    );
    assert_eq!(
        data["reconcile"]["dead_names"].as_array().map(Vec::len),
        Some(3),
        "the floor still installed the rice"
    );
    assert!(
        run.warnings()
            .iter()
            .any(|warning| warning.contains("terminated"))
    );
}

/// A run that starts and then stalls mid-stream is the other arm: it has
/// records, and is killed all the same.
#[test]
fn a_run_that_stalls_mid_stream_is_terminated() {
    let sandbox = Sandbox::new();
    // A header and a start, then nothing: the stream opened and went quiet. The
    // preamble is what gets written before the scripted sleep, so the first-byte
    // budget is satisfied and it is the *stall* budget that has to catch it.
    let partial = format!(
        "{}\n{}\n",
        json!({ "type": "session", "version": 3, "id": "3f0b", "cwd": "/tmp" }),
        json!({ "type": "agent_start" }),
    );
    sandbox.pi_preamble(&partial);
    sandbox.pi_stream(&stopped_stream(&answer_document("demo")));
    sandbox.delay_on("pi", "--offline", 30);
    sandbox.research_budget(5, 5);
    sandbox.set_env("RICESWAP_PI_STALL_SECONDS", "1");
    let tree = unknown_rice(&sandbox, "demo");

    let data = install(&sandbox, &tree).assert_ok();

    assert!(!recipe_file(&sandbox, "demo").exists());
    let research = research_of(&data);
    assert!(
        research["failure"]
            .as_str()
            .is_some_and(|reason| reason.contains("stalled")),
        "{research}"
    );
    assert_eq!(research["partial_records"], json!(2));
    assert_eq!(data["manifest_written"], json!(true));
}

/// `agent_settled` is the contract's end of automatic work, and a run that
/// settles and then lingers is finished. The stub writes its whole stream —
/// answer included — and then sleeps for longer than the whole budget; the
/// install must not wait for that, because there is nothing left to read.
#[test]
fn a_run_that_settles_is_finished_on_its_settle_record_not_on_its_exit() {
    let sandbox = Sandbox::new();
    sandbox.pi_stream(&stopped_stream(&answer_document("demo")));
    // Thirty seconds of silence after a complete stream, against budgets that
    // blow in one. Two regressions are on the table: waiting for the process
    // instead of the record (thirty seconds), and blowing the stall budget
    // instead of reading the record (a floor case with no recipe). The
    // assertions below catch both.
    sandbox.delay_after("--offline", 30);
    sandbox.research_budget(1, 1);
    sandbox.set_env("RICESWAP_PI_STALL_SECONDS", "1");
    let tree = unknown_rice(&sandbox, "demo");

    let started = std::time::Instant::now();
    let data = install(&sandbox, &tree).assert_ok();
    let elapsed = started.elapsed();

    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "the run finished on the settle record, not on the process ({elapsed:?})"
    );
    assert!(
        recipe_file(&sandbox, "demo").exists(),
        "a settled run is a complete run: its answer is a recipe"
    );
    let research = research_of(&data);
    assert_eq!(research["asked"], json!(true));
    assert_eq!(research["invocations"], json!(1));
    assert_eq!(research["confidence"], json!(0.72));
    assert!(
        research.get("failure").is_none(),
        "no budget ran out: {research}"
    );
}

// ------------------------------------------------------------- never clobbered

/// A shell that already has a user-tier recipe is not researched again: the
/// file is this tier's own answer for that shell, so the stub is given a
/// perfectly good answer that must go unasked for, and the file the user
/// hand-edited must come out byte for byte as it went in.
#[test]
fn a_shell_that_already_has_a_user_recipe_is_never_researched_again() {
    let sandbox = Sandbox::new();
    let recipe_path = recipe_file(&sandbox, "demo");
    fs::create_dir_all(recipe_path.parent().expect("a parent")).expect("make the recipes dir");
    let handwritten = "schema_version = 1\n\n[shell]\nname = \"demo\"\nappid = \"handwritten\"\n";
    fs::write(&recipe_path, handwritten).expect("write the hand-edited recipe");

    // A good answer, which must go unused: the point is that nothing is asked.
    sandbox.pi_stream(&stopped_stream(&answer_document("from-the-model")));
    let tree = unknown_rice(&sandbox, "demo");

    let data = install(&sandbox, &tree).assert_ok();

    assert!(
        sandbox.pi_runs().is_empty(),
        "a shell this tier already answered for is never asked about: {:?}",
        sandbox.pi_runs()
    );
    assert_eq!(
        read(&recipe_path),
        handwritten,
        "the file a user wrote is byte for byte what it was"
    );
    let research = research_of(&data);
    assert_eq!(research["asked"], json!(false));
    assert_eq!(research["invocations"], json!(0));
    assert!(
        research["skipped"]
            .as_str()
            .is_some_and(|reason| reason.contains("user-tier recipe")),
        "and the envelope says why: {research}"
    );
    assert_eq!(
        research["recipe"]["written"],
        json!(false),
        "no write-back was attempted: {research}"
    );
    // The adapt pass still read the file that was there, and the install went
    // on to use it.
    assert_eq!(data["reconcile"]["layers"], json!(["user"]));
}

/// The refresh path, which is deleting the file: with the recipe gone the shell
/// is unknown again, and the tier asks. Nothing else re-opens the question —
/// a re-run, a new install, a switch — because none of them is a research
/// trigger once a recipe exists.
#[test]
fn deleting_the_user_recipe_is_what_makes_a_shell_researched_again() {
    let sandbox = Sandbox::new();
    sandbox.pi_stream(&stopped_stream(&answer_document("demo")));

    let first = unknown_rice(&sandbox, "demo");
    let data = install(&sandbox, &first).assert_ok();
    assert_eq!(research_of(&data)["asked"], json!(true));
    assert!(recipe_file(&sandbox, "demo").exists());
    assert_eq!(sandbox.pi_runs().len(), 1);

    // A different shell is unknown, and is asked about, whatever is on disk for
    // the first one.
    let other = unknown_rice(&sandbox, "other");
    install(&sandbox, &other).assert_ok();
    assert!(recipe_file(&sandbox, "other").exists());
    assert_eq!(sandbox.pi_runs().len(), 2, "one run per unknown shell");

    // Delete the explicit refresh, and the same shell is asked about again. The
    // profile goes first, because an install refuses to overwrite one that
    // already exists (#37) — the research trigger is downstream of that refusal,
    // so a re-install has to be a real re-install to reach it.
    sandbox.run(&["delete", "demo", "--force"]).assert_ok();
    fs::remove_file(recipe_file(&sandbox, "demo")).expect("delete the recipe");
    let again = install(&sandbox, &first).assert_ok();
    assert_eq!(research_of(&again)["asked"], json!(true));
    assert!(recipe_file(&sandbox, "demo").exists());
    assert_eq!(sandbox.pi_runs().len(), 3);
}

/// The tier writes the user tier and only the user tier. `adapt.toml` is the
/// human tier, and no research run ever creates one — not even when the answer
/// carries a `[foreign]` entry and env values that would sit perfectly well in
/// it.
#[test]
fn a_research_run_writes_the_user_tier_and_never_an_adapt_toml() {
    let sandbox = Sandbox::new();
    sandbox.pi_stream(&stopped_stream(&answer_document("demo")));
    let tree = unknown_rice(&sandbox, "demo");

    install(&sandbox, &tree).assert_ok();

    let profile = sandbox.profile_dir("demo");
    assert!(
        recipe_file(&sandbox, "demo").exists(),
        "the user tier is where an answer goes"
    );
    assert!(
        !profile.join("adapt.toml").exists(),
        "the human tier is the user's file, and nothing here writes it"
    );
    // Nor anywhere else: the whole fake `$HOME` is searched for the file.
    let elsewhere: Vec<String> = sandbox
        .home_tree()
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| name.ends_with("adapt.toml"))
        .collect();
    assert!(elsewhere.is_empty(), "{elsewhere:?}");
}

/// A recipe for one shell says nothing about another. The skip is per shell,
/// not per data directory, so a second rice in the same store is still unknown.
#[test]
fn each_shell_is_researched_on_its_own_account() {
    let sandbox = Sandbox::new();
    sandbox.pi_stream(&stopped_stream(&answer_document("demo")));
    let tree = unknown_rice(&sandbox, "demo");
    install(&sandbox, &tree).assert_ok();

    let other = unknown_rice(&sandbox, "other");
    let data = install(&sandbox, &other).assert_ok();

    assert_eq!(data["profile"], json!("other"));
    assert_eq!(research_of(&data)["asked"], json!(true));
    assert!(recipe_file(&sandbox, "other").exists());
    assert_eq!(sandbox.pi_runs().len(), 2, "one run per shell");
}
