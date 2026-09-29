//! Tier C verification, and the floor under it (issue #39).
//!
//! A switch that finishes is not a switch that worked. Ten steps can complete
//! and leave a desktop with a bar that is not there, keybinds that resolve to
//! nothing, and a shell that died inside its own start window — every one of
//! them reported as `ok: true`, because the switch only knows whether it *ran*
//! its sequence. This module is the part that looks afterwards and says whether
//! the desktop is alive and wired.
//!
//! #30 froze the shape and the vocabulary, and this file implements exactly it.
//!
//! **Tier C is the always-available core**, and it is deliberately three checks
//! and no more, because every one of them has to work headless, on a machine
//! with no compositor, inside a sandbox with stubbed tools:
//!
//! 1. `shell-alive` — the target's shell survived the grace window its start
//!    was given;
//! 2. `invariant-live` — the engine's post-rewrite invariant (#27, #29)
//!    re-proved against the **live** `hyprctl globalshortcuts` registry rather
//!    than the one derived from the profile's QML, which also makes it the
//!    drift detector for a recipe cached at adapt time;
//! 3. `ipc-liveness` — the recipe/adapt-declared IPC probe, when one is
//!    declared ([`crate::recipe::Ipc`]).
//!
//! Tier F — the functional half, the one that drives pixels and synthetic input
//! through `grim` and `ydotool` — is #40's. No verdict name is reserved or
//! invented for it here: the vocabulary this ticket can produce is closed at
//! `verified-core` and `fail`.
//!
//! ## The degrade rule: a check that cannot run is not a check that failed
//!
//! `hyprctl` absent, a compositor that is not answering, a sandbox whose stub
//! prints a version banner and nothing else — on such a machine the live
//! registry is *unknown*, and reporting every dispatched name as dead would be a
//! lie that also rolls the user's desktop back for nothing. So an unavailable
//! probe is recorded as a skipped check: `ok: true`, with the reason in the
//! evidence, and the verdict stays the honest degraded pass, `verified-core`.
//!
//! The frozen key set is `{id, tier, ok, evidence}` per check, so "skipped" is
//! said in the evidence text rather than by adding a key the contract does not
//! have: no `skipped` boolean, no fourth verdict name, nothing for a consumer to
//! parse that #31's assertions were not written against.
//!
//! What is *not* degraded away: a check that ran and found a dead bind. That is
//! a fact about the running desktop, and it fails.
//!
//! ## What a failure costs, and what it does not
//!
//! A failed verdict hands the switch back a [`Report`] carrying the frozen
//! reason code and the frozen facts (`{shell, registry_n, dead_names[],
//! proposals[]}`); the switch then puts the old profile back. Packages the
//! failed switch installed or removed are **enumerated in the envelope's report
//! and not reverted**: a package transaction has its own confirmation, its own
//! privilege path and its own half-finished states, and silently running the
//! opposite transaction to undo one is the kind of magic this tool does not do.
//! The envelope says so in `next` and in the check evidence, so the boundary is
//! stated rather than assumed.

