//! The dispatcher-reconciliation engine (issue #35).
//!
//! A rice's Hyprland configs dispatch the desktop shell's shortcuts by name —
//! `hl.dsp.global("quickshell:lock")`, or the same string carried inside a
//! plain-conf `hyprctl dispatch 'hl.dsp.global("quickshell:lock")'`. A profile
//! captured while one shell ran therefore speaks that shell's vocabulary, and
//! switching to a profile whose shell registers *different* names leaves every
//! shell bind dead. This module is the step that makes that fix itself: on every
//! switch it derives the activating shell's registry, finds the dispatched names
//! nothing answers to, moves the ones it can prove, and reports the rest.
//!
//! Four rules make it safe to run unattended, and each is why the code is shaped
//! the way it is.
//!
//! **The registry is derived, never assumed.** It comes from a static parse of
//! the activating profile's own QML — `.config/quickshell/<shell>/`, the name
//! the manifest's `[shell]` table gives. Only what the parse can *prove* counts:
//! a literal `name:` inside a `GlobalShortcut`/`CustomShortcut` block. A
//! registration built from a variable is not a registration as far as this
//! engine is concerned: the parse records it as uncertain, and everything
//! downstream treats the registry as incomplete rather than guessing.
//!
//! **Deadness is set membership, not a prefix rule.** A dispatched `ns:name` is
//! dead when it is in none of the derived registry, the built-in foreign set, or
//! the explicit keeps. No namespace is ever assumed to be the donor, which is
//! what leaves ii's own `quickshell:*` binds alone — they resolve against the
//! registry derived from ii's own QML.
//!
//! **Only exact same-name moves are applied.** `quickshell:lock` becomes
//! `caelestia:lock` because the registry proves the activating shell registers
//! `lock`, and under exactly one appid, so the target is not a guess.
//! `regionScreenshot` against a registry holding `screenshotClip` is a different
//! name: it is reported as a proposal and left exactly as it is. Resolving
//! proposals is the recipe/AI tier's job (issue #28), not this one's.
//!
//! **A clean profile costs nothing.** Every file is read, every candidate
//! rewrite computed, and the result compared against what is already on disk
//! before anything is written. A second run over an already-reconciled profile
//! writes zero files and makes zero backups, and the pre-rewrite bytes of every
//! file that is touched are kept content-addressed under the profile's
//! `backups/`.
//!
//! The one thing this engine will not do by itself is guess what a name *meant*.
//! That half of the problem is the recipe tier's ([`crate::recipe`]), and it
//! runs inside this same pass, in this order:
//!
//! 1. the layers load — `adapt.toml`, the built-in recipe for this shell, the
//!    user tier — and their `[foreign]` entries join the keep set, so a name
//!    any layer calls another live shell's is never even a proposal;
//! 2. the dead set is computed exactly as above, from the derived registry;
//! 3. a name a layer speaks for gets that decision — `to` re-points the
//!    dispatch (and is refused unless the derived registry holds the target),
//!    `exec` replaces it with a command in the file's own dialect, `drop`
//!    removes the statement. Only a name no layer speaks for falls through to
//!    the engine's own exact-name move, and a name neither answers for stays a
//!    proposal, byte for byte as it was;
//! 4. the winner's `[env]` is materialized as a managed block in the profile's
//!    own `custom/env.lua`, and its `[[dirs]]` are guaranteed on disk.
//!
//! The two kinds of authority never overlap in what they decide. Derivation
//! says what *exists* — which namespace the activating shell registers under,
//! and whether a recipe's `to` target is a real shortcut — and a recipe says
//! what a key *means*. So a declared decision is applied even for a name the
//! registry could move mechanically (that is the whole point of a codebook:
//! "this key means something else"), while an unproved target is reported and
//! never written. And a recipe that cannot be read silences its own tier rather
//! than half-applying it.

use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::recipe;

/// The appid a Quickshell config registers under when its QML says nothing
/// else. Quickshell's own default, and the reason ii's shortcuts are all
/// `quickshell:*`.
pub const DEFAULT_APPID: &str = "quickshell";

/// Namespaces that belong to *other* live shells, never to the one being
/// reconciled, and that the engine therefore never rewrites whatever the
/// derived registry says.
///
/// `quickshell:riceswap-toggle` is the panel RiceSwap itself runs (`qs -c
/// riceswap`, which both real profiles launch from `execs.lua`). It resolves
/// against no profile shell's registry — there is no such registration to
/// resolve against — and rewriting it would break the tool doing the rewriting.
pub const FOREIGN: &[&str] = &["quickshell:riceswap-toggle"];

/// What one pass learned, in the shape the switch envelope carries.
#[derive(Debug, Default, Serialize)]
pub struct Report {
    /// The shell the engine derived against — the manifest's `[shell] name`.
    pub shell: Option<String>,
    /// The appid that shell registers under.
    pub appid: Option<String>,
    /// How many registrations the parse proved.
    pub registered: usize,
    /// How many config files were read for dispatched names.
    pub scanned_files: usize,
    /// Dispatched names nothing answers to and that survive the pass, each still
    /// needing a recipe to resolve. A name a recipe resolved is *not* here: the
    /// dead set is what is still dead once every layer has had its say.
    pub dead_names: Vec<String>,
    /// Every dead name the engine found, with the resolution it could prove for
    /// it — the exact-name move that was applied, or `null` for "no
    /// counterpart" — and the lines that dispatch it. This is the proposal
    /// vocabulary the recipe tier consumes verbatim: one entry per dead name,
    /// carrying the per-line detail that tier resolves against. A name listed
    /// here was dead when the pass started; whether it still is, is what
    /// `dead_names` and `resolutions` say.
    pub proposals: Vec<Proposal>,
    /// Every proposal a recipe resolved, and what it became. The kind is one of
    /// `to`, `exec`, `drop` — the same three the schema admits, and no fourth.
    pub resolutions: Vec<Applied>,
    /// The recipe tiers that spoke, in the order a lookup walks them: `adapt`,
    /// `builtin`, `user`.
    pub layers: Vec<String>,
    /// The `[shell] appid` pins that disagree with the registry derived from the
    /// activating profile's own QML. Derivation always won; this is the drift
    /// check's finding, not an instruction.
    pub appid_drift: Vec<String>,
    /// The keep set this pass ran with: the built-in foreign guard, the caller's
    /// explicit keeps, and every recipe's `[foreign] entries`. A dispatched name
    /// in this set is never rewritten and never reported.
    pub foreign_entries: Vec<String>,
    /// The managed env block, when the winning layers declared `[env]`.
    pub env: Option<EnvBlock>,
    /// Paths guaranteed outside the profile on this pass — the wallpaper-dir
    /// class. A path that was already correct is not here: this list is what was
    /// created, not what exists.
    pub dirs_created: Vec<String>,
    /// Config files the engine rewrote, as `$HOME`-relative paths, including the
    /// managed env file when this pass wrote it.
    pub files_written: Vec<String>,
    /// Where the pre-rewrite bytes of each of those were kept, profile-relative.
    pub backups: Vec<String>,
    /// Helper scripts the configs `exec`: what they dispatch, reported because a
    /// blind edit into a shell script is not something this engine does.
    pub externals: Vec<External>,
    /// Parse findings the engine could not resolve — dynamic registrations,
    /// ambiguous appids, unreadable files. A non-empty list means the registry
    /// is incomplete: a reason to distrust the report, not a licence to rewrite
    /// more.
    pub uncertain: Vec<String>,
    /// Set instead of doing anything, naming why: a profile that names no
    /// shell, one whose QML tree is missing, or one with no Hyprland config.
    pub skipped: Option<String>,
    /// Anything else the user should see — a file that could not be read, a
    /// backup that could not be written, a recipe that could not be read, a
    /// resolution that named a shortcut this shell does not register.
    pub warnings: Vec<String>,
}

/// One proposal a recipe resolved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Applied {
    /// The dead entry as the config dispatched it, `ns:name`.
    pub dispatched: String,
    /// `to`, `exec` or `drop`.
    pub kind: String,
    /// The entry the dispatch was re-pointed at, for `to` — namespace included,
    /// and always one the derived registry holds.
    pub to: Option<String>,
    /// The command the dispatch was replaced with, for `exec`.
    pub exec: Option<String>,
    /// The tier whose entry won: `adapt`, `builtin` or `user`.
    pub layer: String,
    /// The lines the resolution landed on. A name on four binds is four
    /// decisions, and all of them are named here.
    pub sites: Vec<ProposalSite>,
}

/// The block the recipe layer maintains inside the profile's `custom/env.lua`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EnvBlock {
    /// The file it lives in, `$HOME`-relative.
    pub file: String,
    /// The variables in it, as `NAME = value` in name order — the value as
    /// declared, tilde and all.
    pub entries: Vec<(String, String)>,
    /// Whether this pass wrote it. A re-run over a settled block reports
    /// `false` and writes nothing.
    pub written: bool,
}

/// Everything the pass needs that is not the profile it is handed: the `$HOME`
/// a declared path is resolved against and bounded by, and RiceSwap's own data
/// directory, where the user tier is read from.
///
/// Both are passed in rather than read from the environment so a sandboxed test
/// is sandboxed the same way a real switch is — and so a profile's declared
/// paths are checked against the home the user actually has.
pub struct Context<'a> {
    home: &'a Path,
    data: &'a Path,
}

impl<'a> Context<'a> {
    /// The switch's own `$HOME` and data directory.
    pub fn new(home: &'a Path, data: &'a Path) -> Context<'a> {
        Context { home, data }
    }

    /// The `$HOME` a declared path is expanded inside.
    pub fn home(&self) -> &'a Path {
        self.home
    }

    /// RiceSwap's own data directory, holding the user tier's recipes.
    pub fn data(&self) -> &'a Path {
        self.data
    }
}

/// One dead dispatched name and what the engine could prove about it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Proposal {
    /// The dead entry as the config dispatched it, `ns:name`.
    pub dispatched: String,
    /// Its name half — the join key a recipe resolves on.
    pub name: String,
    /// The exact-name move that was applied, or `null` when the name has no
    /// counterpart in the derived registry.
    pub target: Option<String>,
    /// Every line that dispatches this name, in file-then-line order.
    ///
    /// Per name, the sites are what make the proposal actionable rather than
    /// merely true: the recipe tier resolves a name *per line* (shortcut, exec
    /// or drop — #28's trilemma), and one name dispatched on four binds is
    /// four decisions, not one. A consumer that wants the dead *set* reads
    /// [`Report::dead_names`]; this is the full account of where each came from.
    pub sites: Vec<ProposalSite>,
}

/// One line that dispatches a dead name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ProposalSite {
    /// The config file, as a `$HOME`-relative path.
    pub file: String,
    /// The 1-based line the dispatch is on.
    pub line: usize,
}

/// A dispatched name found in a file the configs `exec` rather than read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct External {
    /// The script, as a `$HOME`-relative path.
    pub path: String,
    /// The `ns:name` entries it dispatches that nothing answers to.
    pub dispatched: Vec<String>,
}

/// Runs the whole pass against one profile directory.
///
/// `shell` is the manifest's `[shell] name` — the Quickshell config the profile
/// owns. `keeps` is the caller's own escape hatch; it joins the recipe layers'
/// `[foreign]` entries, and a kept entry behaves exactly like a foreign one:
/// never rewritten, never reported dead.
pub fn reconcile(
    profile: &Path,
    context: &Context,
    shell: Option<&str>,
    keeps: &BTreeSet<String>,
) -> Report {
    let mut report = Report::default();
    let Some(shell) = shell else {
        report.skipped = Some(
            "the profile names no shell, so its dispatches have no registry to resolve against"
                .to_string(),
        );
        return report;
    };
    let registry = Registry::derive(profile, shell);
    report.shell = Some(shell.to_string());
    if let Some(reason) = &registry.skipped {
        report.skipped = Some(reason.clone());
        return report;
    }
    report.appid = Some(registry.appid.clone());
    report.registered = registry.entries.len();
    report.uncertain = registry.uncertain.clone();

    // The layers load before anything is looked up, so their `[foreign]`
    // entries can keep a name out of the dead set entirely — and a recipe that
    // could not be read is said so whether or not it turned out to matter.
    let (layers, findings) = recipe::Layers::load(profile, context.data(), shell);
    report.warnings.extend(findings);
    report.layers = layers.names().into_iter().map(str::to_string).collect();

    // The appid pin is a drift check, never a source of truth: the registry
    // derived above is what says what this shell registers, so a pin that
    // disagrees is reported and ignored.
    if let Some((tier, pinned)) = layers.appid_pin()
        && pinned != registry.appid
    {
        report.appid_drift.push(format!(
            "the {tier} tier pins `{shell}`'s shortcut namespace to `{pinned}`, but its own \
             QML registers under `{}`; the derived namespace is what was used",
            registry.appid
        ));
    }

    let mut keep: BTreeSet<String> = keeps.clone();
    keep.extend(FOREIGN.iter().map(|entry| entry.to_string()));
    keep.extend(layers.foreign());
    report.foreign_entries = keep.iter().cloned().collect();

    let hypr = profile.join(".config").join("hypr");
    if !hypr.is_dir() {
        report.skipped = Some(format!(
            "{} holds no Hyprland config, so it dispatches nothing",
            profile.display()
        ));
        return report;
    }
    let configs = configs_in_scope(&hypr);
    report.scanned_files = configs.len();
    let backups = profile.join("backups");

    // The dead set and the moves that resolve it, gathered in one pass over the
    // configs. Each config file is read and scanned exactly once here: the
    // dead set, the proposals' line sites and the planned edits all come out of
    // the one `Scan`, which is kept for the rewrite below rather than
    // recomputed. (`scan_externals` re-reads the configs afterwards, for the
    // exec lines it needs — a separate, read-only sweep over the same files.)
    let mut moved: BTreeMap<String, String> = BTreeMap::new();
    let mut dead: BTreeMap<String, Vec<ProposalSite>> = BTreeMap::new();
    let mut applied: BTreeMap<String, Applied> = BTreeMap::new();
    let mut scans: Vec<Scan> = Vec::with_capacity(configs.len());
    let mut contents: Vec<Option<String>> = Vec::with_capacity(configs.len());
    let mut plans: Vec<Vec<Edit>> = Vec::with_capacity(configs.len());
    for path in &configs {
        let Ok(text) = std::fs::read_to_string(path) else {
            report
                .warnings
                .push(format!("cannot read {}: skipped", relative(path)));
            scans.push(Scan::default());
            contents.push(None);
            plans.push(Vec::new());
            continue;
        };
        let scan = scan(&text);
        for dynamic in &scan.dynamic {
            report.uncertain.push(format!(
                "{}: `{}` builds its dispatcher name at runtime, so it cannot be resolved",
                relative(path),
                dynamic
            ));
        }
        let mut edits: Vec<Edit> = Vec::new();
        for site in &scan.sites {
            let entry = site.entry();
            if keep.contains(&entry) {
                continue;
            }
            if registry.holds(&site.appid, &site.name) {
                continue; // Resolves as it stands: nothing to fix, nothing to say.
            }
            let site_of = || ProposalSite {
                file: relative(path),
                line: site.line,
            };
            if let Some(found) = layers.resolve(&entry) {
                // A declared decision is the answer, whenever there is one. The
                // engine's exact-name move is what runs in the *absence* of a
                // recipe, not a rival to it: "the same name is registered
                // elsewhere" is a mechanical fact about the namespace, and a
                // recipe that says this key means something else is exactly the
                // authority the tier exists to have. What derivation still
                // decides is whether the *target* exists — that is checked
                // inside `plan`, and an unproved target is refused.
                let decision = &found.entry.decision;
                match plan(&text, site, decision, &registry, &mut report.warnings) {
                    Ok((resolution, mut planned)) => {
                        for edit in &mut planned {
                            edit.entry = entry.clone();
                        }
                        edits.append(&mut planned);
                        let record = applied.entry(entry.clone()).or_insert_with(|| Applied {
                            dispatched: entry.clone(),
                            kind: decision.kind().to_string(),
                            to: None,
                            exec: None,
                            layer: found.tier.to_string(),
                            sites: Vec::new(),
                        });
                        record.sites.push(site_of());
                        match decision {
                            recipe::Decision::To { .. } => record.to = Some(resolution),
                            recipe::Decision::Exec { command } => {
                                record.exec = Some(command.clone())
                            }
                            recipe::Decision::Drop => {}
                        }
                    }
                    Err(reason) => report.warnings.push(format!(
                        "{}: `{}` is left as it is: {reason} (as {} declared it for `{}`)",
                        relative(path),
                        entry,
                        found.origin,
                        found.entry.dispatched
                    )),
                }
            } else if let Some(appid) = registry.provable_appid(&site.name) {
                // No layer speaks for this name, so the engine moves it itself:
                // the registry proves the same name is registered elsewhere, and
                // under exactly one appid, so the target is not a guess. This is
                // the fallback the recipe layer answers over, not a rival to it.
                let target = format!("{appid}:{}", site.name);
                moved.insert(entry.clone(), target.clone());
                edits.push(Edit {
                    span: token_of(site),
                    replacement: target,
                    entry: entry.clone(),
                });
            }
            dead.entry(entry).or_default().push(site_of());
        }
        scans.push(scan);
        contents.push(Some(text));
        plans.push(edits);
    }

    // The proposal vocabulary, in a stable order: every dead name with the move
    // that was applied (or the honest absence of one) and every line that
    // dispatches it.
    report.proposals = dead
        .iter()
        .map(|(entry, sites)| Proposal {
            dispatched: entry.clone(),
            name: recipe::name_of(entry).to_string(),
            target: moved.get(entry).cloned(),
            sites: sites.clone(),
        })
        .collect();
    // What survives: nothing moved it, and a recipe that answered only some of
    // a name's sites has not finished with it. A name on four binds where three
    // landed is still one dead bind, and the report has to keep saying so.
    report.dead_names = dead
        .iter()
        .filter(|(entry, sites)| {
            !moved.contains_key(*entry)
                && applied
                    .get(*entry)
                    .is_none_or(|done| done.sites.len() < sites.len())
        })
        .map(|(entry, _)| entry.clone())
        .collect();
    report.resolutions = applied.into_values().collect();

    for ((path, content), edits) in configs.iter().zip(&contents).zip(&plans) {
        let Some(text) = content else {
            continue; // Already reported above.
        };
        let rewritten = apply(text, edits.clone(), &mut report.warnings);
        if rewritten == *text {
            continue; // Nothing to change: what keeps a clean profile untouched.
        }
        write(
            &mut report,
            &backups,
            path,
            Some(text.as_bytes()),
            &rewritten,
        );
    }

    // The managed env block and the guaranteed directories run after the
    // rewrites: both are in the same profile, both are compared before they are
    // written, and both are already-written states on every switch after the
    // first.
    report.env = materialize_env(&mut report, &backups, profile, context, &layers);
    materialize_dirs(&mut report, context, &layers);

    report.externals = scan_externals(profile, &hypr, &configs, &registry, &keep);
    report
}

