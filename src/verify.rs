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
//! Tier F is #40's, and this file implements exactly it: the desktop gets
//! *driven*. A recipe declares probe scripts ([`crate::recipe::Probe`]) — tap
//! Super, the launcher opens; type `>wal`, the action exists; press Enter, the
//! wallpaper applies — and the tier runs them through `ydotool` and `grim` and
//! reports what it saw. Every probe contributes a row to the same frozen `checks`
//! array under `tier: "F"`, and all of them answering turns the degraded pass
//! into `verified-full`.
//!
//! It runs **after** Tier C, and only over a desktop Tier C did not reject:
//! driving a shell that never came up, or typing into a session whose registry
//! has just failed, is both meaningless and destructive.
//!
//! ## The degrade rule, unchanged and one tier wider
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
//! Tier F takes the same rule one tier wider: `grim` or `ydotool` missing or
//! unusable and the whole functional tier **does not run**. The verdict stays
//! `verified-core`, `checks` is exactly what Tier C put there, and the switch's
//! warnings carry the reason. There is deliberately no row for a tier that did
//! not run: a check whose absence has to be inferred from a warning is a check a
//! consumer has to guess about, and the point of the frozen key set is that a
//! row means something was read.
//!
//! What is *not* degraded away: a check that ran and found a dead bind, and a
//! probe that ran and did not answer. Both are facts about the running desktop,
//! and both fail.
//!
//! ## The screenshot primitive is a byte floor, and says so
//!
//! #30 called for a pixel diff and deferred the arithmetic. This build takes the
//! honest floor of it: `grim` is asked for the same region twice — once before
//! the probe's input, once while watching for the outcome — and the two
//! encodings are compared for **inequality**, with a size sanity check so an
//! empty file is an error rather than a change. That is "something on that
//! region is not what it was", which is a weaker claim than "the launcher is
//! visible": a clock ticking in the corner satisfies it. Every piece of evidence
//! this tier writes about a screenshot says which of the two it measured, and
//! real per-pixel comparison — decoding the PNG, thresholding a changed-pixel
//! count — stays the acceptance tier's to earn.
//!
//! ## Lock is invariant-only in production
//!
//! A probe that could plausibly lock the session is **refused before it runs**,
//! and the refusal is itself a row, so the report stays complete: the probe is
//! named, the offending name is named, and the tier carries on with the rest.
//! The test is by *name* — a token in a probe's id, keycodes, text or command
//! that begins with `lock` or `unlock`, the pair the registered-shortcut
//! vocabulary of #24's research names as the lock surface (FC-2, FC-9) — and it
//! is deliberately blunt. A false positive costs one refused probe; a false
//! negative would leave a user's session behind a lock screen with no way in.
//! Nothing is dispatched to find out, so the production check for the lock
//! capability remains exactly what #39 made it: a name in the live registry,
//! read by `hyprctl globalshortcuts`. The functional lock→unlock cycle belongs
//! to the disposable acceptance account (#41).
//!
//! ## Timing is the tier's business, not the declaration's
//!
//! A launcher that takes 400ms to appear is a launcher that is working, so an
//! observation is retried until its expected outcome holds, every 250ms, for at
//! most 10s per probe — and a probe that still has not answered is run once
//! more, because a probe that fails the first time and passes the second is a
//! flaky probe, and a verdict built from one sample of a flaky thing is not a
//! verdict. The tier as a whole has 60s; a probe the budget can no longer cover
//! is reported as not run rather than passed, and the verdict degrades with it.
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
//!
//! A `probe-failed:<id>` is the *same* failure as `invariant-dead-names` in
//! every way that matters: the same rollback, the same payload, the same
//! `phase`. Tier F is more of the same gate, not a second gate with a recovery
//! of its own.

use crate::operations::START_GRACE;
use crate::profile::Shell;
use crate::recipe;
use crate::reconcile;
use crate::tools::Tool;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// Every executed check passed and the functional tier ran all of its probes:
/// the desktop was not merely alive and wired, it answered.
pub const VERIFIED_FULL: &str = "verified-full";