use crate::operations::START_GRACE;
use crate::profile::Shell;
use crate::recipe;
use crate::reconcile;
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Every executed check passed. The functional tier (#40) would say
/// `verified-full`; this ticket can only ever say this.
pub const VERIFIED_CORE: &str = "verified-core";

/// A check ran and found the desktop not alive and wired, so the switch puts the
/// old profile back.
pub const FAIL: &str = "fail";

/// The target's shell did not survive its start. Frozen by #30.
pub const SHELL_NOT_ALIVE: &str = "shell-not-alive";

/// A dispatched name resolves against nothing live. Frozen by #30.
pub const INVARIANT_DEAD_NAMES: &str = "invariant-dead-names";

/// A recipe's declared namespace disagrees with the live registry's. Frozen by
/// #30.
pub const RECIPE_DRIFT: &str = "recipe-drift";

/// The prefix of the code a declared probe contributes when it does not answer.
pub const PROBE_FAILED: &str = "probe-failed";

/// One check, in the shape the envelope's `checks` array freezes.
#[derive(Clone, Debug, Serialize)]
pub struct Check {
    /// What was checked: `shell-alive`, `invariant-live` or `ipc-liveness`.
    pub id: &'static str,
    /// The tier it belongs to. Only `C` exists in this build.
    pub tier: &'static str,
    /// Whether the check passed. A skipped check is `true`, with the reason in
    /// the evidence — see the module docs for why an unavailable probe is not a
    /// failure.
    pub ok: bool,
    /// What the check actually saw: the names it resolved against, the count of
    /// live registrations, the stderr a probe answered with, or why it could
    /// not run at all.
    pub evidence: String,
}

impl Check {
    fn new(id: &'static str, ok: bool, evidence: impl Into<String>) -> Check {
        Check {
            id,
            tier: "C",
            ok,
            evidence: evidence.into(),
        }
    }
}

/// The frozen diagnosis payload of #29, carried in `data.facts` when the verdict
/// is `fail`.
#[derive(Debug, Default, Serialize)]
pub struct Facts {
    /// The shell the checks were about.
    pub shell: Option<String>,
    /// How many `appid:name` registrations the live registry held. Zero when the
    /// probe could not be run, which is itself worth knowing: it is the
    /// difference between "nothing is registered" and "we could not look".
    pub registry_n: usize,
    /// The dispatched names that resolved against nothing.
    pub dead_names: Vec<String>,
    /// The engine's proposal vocabulary: every dead name with the move that was
    /// applied for it, if any, and the lines that dispatch it. This is the next
    /// research brief, and it rides verbatim rather than summarized.
    pub proposals: Vec<reconcile::Proposal>,
}

/// What one pass concluded, in the shape `data.report.verification` freezes: the
/// verdict and the checks. The reason code and the facts are [`serde::skip`]ped
/// here and emitted beside it on the failure path, because the pass envelope is
/// the two-key object #30 pinned and a pass must not carry a failure's payload.
#[derive(Debug, Serialize)]
pub struct Report {
    pub verdict: &'static str,
    pub checks: Vec<Check>,
    #[serde(skip)]
    pub reason_code: Option<String>,
    #[serde(skip)]
    pub facts: Facts,
}

impl Report {
    /// Whether this pass rejected the desktop the switch just activated.
    pub fn failed(&self) -> bool {
        self.verdict == FAIL
    }
}

/// Everything the pass needs about the switch that just ran, and nothing it can
/// read off the filesystem itself.
pub struct Subject<'a> {
    /// The `$HOME` the switch ran against.
    pub home: &'a Path,
    /// RiceSwap's own data directory, where the user tier's recipes live.
    pub data: &'a Path,
    /// The profile the switch activated.
    pub profile: &'a Path,
    /// The manifest's `[shell]`, when the profile names one.
    pub shell: Option<&'a Shell>,
    /// Whether the switch *tried* to start a shell. A profile whose shell did
    /// not change is not asked about its shell: nothing was started, so nothing
    /// can have failed to start.
    pub shell_attempted: bool,
    /// The shell the switch reported as started, when one survived.
    pub shell_started: Option<&'a str>,
    /// The engine's own pass, whose uncertainty bounds what any registry can be
    /// asked to prove.
    pub reconcile: &'a reconcile::Report,
}

/// How long the live probe and a declared IPC probe may take before they are
/// given up on. A verification step that waits forever is a switch that never
/// finishes, on a desktop it has already changed.
const PROBE_BUDGET: Duration = Duration::from_millis(2500);

/// Runs Tier C over the desktop the switch just activated.
///
/// Read-only by construction: it reads the live registry, runs at most one
/// declared probe, and never writes into any profile. Everything a failure needs
/// in order to undo something lives in the switch that calls it.
pub fn run(subject: &Subject) -> Report {
    let context = reconcile::Context::new(subject.home, subject.data);
    let mut outcomes = vec![shell_alive(subject)];
    let mut facts = Facts {
        shell: subject
            .shell
            .map(|shell| shell.name.clone())
            .or_else(|| subject.reconcile.shell.clone()),
        // The engine's own dead set is the honest account of what the profile
        // dispatches and nothing answers to, whether or not a compositor was
        // there to confirm it. A live check that ran overwrites it with what the
        // session itself says.
        dead_names: subject.reconcile.dead_names.clone(),
        proposals: subject.reconcile.proposals.clone(),
        ..Facts::default()
    };

    // The dispatched side of the invariant is read first: a profile that
    // dispatches nothing has nothing to prove, and one with no shell has no
    // registry to prove it against. Both skip without running a command at all,
    // so an ordinary profile pays nothing for the tier.
    let dispatched = reconcile::dispatched(
        subject.profile,
        &context,
        subject.shell.map(|shell| shell.name.as_str()),
        &BTreeSet::new(),
    );
    outcomes.push(invariant_live(&dispatched, subject.reconcile, &mut facts));

    // The declared probe, when a layer declares one. A shell with no probe
    // contributes no check at all, which is how a consumer tells "not run" from
    // "passed": the id is simply absent from `checks`.
    if let Some(probe) = declared_probe(subject, &context) {
        outcomes.push(ipc_liveness(&probe));
    }

    // The first check to fail names the failure, in the order the checks run —
    // a shell that is not up explains more than a registry read that followed a
    // desktop which was never there.
    let reason_code = outcomes.iter().find_map(|outcome| outcome.reason.clone());
    Report {
        verdict: if reason_code.is_some() {
            FAIL
        } else {
            VERIFIED_CORE
        },
        checks: outcomes.into_iter().map(|outcome| outcome.check).collect(),
        reason_code,
        facts,
    }
}