/// What a read-only look at a tree found, in the shape the research brief reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Survey {
    /// The appid the shell's tree declares, when the parse could establish one.
    /// `null` when the tree is missing or its appid is ambiguous — the two cases
    /// where naming one would be a guess.
    pub appid: Option<String>,
    /// How many registrations the parse proved.
    pub registered: usize,
    /// Every dispatched name nothing in the tree answers to, with the file and
    /// line it is on. These are the engine's proposal vocabulary for a tree that
    /// has not been installed yet, and the questions a research brief is built
    /// from: a name on four binds is four lines to look at.
    pub dead: Vec<SurveySite>,
    /// Everything the parse could not settle — the same findings the pass
    /// reports, so a brief and a report never disagree about what is uncertain.
    pub uncertain: Vec<String>,
    /// Set instead of surveying, naming why: a tree with no QML for this shell,
    /// or no Hyprland config to dispatch anything from.
    pub skipped: Option<String>,
}

/// One dispatched name a survey found dead.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct SurveySite {
    /// The dead entry as the config dispatched it, `ns:name`.
    pub dispatched: String,
    /// The config file, `$HOME`-relative when the tree sits under one.
    pub file: String,
    /// The 1-based line the dispatch is on.
    pub line: usize,
}

/// Reads a tree and says what the engine could prove about it — without
/// rewriting anything, because the tree being surveyed is the *source*: a
/// clone this tool made, or the user's own directory, and an install must not
/// take bytes out of either.
///
/// This is [`reconcile`] with the writes removed, and it exists because the
/// research tier has to ask its question at a point where no profile has been
/// materialized yet: the brief is built from dead names and parse findings, and
/// both are facts the engine already computes. Deriving them twice — once here
/// for the brief, once in the real pass after the write-back — is the price of
/// asking before the profile exists, and it is the price worth paying: the
/// write-back has to land *before* the pass that consumes it reads the layers.
pub fn survey(tree: &Path, shell: &str) -> Survey {
    let mut survey = Survey {
        appid: None,
        registered: 0,
        dead: Vec::new(),
        uncertain: Vec::new(),
        skipped: None,
    };
    let registry = Registry::derive(tree, shell);
    if let Some(reason) = &registry.skipped {
        survey.skipped = Some(reason.clone());
        return survey;
    }
    survey.appid = Some(registry.appid.clone());
    survey.registered = registry.entries.len();
    survey.uncertain = registry.uncertain.clone();

    let hypr = tree.join(".config").join("hypr");
    if !hypr.is_dir() {
        survey.skipped = Some(format!(
            "{} holds no Hyprland config, so it dispatches nothing",
            tree.display()
        ));
        return survey;
    }
    // Only the built-in foreign guard, because there is nothing else yet: no
    // profile means no `adapt.toml`, and a research run has not written the user
    // tier yet. The pass that runs afterwards re-derives the same dead set with
    // every layer loaded, and the recipe this brief produces is an input to it.
    let keep: BTreeSet<String> = FOREIGN.iter().map(|entry| entry.to_string()).collect();
    for path in configs_in_scope(&hypr) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            survey
                .uncertain
                .push(format!("{}: could not be read", relative(&path)));
            continue;
        };
        let scan = scan(&text);
        for dynamic in &scan.dynamic {
            survey.uncertain.push(format!(
                "{}: `{}` builds its dispatcher name at runtime, so it cannot be resolved",
                relative(&path),
                dynamic
            ));
        }
        for site in &scan.sites {
            let entry = site.entry();
            if keep.contains(&entry) || registry.holds(&site.appid, &site.name) {
                continue;
            }
            survey.dead.push(SurveySite {
                dispatched: entry,
                file: relative(&path),
                line: site.line,
            });
        }
    }
    survey.dead.sort();
    survey.dead.dedup();
    survey
}

/// Keeps the pre-change bytes, then writes the new ones — the one place a
/// config is ever replaced, so the "compare first" discipline has a single name.
///
/// `original` is `None` for a file the pass is creating: there is nothing of
/// the user's to keep.
fn write(
    report: &mut Report,
    backups: &Path,
    path: &Path,
    original: Option<&[u8]>,
    rewritten: &str,
) -> bool {
    if let Some(original) = original {
        match store_backup(backups, path, original) {
            Ok(stored) => report.backups.push(stored),
            Err(error) => {
                report
                    .warnings
                    .push(format!("{}: {error}; left as it was", relative(path)));
                return false;
            }
        }
    }
    // The managed env file can be the first thing a recipe ever puts in a
    // profile's `custom/` directory, so the directory is made rather than
    // reported as a missing parent.
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        report
            .warnings
            .push(format!("cannot create {}: {error}", relative(parent)));
        return false;
    }
    if let Err(error) = std::fs::write(path, rewritten) {
        report
            .warnings
            .push(format!("cannot rewrite {}: {error}", relative(path)));
        return false;
    }
    report.files_written.push(relative(path));
    true
}

/// Writes the recipe's `[env]` as a managed block in the profile's own
/// `custom/env.lua`.
///
/// The file is inside the profile, so this is an ordinary profile file: the
/// pre-change bytes are kept content-addressed like any other, and a pass that
/// would leave it identical writes nothing at all — which is the case on every
/// switch after the first. An unterminated block is refused, and reported.
fn materialize_env(
    report: &mut Report,
    backups: &Path,
    profile: &Path,
    context: &Context,
    layers: &recipe::Layers,
) -> Option<EnvBlock> {
    let env = layers.env();
    if env.is_empty() {
        return None;
    }
    let path = profile
        .join(".config")
        .join("hypr")
        .join("custom")
        .join("env.lua");
    let file = relative(&path);
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            report
                .warnings
                .push(format!("cannot read {}: {error}", file));
            return None;
        }
    };
    let before = existing.as_deref().unwrap_or_default();
    let block = recipe::env_block(&env, context.home());
    let spliced = match recipe::splice_env(before, &block) {
        Ok(spliced) => spliced,
        Err(reason) => {
            report.warnings.push(format!("{file}: {reason}"));
            return None;
        }
    };
    let entries: Vec<(String, String)> = env.into_iter().collect();
    let written = spliced != before;
    if !written {
        return Some(EnvBlock {
            file,
            entries,
            written,
        });
    }
    let kept = write(
        report,
        backups,
        &path,
        existing.as_deref().map(str::as_bytes),
        &spliced,
    );
    Some(EnvBlock {
        file,
        entries,
        written: kept,
    })
}

/// Guarantees the recipe's declared directories and symlinks.
///
/// The outside-the-profile write budget of this whole layer: every path was
/// bounded against `$HOME` at parse time, and a path that is already correct
/// costs nothing.
fn materialize_dirs(report: &mut Report, context: &Context, layers: &recipe::Layers) {
    for dir in layers.dirs() {
        match recipe::guarantee(context.home(), &dir) {
            recipe::Guaranteed::Created(path) => {
                report
                    .dirs_created
                    .push(home_relative(context.home(), &path));
            }
            recipe::Guaranteed::AlreadyCorrect => {}
            recipe::Guaranteed::Refused(why) | recipe::Guaranteed::Failed(why) => {
                report.warnings.push(why);
            }
        }
    }
}

/// One splice the pass would make in a config file.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Edit {
    /// The bytes it replaces. An empty replacement is a removal.
    span: Span,
    /// What goes there.
    replacement: String,
    /// The dispatch it answers, named if a later edit has to be refused for
    /// overlapping it.
    entry: String,
}

/// A byte range in a file's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Span {
    start: usize,
    end: usize,
}

/// Applies the planned edits, leaving every other byte of the file where it is.
///
/// Two edits that overlap are not merged and not applied in some order: the
/// earlier one lands, the later is reported and dropped, because a rewrite this
/// engine cannot reason about end to end is one it should not make.
fn apply(text: &str, mut edits: Vec<Edit>, refused: &mut Vec<String>) -> String {
    edits.sort_by_key(|edit| (edit.span.start, edit.span.end));
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for edit in edits {
        if edit.span.start < cursor {
            refused.push(format!(
                "`{}` and another change to the same statement overlap; the second was not made",
                edit.entry
            ));
            continue;
        }
        out.push_str(&text[cursor..edit.span.start]);
        out.push_str(&edit.replacement);
        cursor = edit.span.end;
    }
    out.push_str(&text[cursor..]);
    out
}

/// How many brackets deep a resolution is willing to reach for the statement it
/// belongs to. Two is what real rice nests: `hl.bind(…, hl.dsp.global(…), {…})`
/// is one level, and a gesture's
/// `hl.gesture({…, action = function() hl.dispatch(…) end })` two. A third
/// level is a submap or a table the pass cannot bound, and it refuses.
const NESTING: usize = 2;

/// The splices one resolution asks for on one dispatch site: the entry the
/// dispatch became, and the edits that make it so.
///
/// Every failure is an `Err` naming why, and an `Err` is always the whole
/// answer — the bind is then left byte for byte as it was, and the reason is
/// reported. Nothing about a resolution is applied "as far as it goes".
fn plan(
    text: &str,
    site: &Site,
    decision: &recipe::Decision,
    registry: &Registry,
    notes: &mut Vec<String>,
) -> Result<(String, Vec<Edit>), String> {
    match decision {
        recipe::Decision::To { target, options } => {
            let entry = target_entry(target, registry)?;
            let mut edits = vec![edit(token_of(site), entry.clone())];
            // A bind option that cannot be placed is a note, not a refusal: the
            // shortcut itself is the semantic that matters, and a `to` the user
            // wrote by hand in a gesture is still correct without it.
            match bind_options(text, site, options) {
                Ok(Some(insert)) => edits.push(insert),
                Ok(None) => {}
                Err(why) => notes.push(format!(
                    "the recipe's bind options for `{}` were not applied: {why}",
                    site.entry()
                )),
            }
            Ok((entry, edits))
        }
        recipe::Decision::Exec { command } => {
            let (span, replacement) = exec_form(text, site, command)?;
            Ok((String::new(), vec![edit(span, replacement)]))
        }
        recipe::Decision::Drop => Ok((
            String::new(),
            vec![edit(drop_span(text, site)?, String::new())],
        )),
    }
}

/// The dispatch token itself — the smallest thing a `to` can rewrite.
fn token_of(site: &Site) -> Span {
    Span {
        start: site.start,
        end: site.end,
    }
}

/// A splice, with the dispatch it answers filled in by the caller once the
/// entry is known.
fn edit(span: Span, replacement: String) -> Edit {
    Edit {
        span,
        replacement,
        entry: String::new(),
    }
}

/// An insertion: an empty span and the text that fills it.
fn insert(at: usize, text: String) -> Edit {
    edit(Span { start: at, end: at }, text)
}

/// The entry a recipe's `to` names, resolved against the derived registry and
/// checked against it.
///
/// The check is the whole point of the tier: a recipe may be authored against a
/// shell version that registered a shortcut this one does not, and writing the
/// name anyway installs a bind that can only ever fail. So an unproved target
/// is reported and the line is left alone.
fn target_entry(target: &str, registry: &Registry) -> Result<String, String> {
    let entry = match target.split_once(':') {
        Some((appid, name)) if !appid.is_empty() && !name.is_empty() => format!("{appid}:{name}"),
        Some(_) => return Err(format!("`{target}` is not an `appid:name` shortcut")),
        // Unprefixed: the derived namespace, never the recipe's pin.
        None => format!("{}:{target}", registry.appid),
    };
    let (appid, name) = entry.split_once(':').expect("joined above");
    if registry.holds(appid, name) {
        Ok(entry)
    } else {
        Err(format!(
            "the recipe points it at `{entry}`, which this profile's own shell does not register"
        ))
    }
}

/// The edits that add `options` to the bind this dispatch sits in — the
/// hand-fix's `release = true`, in the form the schema can declare.
///
/// The option table is the bind's own, so an option the line already sets is
/// left alone: the recipe describes the bind the hand-fix wrote, and a user who
/// has changed it since keeps their change.
fn bind_options(text: &str, site: &Site, options: &[String]) -> Result<Option<Edit>, String> {
    if options.is_empty() {
        return Ok(None);
    }
    let call = dispatch_call(text, site)
        .ok_or("it is not inside a quoted `global(…)` call".to_string())?;
    let bind = enclosing_call(text, call.span, &["bind"]).ok_or(
        "it is not in a `bind(…)` statement, so a bind option has nowhere to go".to_string(),
    )?;
    let assignments = options
        .iter()
        .map(|option| format!("{option} = true"))
        .collect::<Vec<String>>()
        .join(", ");
    let arguments = arguments(text, bind);
    match arguments.len() {
        0..=1 => Err("a `bind(…)` with fewer than two arguments has no option table".to_string()),
        2 => Ok(Some(insert(bind.end - 1, format!(", {{ {assignments} }}")))),
        _ => {
            let table = table_at(text, arguments[2])
                .ok_or("its third argument is not an option table".to_string())?;
            if options.iter().any(|option| sets(text, table, option)) {
                return Ok(None); // Already what the recipe declares.
            }
            let body = &text[table.start + 1..table.end - 1];
            let empty = body.trim().is_empty();
            // After whatever space the table already had, so `{ release = true,
            // description = … }` never grows a doubled separator.
            let at = table.start + 1 + (body.len() - body.trim_start().len());
            Ok(Some(insert(
                at,
                if empty {
                    format!("{assignments} ")
                } else {
                    format!("{assignments}, ")
                },
            )))
        }
    }
}

/// The expression a dead dispatch becomes under an `exec` resolution, in the
/// dialect of the file it lands in.
fn exec_form(text: &str, site: &Site, command: &str) -> Result<(Span, String), String> {
    // The Lua dialect: `hl.dsp.global("ns:name")` becomes
    // `hl.dsp.exec_cmd("…")`. The same form also carries inside a plain conf's
    // shell string, which is what this Hyprland evaluates — it reads `dispatch`
    // arguments as Lua — so a resolution lands in the file's own idiom without
    // the pass having to know which file it is in.
    if let Some(call) = dispatch_call(text, site) {
        let Some((head, tail)) = text[call.callee.start..call.callee.end].rsplit_once('.') else {
            return Err("it is not a qualified `…global(…)` call".to_string());
        };
        if tail != KEYWORD {
            return Err("it is not a `…global(…)` call".to_string());
        }
        return Ok((
            call.span,
            format!("{head}.exec_cmd({})", recipe::lua_string(command)),
        ));
    }
    // The plain-conf dialect: the shell command the dispatch is the tail of, or
    // the `global` keyword a `bind =` line dispatches with.
    match conf_dispatch(text, site) {
        Some(ConfDispatch::Command(span)) => Ok((span, command.to_string())),
        Some(ConfDispatch::Keyword(span)) => Ok((span, format!("exec, {command}"))),
        None => Err("it is not a dispatch this can replace with a command".to_string()),
    }
}

/// Where a dropped dispatch takes its whole statement with it.
///
/// A drop removes a *statement*, not a token: a bind spread over two lines, or a
/// gesture whose only content is the dispatch, is one statement and goes as
/// one. So the span widens out to the enclosing call — `bind(`, `gesture(`, or
/// the bare `hl.dispatch(` — and then to the whole lines that call spans, which
/// is only allowed when the call both begins its line and ends it. A dispatch
/// this cannot bound is refused rather than half-removed.
fn drop_span(text: &str, site: &Site) -> Result<Span, String> {
    const UNBOUND: &str = "it is not in a statement this can remove whole";
    if let Some(call) = dispatch_call(text, site) {
        let statement = enclosing_call(text, call.span, &["bind", "gesture", "action", "dispatch"])
            .map(|pair| call_expression(text, pair))
            .ok_or_else(|| UNBOUND.to_string())?;
        if !bounded(text, statement) {
            return Err(UNBOUND.to_string());
        }
        return line_span(text, statement).ok_or_else(|| UNBOUND.to_string());
    }
    // The plain-conf dialect is a line-oriented one: `bind = KEYS, global,
    // ns:name` declares exactly one thing on its line, and that line is the
    // bind — so the line goes, whole.
    conf_dispatch(text, site).ok_or_else(|| UNBOUND.to_string())?;
    line_span(text, token_of(site)).ok_or_else(|| UNBOUND.to_string())
}

/// Whether removing `span` cannot take a neighbour with it: the statement
/// begins its line, and nothing but whitespace or a comment shares the ends of
/// it with anything else.
fn bounded(text: &str, span: Span) -> bool {
    let line_start = text[..span.start].rfind('\n').map_or(0, |at| at + 1);
    let line_end = text[span.end..]
        .find('\n')
        .map_or(text.len(), |at| span.end + at);
    text[line_start..span.start].trim().is_empty() && trailing_is_comment(&text[span.end..line_end])
}