/// Every executed check passed, and the functional tier either did not run
/// (nothing to drive, or no usable `grim`/`ydotool`) or did not run all of it.
/// The honest degraded pass, and the only verdict a headless machine reaches.
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
    /// What was checked: `shell-alive`, `invariant-live` and `ipc-liveness` for
    /// Tier C, and for Tier F the `id` the probe script declared — which is why
    /// this is a `String` and not a `&'static str`: a probe's name comes from
    /// the user's recipe, and it is also the half of `probe-failed:<id>`.
    pub id: String,
    /// The tier it belongs to: `C` or `F`.
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
    /// A Tier C row. Its id is one of this build's own, which is why it can be
    /// spelled here rather than passed in.
    fn core(id: &'static str, ok: bool, evidence: impl Into<String>) -> Check {
        Check {
            id: id.to_string(),
            tier: "C",
            ok,
            evidence: evidence.into(),
        }
    }

    /// A Tier F row, under the id the probe declared.
    fn probe(id: String, ok: bool, evidence: impl Into<String>) -> Check {
        Check {
            id,
            tier: "F",
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
/// The warnings are skipped for the same reason and a different one: a
/// degraded tier is a *fact about this run*, and the envelope already has a
/// place for that — `warnings`, which the switch folds them into.
#[derive(Debug, Serialize)]
pub struct Report {
    pub verdict: &'static str,
    pub checks: Vec<Check>,
    #[serde(skip)]
    pub reason_code: Option<String>,
    #[serde(skip)]
    pub facts: Facts,
    #[serde(skip)]
    pub warnings: Vec<String>,
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

/// How long one observation of a probe may keep looking for its expected
/// outcome, and the cap a declaration may ask for. Frozen by #30, and the
/// reason a launcher that takes 400ms to appear is not a launcher that failed.
const SETTLE: Duration = Duration::from_secs(10);

/// The gap between one look and the next while a probe settles.
const POLL: Duration = Duration::from_millis(250);

/// The whole functional tier's wall clock. A switch that has already changed a
/// desktop may not sit there driving it for ever, and this is the number that
/// says so.
const TIER_BUDGET: Duration = Duration::from_secs(60);

/// The synthetic-input tool the probe primitives drive. It is deliberately not
/// a [`Tool`]: #34 made it a stub-only seam with no version contract to
/// honour, and a roster entry would have `detect` report a verdict about a
/// binary whose usability is exactly the question this tier asks.
const YDOTOOL: &str = "ydotool";

/// The registered shortcut names that are the session's lock surface. #24's
/// research names the pair (`lock` and `unlock`, plus the `lockFocus` shape)
/// as one capability in every shell it read; matching a token that *begins* with
/// either name is blunt on purpose, because the failure modes are not
/// symmetrical — see the module docs.
const LOCK_NAMES: &[&str] = &["lock", "unlock"];

/// Runs the whole pass: Tier C over the desktop, then Tier F over what Tier C
/// did not reject.
///
/// Read-only by construction: it reads the live registry, runs at most one
/// declared IPC probe, and — when the functional tier is available — drives the
/// session with synthetic input and screenshots into a private scratch
/// directory. It never writes into any profile, and the one place it could
/// touch the user's screen (a keystroke) is bounded by the lock refusal above.
/// Everything a failure needs in order to undo something lives in the switch
/// that calls it.
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

    // The recipe tiers are read once and both tiers read from them: the `[ipc]`
    // liveness probe and the `[[probe]]` scripts are declarations by the same
    // documents under the same precedence, and loading them twice would mean two
    // implementations of that precedence to keep in step. `None` when the
    // profile names no shell at all — there is no shell to ask and no desktop
    // to drive.
    let layers = declared_layers(subject, &context);
    if let Some(probe) = layers.as_ref().and_then(recipe::Layers::ipc_probe) {
        outcomes.push(ipc_liveness(probe.1));
    }

    // The first check to fail names the failure, in the order the checks run —
    // a shell that is not up explains more than a registry read that followed a
    // desktop which was never there.
    let mut reason_code = outcomes.iter().find_map(|outcome| outcome.reason.clone());
    let mut checks: Vec<Check> = outcomes.into_iter().map(|outcome| outcome.check).collect();
    let mut warnings = Vec::new();
    let mut verdict = if reason_code.is_some() {
        FAIL
    } else {
        VERIFIED_CORE
    };

    // Tier F, over a desktop Tier C did not reject. Everything the tier decides
    // is decided from here on, and the only thing that can *raise* the verdict
    // is a probe that ran and answered.
    if reason_code.is_none() {
        let plan = plan(layers.as_ref(), subject);
        warnings.extend(plan.warnings);
        if !plan.probes.is_empty() {
            match Desktop::probe() {
                Err(why) => warnings.push(format!(
                    "the functional verification tier (F) did not run: {why}; the verdict stays \
                     `{VERIFIED_CORE}` and no probe is reported, because a probe that could not \
                     run is not a probe that passed"
                )),
                Ok(desktop) => {
                    let tier = drive(&plan.probes, &desktop);
                    warnings.extend(tier.warnings);
                    for outcome in tier.outcomes {
                        // Tier C's failure is the one that names a rollback; a
                        // probe's is folded in behind it, so the reason code is
                        // always the earliest thing that went wrong.
                        if reason_code.is_none() {
                            reason_code = outcome.reason;
                        }
                        checks.push(outcome.check);
                    }
                    verdict = if reason_code.is_some() {
                        FAIL
                    } else if tier.proved && plan.complete {
                        VERIFIED_FULL
                    } else {
                        VERIFIED_CORE
                    };
                }
            }
        }
    }

    Report {
        verdict,
        checks,
        reason_code,
        facts,
        warnings,
    }
}

/// The recipe tiers that speak for the shell this switch activated, read once
/// for both tiers. The precedence is the same one the engine's own pass uses:
/// the profile's `adapt.toml` first, then the built-in recipe, then the user
/// tier.
fn declared_layers(subject: &Subject, context: &reconcile::Context<'_>) -> Option<recipe::Layers> {
    let shell = subject
        .shell
        .map(|shell| shell.name.as_str())
        .or(subject.reconcile.shell.as_deref())?;
    let (layers, _) = recipe::Layers::load(subject.profile, context.data(), shell);
    Some(layers)
}

/// What the functional tier intends to run, before it has looked at the
/// machine it would run it on.
struct Plan {
    /// The scripts to run: the recipe's own, or the built-in floor.
    probes: Vec<recipe::Probe>,
    /// What planning itself already has to say.
    warnings: Vec<String>,
    /// Whether the plan is the whole of what the recipes asked for. A tier that
    /// was handed a script it could not read cannot pass the pass, whatever
    /// else it did: the verdict is about what was *declared*, and a dropped
    /// entry is a hole in that.
    complete: bool,
}

/// The scripts for this pass, and the floor when a recipe declares none.
///
/// The floor is #30's, in the only form this ticket can justify: tap Super and
/// require the screen to change. It is deliberately *not* "apply a wallpaper and
/// watch a path change", even though a recipe that declares a wallpaper dir
/// (`[[dirs]]`) is exactly the case that probe was imagined for — that probe
/// would have to type into a launcher it cannot see, press a key whose target
/// depends on which wallpaper happens to be first, and then read a file whose
/// new contents it cannot predict. Asserting on a predicted outcome is how a
/// probe starts failing desktops for the wrong reason, so the second half of the
/// floor waits for the acceptance account (#41), where the expected result can
/// be written down by hand for one machine.
fn plan(layers: Option<&recipe::Layers>, subject: &Subject) -> Plan {
    let declared = layers.map(recipe::Layers::probes).unwrap_or_default();
    let broken = layers.map(recipe::Layers::broken_probes).unwrap_or(0);
    let mut warnings = Vec::new();
    if broken > 0 {
        warnings.push(format!(
            "{broken} declared `[[probe]]` entry/entries could not be read and were not run; the \
             report's findings say which, and the verdict is degraded to `{VERIFIED_CORE}` \
             because a tier that was asked to be driven and was not has not been proved"
        ));
    }
    // The floor needs a shell to be a floor for: a profile that names none has
    // no desktop of its own to tap, and one that names a shell RiceSwap has no
    // recipe for still has a desktop worth tapping.
    let probes = if !declared.is_empty() || subject.shell.is_none() {
        declared
    } else {
        vec![floor()]
    };
    Plan {
        probes,
        warnings,
        complete: broken == 0,
    }
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
        return Outcome::passed(Check::core(
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
        Some(name) => Outcome::passed(Check::core(
            "shell-alive",
            true,
            format!(
                "`{name}` survived its {}ms start window and is still running",
                START_GRACE.as_millis()
            ),
        )),
        None => Outcome::failed(
            Check::core(
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
        return Outcome::passed(Check::core(
            "invariant-live",
            true,
            format!("skipped: {why}"),
        ));
    }
    if dispatched.names.is_empty() {
        return Outcome::passed(Check::core(
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
        return Outcome::passed(Check::core(
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
            return Outcome::passed(Check::core(
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
        return Outcome::passed(Check::core(
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
        Check::core(
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
        Ok(answer) if answer.succeeded => Outcome::passed(Check::core(
            "ipc-liveness",
            true,
            format!("the recipe's IPC probe `{probe}` answered"),
        )),
        Ok(answer) => Outcome::failed(
            Check::core(
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
            Check::core(
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
    stdout: String,
    stderr: String,
}

/// Runs one command and reads how it ended, in bounded time.
///
/// The bound is the point: `hyprctl` talks to a compositor that can be wedged,
/// and a verification step that waits forever on a wedged compositor is a switch
/// that never returns on a desktop it has already changed. A command still
/// running at the deadline is killed and reported as no answer at all, which is
/// the same thing a dead one is.
///
/// stdout is discarded on purpose: Tier C's commands are all *did it answer*
/// questions, and a compositor that prints megabytes into a pipe nobody reads
/// is a compositor that can wedge the tier by writing to it.
fn run_query(command: &str) -> Result<Answer, String> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot run `{command}`: {error}"))?;
    let status = bounded_wait(&mut child, PROBE_BUDGET, command)?
        .ok_or_else(|| format!("`{command}` did not answer in time"))?;
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        use std::io::Read;
        let _ = pipe.read_to_string(&mut stderr);
    }
    Ok(Answer {
        succeeded: status.success(),
        stdout: String::new(),
        stderr,
    })
}

/// Waits for `child` and answers with how it ended, killing and answering `None`
/// if it is still running at the deadline.
///
/// Killing rather than abandoning is deliberate: `grim` and `ydotool` are the
/// two processes here that touch the user's session, and a screenshot left
/// half-written into a scratch directory is a fact nobody downstream reads.
fn bounded_wait(
    child: &mut Child,
    budget: Duration,
    label: &str,
) -> Result<Option<ExitStatus>, String> {
    let deadline = Instant::now() + budget;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(Some(status)),
            Ok(None) => {}
            Err(error) => return Err(format!("cannot watch `{label}`: {error}")),
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
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

// ------------------------------------------------------------------ tier F
//
// Everything below drives the session. Nothing above it does, which is the
// point: the degrade rule and the whole of Tier C have to be readable without
// thinking about synthetic input at all.

/// A machine this session can be driven on.
///
/// The name is the whole type. There is nothing to configure and nothing to hold
/// — one probe at the start of the tier decides whether the primitives exist,
/// and a [`Desktop`] is the receipt. A machine without both answers
/// [`Desktop::probe`] with why, and the tier does not run.
struct Desktop;

/// Asks the two tools the probe primitives need whether they are here, once.
///
/// The asymmetry is deliberate. `grim` is asked through the tool roster, which
/// already knows that `grim`'s getopt rejects `--version` and that its clean
/// answer is the `-h` banner — asking it the way `detect` asks it is what keeps
/// a perfectly usable screenshot tool from degrading a whole machine. `ydotool`
/// has no roster entry to borrow (#34 made it a stub-only seam with no version
/// contract), so it is asked directly, and `help` is the question because that
/// is what a `ydotool` that cannot answer it cannot do with a keycode either.
///
/// Either answer is fatal to the tier, not to the individual probe: the verdict
/// is one word, and a tier that ran half of itself would make that word a lie.
impl Desktop {
    fn probe() -> Result<Desktop, String> {
        let ydotool = [YDOTOOL.to_string(), "help".to_string()];
        match capture(&ydotool) {
            Ok(answer) if answer.succeeded => {}
            Ok(answer) => {
                return Err(format!(
                    "`{YDOTOOL} help` exited non-zero{}",
                    first_line(&answer.stderr)
                ));
            }
            Err(why) => return Err(format!("`{YDOTOOL}` could not be run ({why})")),
        }
        let grim = crate::tools::probe(Tool::Grim);
        if !grim.succeeded() {
            return Err(match grim.error {
                Some(error) => format!("`grim -h` is not usable here ({error})"),
                None => format!("`grim -h` exited with {}", grim.exit_code.unwrap_or(-1)),
            });
        }
        Ok(Desktop)
    }
}

/// Runs `argv` and reads everything it said, in bounded time.
///
/// The Tier F sibling of [`run_query`], and a separate function on purpose:
/// there is one place in this file that pipes stdout to `/dev/null` and it is
/// not this one, because a probe's `ipc` step asserts on the *contents* of an
/// answer while Tier C's probes only ever asked whether one arrived.
fn capture(argv: &[String]) -> Result<Answer, String> {
    let Some((program, args)) = argv.split_first() else {
        return Err("no command to run".to_string());
    };
    let label = argv.join(" ");
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot run `{label}`: {error}"))?;
    let (succeeded, mut stderr) = match bounded_wait(&mut child, PROBE_BUDGET, &label)? {
        Some(status) => (status.success(), String::new()),
        // A tool still running at the deadline is killed and reported the same
        // way a failed one is, through the channel every caller already reads —
        // so a wedged screenshot tool and a broken one look alike in the
        // evidence rather than in a new error shape.
        None => (false, format!("{label} did not answer in time\n")),
    };
    let mut out = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        use std::io::Read;
        let _ = pipe.read_to_string(&mut out);
    }
    if let Some(mut pipe) = child.stderr.take() {
        use std::io::Read;
        let _ = pipe.read_to_string(&mut stderr);
    }
    Ok(Answer {
        succeeded,
        stdout: out,
        stderr,
    })
}

/// A scratch directory for the screenshots, removed when the tier is done with
/// it — including when the tier panics, which is what the `Drop` is for. A
/// verification pass has no business leaving files behind in `/tmp` on a
/// machine that is about to be told its desktop is fine.
struct Scratch(PathBuf);

impl Scratch {
    fn create() -> Result<Scratch, String> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or(0);
        let path =
            std::env::temp_dir().join(format!("riceswap-probe-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&path)
            .map_err(|error| format!("{} cannot be created ({error})", path.display()))?;
        Ok(Scratch(path))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// One probe's row, and the reason code a failure of it contributes.
///
/// `proves` is the difference between *this check read something* and *this
/// check could not*, and it is what decides whether the pass may say
/// `verified-full`: a refused probe and a probe the budget never covered are
/// both `ok: true` under the frozen key set, and neither is evidence that the
/// desktop answered.
struct ProbeOutcome {
    check: Check,
    reason: Option<String>,
    proves: bool,
}

impl ProbeOutcome {
    /// A probe that ran and answered.
    fn passed(id: String, evidence: impl Into<String>) -> ProbeOutcome {
        ProbeOutcome {
            check: Check::probe(id.clone(), true, evidence),
            reason: None,
            proves: true,
        }
    }

    /// A probe that ran and did not answer. The reason code is the frozen
    /// `probe-failed:<id>`, which the switch's failure path already knows how to
    /// roll back from — the same code the `ipc-liveness` check contributes, so
    /// one recovery serves both.
    fn failed(id: String, why: impl Into<String>) -> ProbeOutcome {
        let reason = format!("{PROBE_FAILED}:{id}");
        ProbeOutcome {
            check: Check::probe(id, false, why),
            reason: Some(reason),
            proves: true,
        }
    }

    /// A probe that did not run: refused, or out of budget, or with nowhere to
    /// write a screenshot. Said in the evidence, because the frozen key set has
    /// no `skipped` to say it in — and counted as no evidence, because the
    /// verdict must not treat silence as agreement.
    fn unrun(id: String, why: impl Into<String>) -> ProbeOutcome {
        ProbeOutcome {
            check: Check::probe(id, true, why),
            reason: None,
            proves: false,
        }
    }
}

/// What the functional tier concluded.
struct TierF {
    /// One row per planned probe, in the order they ran.
    outcomes: Vec<ProbeOutcome>,
    /// Whether the pass may be called `verified-full`: every planned probe ran
    /// and answered.
    proved: bool,
    warnings: Vec<String>,
}

/// Runs every planned probe over a session that can be driven.
fn drive(probes: &[recipe::Probe], _desktop: &Desktop) -> TierF {
    // Declared first so it is dropped last: the driver writes into it, and the
    // directory must outlive every screenshot.
    let scratch = match Scratch::create() {
        Ok(scratch) => scratch,
        Err(why) => {
            return TierF {
                outcomes: probes
                    .iter()
                    .map(|probe| {
                        ProbeOutcome::unrun(
                            probe.id.clone(),
                            format!("skipped: there was nowhere to write a screenshot — {why}"),
                        )
                    })
                    .collect(),
                proved: false,
                warnings: vec![format!(
                    "the functional verification tier (F) ran no probe: {why}; the verdict stays \
                     `{VERIFIED_CORE}`"
                )],
            };
        }
    };
    let mut driver = Driver {
        scratch: scratch.0.clone(),
        started: Instant::now(),
        shots: 0,
    };

    let mut outcomes = Vec::with_capacity(probes.len());
    let mut proved = true;
    for probe in probes {
        let outcome = driver.run(probe);
        proved &= outcome.proves && outcome.reason.is_none();
        outcomes.push(outcome);
    }
    TierF {
        outcomes,
        proved,
        warnings: Vec::new(),
    }
}

/// Holds the two pieces of state a probe run needs: where its screenshots go,
/// and when the tier started.
struct Driver {
    scratch: PathBuf,
    started: Instant,
    shots: u32,
}

impl Driver {
    /// The moment the tier's budget runs out.
    fn deadline(&self) -> Instant {
        self.started + TIER_BUDGET
    }

    /// Whether the tier's budget is already gone.
    fn spent(&self) -> bool {
        Instant::now() >= self.deadline()
    }

    /// Runs one probe: refuse it, run it, and re-run it once if it did not
    /// answer.
    fn run(&mut self, probe: &recipe::Probe) -> ProbeOutcome {
        // The refusal comes first, before anything at all: a probe that could
        // lock the session is not one this tier gets to try and see about.
        if let Some(name) = lock_sensitive(probe) {
            return ProbeOutcome::unrun(
                probe.id.clone(),
                format!(
                    "refused: this probe names the lock capability `{name}`, and this tier never \
                     drives a session lock in production — a lock is checked by name against the \
                     live registry, never by pressing it, so the probe was not run and the rest \
                     of the tier carried on"
                ),
            );
        }
        if self.spent() {
            return ProbeOutcome::unrun(
                probe.id.clone(),
                format!(
                    "skipped: the {}ms functional-tier budget was already spent when this probe \
                     was reached, so it was not run",
                    TIER_BUDGET.as_millis()
                ),
            );
        }
        let first = match self.attempt(probe) {
            Ok(evidence) => return ProbeOutcome::passed(probe.id.clone(), evidence),
            Err(why) => why,
        };
        // A probe that failed once and passes the second is a flaky probe, and
        // one sample of a flaky thing is not a verdict — so it gets exactly one
        // more go, and which of the two answers is reported says so.
        if self.spent() {
            return ProbeOutcome::failed(
                probe.id.clone(),
                format!(
                    "{first}; and the {}ms tier budget left no time to re-run it",
                    TIER_BUDGET.as_millis()
                ),
            );
        }
        match self.attempt(probe) {
            Ok(evidence) => ProbeOutcome::passed(
                probe.id.clone(),
                format!("{evidence} — this is the re-run; the first one did not answer ({first})"),
            ),
            Err(second) => ProbeOutcome::failed(
                probe.id.clone(),
                format!("{second}, and the re-run did not answer either ({first})"),
            ),
        }
    }

    /// One run of a probe's steps, in order, and the prose that says what
    /// happened.
    ///
    /// The baselines are taken here, before the first step runs, which is the
    /// whole reason a `shot` step can say anything: the "before" has to be a
    /// before. Every observed region gets one, and a probe that observes the
    /// same region twice compares both looks against the same baseline — the
    /// question a probe asks is "is this region what it was when I started",
    /// not "is it what it was a moment ago".
    fn attempt(&mut self, probe: &recipe::Probe) -> Result<String, String> {
        let (deadline, window) = settle_window(probe, Instant::now(), self.deadline());

        let mut baseline: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        for region in regions(probe) {
            baseline.insert(region.clone(), self.shot(&region)?);
        }

        let mut said = Vec::with_capacity(probe.steps.len());
        for step in &probe.steps {
            said.push(match step.action {
                recipe::Action::Key => self.key(step)?,
                recipe::Action::Type => self.typed(step)?,
                recipe::Action::Shot => self.watch(step, &baseline, deadline, window)?,
                recipe::Action::Ipc => self.answered(step, deadline, window)?,
            });
        }
        Ok(format!(
            "{} step(s): {}",
            probe.steps.len(),
            said.join("; ")
        ))
    }

    /// Sends a key press, and says so.
    fn key(&mut self, step: &recipe::ProbeStep) -> Result<String, String> {
        let keycodes = step.keycodes.as_deref().unwrap_or_default();
        let mut argv = vec![YDOTOOL.to_string(), "key".to_string()];
        argv.extend(keycodes.split_whitespace().map(str::to_string));
        match capture(&argv) {
            Ok(answer) if answer.succeeded => Ok(format!("`{YDOTOOL} key {keycodes}` sent")),
            Ok(answer) => Err(format!(
                "`{YDOTOOL} key {keycodes}` exited non-zero{}",
                first_line(&answer.stderr)
            )),
            Err(why) => Err(why),
        }
    }

    /// Types text, and says so.
    fn typed(&mut self, step: &recipe::ProbeStep) -> Result<String, String> {
        let text = step.text.as_deref().unwrap_or_default();
        // `--` before the text is how `ydotool type` is told the string ends
        // here, so a probe typing `-` or `--delay` types those characters
        // instead of being read as flags.
        let argv = vec![
            YDOTOOL.to_string(),
            "type".to_string(),
            "--".to_string(),
            text.to_string(),
        ];
        match capture(&argv) {
            Ok(answer) if answer.succeeded => Ok(format!("`{YDOTOOL} type -- {text:?}` sent")),
            Ok(answer) => Err(format!(
                "`{YDOTOOL} type` exited non-zero{}",
                first_line(&answer.stderr)
            )),
            Err(why) => Err(why),
        }
    }

    /// Looks at a region until it differs from its baseline, or stops being
    /// different, as the step declared.
    ///
    /// The evidence names the sizes on both sides rather than only saying
    /// "changed", because "changed" is the *floor* this build measures — the
    /// encoding of the region is not what it was, which a ticking clock
    /// satisfies. A reader who knows that cannot be misled by the row.
    fn watch(
        &mut self,
        step: &recipe::ProbeStep,
        baseline: &BTreeMap<String, Vec<u8>>,
        deadline: Instant,
        window: Duration,
    ) -> Result<String, String> {
        let region = step.region_key();
        let before = baseline
            .get(&region)
            .ok_or_else(|| format!("the {} was never taken a `before` for", describe(&region)))?;
        let wanted = step.expect.as_deref().unwrap_or(recipe::CHANGE);
        let holds = if wanted == recipe::SAME {
            "stayed the same"
        } else {
            "changed"
        };
        let where_ = describe(&region);
        let mut looks = 0;
        loop {
            looks += 1;
            let after = self.shot(&region)?;
            let changed = after != *before;
            if changed == (wanted != recipe::SAME) {
                return Ok(format!(
                    "{where_} {holds} on look {looks} ({} B before, {} B after — a byte \
                     comparison, not an image diff)",
                    before.len(),
                    after.len()
                ));
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "{where_} never {holds} in {}ms ({looks} look(s) against a baseline of {} B)",
                    window.as_millis(),
                    before.len()
                ));
            }
            self.wait(deadline);
        }
    }

    /// Runs a command until its answer carries the expected substring.
    ///
    /// A non-zero exit is not a failure here but "not yet": a shell that is
    /// still initializing answers nothing, and the settle loop is what turns
    /// that into patience rather than into a verdict.
    fn answered(
        &mut self,
        step: &recipe::ProbeStep,
        deadline: Instant,
        window: Duration,
    ) -> Result<String, String> {
        let cmd = step.cmd.as_deref().unwrap_or_default();
        let expect = step.expect.as_deref().unwrap_or_default();
        let mut tries = 0;
        loop {
            tries += 1;
            let answer = capture(&["sh".to_string(), "-c".to_string(), cmd.to_string()])?;
            if answer.succeeded && answer.stdout.contains(expect) {
                return Ok(format!(
                    "`{cmd}` answered with `{}` on try {tries}",
                    excerpt(&answer.stdout, expect)
                ));
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "`{cmd}` never answered with `{expect}` in {}ms ({tries} {})",
                    window.as_millis(),
                    if tries == 1 { "try" } else { "tries" }
                ));
            }
            self.wait(deadline);
        }
    }

    /// Takes one screenshot of a region and reads it back.
    ///
    /// A fresh file per look, always: two looks compared against each other
    /// would be a diff of the tool's own writes rather than of the screen, and
    /// the directory is what guarantees the bytes came from this call.
    fn shot(&mut self, region: &str) -> Result<Vec<u8>, String> {
        let path = self.scratch.join(format!("shot-{}.png", self.shots));
        self.shots += 1;
        let mut argv = vec!["grim".to_string()];
        if !region.is_empty() {
            argv.push("-g".to_string());
            argv.push(region.to_string());
        }
        argv.push(path.display().to_string());
        match capture(&argv) {
            Ok(answer) if answer.succeeded => {}
            Ok(answer) => {
                return Err(format!(
                    "`grim` exited non-zero{}",
                    first_line(&answer.stderr)
                ));
            }
            Err(why) => return Err(why),
        }
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("`grim` wrote nothing to read back ({error})"))?;
        // The size sanity check the byte floor needs: an empty file is not a
        // picture that happens to be identical to the last one, it is nothing.
        if bytes.is_empty() {
            return Err("`grim` wrote an empty file, which is not a picture".to_string());
        }
        Ok(bytes)
    }

    /// Waits out the poll interval, or whatever is left of the budget.
    fn wait(&self, deadline: Instant) {
        let now = Instant::now();
        if now < deadline {
            std::thread::sleep(POLL.min(deadline - now));
        }
    }
}

/// The distinct regions a probe observes, in the order it first looks at them.
fn regions(probe: &recipe::Probe) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for step in &probe.steps {
        if step.action == recipe::Action::Shot {
            let region = step.region_key();
            if !seen.contains(&region) {
                seen.push(region);
            }
        }
    }
    seen
}

/// Whether a probe could plausibly lock the session, and the name that says so.
///
/// By name, never by trial: the check is a reading of the declaration, and
/// nothing is dispatched to find out. `expect` and `region` are not scanned —
/// a substring of a pixel region is not a shortcut, and the expected outcome is
/// something the probe wants to *read*.
fn lock_sensitive(probe: &recipe::Probe) -> Option<String> {
    // `id` first, because the id is the name a reader sees in `checks` and in
    // `probe-failed:<id>`, and a probe called `lock-cycle` is the one thing a
    // human must not have to look inside to notice.
    Some(probe.id.as_str())
        .into_iter()
        .chain(probe.steps.iter().flat_map(|step| {
            [
                step.keycodes.as_deref(),
                step.text.as_deref(),
                step.cmd.as_deref(),
            ]
            .into_iter()
            .flatten()
        }))
        .find_map(lock_token)
}

/// The first lock-shaped word in `text`, lowercased.
fn lock_token(text: &str) -> Option<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_ascii_lowercase)
        .find(|token| LOCK_NAMES.iter().any(|name| token.starts_with(name)))
}

/// The built-in floor, in full: tap Super, and the screen must not be what it
/// was. Everything else a probe can say is a recipe's to declare; this is the
/// one thing a machine with no recipe at all can still be asked.
///
/// `125` is `KEY_LEFTMETA` in the evdev numbering `ydotool` speaks, so
/// `125:1 125:0` is Super pressed and released. The whole screen is the region
/// on purpose: a floor probe that named a region would be asserting that some
/// particular part of some particular layout is where a launcher appears, which
/// is a claim about a rice this build has never seen.
fn floor() -> recipe::Probe {
    recipe::Probe {
        id: "floor-super-opens-something".to_string(),
        settle_seconds: None,
        steps: vec![
            recipe::ProbeStep {
                action: recipe::Action::Key,
                keycodes: Some("125:1 125:0".to_string()),
                text: None,
                region: None,
                cmd: None,
                expect: None,
            },
            recipe::ProbeStep {
                action: recipe::Action::Shot,
                keycodes: None,
                text: None,
                region: None,
                cmd: None,
                expect: Some(recipe::CHANGE.to_string()),
            },
        ],
    }
}

/// When an attempt has to stop looking, and how long it really had.
///
/// Three numbers meet here and one of them wins. The declaration asks for a
/// window; the tier's own 10s is both the default and the cap, so a probe cannot
/// ask for a patience this build does not have; and the tier's 60s budget is the
/// wall. The second half of the answer is the window *as it was*, because that is
/// what the evidence quotes — a probe whose declared window was cut short by the
/// budget should say so in milliseconds rather than in the number it asked for.
fn settle_window(probe: &recipe::Probe, began: Instant, budget: Instant) -> (Instant, Duration) {
    let asked = probe
        .settle_seconds
        .unwrap_or(SETTLE.as_secs())
        .min(SETTLE.as_secs());
    let deadline = (began + Duration::from_secs(asked)).min(budget);
    (deadline, deadline.saturating_duration_since(began))
}

/// How a region is named in prose: the whole screen, or its geometry.
fn describe(region: &str) -> String {
    if region.is_empty() {
        "the whole screen".to_string()
    } else {
        format!("the `{region}` region")
    }
}

/// The line the expectation was found on, trimmed and capped.
///
/// A *line* rather than a window into the stream, and it is bounded to 120
/// characters for the same reason [`first_line`] is: a probe's evidence is a
/// sentence, and a command that answers with a whole log is not a reason to
/// carry the log into the envelope. The matched substring is never truncated
/// away — an answer too long to quote says so rather than quoting a half of it
/// that does not contain what it claimed.
fn excerpt(text: &str, expect: &str) -> String {
    let line = text
        .lines()
        .find(|line| line.contains(expect))
        .or_else(|| text.lines().find(|line| !line.trim().is_empty()))
        .unwrap_or("")
        .trim();
    if line.chars().count() > 120 {
        return format!("…{}", line.chars().take(119).collect::<String>());
    }
    line.to_string()
}

#[cfg(test)]
mod tests {
    //! The parts of the functional tier that are arithmetic rather than process
    //! work: the lock vocabulary, the timing policy, and what a region looks
    //! like in a sentence. Everything that runs a command is proved in
    //! `tests/verifyf.rs`, against the same stubs the switch runs against.

    use super::*;

    fn probe(id: &str, steps: Vec<recipe::ProbeStep>, settle: Option<u64>) -> recipe::Probe {
        recipe::Probe {
            id: id.to_string(),
            settle_seconds: settle,
            steps,
        }
    }

    fn step(action: recipe::Action) -> recipe::ProbeStep {
        recipe::ProbeStep {
            action,
            keycodes: None,
            text: None,
            region: None,
            cmd: None,
            expect: None,
        }
    }

    fn with_field(mut step: recipe::ProbeStep, name: &str, value: &str) -> recipe::ProbeStep {
        match name {
            "keycodes" => step.keycodes = Some(value.to_string()),
            "text" => step.text = Some(value.to_string()),
            "cmd" => step.cmd = Some(value.to_string()),
            "region" => step.region = Some(value.to_string()),
            _ => step.expect = Some(value.to_string()),
        }
        step
    }

    /// The lock refusal is by name and it is blunt on purpose. Every shape a
    /// real config spells a lock in is refused — the evdev keycode, the
    /// registered name, the `hyprctl dispatch` form, the id — and so is a word
    /// that merely starts the same way, which is the false positive the module
    /// docs say is the cheap direction to be wrong in.
    #[test]
    fn the_lock_vocabulary_is_matched_by_name_wherever_it_is_written() {
        let refused = [
            ("lock-cycle", "lock"),
            ("unlock-cycle", "unlock"),
            ("lockFocus", "lockfocus"),
            ("the-lockscreen", "lockscreen"),
        ];
        for (id, expected) in refused {
            let probe = probe(
                id,
                vec![with_field(step(recipe::Action::Shot), "expect", "change")],
                None,
            );
            assert_eq!(
                lock_sensitive(&probe).as_deref(),
                Some(expected),
                "`{id}` names the lock surface"
            );
        }

        // The id is only one of the four places a name can hide.
        let by_keycode = probe(
            "open-panel",
            vec![with_field(
                step(recipe::Action::Key),
                "keycodes",
                "125:1 28:1 lock:1",
            )],
            None,
        );
        assert_eq!(lock_sensitive(&by_keycode).as_deref(), Some("lock"));
        let by_text = probe(
            "type-it",
            vec![with_field(step(recipe::Action::Type), "text", "type /lock")],
            None,
        );
        assert_eq!(lock_sensitive(&by_text).as_deref(), Some("lock"));
        let by_command = probe(
            "dispatch-it",
            vec![with_field(
                with_field(
                    step(recipe::Action::Ipc),
                    "cmd",
                    "hyprctl dispatch 'caelestia:unlock'",
                ),
                "expect",
                "ok",
            )],
            None,
        );
        assert_eq!(lock_sensitive(&by_command).as_deref(), Some("unlock"));

        // And what is *not* a lock: the common case, which has to keep running,
        // and the two arguments that are not names at all — a pixel region and
        // a substring an answer is expected to carry.
        for (id, steps) in [
            (
                "launcher-opens",
                vec![
                    with_field(step(recipe::Action::Key), "keycodes", "125:1 125:0"),
                    with_field(
                        with_field(step(recipe::Action::Shot), "region", "800x600+0+0"),
                        "expect",
                        "change",
                    ),
                ],
            ),
            (
                "clock-tick",
                vec![
                    with_field(
                        with_field(step(recipe::Action::Shot), "region", "100x30+0+0"),
                        "expect",
                        "change",
                    ),
                    with_field(
                        with_field(step(recipe::Action::Ipc), "cmd", "date +%s"),
                        "expect",
                        "blocked",
                    ),
                ],
            ),
        ] {
            assert_eq!(
                lock_sensitive(&probe(id, steps, None)),
                None,
                "`{id}` is not a lock and must run"
            );
        }
    }

    /// The timing policy, as arithmetic: the tier's own 10s is the default *and*
    /// the cap, so no declaration can buy a patience this build does not have,
    /// and the whole tier's 60s wall narrows whatever is left of it. The second
    /// half of the answer is what the evidence quotes, so a window cut short by
    /// the budget is reported as the window that was actually had.
    #[test]
    fn the_settle_window_is_the_declared_one_narrowed_by_the_cap_and_the_budget() {
        let began = Instant::now();
        let generous = began + TIER_BUDGET;

        let (deadline, window) = settle_window(&probe("p", vec![], None), began, generous);
        assert_eq!(window, SETTLE, "ten seconds is the default");
        assert_eq!(deadline, began + SETTLE);

        let (deadline, window) = settle_window(&probe("p", vec![], Some(2)), began, generous);
        assert_eq!(window, Duration::from_secs(2), "a probe may ask for less");
        assert_eq!(deadline, began + Duration::from_secs(2));

        let (_, window) = settle_window(&probe("p", vec![], Some(900)), began, generous);
        assert_eq!(window, SETTLE, "and never for more than the cap");

        // The tier's own budget is the wall: an attempt that starts with two
        // seconds of tier left gets two seconds, not ten and not zero.
        let nearly_spent = began + Duration::from_secs(2);
        let (deadline, window) = settle_window(&probe("p", vec![], None), began, nearly_spent);
        assert_eq!(window, Duration::from_secs(2));
        assert_eq!(deadline, nearly_spent);
        let (deadline, window) = settle_window(&probe("p", vec![], None), began, began);
        assert_eq!(
            window,
            Duration::ZERO,
            "a spent budget ends the attempt now"
        );
        assert_eq!(deadline, began);
    }

    /// The floor is the #35 session's first move, and it is the whole screen
    /// rather than a guessed region: a floor probe that named a region would be
    /// asserting where a launcher appears on a rice this build has never seen.
    #[test]
    fn the_floor_is_a_super_tap_and_a_whole_screen_watch() {
        let floor = floor();
        floor.validate().expect("the floor is a runnable probe");
        assert_eq!(floor.id, "floor-super-opens-something");
        assert_eq!(
            floor.settle_seconds, None,
            "the floor waits the tier's default, rather than asking for one of its own"
        );
        assert_eq!(floor.steps.len(), 2);
        assert_eq!(floor.steps[0].action, recipe::Action::Key);
        assert_eq!(
            floor.steps[0].keycodes.as_deref(),
            Some("125:1 125:0"),
            "KEY_LEFTMETA down then up, as `ydotool` spells it"
        );
        assert_eq!(floor.steps[1].action, recipe::Action::Shot);
        assert_eq!(floor.steps[1].region, None, "the whole screen");
        assert_eq!(floor.steps[1].expect.as_deref(), Some(recipe::CHANGE));
        assert_eq!(
            lock_sensitive(&floor),
            None,
            "the built-in floor must never be the one probe the tier refuses"
        );
    }

    /// A region is named in a sentence rather than as geometry alone, because
    /// the evidence is read by a person deciding whether a failure is real, and
    /// "the `800x600+0+0` region never changed" says what was watched where
    /// "`-g 800x600+0+0`" does not.
    #[test]
    fn a_region_is_named_in_prose() {
        assert_eq!(describe(""), "the whole screen");
        assert_eq!(describe("800x600+0+0"), "the `800x600+0+0` region");
    }

    /// An answer is quoted as one line, trimmed, and never so trimmed that the
    /// matched substring falls out of it — the evidence is a sentence about what
    /// the command said, and a sentence that omits the thing it matched is
    /// worse than a shorter one.
    #[test]
    fn an_answer_is_quoted_as_the_line_that_matched() {
        assert_eq!(excerpt("wallpaper is set\n", "is set"), "wallpaper is set");
        assert_eq!(
            excerpt("first line\nthe one that matters\nlast\n", "matters"),
            "the one that matters",
            "the line the expectation was found on, not the first line"
        );
        assert_eq!(
            excerpt("  padded  \n", "padded"),
            "padded",
            "trimmed: a quote in a sentence carries no indent"
        );
        assert_eq!(excerpt("", "anything"), "");
        assert_eq!(
            excerpt(&"x".repeat(500), &"x".repeat(500)),
            format!("…{}", "x".repeat(119)),
            "a very long answer is capped rather than carried into the envelope"
        );
    }
}