/// The `[ipc] probe` the winning tier declares, for the shell this switch
/// activated. The precedence is `Layers`' own: the profile's `adapt.toml` first,
/// then the built-in recipe, then the user tier.
fn declared_probe(subject: &Subject, context: &reconcile::Context<'_>) -> Option<String> {
    let shell = subject
        .shell
        .map(|shell| shell.name.as_str())
        .or(subject.reconcile.shell.as_deref())?;
    let (layers, _) = recipe::Layers::load(subject.profile, context.data(), shell);
    layers.ipc_probe().map(|(_, probe)| probe.to_string())
}

/// Check 1: the target's shell survived the grace window its start was given.
///
/// The switch already made this decision — [`crate::operations::start_daemonized`]
/// reads a command still running at the deadline as a daemon coming up — and
/// reported a shell that would not start as a *warning*, because at that point
/// the packages, the links and the config were all correct and only the shell
/// was missing. That was the right call at step 10, where nothing had been
/// checked and the profile was not yet known to be broken. It is the wrong place
/// to end the story: a switch that activated a profile and left no desktop
/// running has not verified.
///
/// So this reads the switch's own answer rather than re-deriving survival, and a
/// start that was *attempted* and did not survive is a failure. A switch that
/// was never asked to start a shell — the target's shell is the one already
/// running, or the profile names none — has nothing to prove and passes.
fn shell_alive(subject: &Subject) -> Outcome {
    if !subject.shell_attempted {
        return Outcome::passed(Check::new(
            "shell-alive",
            true,
            match subject.shell {
                Some(shell) => format!(
                    "this switch started no shell, so it has none to prove: `{}` is the shell \
                     that was already running",
                    shell.name
                ),
                None => {
                    "this profile names no shell, so none was started and none is owed".to_string()
                }
            },
        ));
    }
    match subject.shell_started {
        Some(name) => Outcome::passed(Check::new(
            "shell-alive",
            true,
            format!(
                "`{name}` survived its {}ms start window and is still running",
                START_GRACE.as_millis()
            ),
        )),
        None => Outcome::failed(
            Check::new(
                "shell-alive",
                false,
                "the target's shell did not survive its start: it exited, or could not be run at \
                 all, and the profile this switch activated has no desktop behind it"
                    .to_string(),
            ),
            SHELL_NOT_ALIVE,
        ),
    }
}

/// One check, and the reason code a failure of it contributes — `None` for a
/// pass and for a skip, because a check that did not run has not failed.
struct Outcome {
    check: Check,
    reason: Option<String>,
}

impl Outcome {
    fn passed(check: Check) -> Outcome {
        Outcome {
            check,
            reason: None,
        }
    }

    fn failed(check: Check, reason: impl Into<String>) -> Outcome {
        Outcome {
            check,
            reason: Some(reason.into()),
        }
    }
}