/// The lines a span sits on, from the start of the first to the start of the one
/// after the last. A deletion over this range takes the whole statement, its
/// indentation and its trailing comment with it.
fn line_span(text: &str, span: Span) -> Option<Span> {
    let start = text[..span.start].rfind('\n').map_or(0, |at| at + 1);
    let end = text[span.end..]
        .find('\n')
        .map(|at| span.end + at + 1)
        .unwrap_or(text.len());
    Some(Span { start, end })
}

/// Whether what follows a statement on its line is only a comment.
fn trailing_is_comment(after: &str) -> bool {
    let trimmed = after.trim_start();
    trimmed.is_empty() || trimmed.starts_with("--") || trimmed.starts_with('#')
}

// ------------------------------------------------------------- the dialects

/// A call a dispatch sits in, reduced to what a rewrite needs: the whole
/// expression, and the identifier chain that names it.
#[derive(Clone, Copy, Debug)]
struct Call {
    /// `…global("ns:name")` in full — quoting and parentheses included, so a
    /// dialect rewrite replaces the expression instead of splicing into it.
    span: Span,
    /// The identifier chain, `hl.dsp.global`.
    callee: Span,
}

/// The `…global("ns:name")` call a dispatch token sits inside, when it is inside
/// one.
fn dispatch_call(text: &str, site: &Site) -> Option<Call> {
    let bytes = text.as_bytes();
    let mut start = site.start;
    if bytes
        .get(start.wrapping_sub(1))
        .is_some_and(|byte| is_quote(*byte))
    {
        start -= 1;
    }
    start = trim_back(bytes, start);
    if bytes.get(start.wrapping_sub(1)) != Some(&b'(') {
        return None;
    }
    let callee = callee_before(text, start - 1)?;
    let mut end = site.end;
    if bytes.get(end).is_some_and(|byte| is_quote(*byte)) {
        end += 1;
    }
    end = trim_forward(bytes, end);
    if bytes.get(end) != Some(&b')') {
        return None;
    }
    Some(Call {
        span: Span {
            start: callee.start,
            end: end + 1,
        },
        callee,
    })
}

/// A call's whole expression, name included: the bracket pair `hl.bind(…)` is
/// only its arguments, and a statement begins at the name.
fn call_expression(text: &str, pair: Span) -> Span {
    match callee_before(text, pair.start) {
        Some(chain) => Span {
            start: chain.start,
            end: pair.end,
        },
        None => pair,
    }
}

/// The identifier chain whose call opens at `open` — `hl.dsp.global` for
/// `hl.dsp.global(`.
///
/// An argument list is what encloses the chain, not part of it: a `(` preceded
/// by `, ` or `= ` names no function, and the pass reads that as "there is no
/// call here" rather than inventing one.
fn callee_before(text: &str, open: usize) -> Option<Span> {
    let bytes = text.as_bytes();
    let end = trim_back(bytes, open);
    let mut start = end;
    while start > 0 && (is_word(bytes[start - 1]) || bytes[start - 1] == b'.') {
        start -= 1;
    }
    (start < end).then_some(Span { start, end })
}

/// How a plain-conf line spells a dispatch, when it spells one at all.
enum ConfDispatch {
    /// `… hyprctl dispatch global ns:name`: a shell command that ends in a
    /// dispatch, where the whole run becomes the command.
    Command(Span),
    /// `bind = KEYS, global, ns:name` — or the same line already on `exec`:
    /// a dispatcher keyword followed by the name, where the keyword is kept and
    /// the name is what the command replaces.
    Keyword(Span),
}

/// The plain-conf spelling of a dispatch token, in either of its two forms.
///
/// The words immediately before the token are what tells them apart, and the
/// distinction matters: one is a shell command that ends in a dispatch, the other
/// is a `bind =` line whose *dispatcher* is the word `global`.
fn conf_dispatch(text: &str, site: &Site) -> Option<ConfDispatch> {
    let line_start = text[..site.start].rfind('\n').map_or(0, |at| at + 1);
    let words = trailing_words(
        &text.as_bytes()[line_start..site.start],
        site.start - line_start,
        3,
    );
    let at = |index: usize| line_start + words[index].0;
    // In reading order, so the dispatcher's own `global` is the *last* word.
    match words
        .iter()
        .map(|(_, word)| word.as_str())
        .collect::<Vec<_>>()
        .as_slice()
    {
        // `hyprctl dispatch global ns:name`, or `dispatch global ns:name`.
        ["hyprctl", "dispatch", "global", ..] => Some(ConfDispatch::Command(Span {
            start: at(0),
            end: site.end,
        })),
        ["dispatch", "global", ..] => Some(ConfDispatch::Command(Span {
            start: at(0),
            end: site.end,
        })),
        // `bind = KEYS, global, ns:name`.
        [.., "global"] => Some(ConfDispatch::Keyword(Span {
            start: at(words.len() - 1),
            end: site.end,
        })),
        // `bind = KEYS, exec, ns:name`: the dispatcher is already the one a
        // command wants, and only the name changes.
        [.., "exec"] => Some(ConfDispatch::Keyword(Span {
            start: at(words.len() - 1),
            end: site.end,
        })),
        _ => None,
    }
}

/// The last `count` words before `at`, with their offsets, in reading order.
fn trailing_words(bytes: &[u8], at: usize, count: usize) -> Vec<(usize, String)> {
    let mut words = Vec::new();
    let mut cursor = at;
    for _ in 0..count {
        while cursor > 0 && (bytes[cursor - 1].is_ascii_whitespace() || bytes[cursor - 1] == b',') {
            cursor -= 1;
        }
        let end = cursor;
        while cursor > 0 && is_word(bytes[cursor - 1]) {
            cursor -= 1;
        }
        if cursor == end {
            break;
        }
        words.push((
            cursor,
            String::from_utf8_lossy(&bytes[cursor..end]).into_owned(),
        ));
    }
    words.reverse();
    words
}

/// The nearest enclosing call of one of `names`, within [`NESTING`] levels.
///
/// Narrowing on the name is what keeps this from being a general-purpose
/// statement finder: the pass only ever needs the bind a dispatch belongs to and
/// the gesture or action that wraps one, and a submap three levels up is not
/// something it will touch.
fn enclosing_call(text: &str, span: Span, names: &[&str]) -> Option<Span> {
    let mut current = span;
    for _ in 0..NESTING {
        let outer = enclosing_pair(text, current)?;
        let chain = callee_before(text, outer.start)?;
        let tail = text[chain.start..chain.end]
            .rsplit('.')
            .next()
            .unwrap_or_default();
        if names.contains(&tail) {
            return Some(outer);
        }
        current = outer;
    }
    None
}

/// The bracket pair that most tightly encloses `span`, strings and comments
/// skipped. `None` when nothing encloses it, or when the brackets before it do
/// not balance — in which case the caller refuses rather than guess.
fn enclosing_pair(text: &str, span: Span) -> Option<Span> {
    let bytes = text.as_bytes();
    let mut stack: Vec<usize> = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'"' | b'\'' => {
                index = skip_string(bytes, index);
                continue;
            }
            b'-' | b'/' if matches!(bytes.get(index + 1), Some(b'-') | Some(b'/')) => {
                index = skip_to_newline(bytes, index);
                continue;
            }
            b'#' => {
                index = skip_to_newline(bytes, index);
                continue;
            }
            b'(' | b'{' | b'[' => stack.push(index),
            b')' | b'}' | b']' => {
                // An unmatched closer is a file this parse cannot bound, and
                // the caller refuses rather than guess where the statements are.
                let open = stack.pop()?;
                if index >= span.end && open < span.start {
                    return Some(Span {
                        start: open,
                        end: index + 1,
                    });
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

/// A Lua-ish string literal's end, from its opening quote.
fn skip_string(bytes: &[u8], open: usize) -> usize {
    let quote = bytes[open];
    let mut index = open + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 1,
            byte if byte == quote => return index + 1,
            _ => {}
        }
        index += 1;
    }
    bytes.len()
}

/// The next byte after the comment a marker opens.
fn skip_to_newline(bytes: &[u8], at: usize) -> usize {
    bytes[at..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(bytes.len(), |found| at + found)
}

/// A call's arguments, as trimmed spans: the keys, the dispatcher, the option
/// table, and whatever else the dialect takes.
///
/// `call` is the *bracket pair* — the span that starts at the `(` — which is
/// what [`enclosing_call`] hands back, and what makes the offsets below the
/// inside of the parentheses.
fn arguments(text: &str, call: Span) -> Vec<Span> {
    let bytes = text.as_bytes();
    let inner = Span {
        start: call.start + 1,
        end: call.end.saturating_sub(1),
    };
    let mut found = Vec::new();
    let mut depth = 0usize;
    let mut start = inner.start;
    let mut index = inner.start;
    while index < inner.end {
        match bytes[index] {
            b'"' | b'\'' => {
                index = skip_string(bytes, index);
                continue;
            }
            b'(' | b'{' | b'[' => depth += 1,
            b')' | b'}' | b']' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                push_trimmed(text, &mut found, start, index);
                start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }
    push_trimmed(text, &mut found, start, inner.end);
    found
}

fn push_trimmed(text: &str, into: &mut Vec<Span>, start: usize, end: usize) {
    let span = trim_range(text, start, end);
    if span.start < span.end {
        into.push(span);
    }
}

/// The braces of an argument that is a table, when it is one.
fn table_at(text: &str, argument: Span) -> Option<Span> {
    let trimmed = trim_range(text, argument.start, argument.end);
    let bytes = text.as_bytes();
    (bytes.get(trimmed.start) == Some(&b'{') && bytes.get(trimmed.end - 1) == Some(&b'}'))
        .then_some(Span {
            start: trimmed.start,
            end: trimmed.end,
        })
}

/// Whether a table already assigns `option` — as `option =` or `option=`, which
/// is how every bind option in these files is written.
fn sets(text: &str, table: Span, option: &str) -> bool {
    let body = &text[table.start + 1..table.end - 1];
    let mut from = 0;
    while let Some(found) = body[from..].find(option) {
        let at = from + found;
        let after = body[at + option.len()..].trim_start();
        if after.starts_with('=') {
            return true;
        }
        from = at + option.len();
    }
    false
}

fn is_quote(byte: u8) -> bool {
    matches!(byte, b'"' | b'\'')
}

/// Skips whitespace backwards, returning the offset just after the last
/// non-whitespace byte before `at`.
fn trim_back(bytes: &[u8], at: usize) -> usize {
    let mut cursor = at;
    while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
        cursor -= 1;
    }
    cursor
}

/// Skips whitespace forwards from `at`.
fn trim_forward(bytes: &[u8], at: usize) -> usize {
    let mut cursor = at;
    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }
    cursor
}

/// A range with its surrounding whitespace trimmed off.
fn trim_range(text: &str, start: usize, end: usize) -> Span {
    let bytes = text.as_bytes();
    let start = {
        let mut cursor = start;
        while cursor < end && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        cursor
    };
    let mut end = end;
    while end > start && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    Span { start, end }
}

/// The activating shell's shortcut registry, derived from its own QML.
struct Registry {
    /// Every provable registration, as `appid:name`.
    entries: BTreeSet<String>,
    /// Names to the appids that register them — the join an exact-name move
    /// reads. A name under two appids is deliberately unjoinable: there is no
    /// single provable target, so nothing moves.
    by_name: BTreeMap<String, BTreeSet<String>>,
    /// The appid the tree registers under.
    appid: String,
    /// The path the tree was expected at, named in `skipped`.
    tree: PathBuf,
    /// The tree itself, when it is there.
    root: Option<PathBuf>,
    /// Findings the parse could not resolve.
    uncertain: Vec<String>,
    /// Why there is no registry at all, when there is none.
    skipped: Option<String>,
}

impl Registry {
    /// Walks the profile's QML tree for the shell and collects its
    /// registrations.
    fn derive(profile: &Path, shell: &str) -> Registry {
        let tree = profile.join(".config").join("quickshell").join(shell);
        let mut registry = Registry {
            entries: BTreeSet::new(),
            by_name: BTreeMap::new(),
            appid: DEFAULT_APPID.to_string(),
            root: tree.is_dir().then(|| tree.clone()),
            tree,
            uncertain: Vec::new(),
            skipped: None,
        };
        let Some(root) = registry.root.clone() else {
            registry.skipped = Some(format!(
                "the profile carries no QML tree at {}, so no registry could be derived",
                registry.tree.display()
            ));
            return registry;
        };

        let mut files = Vec::new();
        collect(&root, &mut files, &mut |path| {
            path.extension().and_then(|found| found.to_str()) == Some("qml")
        });
        files.sort();

        // The shell-wide appid. A Quickshell config registers under one appid,
        // and a shell that renames it says so once — in the base component every
        // registration inherits from (caelestia's `CustomShortcut.qml`), not in
        // each of the two dozen blocks that use it. So an appid declared
        // anywhere in the tree is the tree's appid, and Quickshell's default
        // applies only when the tree declares none. Two different declarations
        // are a tree this parse cannot reason about: the registrations that do
        // not name an appid themselves stop being provable.
        let mut declared: BTreeSet<String> = BTreeSet::new();
        let mut blocks: Vec<(String, Block)> = Vec::new();
        for file in &files {
            let Ok(text) = std::fs::read_to_string(file) else {
                registry
                    .uncertain
                    .push(format!("{}: could not be read", relative(file)));
                continue;
            };
            for block in blocks_of(&text) {
                if let Property::Literal(appid) = &block.appid {
                    declared.insert(appid.clone());
                }
                blocks.push((relative(file), block));
            }
        }
        let unambiguous = declared.len() <= 1;
        if unambiguous {
            // The one appid the tree names, or Quickshell's own default.
            registry.appid = declared
                .iter()
                .next()
                .cloned()
                .unwrap_or_else(|| DEFAULT_APPID.to_string());
        } else {
            registry.uncertain.push(format!(
                "{}: declares more than one shortcut appid ({}), so a registration that does \
                 not name one is not provable",
                relative(&root),
                declared.iter().cloned().collect::<Vec<_>>().join(", ")
            ));
        }

        for (file, block) in blocks {
            let name = match block.name {
                Property::Literal(name) => name,
                Property::Dynamic => {
                    registry.uncertain.push(format!(
                        "{file}: a shortcut's name is built at runtime, so it is not a \
                         registration this engine can prove"
                    ));
                    continue;
                }
                Property::Absent => continue,
            };
            let appid = match block.appid {
                Property::Literal(appid) => appid,
                _ if unambiguous => registry.appid.clone(),
                _ => {
                    registry.uncertain.push(format!(
                        "{file}: shortcut `{name}` does not name an appid and the tree's is \
                         ambiguous, so the registration is not provable"
                    ));
                    continue;
                }
            };
            registry.entries.insert(format!("{appid}:{name}"));
            registry.by_name.entry(name).or_default().insert(appid);
        }
        registry
    }

    /// Whether `appid:name` is registered as it stands — the alive answer.
    fn holds(&self, appid: &str, name: &str) -> bool {
        self.entries.contains(&format!("{appid}:{name}"))
    }

    /// The one appid registering `name`, when there is exactly one. Two is not
    /// one: an ambiguous target is no target, and the name stays a proposal.
    fn provable_appid(&self, name: &str) -> Option<&str> {
        match self.by_name.get(name) {
            Some(appids) if appids.len() == 1 => appids.iter().next().map(String::as_str),
            _ => None,
        }
    }
}

/// One `GlobalShortcut`/`CustomShortcut` declaration, reduced to the two
/// properties this engine reads.
struct Block {
    name: Property,
    appid: Property,
}

/// A property the parse either proved, found computed at runtime, or never saw.
#[derive(Debug)]
enum Property {
    Absent,
    Literal(String),
    Dynamic,
}

/// A dispatched `ns:name` token, and where it sits in the text it came from.
struct Site {
    appid: String,
    name: String,
    start: usize,
    end: usize,
    /// The 1-based line the token is on, which is what a proposal has to name:
    /// the recipe tier resolves a name per line, so "this name is dead somewhere
    /// in this file" is not a question it can answer.
    line: usize,
}

impl Site {
    fn entry(&self) -> String {
        format!("{}:{}", self.appid, self.name)
    }
}

/// Everything one text scan found: the dispatch sites, and the dispatches built
/// at runtime that no amount of reading can pin down.
#[derive(Default)]
struct Scan {
    sites: Vec<Site>,
    dynamic: Vec<String>,
}

/// Finds every dispatched `ns:name` in one config file.
///
/// The forms are the ones real rice use: the Lua call
/// `hl.dsp.global("quickshell:lock")` (either quote), the same string carried
/// inside a plain-conf shell command, the unquoted
/// `dispatch … global(quickshell:lock)`, and the bare `hyprctl dispatch global
/// quickshell:lock`. Comments are masked first, so a line that *talks about* a
/// dispatch is never rewritten.
fn scan(text: &str) -> Scan {
    let mut scan = Scan::default();
    let masked = comments(text);
    let bytes = text.as_bytes();
    let mut index = 0;
    while let Some(found) = text[index..].find(KEYWORD) {
        let start = index + found;
        index = start + KEYWORD.len();
        if start > 0 && is_word(bytes[start - 1]) {
            continue;
        }
        if bytes.get(index).is_some_and(|byte| is_word(*byte)) || masked[start] {
            continue;
        }
        let mut cursor = index;
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        let parenthesized = bytes.get(cursor) == Some(&b'(');
        if parenthesized {
            cursor += 1;
            while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
                cursor += 1;
            }
        } else if bytes.get(cursor) == Some(&b',') {
            // A plain conf line separates its words with commas, and the
            // dispatcher is one of them: `bind = KEYS, global, ns:name`. The
            // recipe tier answers that form, so the scan has to see it.
            cursor += 1;
            while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
                cursor += 1;
            }
        }
        if matches!(bytes.get(cursor), Some(b'"') | Some(b'\'')) {
            cursor += 1;
        }
        let token_start = cursor;
        while bytes.get(cursor).is_some_and(|byte| is_token(*byte)) {
            cursor += 1;
        }
        let token = &text[token_start..cursor];
        match recipe::split_entry(token) {
            Some((appid, name)) => scan.sites.push(Site {
                appid: appid.to_string(),
                name: name.to_string(),
                start: token_start,
                end: cursor,
                line: 0,
            }),
            // `global(` with something the parse cannot read is a dispatch it
            // refuses to guess at: reported, never rewritten.
            None if parenthesized && !token.is_empty() => scan.dynamic.push(token.to_string()),
            None => {}
        }
    }
    scan.sites.sort_by_key(|site| site.start);
    number_lines(text, &mut scan.sites);
    scan
}