/// Check 2: every dispatched global name in the switched profile's configs
/// resolves against the **live** registry, the foreign namespaces, or the keeps.
///
/// This is the engine's own post-rewrite invariant with the other half of the
/// equation supplied by the running session instead of by a static parse. It is
/// the gate that needs no `grim` and no `ydotool`, and it is the same condition
/// that doubles as the cached-recipe drift detector: a recipe derived at adapt
/// time that no longer matches what the shell registers live is caught here,
/// rather than three switches later.
fn invariant_live(
    dispatched: &reconcile::Dispatched,
    engine: &reconcile::Report,
    facts: &mut Facts,
) -> Outcome {
    if let Some(why) = &dispatched.skipped {
        return Outcome::passed(Check::new(
            "invariant-live",
            true,
            format!("skipped: {why}"),
        ));
    }
    if dispatched.names.is_empty() {
        return Outcome::passed(Check::new(
            "invariant-live",
            true,
            format!(
                "skipped: this profile's configs dispatch no global names, so the invariant holds \
                 vacuously ({} config file(s) read)",
                dispatched.scanned_files
            ),
        ));
    }
    if let Some(why) = engine.uncertain.first() {
        // The engine's own parse found registrations it could not resolve — a
        // name built at runtime, an ambiguous appid. Those exist live and are
        // invisible to both parsers, so a "dead" verdict here would be a guess
        // about a registry nobody can read. Skipped, loudly, with the finding
        // named.
        return Outcome::passed(Check::new(
            "invariant-live",
            true,
            format!(
                "skipped: this profile's registry could not be derived in full ({} finding(s), \
                 first: {why}), so a live miss cannot be told apart from a name the parse cannot \
                 see",
                engine.uncertain.len()
            ),
        ));
    }

    let registry = match read_live_registry() {
        Ok(registry) => registry,
        Err(why) => {
            return Outcome::passed(Check::new(
                "invariant-live",
                true,
                format!("skipped: the live registry is unavailable here — {why}"),
            ));
        }
    };
    facts.registry_n = registry.len();
    let keep: BTreeSet<&str> = dispatched.keep.iter().map(String::as_str).collect();
    let dead: Vec<String> = dispatched
        .names
        .iter()
        .filter(|entry| !registry.contains(entry.as_str()) && !keep.contains(entry.as_str()))
        .cloned()
        .collect();

    if dead.is_empty() {
        return Outcome::passed(Check::new(
            "invariant-live",
            true,
            format!(
                "every dispatched name resolves against the live session: {} dispatched, {} \
                 registered live, {} kept",
                dispatched.names.len(),
                registry.len(),
                keep.len()
            ),
        ));
    }

    facts.dead_names = dead.clone();
    let drift = drift_against(&dead, &registry, dispatched.pinned_appid.as_deref());
    let reason = if drift.is_some() {
        RECIPE_DRIFT
    } else {
        INVARIANT_DEAD_NAMES
    };
    Outcome::failed(
        Check::new(
            "invariant-live",
            false,
            match &drift {
                Some(why) => format!("{} — {why}", dead.join(", ")),
                None => format!(
                    "dispatched but registered by nothing: {} ({} registration(s) live, {} kept)",
                    dead.join(", "),
                    registry.len(),
                    keep.len()
                ),
            },
        ),
        reason,
    )
}

/// The explanation when the misses are not dead binds but a recipe that is out of
/// date, and `None` when they are ordinary dead binds.
///
/// The distinction is worth a reason code of its own because the two need
/// different repairs: a dead bind is a name the shell never had, and only a
/// codebook can say what the key meant; a namespace that moved wholesale is a
/// recipe written against a version of the shell that is no longer running, and
/// the fix is one line in the recipe. A miss counts as drift only when the recipe
/// pinned a namespace *and* every missed name is registered live under a
/// different one — one coincidental same-name match does not make a whole set
/// drift, and a half-drifted set is a set with a genuinely dead bind in it.
fn drift_against(
    dead: &[String],
    registry: &BTreeSet<String>,
    pinned: Option<&str>,
) -> Option<String> {
    let pinned = pinned?;
    let elsewhere: Vec<&str> = registry
        .iter()
        .map(String::as_str)
        .filter(|entry| !entry.starts_with(&format!("{pinned}:")))
        .collect();
    let drifted: Vec<&str> = dead
        .iter()
        .filter(|entry| {
            let half = recipe::name_of(entry);
            elsewhere
                .iter()
                .any(|candidate| recipe::name_of(candidate) == half)
        })
        .map(String::as_str)
        .collect();
    if drifted.len() != dead.len() {
        return None;
    }
    let namespaces: BTreeSet<&str> = elsewhere
        .iter()
        .filter_map(|entry| entry.split_once(':').map(|(appid, _)| appid))
        .collect();
    Some(format!(
        "the recipe pins this shell's namespace to `{pinned}`, but the live session registers \
         {} under {} instead",
        drifted.join(", "),
        namespaces.into_iter().collect::<Vec<_>>().join(", ")
    ))
}

/// Check 3: the IPC probe a recipe tier declared, run and believed.
///
/// A shell with no declared probe contributes no check at all. A probe that *is*
/// declared and does not answer is a failure: the declaration is a claim about
/// the shell made by whoever wrote the recipe, and the running desktop is the
/// only thing that can settle it.
fn ipc_liveness(probe: &str) -> Outcome {
    match run_query(probe) {
        Ok(answer) if answer.succeeded => Outcome::passed(Check::new(
            "ipc-liveness",
            true,
            format!("the recipe's IPC probe `{probe}` answered"),
        )),
        Ok(answer) => Outcome::failed(
            Check::new(
                "ipc-liveness",
                false,
                format!(
                    "the recipe's IPC probe `{probe}` did not answer: it exited non-zero{}",
                    first_line(&answer.stderr)
                ),
            ),
            format!("{PROBE_FAILED}:ipc-liveness"),
        ),
        Err(why) => Outcome::failed(
            Check::new(
                "ipc-liveness",
                false,
                format!("the recipe's IPC probe `{probe}` could not be run: {why}"),
            ),
            format!("{PROBE_FAILED}:ipc-liveness"),
        ),
    }
}

/// What a finished command left: how it ended, and whatever it said.
struct Answer {
    succeeded: bool,
    stderr: String,
}

/// Runs one command and reads how it ended, in bounded time.
///
/// The bound is the point: `hyprctl` talks to a compositor that can be wedged,
/// and a verification step that waits forever on a wedged compositor is a switch
/// that never returns on a desktop it has already changed. A command still
/// running at the deadline is killed and reported as no answer at all, which is
/// the same thing a dead one is.
fn run_query(command: &str) -> Result<Answer, String> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot run `{command}`: {error}"))?;
    let deadline = Instant::now() + PROBE_BUDGET;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(error) => return Err(format!("cannot watch `{command}`: {error}")),
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("`{command}` did not answer in time"));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        use std::io::Read;
        let _ = pipe.read_to_string(&mut stderr);
    }
    Ok(Answer {
        succeeded: status.is_some_and(|status| status.success()),
        stderr,
    })
}

/// The live registry, as the `appid:name` entries a `hyprctl globalshortcuts`
/// answer carries, or why there is none to be had.
fn read_live_registry() -> Result<BTreeSet<String>, String> {
    let output = Command::new("hyprctl")
        .arg("globalshortcuts")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| {
            format!(
                "`hyprctl globalshortcuts` could not be run ({error}); there is no compositor here \
                 to ask"
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "`hyprctl globalshortcuts` exited non-zero{}",
            first_line(&String::from_utf8_lossy(&output.stderr))
        ));
    }
    let answer = String::from_utf8_lossy(&output.stdout).into_owned();
    let registry = parse_globalshortcuts(&answer);
    if registry.is_empty() {
        return Err(format!(
            "`hyprctl globalshortcuts` answered with no `appid:name` registration in it{}",
            first_line(&answer)
        ));
    }
    Ok(registry)
}

/// The `appid:name` entries in a `hyprctl globalshortcuts` answer.
///
/// Only `bind` lines are read, which is the shape the real command prints, and
/// the `appid:name` token is taken wherever it appears in one — as the
/// dispatcher argument (`bind = SUPER, K, global, quickshell:lock`) or wrapped in
/// a dispatch call (`bind = , , exec, hl.dsp.global("quickshell:lock")`). A
/// `bind =` line the parse cannot read contributes nothing, which is what turns
/// an unreadable answer into a skip rather than a wall of false deaths.
fn parse_globalshortcuts(answer: &str) -> BTreeSet<String> {
    let mut registry = BTreeSet::new();
    for line in answer.lines() {
        if !line.trim_start().starts_with("bind") {
            continue;
        }
        for field in line.split([',', '"', '\'', '(', ')', '=', ';', ' ']) {
            let field = field.trim();
            if is_entry(field) {
                registry.insert(field.to_string());
            }
        }
    }
    registry
}

/// Whether `token` is exactly one `appid:name`: one colon, two non-empty halves,
/// and only the characters a Hyprland appid and a Quickshell shortcut name are
/// made of. Anything else — a description, a path, a flag — is not a
/// registration.
fn is_entry(token: &str) -> bool {
    let mut halves = token.split(':');
    let (Some(appid), Some(name), None) = (halves.next(), halves.next(), halves.next()) else {
        return false;
    };
    !appid.is_empty()
        && !name.is_empty()
        && appid
            .chars()
            .chain(name.chars())
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// The first non-empty line of an answer, trimmed and capped, as a phrase.
fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| format!(" (said: {})", line.chars().take(120).collect::<String>()))
        .unwrap_or_default()
}