/// Numbers each site with the line it sits on, in one forward pass over the
/// text.
///
/// The sites arrive in ascending byte order, so a single cursor through the
/// text serves them all — the count is one pass, not one per site, and the file
/// is read exactly once.
fn number_lines(text: &str, sites: &mut [Site]) {
    let mut line = 1usize;
    let mut walked = 0usize;
    for site in sites {
        line += text[walked..site.start].matches('\n').count();
        walked = site.start;
        site.line = line;
    }
}

/// The dispatcher keyword a global dispatch is spelled with.
const KEYWORD: &str = "global";

/// Marks every byte that sits inside a comment, so a scan skips it.
///
/// `--` (Lua) and `#` (plain conf) open a comment anywhere outside a quoted
/// string, and `//` does too, since a `.conf` may carry either. Strings are
/// tracked, so a colour like `"#RRGGBB"` or a `--` inside a message stays text.
fn comments(text: &str) -> Vec<bool> {
    let bytes = text.as_bytes();
    let mut masked = vec![false; bytes.len()];
    let mut index = 0;
    let mut quote: Option<u8> = None;
    let mut in_comment = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(open) = quote {
            if byte == b'\\' {
                index += 2;
                continue;
            }
            if byte == open {
                quote = None;
            }
            index += 1;
            continue;
        }
        match byte {
            b'"' | b'\'' => quote = Some(byte),
            b'#' => {
                in_comment = true;
                masked[index] = true;
                index += 1;
                continue;
            }
            b'-' | b'/' if matches!(bytes.get(index + 1), Some(&b'-') | Some(&b'/')) => {
                in_comment = true;
                index += 2;
                continue;
            }
            _ => {}
        }
        masked[index] = in_comment;
        if byte == b'\n' {
            in_comment = false;
        }
        index += 1;
    }
    masked
}

/// Every `*.lua` and `*.conf` under the profile's Hyprland config, in a stable
/// order.
///
/// The extension is the whole filter, and that is deliberate: a captured rice
/// keeps scratch siblings beside the files it loads (`hypridle.conf.new`,
/// `general.lua.bak`, `general.lua.save`) and Hyprland reads none of them.
/// Rewriting a scratch file would put the edit where nobody can see it land.
fn configs_in_scope(hypr: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect(hypr, &mut found, &mut |path| {
        matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("lua") | Some("conf")
        )
    });
    found.sort();
    found
}

/// Reads the files the configs `exec` and reports the dispatches inside them.
///
/// A helper script is a different kind of file — shell quoting, expansion, no
/// `dsp.global` to speak of — and a blind edit into one is how a config becomes
/// a syntax error nobody sees until the key stops working. So the engine looks,
/// and reports.
///
/// This is a sweep of its own, and it re-reads: the configs are read a second
/// time here for their `exec` lines, and each resolved script is read once for
/// its own dispatches. Nothing it produces is fed back into the rewrite — the
/// rewrite works from the pass's own `Scan` — so the second read is a report
/// cost, never a second chance to touch a file. It happens after the rewrite
/// rather than alongside it, so the bytes this reports on are the bytes the
/// pass leaves behind.
fn scan_externals(
    profile: &Path,
    hypr: &Path,
    configs: &[PathBuf],
    registry: &Registry,
    keeps: &BTreeSet<String>,
) -> Vec<External> {
    let mut found: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
    for config in configs {
        let Ok(text) = std::fs::read_to_string(config) else {
            continue;
        };
        for token in script_tokens(&text) {
            let Some(script) = resolve(profile, hypr, &token) else {
                continue;
            };
            if configs.contains(&script) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&script) else {
                continue;
            };
            let dispatched: BTreeSet<String> = scan(&text)
                .sites
                .iter()
                .map(Site::entry)
                .filter(|entry| {
                    !keeps.contains(entry)
                        && !FOREIGN.contains(&entry.as_str())
                        && !registry.holds(recipe::appid_of(entry), recipe::name_of(entry))
                })
                .collect();
            if !dispatched.is_empty() {
                found.entry(script).or_default().extend(dispatched);
            }
        }
    }
    found
        .into_iter()
        .map(|(path, dispatched)| External {
            path: relative(&path),
            dispatched: dispatched.into_iter().collect(),
        })
        .collect()
}

/// The script paths an `exec` line names, with `$HOME` and `~` already off.
fn script_tokens(text: &str) -> Vec<String> {
    let masked = comments(text);
    let mut tokens = Vec::new();
    for line in text.lines() {
        let offset = line.as_ptr() as usize - text.as_ptr() as usize;
        if !(line.contains("exec") || line.contains("$HOME") || line.contains('~')) {
            continue;
        }
        let bytes = line.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            if !(bytes[index].is_ascii_alphabetic() || bytes[index] == b'/' || bytes[index] == b'~')
            {
                index += 1;
                continue;
            }
            let start = index;
            while index < bytes.len()
                && (is_token(bytes[index]) || matches!(bytes[index], b'/' | b'~'))
            {
                index += 1;
            }
            if masked[offset + start] {
                continue;
            }
            let token = line[start..index].trim_start_matches(['/', '~']);
            if token.ends_with(".sh") {
                tokens.push(token.to_string());
            }
        }
    }
    tokens
}

/// Where a token an `exec` line names lives inside the profile's own tree.
///
/// The live tree is symlinked into `$HOME`, so a script is referenced by its
/// `$HOME` path while the engine reads the profile. `$HOME`-relative and
/// hypr-relative readings are tried first, then the basename anywhere under the
/// Hyprland config, which is what a Lua `hyprScripts .. "/x.sh"` concatenation
/// leaves behind. A token that is not a path at all (`pkill qs`) resolves to
/// nothing.
fn resolve(profile: &Path, hypr: &Path, token: &str) -> Option<PathBuf> {
    let relative = token
        .strip_prefix(".config/")
        .or_else(|| token.strip_prefix("config/"))
        .map(|rest| format!(".config/{rest}"))
        .unwrap_or_else(|| token.to_string());
    for candidate in [profile.join(&relative), hypr.join(&relative)] {
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let name = Path::new(&relative).file_name()?;
    let mut scripts = Vec::new();
    collect(hypr, &mut scripts, &mut |path| {
        path.file_name().is_some_and(|found| found == name)
    });
    scripts.sort();
    scripts.into_iter().next()
}

/// Keeps the pre-rewrite bytes of a file the engine is about to touch, named by
/// what they are: `backups/<content fingerprint>.<filename>`. The same content
/// always lands on the same name, so a second pass over an already-repaired
/// profile finds its backup already there and writes nothing.
fn store_backup(backups: &Path, path: &Path, original: &[u8]) -> Result<String, String> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config".to_string());
    let fingerprint = content_hash(original);
    let stored = backups.join(format!("{fingerprint}.{name}"));
    let reported = format!("backups/{fingerprint}.{name}");
    if stored.exists() {
        return Ok(reported);
    }
    std::fs::create_dir_all(backups)
        .map_err(|error| format!("cannot create {}: {error}", backups.display()))?;
    // Staged and renamed, the primitive the switch's own backups use: a crash
    // mid-write must not leave a truncated file where the config used to be.
    let staged = backups.join(format!("{fingerprint}.{name}.riceswap-staged"));
    let _ = std::fs::remove_file(&staged);
    std::fs::write(&staged, original)
        .map_err(|error| format!("cannot stage the backup at {}: {error}", staged.display()))?;
    if let Err(error) = std::fs::rename(&staged, &stored) {
        let _ = std::fs::remove_file(&staged);
        return Err(format!(
            "cannot store the backup at {}: {error}",
            stored.display()
        ));
    }
    Ok(reported)
}

/// The content fingerprint a backup is named by: FNV-1a, 64-bit, hex.
///
/// Deliberately not a cryptographic digest. Nothing here authenticates anything
/// — the name only has to say *which bytes* a backup holds, and it has to say
/// it identically on every machine and every release, which is exactly what a
/// hand-rolled FNV gives. See the module tests for the known-answer vectors.
fn content_hash(bytes: &[u8]) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    format!("{hash:016x}")
}

/// The shortcut declarations of one QML file, each reduced to the properties
/// this engine reads.
fn blocks_of(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    for keyword in ["GlobalShortcut", "CustomShortcut"] {
        let mut index = 0;
        while let Some(found) = text[index..].find(keyword) {
            let start = index + found;
            index = start + keyword.len();
            if start > 0 && is_word(text.as_bytes()[start - 1]) {
                continue;
            }
            let mut cursor = index;
            while text
                .as_bytes()
                .get(cursor)
                .is_some_and(u8::is_ascii_whitespace)
            {
                cursor += 1;
            }
            if text.as_bytes().get(cursor) != Some(&b'{') {
                continue; // A type reference, not a declaration.
            }
            let Some(body) = balanced(text, cursor + 1) else {
                break; // Unterminated: nothing after it can be trusted either.
            };
            blocks.push(properties(&text[body.0..body.1]));
        }
    }
    blocks
}

/// The end of a `{ … }` body opened just before `start`, skipping strings and
/// comments so a brace inside a string does not close the block.
fn balanced(text: &str, start: usize) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut index = start;
    let mut quote: Option<u8> = None;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(open) = quote {
            if byte == b'\\' {
                index += 2;
                continue;
            }
            if byte == open {
                quote = None;
            }
            index += 1;
            continue;
        }
        match byte {
            b'"' | b'\'' => quote = Some(byte),
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index = text[index..]
                    .find('\n')
                    .map_or(bytes.len(), |at| index + at);
                continue;
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index = text[index..]
                    .find("*/")
                    .map_or(bytes.len(), |at| index + at + 2);
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                if depth == 0 {
                    return Some((start, index));
                }
                depth -= 1;
            }
            _ => {}
        }
        index += 1;
    }
    None
}

/// The top-level `name` and `appid` of one shortcut body. Only properties of
/// the declaration itself count: a `name:` nested inside an `onPressed` handler
/// belongs to something else entirely.
fn properties(body: &str) -> Block {
    let bytes = body.as_bytes();
    let mut block = Block {
        name: Property::Absent,
        appid: Property::Absent,
    };
    let mut index = 0;
    let mut depth = 0usize;
    let mut quote: Option<u8> = None;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(open) = quote {
            if byte == b'\\' {
                index += 2;
                continue;
            }
            if byte == open {
                quote = None;
            }
            index += 1;
            continue;
        }
        match byte {
            b'"' | b'\'' => {
                quote = Some(byte);
                index += 1;
                continue;
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index += body[index..]
                    .find('\n')
                    .map_or(bytes.len(), |at| index + at);
                continue;
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index += body[index..]
                    .find("*/")
                    .map_or(bytes.len(), |at| index + at + 2);
                continue;
            }
            b'{' => {
                depth += 1;
                index += 1;
                continue;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                index += 1;
                continue;
            }
            _ => {}
        }
        if depth == 0
            && let Some((property, next)) = key_at(body, index)
        {
            match &block.name {
                Property::Absent if property == "name" => block.name = value(body, next),
                Property::Absent if property == "appid" => block.appid = value(body, next),
                _ => {}
            }
            index = next;
            continue;
        }
        index += 1;
    }
    block
}

/// The property key starting at `index`, if one does, with the offset just past
/// its colon.
fn key_at(body: &str, index: usize) -> Option<(&'static str, usize)> {
    const KEYS: [&str; 2] = ["name", "appid"];
    let bytes = body.as_bytes();
    if index > 0 && is_word(bytes[index - 1]) {
        return None;
    }
    for key in KEYS {
        if !body[index..].starts_with(key) {
            continue;
        }
        let mut cursor = index + key.len();
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if bytes.get(cursor) == Some(&b':') {
            return Some((key, cursor + 1));
        }
    }
    None
}

/// The value of the property whose colon ends at `start`.
fn value(body: &str, start: usize) -> Property {
    let bytes = body.as_bytes();
    let mut cursor = start;
    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }
    match bytes.get(cursor) {
        Some(quote @ (b'"' | b'\'')) => {
            let value_start = cursor + 1;
            let rest = &body[value_start..];
            match rest.find(*quote as char) {
                Some(end) => Property::Literal(rest[..end].to_string()),
                None => Property::Dynamic,
            }
        }
        // A computed value, or nothing at all — either way not a literal.
        Some(_) => Property::Dynamic,
        None => Property::Absent,
    }
}

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn is_token(byte: u8) -> bool {
    // The colon is in because the token being read *is* an `appid:name` pair;
    // [`recipe::split_entry`] is what decides whether what came out is one.
    is_word(byte) || matches!(byte, b'-' | b'.' | b'/' | b'+' | b':')
}

/// A path under `$HOME`, reported the way a user would say it out loud:
/// `Wallpapers`, not `/home/r__idox/Wallpapers`. Anything outside one is
/// reported as it stands.
fn home_relative(home: &Path, path: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if !rest.as_os_str().is_empty() => rest.to_string_lossy().into_owned(),
        _ => path.to_string_lossy().into_owned(),
    }
}

/// A `$HOME`-relative path, the shape the switch report already uses everywhere
/// else; anything outside a `$HOME` is reported as it stands.
fn relative(path: &Path) -> String {
    let text = path.to_string_lossy();
    match text.rsplit_once("/.config/") {
        Some((_, tail)) => format!(".config/{tail}"),
        None => text.into_owned(),
    }
}

/// Recursive walk that keeps only what `wanted` accepts.
fn collect(root: &Path, into: &mut Vec<PathBuf>, wanted: &mut dyn FnMut(&Path) -> bool) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return; // An absent tree is a profile that ships no such config.
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            collect(&path, into, wanted);
        } else if metadata.is_file() && wanted(&path) {
            into.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The engine's unit tests run against two ground-truth fixture profiles
    /// copied out of the machine's profile store, and against synthetic trees
    /// for the shapes no real rice happens to have. The engine only ever writes
    /// inside the sandbox, so a test that reconciled a fixture in place would
    /// quietly turn the ground truth into whatever the engine last believed.
    fn fixture(name: &str) -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("sandbox");
        copy(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/reconcile")
                .join(name),
            root.path(),
        );
        root
    }

    fn copy(from: &Path, to: &Path) {
        for entry in std::fs::read_dir(from)
            .expect("fixture directory")
            .flatten()
        {
            let path = entry.path();
            let target = to.join(entry.file_name());
            if path.is_dir() {
                std::fs::create_dir_all(&target).expect("create fixture dir");
                copy(&path, &target);
            } else {
                std::fs::create_dir_all(target.parent().expect("parent")).expect("create dir");
                std::fs::copy(&path, &target).expect("copy fixture file");
            }
        }
    }

    /// The fixture tree these tests read, copied out of the repository rather
    /// than into it — nothing here ever edits the ground truth.
    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/reconcile")
    }

    /// A synthetic profile: the QML files and the Hyprland configs, written
    /// exactly as given.
    fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("sandbox");
        for (relative, contents) in files {
            let path = root.path().join(relative);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("create dir");
            std::fs::write(&path, contents).expect("write fixture file");
        }
        root
    }

    /// Every bind a config declares, as key combo → (dispatcher expression,
    /// option keys) — the shape two versions of one rice can be compared on
    /// when the two differ in everything a dispatcher rewrite does not own.
    ///
    /// Reads every `hl.dsp.…(…)` dispatch, not only the `global` ones, because
    /// half of what the hand-fix did was answer a dead name with a command: a
    /// map of the shortcuts alone would not show those binds at all, and the
    /// comparison would be quietly easier than it looks.
    fn binds(text: &str) -> BTreeMap<String, (String, BTreeSet<String>)> {
        let mut found: BTreeMap<String, (String, BTreeSet<String>)> = BTreeMap::new();
        for (at, _) in text.match_indices("hl.dsp.") {
            let name = at + "hl.dsp.".len();
            let Some(open) = text[name..].find('(').map(|found| name + found) else {
                continue;
            };
            let Some(close) = matching_closer(text, open) else {
                continue;
            };
            let dispatch = Span {
                start: at,
                end: close + 1,
            };
            let Some(bind) = enclosing_call(text, dispatch, &["bind"]) else {
                continue;
            };
            let arguments = arguments(text, bind);
            let Some(keys) = arguments.first().and_then(|keys| quoted(text, *keys)) else {
                continue;
            };
            found.entry(keys).or_insert_with(|| {
                let options = arguments
                    .get(2)
                    .map(|table| assignments(text, *table))
                    .unwrap_or_default();
                (canonical(&text[dispatch.start..dispatch.end]), options)
            });
        }
        found
    }

    /// The offset of the bracket that closes the one at `open`, strings and
    /// comments skipped.
    fn matching_closer(text: &str, open: usize) -> Option<usize> {
        enclosing_pair(
            text,
            Span {
                start: open + 1,
                end: open + 1,
            },
        )
        .map(|pair| pair.end - 1)
    }

    /// The text inside a quoted argument, when it is nothing but a string. A
    /// concatenation (`"SUPER + " .. key`) is not one, and reading its middle
    /// quote as an end would invent a key combo nobody bound.
    fn quoted(text: &str, span: Span) -> Option<String> {
        let bytes = text.as_bytes();
        let quote = *bytes.get(span.start)?;
        if !is_quote(quote) {
            return None;
        }
        let rest = &text[span.start + 1..span.end];
        let end = rest.find(char::from(quote))?;
        if !rest[end + 1..].trim().is_empty() {
            return None;
        }
        Some(rest[..end].to_string())
    }

    /// The `name =` keys a table assigns, which is what a bind's options are.
    fn assignments(text: &str, table: Span) -> BTreeSet<String> {
        let body = &text[table.start + 1..table.end.saturating_sub(1)];
        let masked = comments(body);
        let mut keys = BTreeSet::new();
        let bytes = body.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            if masked.get(index).copied().unwrap_or(false) {
                index += 1;
                continue;
            }
            let start = index;
            while index < bytes.len() && is_word(bytes[index]) {
                index += 1;
            }
            if start == index {
                index += 1;
                continue;
            }
            let mut after = index;
            while after < bytes.len() && bytes[after].is_ascii_whitespace() {
                after += 1;
            }
            if bytes.get(after) == Some(&b'=') {
                keys.insert(body[start..index].to_string());
            }
        }
        keys
    }

    /// Runs of whitespace collapsed, and the space a line wrap leaves next to a
    /// bracket removed — so a call the hand-fix wrote across three lines reads
    /// the same as the one the pass wrote on one. Whitespace *inside* a string
    /// is left alone, because in a command that whitespace is the command.
    fn canonical(text: &str) -> String {
        let squeezed = text.split_whitespace().collect::<Vec<&str>>().join(" ");
        squeezed.replace("( ", "(").replace(" )", ")")
    }

    fn read(root: &Path, relative: &str) -> String {
        std::fs::read_to_string(root.join(relative))
            .unwrap_or_else(|error| panic!("read {}: {error}", root.join(relative).display()))
    }

    fn none() -> BTreeSet<String> {
        BTreeSet::new()
    }

    /// The whole pass — engine and recipe layer together — against a sandboxed
    /// `$HOME` of the test's own.
    ///
    /// The fixture profile and the home are the same directory here, which is
    /// not how a real profile is laid out but keeps the recipe layer's paths
    /// inside the sandbox: `~/Wallpapers` expands under the temporary root and
    /// is torn down with it.
    fn pass(profile: &Path, home: &Path, shell: &str) -> Report {
        let data = home.join(".local").join("share").join("riceswap");
        reconcile(profile, &Context::new(home, &data), Some(shell), &none())
    }

    fn entries(scan: &Scan) -> Vec<String> {
        scan.sites.iter().map(Site::entry).collect()
    }

    // ---------------------------------------------------------------- registry

    /// The registry comes from the activating profile's own QML, and a shell
    /// that renames its appid says so once — in the base component every
    /// registration inherits from. Deriving per-block instead would file
    /// caelestia's 22 names under `quickshell`, which is the very namespace the
    /// donor configs already dispatch and the very mistake this engine exists
    /// not to make.
    #[test]
    fn a_shell_that_renames_its_appid_once_is_derived_from_that_declaration() {
        let donor = fixture("donor");
        let registry = Registry::derive(donor.path(), "caelestia");

        assert_eq!(registry.appid, "caelestia");
        assert_eq!(
            registry.by_name.keys().cloned().collect::<Vec<String>>(),
            [
                "brightnessDown",
                "brightnessUp",
                "clearNotifs",
                "dashboard",
                "launcher",
                "launcherInterrupt",
                "lock",
                "mediaNext",
                "mediaPrev",
                "mediaStop",
                "mediaToggle",
                "nexus",
                "refreshDevices",
                "screenshot",
                "screenshotClip",
                "screenshotFreeze",
                "screenshotFreezeClip",
                "session",
                "showall",
                "sidebar",
                "unlock",
                "utilities",
            ],
            "the 22 names the live `hyprctl globalshortcuts` probe reported (FC-2)"
        );
        assert_eq!(registry.entries.len(), 22);
        assert!(
            registry.uncertain.is_empty(),
            "the real tree parses cleanly: {:?}",
            registry.uncertain
        );
    }

    /// A tree that names no appid registers under Quickshell's own default,
    /// which is what makes ii's `quickshell:*` binds its own live vocabulary
    /// rather than a donor namespace to be rewritten.
    #[test]
    fn a_shell_that_names_no_appid_registers_under_the_quickshell_default() {
        let ii = fixture("ii");
        let registry = Registry::derive(ii.path(), "ii");

        assert_eq!(registry.appid, DEFAULT_APPID);
        assert_eq!(registry.entries.len(), 48);
        for name in ["lock", "lockFocus", "regionScreenshot", "barToggle"] {
            assert!(
                registry.holds("quickshell", name),
                "ii registers `{name}` under the default appid"
            );
        }
        assert!(
            registry.uncertain.is_empty(),
            "the real tree parses cleanly: {:?}",
            registry.uncertain
        );
    }

    /// The foreign guard is checked *before* the registry, and this pins that
    /// ordering rather than its outcome.
    ///
    /// The fixture is built so that every later check would wave the dispatch
    /// through: the activating shell declares its own appid, so the default
    /// `quickshell` namespace is not the tree's; `quickshell:riceswap-toggle`
    /// is therefore not registered as it stands (`registry.holds` is false), and
    /// the *name* `riceswap-toggle` is registered under `demo` — which is to
    /// say the exact-name move the engine applies everywhere else is available
    /// and provable here. Reorder the guard behind the registry and this bind
    /// gets rewritten to `demo:riceswap-toggle` and reported as a proposal: a
    /// future shell that happens to ship its own `riceswap-toggle` would quietly
    /// take over RiceSwap's panel hotkey.
    ///
    /// The appid is deliberately *not* `quickshell`. A shell that registered
    /// the name under the default appid would be saved by the "resolves as it
    /// stands" check instead, and this test would pass without the guard doing
    /// anything at all.
    #[test]
    fn a_shell_registering_riceswap_toggle_itself_cannot_reopen_the_guard() {
        let profile = tree(&[
            (
                ".config/quickshell/demo/components/misc/CustomShortcut.qml",
                "import Quickshell.Hyprland\n\nGlobalShortcut {\n    appid: \"demo\"\n}\n",
            ),
            (
                ".config/quickshell/demo/modules/Shortcuts.qml",
                "CustomShortcut {\n    name: \"riceswap-toggle\"\n    onPressed: {}\n}\n",
            ),
            (
                ".config/hypr/hyprland/keybinds.lua",
                concat!(
                    "hl.bind(\"SUPER + R\", hl.dsp.global(\"quickshell:riceswap-toggle\"), ",
                    "{ description = \"Shell: Toggle RiceSwap\" })\n",
                ),
            ),
        ]);
        let registry = Registry::derive(profile.path(), "demo");

        // The two facts that make the ordering load-bearing.
        assert_eq!(registry.appid, "demo", "the tree renamed its appid");
        assert!(
            !registry.holds("quickshell", "riceswap-toggle"),
            "so the dispatch is not registered as it stands"
        );
        assert_eq!(
            registry.provable_appid("riceswap-toggle"),
            Some("demo"),
            "and the exact-name move that would capture it is provable"
        );

        let before = read(profile.path(), ".config/hypr/hyprland/keybinds.lua");
        let report = pass(profile.path(), profile.path(), "demo");

        assert!(
            report.files_written.is_empty(),
            "RiceSwap's own hotkey is never rewritten: {report:?}"
        );
        assert!(report.dead_names.is_empty(), "{report:?}");
        assert!(report.proposals.is_empty(), "nor even reported: {report:?}");
        assert_eq!(
            before,
            read(profile.path(), ".config/hypr/hyprland/keybinds.lua"),
            "the bind is byte-for-byte what it was"
        );
        assert!(report.externals.is_empty());
    }

    /// Only a literal counts. A name built at runtime is not a registration this
    /// engine can prove, and an unprovable registration is a reason to distrust
    /// the registry — never a reason to guess what it holds.
    #[test]
    fn a_name_built_at_runtime_is_never_counted_as_registered() {
        let profile = tree(&[(
            ".config/quickshell/demo/modules/Shortcuts.qml",
            r#"
import Quickshell.Hyprland

GlobalShortcut {
    name: "literal"
    onPressed: {}
}

GlobalShortcut {
    name: prefix + "computed"
    onPressed: {}
}
"#,
        )]);
        let registry = Registry::derive(profile.path(), "demo");

        assert!(registry.holds("quickshell", "literal"));
        assert!(
            !registry.holds("quickshell", "prefixcomputed"),
            "a computed name is not a registration"
        );
        assert_eq!(registry.entries.len(), 1);
        assert!(
            registry
                .uncertain
                .iter()
                .any(|finding| finding.contains("built at runtime")),
            "the parse says so: {:?}",
            registry.uncertain
        );
    }

    /// Two appids in one tree is a tree this parse cannot attribute, so the
    /// registrations that do not name an appid themselves stop being provable —
    /// which means dispatches of them are reported, never moved.
    #[test]
    fn a_tree_naming_two_appids_proves_nothing_it_cannot_attribute() {
        let profile = tree(&[
            (
                ".config/quickshell/demo/Base.qml",
                "import Quickshell.Hyprland\nGlobalShortcut { appid: \"one\" }\n",
            ),
            (
                ".config/quickshell/demo/Other.qml",
                "import Quickshell.Hyprland\nGlobalShortcut { appid: \"two\" }\n",
            ),
            (
                ".config/quickshell/demo/modules/Shortcuts.qml",
                "CustomShortcut { name: \"orphan\" onPressed: {} }\n",
            ),
        ]);
        let registry = Registry::derive(profile.path(), "demo");

        assert!(
            registry.entries.is_empty(),
            "nothing is provable: {:?}",
            registry.entries
        );
        assert_eq!(registry.appid, DEFAULT_APPID);
        assert!(
            registry
                .uncertain
                .iter()
                .any(|finding| finding.contains("more than one shortcut appid")),
            "{:?}",
            registry.uncertain
        );
    }

    /// A profile that names no shell, and one whose QML tree is not there, are
    /// both skips — reported by name, never a silent zero, and never a guess.
    #[test]
    fn a_profile_with_nothing_to_derive_from_is_skipped_and_says_why() {
        let empty = tree(&[(".config/hypr/hypridle.conf", "$x = 1\n")]);

        let nameless = reconcile(
            empty.path(),
            &Context::new(empty.path(), empty.path()),
            None,
            &none(),
        );
        assert!(nameless.files_written.is_empty());
        assert!(
            nameless
                .skipped
                .as_deref()
                .is_some_and(|why| why.contains("names no shell")),
            "{nameless:?}"
        );

        let treeless = pass(empty.path(), empty.path(), "demo");
        assert!(treeless.files_written.is_empty());
        assert!(
            treeless
                .skipped
                .as_deref()
                .is_some_and(|why| why.contains("no QML tree")),
            "{treeless:?}"
        );
    }

    // ------------------------------------------------------------------- scan

    /// The four dispatch forms real rice uses, all found; a line that only
    /// *talks* about a dispatch is not one; and one built at runtime is reported
    /// as unresolvable rather than guessed at.
    #[test]
    fn every_dispatch_form_a_real_rice_uses_is_found() {
        let text = r#"
-- hl.dsp.global("quickshell:commentedOut")
# dispatch global quickshell:alsoCommented
hl.bind("SUPER + L", hl.dsp.global("quickshell:lock"))
hl.bind("SUPER + M", hl.dsp.global('quickshell:media'))
bind = SUPER, N, exec, hyprctl dispatch global quickshell:nexus
$lock_cmd = hyprctl dispatch 'hl.dsp.global("quickshell:screenshot")'
hl.bind("SUPER + P", hl.dsp.global(prefix .. ":computed"))
"#;
        let scan = scan(text);

        assert_eq!(
            entries(&scan),
            [
                "quickshell:lock",
                "quickshell:media",
                "quickshell:nexus",
                "quickshell:screenshot",
            ],
            "quoted, single-quoted, bare and unquoted forms, in order"
        );
        assert_eq!(scan.dynamic, ["prefix"], "the computed one is reported");
    }

    /// The rewrite touches the token and nothing else — the surrounding Lua,
    /// the quoting, the comments and the rest of the line stay byte for byte.
    #[test]
    fn a_rewrite_touches_the_dispatcher_token_and_nothing_else() {
        let text = "hl.bind(\"SUPER + L\", hl.dsp.global(\"quickshell:lock\"), { description = \"Lock\" })\n";
        let scan = scan(text);
        let edits: Vec<Edit> = scan
            .sites
            .iter()
            .map(|site| edit(token_of(site), "caelestia:lock".to_string()))
            .collect();
        let mut refused = Vec::new();

        assert_eq!(
            apply(text, edits, &mut refused),
            "hl.bind(\"SUPER + L\", hl.dsp.global(\"caelestia:lock\"), { description = \"Lock\" })\n"
        );
        assert!(refused.is_empty(), "{refused:?}");
    }

    // ----------------------------------------------------------------- golden

    /// The engine alone, on a tree no recipe speaks for: a name the registry
    /// cannot prove is never moved, however similar it looks to one it can.
    ///
    /// This is the half of the claim #35 made and the recipe layer must never
    /// erode — the shapes here are the real caelestia ones (`launcher` and
    /// `screenshotClip` are registered; `searchToggleRelease` and
    /// `regionScreenshot` are not), and the shell is called `demo` precisely so
    /// no built-in recipe is in play.
    #[test]
    fn a_name_the_registry_cannot_prove_is_never_moved_by_the_engine_alone() {
        let profile = tree(&[
            (
                ".config/quickshell/demo/modules/Shortcuts.qml",
                concat!(
                    "import Quickshell.Hyprland\n\n",
                    "GlobalShortcut {\n    appid: \"demo\"\n}\n",
                    "CustomShortcut {\n    name: \"lock\"\n    onPressed: {}\n}\n",
                    "CustomShortcut {\n    name: \"launcher\"\n    onPressed: {}\n}\n",
                    "CustomShortcut {\n    name: \"screenshotClip\"\n    onPressed: {}\n}\n",
                ),
            ),
            (
                ".config/hypr/hyprland/keybinds.lua",
                concat!(
                    "hl.bind(\"SUPER + L\", hl.dsp.global(\"quickshell:lock\"))\n",
                    "hl.bind(\"SUPER + SPACE\", hl.dsp.global(\"quickshell:searchToggleRelease\"))\n",
                    "hl.bind(\"SUPER + SHIFT + S\", hl.dsp.global(\"quickshell:regionScreenshot\"))\n",
                ),
            ),
        ]);
        let before = read(profile.path(), ".config/hypr/hyprland/keybinds.lua");

        let report = pass(profile.path(), profile.path(), "demo");

        assert_eq!(
            report.files_written,
            [".config/hypr/hyprland/keybinds.lua"],
            "the one name the registry registers is the one name that moves"
        );
        let after = read(profile.path(), ".config/hypr/hyprland/keybinds.lua");
        assert!(
            after.contains(r#"hl.dsp.global("demo:lock")"#),
            "the exact-name move: {after}"
        );
        for similar in [
            "quickshell:searchToggleRelease",
            "quickshell:regionScreenshot",
        ] {
            assert!(
                after.contains(similar),
                "`{similar}` is in no registry, and no recipe answers for a shell that has \
                 none, so it stays byte for byte: {after}"
            );
            let proposal = report
                .proposals
                .iter()
                .find(|proposal| &proposal.dispatched == similar)
                .unwrap_or_else(|| panic!("{similar} must be proposed: {:?}", report.proposals));
            assert_eq!(proposal.target, None, "no counterpart, so no rewrite");
        }
        assert_eq!(
            report.dead_names,
            [
                "quickshell:regionScreenshot",
                "quickshell:searchToggleRelease"
            ],
            "and both stay in the dead set a recipe is written against"
        );
        assert!(
            report.resolutions.is_empty(),
            "nothing answered them: {:?}",
            report.resolutions
        );
        assert_ne!(before, after, "the lock move is the only change");
    }

    /// The same donor graft, with the built-in caelestia recipe in play — the
    /// pass as a switch actually runs it.
    ///
    /// The engine's own move is unchanged and still the only one it makes by
    /// itself: `quickshell:lock` is a name caelestia registers, so it moves to
    /// `caelestia:lock` in hypridle, with no recipe entry anywhere. What the
    /// recipe adds is the *answers* — and the report says which file each answer
    /// came from, so a user can see that the fuzzy mappings were declared rather
    /// than guessed.
    #[test]
    fn the_donor_graft_moves_what_the_registry_proves_and_answers_the_rest() {
        let donor = fixture("donor");
        let before = read(donor.path(), ".config/hypr/hypridle.conf");

        let report = pass(donor.path(), donor.path(), "caelestia");

        // (a) the exact-name move, and the file holding it.
        assert_eq!(report.appid.as_deref(), Some("caelestia"));
        assert_eq!(report.registered, 22);
        assert!(
            report
                .files_written
                .contains(&".config/hypr/hypridle.conf".to_string()),
            "{:?}",
            report.files_written
        );
        let after = read(donor.path(), ".config/hypr/hypridle.conf");
        assert!(
            after.contains(r#"hl.dsp.global("caelestia:lock")"#),
            "lock is registered, so it moves: {after}"
        );
        assert_eq!(
            after.matches("caelestia:lock").count(),
            1,
            "the second lock line dispatched `lockFocus`, a name caelestia does not register"
        );
        assert!(
            after.contains(r#"hl.dsp.global("quickshell:lockFocus")"#),
            "lockFocus has no counterpart and no recipe entry, so it is left exactly as it \
             was: {after}"
        );
        assert_ne!(before, after);

        // (b) the fuzzy names are now answered, by the built-in layer, and the
        // report says which layer won for each of them.
        assert_eq!(report.layers, ["builtin"]);
        let applied = |name: &str| {
            report
                .resolutions
                .iter()
                .find(|applied| &applied.dispatched == name)
                .unwrap_or_else(|| panic!("{name} must be resolved: {:?}", report.resolutions))
        };
        assert_eq!(
            applied("quickshell:regionScreenshot").to.as_deref(),
            Some("caelestia:screenshotClip")
        );
        assert_eq!(applied("quickshell:regionScreenshot").kind, "to");
        assert_eq!(applied("quickshell:barToggle").kind, "drop");
        assert_eq!(
            applied("quickshell:overviewClipboardToggle")
                .exec
                .as_deref(),
            Some("pkill fuzzel || caelestia clipboard")
        );
        assert!(
            report
                .resolutions
                .iter()
                .all(|applied| applied.layer == "builtin"),
            "the human tier has said nothing here, so every answer is the built-in's"
        );
        // One name, several decisions: the sites are per line, and `lock` was
        // moved by the engine while `regionScreenshot` was answered twice.
        assert_eq!(
            applied("quickshell:regionScreenshot").sites,
            [
                ProposalSite {
                    file: ".config/hypr/custom/keybinds.lua".to_string(),
                    line: 4,
                },
                ProposalSite {
                    file: ".config/hypr/hyprland/keybinds.lua".to_string(),
                    line: 68,
                },
            ],
            "both binds, in file-then-line order"
        );

        // (c) what the engine could not prove and no recipe answers is all
        // that is left dead, and it is the composite hypridle case FC-7 records.
        assert_eq!(report.dead_names, ["quickshell:lockFocus"]);
        assert_eq!(report.warnings, Vec::<String>::new());

        // (d) the panel's own shortcut survives: foreign, and not even reported.
        let keybinds = read(donor.path(), ".config/hypr/hyprland/keybinds.lua");
        assert!(keybinds.contains(r#"hl.dsp.global("quickshell:riceswap-toggle")"#));
        assert!(
            !report
                .dead_names
                .iter()
                .chain(report.proposals.iter().map(|p| &p.dispatched))
                .any(|entry| entry.contains("riceswap-toggle")),
            "RiceSwap's own hotkey is built-in foreign: {:?}",
            report.dead_names
        );
        assert_eq!(
            report.foreign_entries,
            ["quickshell:riceswap-toggle"],
            "and the recipe names it too, so the pass ran with it in the keep set"
        );
    }

    /// The invariant the engine exists to establish, checked on the file it
    /// wrote rather than on what it intended: after the pass, the only
    /// dispatches left unresolved are the ones the report named.
    #[test]
    fn every_dispatch_resolves_after_the_rewrite_but_the_reported_ones() {
        let donor = fixture("donor");
        let report = pass(donor.path(), donor.path(), "caelestia");
        let registry = Registry::derive(donor.path(), "caelestia");

        let mut unresolved: BTreeSet<String> = BTreeSet::new();
        let mut files: Vec<PathBuf> = Vec::new();
        collect(
            &donor.path().join(".config/hypr"),
            &mut files,
            &mut |path| {
                matches!(
                    path.extension().and_then(|found| found.to_str()),
                    Some("lua") | Some("conf")
                )
            },
        );
        for path in files {
            for site in &scan(&std::fs::read_to_string(&path).expect("read")).sites {
                let entry = site.entry();
                if !registry.holds(&site.appid, &site.name) && !FOREIGN.contains(&entry.as_str()) {
                    unresolved.insert(entry);
                }
            }
        }
        assert_eq!(
            unresolved,
            report
                .dead_names
                .iter()
                .cloned()
                .collect::<BTreeSet<String>>(),
            "what is left dead is exactly what the report says, and nothing else"
        );
    }

    /// Idempotence, measured rather than asserted: a second pass over an
    /// already-repaired profile writes no file and makes no backup, creates no
    /// directory and rewrites no env block, and every byte on disk —
    /// modification times included — is what the first pass left.
    ///
    /// The stamp covers the whole profile tree, which is why the managed env
    /// file and the guaranteed directory are inside the claim rather than
    /// around it: the first pass has real work to do in all three.
    #[test]
    fn a_second_pass_over_a_repaired_profile_writes_nothing() {
        let donor = fixture("donor");
        let first = pass(donor.path(), donor.path(), "caelestia");
        assert_eq!(
            first.files_written,
            [
                ".config/hypr/custom/general.lua",
                ".config/hypr/custom/keybinds.lua",
                ".config/hypr/hypridle.conf",
                ".config/hypr/hyprland/keybinds.lua",
                ".config/hypr/custom/env.lua",
            ],
            "the first pass has work to do: four configs and the managed env block"
        );
        assert_eq!(first.dirs_created, ["Wallpapers"]);

        let before = stamp(donor.path());
        let second = pass(donor.path(), donor.path(), "caelestia");
        let after = stamp(donor.path());

        assert!(second.files_written.is_empty(), "{second:?}");
        assert!(second.backups.is_empty(), "no second backup: {second:?}");
        assert!(
            second.dirs_created.is_empty(),
            "and the guaranteed directory is a state, not a repeated act: {second:?}"
        );
        assert_eq!(
            second.env.as_ref().map(|block| block.written),
            Some(false),
            "the env block is settled, so nothing was written: {second:?}"
        );
        assert_eq!(before, after, "not one byte or timestamp moved");
        assert_eq!(
            first.dead_names, second.dead_names,
            "the unresolvable names are still unresolvable, and that is the whole report"
        );
        assert_eq!(
            first.resolutions.len(),
            23,
            "the first pass answered 23 dead names"
        );
        assert!(
            second.resolutions.is_empty(),
            "and the second has nothing left to answer: a repaired profile has no dead \
             dispatches for a recipe to resolve, which is what converged means: {second:?}"
        );
    }

    /// The regression guard. ii's configs dispatch `quickshell:*` and ii
    /// registers those very names, so the correct answer is to change nothing
    /// at all — no rewrites, no backups, not a byte moved anywhere in the tree.
    #[test]
    fn iis_own_profile_comes_out_untouched() {
        let ii = fixture("ii");
        let before = stamp(ii.path());

        let report = pass(ii.path(), ii.path(), "ii");

        assert_eq!(report.appid.as_deref(), Some(DEFAULT_APPID));
        assert_eq!(report.registered, 48);
        assert!(report.scanned_files > 20, "the real config tree was read");
        assert!(report.files_written.is_empty(), "{report:?}");
        assert!(report.backups.is_empty(), "{report:?}");
        assert!(
            report.dead_names.is_empty(),
            "nothing of ii's is dead: {report:?}"
        );
        assert!(report.proposals.is_empty(), "{report:?}");
        assert!(report.warnings.is_empty(), "{report:?}");
        // With the recipe layer in play: ii *has* a built-in recipe, it is
        // consulted, and it has nothing to say — no resolutions, no env, no
        // dirs — so the whole tree still comes out untouched.
        assert_eq!(report.layers, ["builtin"], "{report:?}");
        assert!(report.resolutions.is_empty(), "{report:?}");
        assert!(report.env.is_none(), "{report:?}");
        assert!(report.dirs_created.is_empty(), "{report:?}");
        assert_eq!(
            report.foreign_entries,
            ["quickshell:riceswap-toggle"],
            "and ii's own recipe names the panel's hotkey, which the built-in guard already held"
        );
        assert!(
            !ii.path().join("backups").exists(),
            "a clean profile does not even get a backups directory"
        );
        assert_eq!(
            before,
            stamp(ii.path()),
            "the tree is byte-for-byte and mtime-for-mtime what it was"
        );
    }

    // ------------------------------------------------------------------ scope

    /// The rewrite scope is the config the compositor loads: `*.lua` and
    /// `*.conf`. A captured rice keeps scratch siblings beside them, and Hyprland
    /// reads none of those — so they are not rewritten, and a helper script the
    /// binds `exec` is reported rather than edited.
    #[test]
    fn only_the_configs_the_compositor_loads_are_rewritten() {
        let profile = tree(&[
            (
                ".config/quickshell/demo/components/Base.qml",
                "import Quickshell.Hyprland\nGlobalShortcut { appid: \"demo\" }\n",
            ),
            (
                ".config/quickshell/demo/modules/Shortcuts.qml",
                "GlobalShortcut { name: \"lock\" }\n",
            ),
            (
                ".config/hypr/hypridle.conf",
                "$lock_cmd = hyprctl dispatch 'hl.dsp.global(\"quickshell:lock\")'\n",
            ),
            (
                ".config/hypr/hypridle.conf.new",
                "$lock_cmd = hyprctl dispatch 'hl.dsp.global(\"quickshell:lock\")'\n",
            ),
            (
                ".config/hypr/hyprland/keybinds.lua.bak",
                "hl.bind(\"SUPER + L\", hl.dsp.global(\"quickshell:lock\"))\n",
            ),
            (
                ".config/hypr/hyprland/scripts/snip.sh",
                "#!/bin/sh\nhyprctl dispatch 'hl.dsp.global(\"quickshell:lock\")'\n",
            ),
            (
                ".config/hypr/hyprland/execs.lua",
                "hl.exec_cmd(\"$HOME/.config/hypr/hyprland/scripts/snip.sh\")\n",
            ),
        ]);
        let script = read(profile.path(), ".config/hypr/hyprland/scripts/snip.sh");

        let report = pass(profile.path(), profile.path(), "demo");

        assert_eq!(report.files_written, [".config/hypr/hypridle.conf"]);
        assert!(
            read(profile.path(), ".config/hypr/hypridle.conf")
                .contains(r#"hl.dsp.global("demo:lock")"#),
            "the one name the registry registers is the one name that moves"
        );
        assert!(
            read(profile.path(), ".config/hypr/hypridle.conf.new").contains("quickshell:lock"),
            "a scratch sibling is not a config"
        );
        assert!(
            read(profile.path(), ".config/hypr/hyprland/keybinds.lua.bak")
                .contains("quickshell:lock")
        );
        assert_eq!(
            script,
            read(profile.path(), ".config/hypr/hyprland/scripts/snip.sh"),
            "an exec'd helper script is never edited"
        );
        assert_eq!(
            report.externals,
            [External {
                path: ".config/hypr/hyprland/scripts/snip.sh".to_string(),
                dispatched: vec!["quickshell:lock".to_string()],
            }],
            "but what it dispatches is reported: {:?}",
            report.externals
        );
    }

    // --------------------------------------------------------------- backups

    /// A backup is named by the bytes it holds: the same content always lands on
    /// the same name (so a repeat run finds it and writes nothing), and
    /// different content never does.
    #[test]
    fn a_backup_is_named_by_the_content_it_holds() {
        let original = b"$lock_cmd = dispatch 'hl.dsp.global(\"quickshell:lock\")'\n";
        let other = b"$lock_cmd = dispatch 'hl.dsp.global(\"quickshell:unlock\")'\n";
        assert_eq!(content_hash(original), content_hash(original));
        assert_ne!(content_hash(original), content_hash(other));
        assert_eq!(content_hash(b"").len(), 16, "a fixed-width hex fingerprint");

        let donor = fixture("donor");
        let untouched = read(donor.path(), ".config/hypr/hypridle.conf")
            .replace("caelestia:lock", "quickshell:lock");
        let report = pass(donor.path(), donor.path(), "caelestia");
        let stored = report
            .backups
            .iter()
            .find(|stored| stored.ends_with(".hypridle.conf"))
            .unwrap_or_else(|| panic!("a backup for the rewritten file: {:?}", report.backups));
        let name = stored.strip_prefix("backups/").expect("profile-relative");
        let kept = read(donor.path(), stored);
        assert_eq!(
            kept, untouched,
            "the backup holds the bytes as they were, not as they became"
        );
        assert!(name.ends_with(".hypridle.conf"), "{stored}");
        assert!(donor.path().join(stored).is_file());
        assert!(
            !report
                .backups
                .iter()
                .any(|stored| stored.ends_with(".env.lua")),
            "a file this pass created has no earlier bytes to keep: {:?}",
            report.backups
        );
    }

    // ----------------------------------------------------------------- wiring

    /// The engine is pure text and never shells out, so a profile with nothing
    /// to repair must not disturb a single file — including the ones only it
    /// would have rewritten had the name resolved.
    #[test]
    fn a_profile_whose_names_all_resolve_is_a_no_op() {
        let profile = tree(&[
            (
                ".config/quickshell/demo/modules/Shortcuts.qml",
                "GlobalShortcut { name: \"lock\" }\n",
            ),
            (
                ".config/hypr/hyprland/keybinds.lua",
                "hl.bind(\"SUPER + L\", hl.dsp.global(\"quickshell:lock\"))\n",
            ),
        ]);
        let before = stamp(profile.path());

        let report = pass(profile.path(), profile.path(), "demo");

        assert!(report.dead_names.is_empty(), "{report:?}");
        assert!(report.proposals.is_empty(), "{report:?}");
        assert_eq!(before, stamp(profile.path()));
    }

    // ------------------------------------------------------- the codebook run

    /// The hand-fixed reference, key by key: the combo, and the dispatch the
    /// hand-fix left on it.
    ///
    /// Read straight off `tests/fixtures/reconcile/reference/`, which is the
    /// caelestia profile's own configs copied after the manual adaptation. The
    /// table is what "the same semantics" means here, so a reader can check any
    /// line of it against the reference file without running anything.
    const HAND_FIXED: &[(&str, &str)] = &[
        ("SUPER + SUPER_L", "hl.dsp.global(\"caelestia:launcher\")"),
        ("SUPER + SUPER_R", "hl.dsp.global(\"caelestia:launcher\")"),
        ("SUPER + Tab", "hl.dsp.global(\"caelestia:showall\")"),
        ("SUPER + A", "hl.dsp.global(\"caelestia:sidebar\")"),
        ("SUPER + B", "hl.dsp.global(\"caelestia:sidebar\")"),
        ("SUPER + O", "hl.dsp.global(\"caelestia:sidebar\")"),
        ("SUPER + N", "hl.dsp.global(\"caelestia:utilities\")"),
        (
            "CTRL + ALT + Delete",
            "hl.dsp.global(\"caelestia:session\")",
        ),
        ("SUPER + R", "hl.dsp.global(\"quickshell:riceswap-toggle\")"),
        (
            "SUPER + SHIFT + S",
            "hl.dsp.global(\"caelestia:screenshotClip\")",
        ),
        (
            "SUPER + SHIFT + R",
            "hl.dsp.exec_cmd(\"caelestia record -r\")",
        ),
        (
            "SUPER + ALT + R",
            "hl.dsp.exec_cmd(\"caelestia record -r\")",
        ),
    ];

    /// Every key the hand-fix removed, because what it dispatched is not
    /// something caelestia has.
    const HAND_DROPPED: &[&str] = &[
        "SUPER_L",
        "SUPER_R",
        "SUPER + ALT + A",
        "SUPER + Slash",
        "SUPER + K",
        "SUPER + M",
        "SUPER + J",
        "CTRL + SUPER + P",
        "SUPER + SHIFT + T",
    ];

    /// The pass over the donor graft lands every semantic the hand-fix declared,
    /// on the same keys.
    ///
    /// The comparison is per key rather than per file because the two files
    /// differ in ways no dispatch resolution describes: the hand-fix rewrapped
    /// binds across lines, reworded every `description`, deleted the donor's
    /// stale `exec` fallbacks, and added prose comments. Comparing binds — the
    /// key, the dispatcher expression, the option table — is what says "the same
    /// semantics", and the deviations below are the ones that are left over.
    #[test]
    fn the_caelestia_codebook_lands_every_semantic_the_hand_fix_declared() {
        let donor = fixture("donor");
        let reference = fixtures().join("reference");
        pass(donor.path(), donor.path(), "caelestia");

        let ours = binds(&read(donor.path(), ".config/hypr/hyprland/keybinds.lua"));
        let hand = binds(&read(&reference, ".config/hypr/hyprland/keybinds.lua"));

        for (combo, dispatch) in HAND_FIXED {
            let landed = ours
                .get(*combo)
                .unwrap_or_else(|| panic!("{combo} must still be bound: {ours:?}"));
            assert_eq!(
                landed.0, *dispatch,
                "{combo} dispatches `{dispatch}` in the hand-fixed config"
            );
            assert_eq!(
                hand.get(*combo).map(|bind| bind.0.as_str()),
                Some(*dispatch),
                "and the reference really does say that, so this table is a claim about the \
                 hand-fix and not only about the pass"
            );
        }

        for combo in HAND_DROPPED {
            assert!(
                !ours.contains_key(*combo),
                "{combo} dispatched an intent caelestia does not have, so the recipe drops it: \
                 {ours:?}"
            );
            assert!(
                !hand.contains_key(*combo),
                "and the hand-fix dropped the same key: {hand:?}"
            );
        }

        // The longest command the codebook carries — the grim/slurp/tesseract
        // pipeline, with its nested quoting and its escaped newlines — comes
        // back out of the Lua string it was rendered into byte for byte, which
        // is the strongest form the convergence claim can take.
        assert_eq!(
            ours["SUPER + SHIFT + X"].0, hand["SUPER + SHIFT + X"].0,
            "the OCR pipeline is the hand-fix's own, character for character"
        );
        assert!(
            ours["SUPER + SHIFT + X"]
                .0
                .starts_with("hl.dsp.exec_cmd(\"pidof slurp || grim"),
            "and it is the pipeline, not a prefix of it: {:?}",
            ours["SUPER + SHIFT + X"].0
        );

        // The one bind option the recipe declares, on both of the binds that
        // dispatched the donor's two-name launcher trick.
        for combo in ["SUPER + SUPER_L", "SUPER + SUPER_R"] {
            assert!(
                ours[combo].1.contains("release"),
                "caelestia has no release variant of its own, so the recipe's `release` option \
                 is what carries the donor's press-and-hold: {combo} -> {:?}",
                ours[combo]
            );
        }

        // A bind the pass rewrote and the hand-fix wrote out the same way,
        // byte for byte: the option table is added where the file already has
        // one, and created where it has none.
        assert!(
            read(donor.path(), ".config/hypr/hyprland/keybinds.lua").contains(
                "hl.bind(\"SUPER + SUPER_R\", hl.dsp.global(\"caelestia:launcher\"), \
                 { release = true })"
            ),
            "the optionless bind gets a table, spelled the way the hand-fix spelled it"
        );
    }

    /// What the hand-fix did that no `[[resolution]]` describes, named one by
    /// one. Each of these is a difference between the pass and the reference
    /// that is *deliberate*: the recipe vocabulary is `to`, `exec` and `drop`
    /// over a dead dispatch name, and these are all something else.
    #[test]
    fn the_differences_from_the_hand_fix_are_the_ones_no_resolution_can_express() {
        let donor = fixture("donor");
        let reference = fixtures().join("reference");
        pass(donor.path(), donor.path(), "caelestia");
        let ours = read(donor.path(), ".config/hypr/hyprland/keybinds.lua");
        let hand = read(&reference, ".config/hypr/hyprland/keybinds.lua");

        // (a) The hand-fix re-pointed a key to a capability of its own choosing
        // — SUPER+G was the donor's widget overlay and became caelestia's
        // dashboard. A resolution answers a dead *name*; it cannot invent a
        // dispatch for a key the user bound to something else. The recipe drops
        // the dead name instead, which is the honest third option.
        assert!(!ours.contains("caelestia:dashboard"));
        assert!(hand.contains("caelestia:dashboard"));
        assert!(!ours.contains("quickshell:overlayToggle"));

        // (b) The hand-fix deleted the donor's `exec` fallbacks and moved their
        // descriptions onto the shortcut bind — FC-4's "strip the `qsIsAlive`
        // guard" note. Those lines dispatch nothing dead, so no resolution
        // reaches them, and the pass leaves both binds in place. The user's
        // guarded fallback is not this layer's to remove.
        for kept in [
            "qsIsAlive .. \" || pkill fuzzel || cliphist list",
            "qsIsAlive .. \" || pkill fuzzel || \" .. hyprScripts",
            "qsIsAlive .. \" || pidof slurp || \" .. hyprScripts",
        ] {
            assert!(
                ours.contains(kept),
                "the donor's own fallback survives: {kept}"
            );
            assert!(!hand.contains(kept), "which the hand-fix deleted: {kept}");
        }

        // (c) The hand-fix converted two live `exec` binds — the donor's
        // brightness IPC calls — into the shortcuts caelestia registers, and
        // the restart bind into `caelestia shell -d`. Again: nothing there was
        // dead, so the trilemma has no third thing to say about it.
        assert!(ours.contains("qsIpcCall .. \" brightness increment"));
        assert!(hand.contains("caelestia:brightnessUp"));
        assert!(ours.contains("qs -c $qsConfig &"));
        assert!(hand.contains("caelestia shell -d & disown"));

        // (d) `wallpaperSelectorToggle` (the UI selector) and
        // `wallpaperSelectorRandom` (the CLI command) shared CTRL+SUPER+T in
        // spirit: the hand-fix put `caelestia wallpaper -r` on the selector's
        // key and deleted the random key's own. The recipe keeps the mapping
        // FC-4 records — drop the selector, run the command on the key that
        // asked for it.
        assert!(ours.contains("hl.dsp.exec_cmd(\"caelestia wallpaper -r\")"));
        assert!(!ours.contains("quickshell:wallpaperSelectorToggle"));

        // (e) The script fallback spells its path the way the recipe declares it
        // ($HOME-relative) rather than through the file's `hyprScripts` local.
        // The same script, the same argument — a different way to write it down.
        assert!(
            ours.contains("pidof slurp || $HOME/.config/hypr/hyprland/scripts/snip_to_search.sh")
        );
        assert!(hand.contains("pidof slurp || \" .. hyprScripts .. \"/snip_to_search.sh"));

        // (f) The hand-fix's own prose: a comment block explaining the
        // namespace, per-line comments, and reworded descriptions. A dispatcher
        // rewrite touches a token; it does not write documentation.
        assert!(!ours.contains("Caelestia's shell shortcuts live under"));
        assert!(hand.contains("Caelestia's shell shortcuts live under"));
        assert!(ours.contains(r#"description = "Shell: Toggle search""#));
        assert!(hand.contains(r#"description = "Shell: Toggle launcher""#));

        // What is left is the two binds no resolution reaches, and nothing else:
        // the hybrid quickshell vocabulary is gone from the file entirely.
        for gone in [
            "quickshell:searchToggleRelease",
            "quickshell:workspaceNumber",
            "quickshell:overviewWorkspacesToggle",
            "quickshell:overviewClipboardToggle",
            "quickshell:overviewEmojiToggle",
            "quickshell:sidebarLeftToggle",
            "quickshell:sidebarLeftToggleDetach",
            "quickshell:sidebarRightToggle",
            "quickshell:cheatsheetToggle",
            "quicksheet:oskToggle",
            "quickshell:oskToggle",
            "quickshell:mediaControlsToggle",
            "quickshell:overlayToggle",
            "quickshell:sessionToggle",
            "quickshell:barToggle",
            "quickshell:wallpaperSelectorToggle",
            "quickshell:wallpaperSelectorRandom",
            "quickshell:toggleLightDark",
            "quickshell:panelFamilyCycle",
            "quickshell:regionScreenshot",
            "quickshell:regionSearch",
            "quickshell:regionOcr",
            "quickshell:screenTranslate",
            "quickshell:regionRecord",
        ] {
            assert!(
                !ours.contains(gone),
                "`{gone}` is answered: {gone} is still in the file"
            );
        }
    }

    /// The other two files the hand-fix wrote, compared whole: they are small
    /// enough that a byte comparison is the honest one.
    #[test]
    fn the_custom_keybinds_and_the_hypridle_move_land_exactly_where_the_hand_fix_left_them() {
        let donor = fixture("donor");
        let reference = fixtures().join("reference");
        let report = pass(donor.path(), donor.path(), "caelestia");

        // One line, one token, and the file comes out byte-for-byte identical to
        // the hand-fixed reference.
        assert_eq!(
            read(donor.path(), ".config/hypr/custom/keybinds.lua"),
            read(&reference, ".config/hypr/custom/keybinds.lua"),
            "CTRL+K dispatches `caelestia:screenshotClip` and the rest of the file is untouched"
        );

        // hypridle: the one line the engine could prove, and nothing else. The
        // hand-fix also rewrote `after_sleep_cmd` into a composite (wake the
        // detached shell, *then* lock it — FC-7), which is not a dispatch
        // resolution, and added two comment blocks, which is not a rewrite.
        fn directives(text: &str) -> Vec<&str> {
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .collect()
        }
        let idle = read(donor.path(), ".config/hypr/hypridle.conf");
        let hand_idle = read(&reference, ".config/hypr/hypridle.conf");
        let ours = directives(&idle);
        let hand = directives(&hand_idle);
        let differing: Vec<(&str, &str)> = ours
            .iter()
            .zip(&hand)
            .filter(|(ours, hand)| ours != hand)
            .map(|(ours, hand)| (*ours, *hand))
            .collect();
        assert_eq!(
            differing,
            [(
                "after_sleep_cmd = hyprctl dispatch 'hl.dsp.global(\"quickshell:lockFocus\")'",
                "after_sleep_cmd = caelestia shell -d >/dev/null 2>&1; hyprctl dispatch \
                 'hl.dsp.global(\"caelestia:lock\")'",
            )],
            "every directive the hand-fix wrote, byte for byte, except the one composite that \
             is not a dispatch resolution"
        );
        assert!(
            idle.contains(
                r#"after_sleep_cmd = hyprctl dispatch 'hl.dsp.global("quickshell:lockFocus")'"#
            ),
            "and the composite the recipe cannot express is still the donor's own, reported as \
             dead: {idle}"
        );
        assert_eq!(report.dead_names, ["quickshell:lockFocus"]);
    }

    /// The env block the recipe materializes says exactly what the hand-fix wrote
    /// by hand, between markers the user can see and delete.
    #[test]
    fn the_managed_env_block_says_what_the_hand_fix_wrote_by_hand() {
        let donor = fixture("donor");
        let reference = fixtures().join("reference");
        pass(donor.path(), donor.path(), "caelestia");

        let ours = read(donor.path(), ".config/hypr/custom/env.lua");
        let hand = read(&reference, ".config/hypr/custom/env.lua");
        let assignment = hand
            .lines()
            .find(|line| line.starts_with("hl.env("))
            .expect("the hand-fix's assignment");
        assert!(
            ours.contains(assignment),
            "the same assignment, byte for byte: {assignment}"
        );
        assert!(ours.starts_with("# >>> riceswap:adapt >>>\n"), "{ours}");
        assert!(ours.ends_with("# <<< riceswap:adapt <<<\n"), "{ours}");
        assert!(
            !ours.contains("does not match this machine"),
            "the prose above it was the hand-fix's, and this layer writes no prose"
        );
    }

    /// The profile's own env file is the user's outside the markers, and the
    /// markers are the only thing a re-run replaces.
    #[test]
    fn the_managed_block_is_written_once_and_never_touches_what_is_around_it() {
        let donor = fixture("donor");
        let env = donor.path().join(".config/hypr/custom/env.lua");
        std::fs::write(&env, "hl.env(\"MINE\", \"1\")\n").expect("a user line");

        let first = pass(donor.path(), donor.path(), "caelestia");
        let after_first = read(donor.path(), ".config/hypr/custom/env.lua");
        assert_eq!(first.env.as_ref().map(|block| block.written), Some(true));
        assert!(
            after_first.starts_with("hl.env(\"MINE\", \"1\")\n"),
            "{after_first}"
        );
        assert!(
            after_first.contains("# >>> riceswap:adapt >>>"),
            "and the block goes after it, not instead of it: {after_first}"
        );
        assert!(
            first
                .backups
                .iter()
                .any(|backup| backup.ends_with(".env.lua")),
            "an existing file's pre-change bytes are kept like any other: {:?}",
            first.backups
        );

        // A user edit above the block, and a value that changes below it: the
        // second pass keeps the edit and replaces only the block's own lines.
        let block = &after_first[after_first.find(recipe::ENV_OPEN).expect("the block")..];
        std::fs::write(
            &env,
            format!("hl.env(\"MINE\", \"1\")\nhl.env(\"ALSO_MINE\", \"2\")\n\n{block}"),
        )
        .expect("a second user line");
        std::fs::write(
            donor.path().join("adapt.toml"),
            "[env]\nCAELESTIA_WALLPAPERS_DIR = \"~/Pictures/Walls\"\n",
        )
        .expect("a human-tier override");
        let second = pass(donor.path(), donor.path(), "caelestia");
        let after_second = read(donor.path(), ".config/hypr/custom/env.lua");

        assert!(after_second.contains("hl.env(\"MINE\", \"1\")\n"));
        assert!(after_second.contains("hl.env(\"ALSO_MINE\", \"2\")\n"));
        assert!(
            after_second.contains("os.getenv(\"HOME\") .. \"/Pictures/Walls\""),
            "and the block now carries the human tier's value: {after_second}"
        );
        assert!(
            !after_second.contains("~/Wallpapers\")"),
            "the old one is gone: {after_second}"
        );
        assert_eq!(second.env.as_ref().map(|block| block.written), Some(true));

        // And a settled block costs nothing at all.
        let stamp_before = stamp(donor.path());
        let third = pass(donor.path(), donor.path(), "caelestia");
        assert_eq!(third.env.as_ref().map(|block| block.written), Some(false));
        assert!(
            !third
                .files_written
                .contains(&".config/hypr/custom/env.lua".to_string())
        );
        assert_eq!(stamp_before, stamp(donor.path()));
    }

    /// A file the layer cannot claim is left whole, and says why: two managed
    /// blocks in one `env.lua` is a file it cannot tell it wrote, so it refuses
    /// exactly as it refuses an unterminated marker — through the same warning,
    /// with the same "not touched" answer.
    #[test]
    fn an_env_file_with_two_managed_blocks_is_refused_and_left_whole() {
        let donor = fixture("donor");
        let env = donor.path().join(".config/hypr/custom/env.lua");
        let block = format!(
            "{}\nhl.env(\"MINE\", \"1\")\n{}\n",
            recipe::ENV_OPEN,
            recipe::ENV_CLOSE
        );
        let twice = format!("{block}{block}");
        std::fs::write(&env, &twice).expect("two blocks");

        let report = pass(donor.path(), donor.path(), "caelestia");

        assert_eq!(
            std::fs::read_to_string(&env).expect("read it back"),
            twice,
            "byte for byte: refusing is the whole point"
        );
        assert!(
            report.env.is_none(),
            "so the report carries no block for this file: {report:?}"
        );
        assert!(
            !report
                .files_written
                .iter()
                .any(|written| written.ends_with("custom/env.lua")),
            "{:?}",
            report.files_written
        );
        let note = report
            .warnings
            .iter()
            .find(|warning| warning.contains("custom/env.lua"))
            .unwrap_or_else(|| panic!("the refusal is reported: {:?}", report.warnings));
        assert!(
            note.contains("opens at line 1 and again at line 4")
                && note.contains("can only hold one"),
            "{note}"
        );
    }

    /// A deleted env file is recreated with the block, and a deleted directory
    /// is guaranteed again: both are guarantees, not one-time setup.
    #[test]
    fn a_deleted_env_file_and_a_deleted_directory_come_back_on_the_next_switch() {
        let donor = fixture("donor");
        pass(donor.path(), donor.path(), "caelestia");
        std::fs::remove_file(donor.path().join(".config/hypr/custom/env.lua")).expect("delete");
        std::fs::remove_dir(donor.path().join("Wallpapers")).expect("delete");

        let report = pass(donor.path(), donor.path(), "caelestia");

        assert_eq!(
            report.dirs_created,
            ["Wallpapers"],
            "the guarantee holds again"
        );
        assert_eq!(report.env.as_ref().map(|block| block.written), Some(true));
        assert!(
            read(donor.path(), ".config/hypr/custom/env.lua")
                .starts_with("# >>> riceswap:adapt >>>"),
            "and the file is back, block and all"
        );
    }

    // ------------------------------------------------------------ the layers

    /// Precedence, end to end: the same dispatched name answered by the human
    /// tier and by the built-in, in opposite ways, and the human tier is the one
    /// that lands.
    #[test]
    fn a_profiles_own_recipe_outranks_the_built_in_one_for_the_same_name() {
        let donor = fixture("donor");
        std::fs::write(
            donor.path().join(recipe::ADAPT_FILE),
            concat!(
                "[[resolution]]\ndispatched = \"quickshell:regionScreenshot\"\n",
                "exec = \"notify-send 'snip' 'done'\"\n",
            ),
        )
        .expect("a human-tier override");

        let report = pass(donor.path(), donor.path(), "caelestia");

        let overridden = report
            .resolutions
            .iter()
            .find(|applied| applied.dispatched == "quickshell:regionScreenshot")
            .expect("the name is answered by somebody");
        assert_eq!(overridden.layer, "adapt", "the human tier answers it");
        assert_eq!(overridden.kind, "exec");
        assert_eq!(
            overridden.exec.as_deref(),
            Some("notify-send 'snip' 'done'")
        );
        let binds = read(donor.path(), ".config/hypr/hyprland/keybinds.lua");
        assert!(binds.contains(r#"hl.dsp.exec_cmd("notify-send 'snip' 'done'")"#));
        assert!(
            !binds.contains("caelestia:screenshotClip"),
            "the built-in's answer did not also land: {binds}"
        );
        // And the built-in still answers everything it is the only one to speak
        // of, so precedence is per name and not per tier.
        assert_eq!(report.layers, ["adapt", "builtin"]);
        assert!(
            report
                .resolutions
                .iter()
                .any(|applied| applied.layer == "builtin"),
            "the rest of the codebook is untouched: {:?}",
            report.resolutions
        );
    }

    /// The engine's exact-name move is what runs in the *absence* of a recipe,
    /// not a rival to one. A dispatch whose bare name the activating shell
    /// registers under its own appid is mechanically movable — and a profile
    /// that says this key means something else is the authority for that, so
    /// the declared target is what lands.
    #[test]
    fn a_declared_decision_outranks_the_mechanical_move_for_the_same_name() {
        let profile = tree(&[
            (
                ".config/quickshell/demo/modules/Shortcuts.qml",
                concat!(
                    "GlobalShortcut {\n    appid: \"demo\"\n}\n",
                    "GlobalShortcut {\n    name: \"screenshot\"\n    onPressed: {}\n}\n",
                    "GlobalShortcut {\n    name: \"screenshotClip\"\n    onPressed: {}\n}\n",
                    "GlobalShortcut {\n    name: \"lock\"\n    onPressed: {}\n}\n",
                ),
            ),
            (
                ".config/hypr/hyprland/keybinds.lua",
                concat!(
                    "hl.bind(\"SUPER + S\", hl.dsp.global(\"quickshell:screenshot\"))\n",
                    "hl.bind(\"SUPER + L\", hl.dsp.global(\"quickshell:lock\"))\n",
                ),
            ),
            (
                "adapt.toml",
                "[[resolution]]\ndispatched = \"quickshell:screenshot\"\nto = \"screenshotClip\"\n",
            ),
        ]);

        let report = pass(profile.path(), profile.path(), "demo");

        let binds = read(profile.path(), ".config/hypr/hyprland/keybinds.lua");
        assert!(
            binds.contains(r#"hl.dsp.global("demo:screenshotClip")"#),
            "the recipe decided what the key means, so its target is what landed: {binds}"
        );
        assert!(
            !binds.contains(r#"hl.dsp.global("demo:screenshot")"#),
            "not the mechanical move, which the recipe outranked: {binds}"
        );
        assert!(
            binds.contains(r#"hl.dsp.global("demo:lock")"#),
            "and the name no layer speaks for still moves by itself: {binds}"
        );
        let decided = report
            .resolutions
            .iter()
            .find(|applied| applied.dispatched == "quickshell:screenshot")
            .expect("the recipe answered it");
        assert_eq!(decided.layer, "adapt");
        assert_eq!(decided.to.as_deref(), Some("demo:screenshotClip"));
        assert!(
            report.dead_names.is_empty(),
            "and neither name is left dead: {report:?}"
        );
        assert_eq!(
            report
                .proposals
                .iter()
                .find(|proposal| proposal.dispatched == "quickshell:screenshot")
                .map(|proposal| proposal.target.as_deref()),
            Some(None),
            "the proposal's `target` is the engine's own move, which is not what was applied"
        );
    }

    /// The user tier is read, and nothing ever writes it: a recipe that lives
    /// in the data directory is authority over itself and over nothing else.
    #[test]
    fn a_user_tier_recipe_is_read_and_never_written() {
        let donor = fixture("donor");
        let data = donor.path().join(".local").join("share").join("riceswap");
        let recipes = data.join(recipe::RECIPES_DIR);
        std::fs::create_dir_all(&recipes).expect("the user tier's directory");
        let user = recipes.join("caelestia.toml");
        std::fs::write(
            &user,
            concat!(
                "schema_version = 1\n",
                "[shell]\nname = \"caelestia\"\nappid = \"caelestia\"\n",
                "[[resolution]]\ndispatched = \"quickshell:lockFocus\"\n",
                "exec = \"caelestia shell -d >/dev/null 2>&1; hyprctl dispatch 'hl.dsp.global(\\\"quickshell:lock\\\")'\"\n",
            ),
        )
        .expect("a user recipe");
        let before = std::fs::read(&user).expect("read it back");

        let report = pass(donor.path(), donor.path(), "caelestia");

        assert_eq!(report.layers, ["builtin", "user"]);
        let answered = report
            .resolutions
            .iter()
            .find(|applied| applied.dispatched == "quickshell:lockFocus")
            .expect("the name is answered");
        assert_eq!(
            answered.layer, "user",
            "the one name the built-in deliberately leaves alone, answered by the tier below it"
        );
        assert_eq!(answered.kind, "exec");
        assert_eq!(
            report
                .resolutions
                .iter()
                .find(|applied| applied.dispatched == "quickshell:barToggle")
                .map(|applied| applied.layer.as_str()),
            Some("builtin"),
            "and a name two tiers speak of still goes to the higher one"
        );
        assert_eq!(
            std::fs::read(&user).expect("read it back again"),
            before,
            "and no tier ever writes another tier's recipe"
        );
        assert_eq!(
            std::fs::read_dir(&recipes).expect("the directory").count(),
            1,
            "nor invents a recipe of its own"
        );
    }

    /// A recipe that names a shortcut this shell does not register is a stale
    /// recipe, and a stale recipe writes nothing: the bind keeps the name it had
    /// and the reason is reported, because silently installing a bind that can
    /// only fail is the one outcome nobody could debug from a keypress.
    #[test]
    fn a_recipe_pointing_at_an_unregistered_shortcut_is_reported_and_writes_nothing() {
        let profile = tree(&[
            (
                ".config/quickshell/demo/modules/Shortcuts.qml",
                "GlobalShortcut {\n    name: \"lock\"\n    onPressed: {}\n}\n",
            ),
            (
                ".config/hypr/hyprland/keybinds.lua",
                "hl.bind(\"SUPER + SHIFT + S\", hl.dsp.global(\"quickshell:regionScreenshot\"))\n",
            ),
            (
                "adapt.toml",
                "[[resolution]]\ndispatched = \"quickshell:regionScreenshot\"\n\
                 to = \"screenshotClip\"\n",
            ),
        ]);
        let before = read(profile.path(), ".config/hypr/hyprland/keybinds.lua");

        let report = pass(profile.path(), profile.path(), "demo");

        assert!(report.files_written.is_empty(), "{report:?}");
        assert_eq!(report.resolutions, Vec::<Applied>::new());
        assert_eq!(report.dead_names, ["quickshell:regionScreenshot"]);
        assert_eq!(
            before,
            read(profile.path(), ".config/hypr/hyprland/keybinds.lua"),
            "byte for byte"
        );
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("quickshell:screenshotClip")
                    && warning.contains("does not register")
                    && warning.contains("adapt.toml")),
            "and it says which name was wanted and why it was refused: {:?}",
            report.warnings
        );
    }

    /// A pinned appid is a fact to check, never an instruction to obey: the
    /// registry derived from the profile's own QML is what says what exists, so
    /// a pin that contradicts it is reported as drift and the derived namespace
    /// is what the pass uses.
    #[test]
    fn an_appid_pin_that_contradicts_the_qml_is_reported_as_drift_and_ignored() {
        let profile = tree(&[
            (
                ".config/quickshell/demo/components/Base.qml",
                "GlobalShortcut {\n    appid: \"demo\"\n}\n",
            ),
            (
                ".config/quickshell/demo/modules/Shortcuts.qml",
                "GlobalShortcut {\n    name: \"screenshotClip\"\n    onPressed: {}\n}\n",
            ),
            (
                ".config/hypr/hyprland/keybinds.lua",
                "hl.bind(\"SUPER + SHIFT + S\", hl.dsp.global(\"quickshell:regionScreenshot\"))\n",
            ),
            (
                "adapt.toml",
                concat!(
                    "[shell]\nappid = \"quickshell\"\n",
                    "[[resolution]]\ndispatched = \"quickshell:regionScreenshot\"\n",
                    "to = \"screenshotClip\"\n",
                ),
            ),
        ]);

        let report = pass(profile.path(), profile.path(), "demo");

        assert_eq!(report.appid.as_deref(), Some("demo"), "what the QML says");
        assert_eq!(report.appid_drift.len(), 1, "{:?}", report.appid_drift);
        assert!(
            report.appid_drift[0].contains("`quickshell`")
                && report.appid_drift[0].contains("registers under `demo`"),
            "naming both: {:?}",
            report.appid_drift
        );
        assert_eq!(
            report.resolutions[0].to.as_deref(),
            Some("demo:screenshotClip"),
            "so the unprefixed target was joined with the *derived* namespace"
        );
        assert!(
            read(profile.path(), ".config/hypr/hyprland/keybinds.lua")
                .contains(r#"hl.dsp.global("demo:screenshotClip")"#),
            "and the bind moved there"
        );
    }

    /// The recipe vocabulary is `to`, `exec` and `drop`, and each is rendered in
    /// the dialect of the file it lands in: the Lua call forms, and both of the
    /// plain-conf ways of spelling one.
    #[test]
    fn every_resolution_lands_in_the_dialect_of_the_file_it_lands_in() {
        let profile = tree(&[
            (
                ".config/quickshell/demo/modules/Shortcuts.qml",
                concat!(
                    "GlobalShortcut {\n    name: \"clipboard\"\n    onPressed: {}\n}\n",
                    "GlobalShortcut {\n    name: \"lock\"\n    onPressed: {}\n}\n",
                ),
            ),
            (
                ".config/hypr/hyprland/keybinds.lua",
                concat!(
                    "hl.bind(\"SUPER + V\", hl.dsp.global(\"donor:clips\"))\n",
                    "hl.bind(\"SUPER + S\", hl.dsp.global(\"donor:cheatsheet\"))\n",
                ),
            ),
            (
                ".config/hypr/hypridle.conf",
                concat!(
                    "bind =SUPER, C, global, donor:clips\n",
                    "bind =SUPER, E, global, donor:cheatsheet\n",
                    "bind =SUPER, G, exec, hyprctl dispatch global donor:screenshot\n",
                    "bind =SUPER, H, exec, donor:launcher\n",
                    "$lock_cmd = hyprctl dispatch 'hl.dsp.global(\"donor:lock\")' & hyprlock\n",
                ),
            ),
            (
                "adapt.toml",
                concat!(
                    "[[resolution]]\ndispatched = \"donor:clips\"\n",
                    "to = \"clipboard\"\nbind_options = [\"locked\"]\n",
                    "[[resolution]]\ndispatched = \"donor:cheatsheet\"\n",
                    "exec = \"pkill fuzzel || fuzzel\"\n",
                    "[[resolution]]\ndispatched = \"donor:screenshot\"\n",
                    "exec = \"pkill slurp || grim -g '$(slurp)' - | wl-copy\"\n",
                ),
            ),
        ]);

        let report = pass(profile.path(), profile.path(), "demo");

        // The Lua dialect: the token moves, and an option goes in the table that
        // is already there.
        let lua = read(profile.path(), ".config/hypr/hyprland/keybinds.lua");
        assert!(
            lua.contains(
                "hl.bind(\"SUPER + V\", hl.dsp.global(\"quickshell:clipboard\"), { locked = true })"
            ),
            "{lua}"
        );
        assert!(
            lua.contains("hl.bind(\"SUPER + S\", hl.dsp.exec_cmd(\"pkill fuzzel || fuzzel\"))\n"),
            "and its neighbour becomes the command: {lua}"
        );
        assert!(!lua.contains("donor:"), "{lua}");

        // The plain-conf dialect, twice: a dispatcher keyword followed by a name
        // keeps the key and becomes `exec`, and a shell command that ends in a
        // dispatch becomes the command itself.
        let conf = read(profile.path(), ".config/hypr/hypridle.conf");
        assert!(
            conf.contains("bind =SUPER, C, global, quickshell:clipboard\n"),
            "a `to` on a keyword line only has to move the name: {conf}"
        );
        assert!(
            conf.contains("bind =SUPER, E, exec, pkill fuzzel || fuzzel\n"),
            "an `exec` swaps the keyword for the command: {conf}"
        );
        assert!(
            conf.contains("bind =SUPER, G, exec, pkill slurp || grim -g '$(slurp)' - | wl-copy\n"),
            "and takes the whole `hyprctl dispatch global …` run with it: {conf}"
        );
        // A conf `exec` dispatcher carrying a bare name is not a global dispatch
        // at all — the name is a command's argument, not a shortcut — so the
        // pass has no opinion about it and says nothing.
        assert!(
            conf.contains("bind =SUPER, H, exec, donor:launcher\n"),
            "untouched, and unreported: {conf}"
        );
        // The Lua call inside a plain conf's shell string stays that form: this
        // Hyprland evaluates `dispatch` arguments as Lua (FC-7), so the engine's
        // exact-name move lands in the file's own idiom.
        assert!(
            conf.contains(
                r#"$lock_cmd = hyprctl dispatch 'hl.dsp.global("quickshell:lock")' & hyprlock"#
            ),
            "{conf}"
        );
        // One note, and it is the honest one: the recipe asks for a bind option,
        // the plain conf line has no option table to put it in, and the shortcut
        // still moved. A note rather than a refusal, because the option is the
        // smaller half of the decision.
        assert_eq!(
            report.warnings,
            [format!(
                "the recipe's bind options for `donor:clips` were not applied: {}",
                "it is not inside a quoted `global(…)` call"
            )],
            "{report:?}"
        );
        assert!(
            report.dirs_created.is_empty() && report.env.is_none(),
            "a recipe that declares no env or dirs materializes neither: {report:?}"
        );
    }

    /// A drop removes a whole statement — including one spread over two lines,
    /// and including a bind nested inside something the pass cannot bound
    /// halfway out of — and refuses the shapes where removal would take a
    /// neighbour with it, because a config with a missing line is a config
    /// nobody can debug from a keypress.
    #[test]
    fn a_drop_takes_the_whole_statement_and_refuses_what_it_cannot_bound() {
        let profile = tree(&[
            (
                ".config/quickshell/demo/modules/Shortcuts.qml",
                "GlobalShortcut {\n    name: \"lock\"\n    onPressed: {}\n}\n",
            ),
            (
                ".config/hypr/hyprland/keybinds.lua",
                concat!(
                    "hl.bind(\"SUPER_L\", hl.dsp.global(\"donor:number\"),\n",
                    "    { ignore_mods = true, release = true })\n",
                    "hl.bind(\"SUPER + K\", hl.dsp.global(\"donor:number\"))\n",
                    "hl.define_submap(\"virtual-machine\", function()\n",
                    "    hl.bind(\"SUPER + ALT + F1\", hl.dsp.global(\"donor:number\"))\n",
                    "end)\n",
                    "hl.bind(\"SUPER + P\", hl.dsp.window.pin()) ",
                    "hl.bind(\"SUPER + J\", hl.dsp.global(\"donor:number\"))\n",
                    "local overlay = {\n",
                    "    dispatch = hl.dsp.global(\"donor:number\"),\n",
                    "}\n",
                    "hl.bind(\"SUPER + R\", hl.dsp.global(\"donor:lock\"))\n",
                ),
            ),
            (
                "adapt.toml",
                concat!(
                    "[[resolution]]\ndispatched = \"donor:number\"\ndrop = true\n",
                    "[[resolution]]\ndispatched = \"donor:lock\"\nexec = \"true\"\n",
                ),
            ),
        ]);

        let report = pass(profile.path(), profile.path(), "demo");

        let binds = read(profile.path(), ".config/hypr/hyprland/keybinds.lua");
        assert!(
            !binds.contains("hl.bind(\"SUPER_L\""),
            "a bind spread over two lines goes as one statement: {binds}"
        );
        assert!(!binds.contains("hl.bind(\"SUPER + K\""), "{binds}");
        assert!(
            binds.contains("hl.define_submap(\"virtual-machine\", function()\nend)"),
            "and the submap it cannot widen out of loses only the bind, never its braces: {binds}"
        );
        assert!(
            binds.contains("local overlay = {\n    dispatch = hl.dsp.global(\"donor:number\"),\n}"),
            "a dispatch inside a table is not a statement this can remove: {binds}"
        );
        assert!(
            binds.contains("hl.bind(\"SUPER + J\", hl.dsp.global(\"donor:number\"))\n"),
            "and neither is one that shares its line with another bind: {binds}"
        );
        assert_eq!(
            report.dead_names,
            ["donor:number"],
            "so what refused to move is still dead, and is still the report's to say"
        );
        let dropped = report
            .resolutions
            .iter()
            .find(|applied| applied.dispatched == "donor:number")
            .expect("three of the five sites were dropped");
        assert_eq!(dropped.kind, "drop");
        assert_eq!(
            dropped.sites.len(),
            3,
            "the three lines it did remove are named; the two it refused are in the warnings \
             below, which is where a refusal belongs: {:?}",
            dropped.sites
        );
        assert_eq!(
            report
                .warnings
                .iter()
                .filter(|warning| warning.contains("not in a statement this can remove whole"))
                .count(),
            2,
            "two refusals, each naming the line it would not touch: {:?}",
            report.warnings
        );
    }

    /// The report is the contract the switch envelope carries, and every claim
    /// the layer makes is a key on it: which tiers spoke, what each dead name
    /// became, what was materialized, and what the keep set was.
    #[test]
    fn the_report_says_what_the_recipe_layer_did() {
        let donor = fixture("donor");
        let report = pass(donor.path(), donor.path(), "caelestia");

        assert_eq!(report.layers, ["builtin"], "which tiers spoke");
        assert_eq!(
            report.registered, 22,
            "the engine's own facts are unchanged"
        );
        assert_eq!(
            report.proposals.len(),
            25,
            "every dead name is still reported"
        );
        assert_eq!(report.dead_names, ["quickshell:lockFocus"]);
        assert_eq!(
            report.foreign_entries,
            ["quickshell:riceswap-toggle"],
            "the keep set the pass ran with"
        );
        assert_eq!(
            report.env,
            Some(EnvBlock {
                file: ".config/hypr/custom/env.lua".to_string(),
                entries: vec![(
                    "CAELESTIA_WALLPAPERS_DIR".to_string(),
                    "~/Wallpapers".to_string()
                )],
                written: true,
            }),
            "the managed block, with the value as declared"
        );
        assert_eq!(
            report.dirs_created,
            ["Wallpapers"],
            "home-relative, as a user says it"
        );
        assert!(donor.path().join("Wallpapers").is_dir());
        assert_eq!(report.appid_drift, Vec::<String>::new());
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);

        // One entry per decision, with the kind and the layer that made it.
        let by_name = |name: &str| {
            report
                .resolutions
                .iter()
                .find(|applied| applied.dispatched == name)
                .cloned()
                .unwrap_or_else(|| panic!("{name} must be resolved"))
        };
        assert_eq!(by_name("quickshell:sessionToggle").kind, "to");
        assert_eq!(
            by_name("quickshell:sessionToggle").to.as_deref(),
            Some("caelestia:session")
        );
        assert_eq!(by_name("quickshell:barToggle").kind, "drop");
        assert_eq!(by_name("quickshell:barToggle").to, None);
        assert_eq!(by_name("quickshell:regionRecord").kind, "exec");
        assert_eq!(
            by_name("quickshell:regionRecord").exec.as_deref(),
            Some("caelestia record -r")
        );
        assert_eq!(by_name("quickshell:barToggle").sites.len(), 1);
        assert_eq!(by_name("quickshell:workspaceNumber").sites.len(), 4);
    }

    /// Every file under the profile, with its bytes and modification time: the
    /// snapshot an idempotence claim is measured against.
    fn stamp(root: &Path) -> BTreeMap<String, (Vec<u8>, std::time::SystemTime)> {
        let mut files = BTreeMap::new();
        let mut into = Vec::new();
        collect(root, &mut into, &mut |_| true);
        for path in into {
            let bytes = std::fs::read(&path).expect("read file");
            let modified = std::fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .expect("modification time");
            files.insert(relative(&path), (bytes, modified));
        }
        files
    }
}
