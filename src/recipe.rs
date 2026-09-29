//! The declarative layer (issue #36) — the recipe tier the engine's proposals
//! are answered by.
//!
//! #35 derived what a profile's shell registers and reported everything else as
//! a proposal, because the half of the problem that needs *taste* is not
//! derivable: that a donor's `regionScreenshot` meant "snip to clipboard", that
//! caelestia calls that `screenshotClip`, and that the intents nothing answers
//! for are better dropped than faked. This module is where that judgement lives
//! — as data, in one TOML schema shared by three tiers:
//!
//! * `adapt.toml`, beside a profile's manifest: the human tier, and the one a
//!   user edits when a rice is theirs rather than ours.
//! * the recipes compiled into the binary from `recipes/`, keyed by shell name.
//! * `<data-dir>/recipes/<shell>.toml`: the user tier. Parsed, and read as
//!   authority over nothing but itself — no tier ever writes another tier's
//!   files (that is the write-back a later ticket adds).
//!
//! When two layers speak for the same dispatched name the human tier wins, then
//! the built-in, then the user tier; names only one layer speaks simply union.
//! A declared `[shell] appid` is a *pin*, never a source of truth: the registry
//! derived from the activating profile's own QML answers what exists, and a pin
//! that disagrees with it is reported as drift (see `appid_pin`).
//!
//! What the schema may say, and nothing else:
//!
//! ```toml
//! schema_version = 1
//!
//! [shell]
//! name = "caelestia"
//! appid = "caelestia"
//!
//! [env]
//! CAELESTIA_WALLPAPERS_DIR = "~/Wallpapers"
//!
//! [[dirs]]
//! path = "~/Wallpapers"
//!
//! [[dirs]]
//! path = "~/Pictures/Wallpapers"
//! symlink_to = "~/Wallpapers"
//!
//! [foreign]
//! entries = ["quickshell:riceswap-toggle"]
//!
//! [ipc]                               # optional; only when the shell has one
//! probe = "qs -c caelestia ipc call panel state"
//!
//! [[resolution]]                      # exactly one of `to`, `exec`, `drop`
//! dispatched = "quickshell:regionScreenshot"
//! to = "screenshotClip"               # or `to = "caelestia:screenshotClip"`
//! bind_options = ["release"]          # only alongside `to`
//!
//! [[resolution]]
//! dispatched = "quickshell:overviewClipboardToggle"
//! exec = "pkill fuzzel || caelestia clipboard"
//!
//! [[resolution]]
//! dispatched = "quickshell:barToggle"
//! drop = true
//! ```
//!
//! Three rules keep the tier from being able to do damage.
//!
//! **A resolution is checked against reality before it is applied.** A `to`
//! target is only rewritten when the derived registry holds it; a target the
//! shell does not register is reported and the bind is left byte for byte as it
//! was. Derivation, not declaration, is what says what exists — and that is the
//! *only* thing derivation decides: whether the shell's own namespace already
//! registers this name is a mechanical fact, and a declared decision about what
//! the key means outranks the engine's fallback move for it.
//!
//! **A declared path is a shape, not a location.** Every `[[dirs]]` path — and
//! every `~` in an `[env]` value — must land under the pass's `$HOME`, and a
//! path carrying a `..` component is refused outright. This is the only class
//! of write the layer performs outside the profile, and it is bounded on
//! purpose.
//!
//! **The managed block is managed, not owned.** `[env]` is materialized into
//! the profile's own `custom/env.lua` between two marker lines: everything
//! outside them is the user's and is never read into a decision or written, and
//! a pass that would leave the file identical writes nothing at all.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The schema version this build reads. A file declaring anything else is
/// reported, never guessed at — the same rule the manifest loader follows.
pub const SCHEMA_VERSION: u32 = 1;

/// The human tier's file, beside a profile's `profile.toml`.
pub const ADAPT_FILE: &str = "adapt.toml";

/// The user tier's directory, under RiceSwap's own data dir.
pub const RECIPES_DIR: &str = "recipes";

/// The markers bounding the block this layer owns inside `custom/env.lua`.
pub const ENV_OPEN: &str = "# >>> riceswap:adapt >>>";
pub const ENV_CLOSE: &str = "# <<< riceswap:adapt <<<";

/// The recipes compiled into the binary, keyed by the shell they are about.
pub const BUILT_IN: &[(&str, &str)] = &[
    ("caelestia", include_str!("../recipes/caelestia.toml")),
    ("ii", include_str!("../recipes/ii.toml")),
];

// ---------------------------------------------------------------- the schema

/// One recipe document. Every tier parses this and nothing else — the human file
/// and the machine recipe are the same artifact type, so a resolution that
/// works in one works in the others.
///
/// It also *renders*, which is what the research tier's write-back needs (#38):
/// a recipe produced by a model has to be the same artifact as one written by
/// hand, and the only way to know that is to render this type and parse it back
/// with the same code that reads a hand-written file. `deny_unknown_fields`
/// stays on both sides, which is what makes that round trip a real proof.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    /// The format marker. Optional so a bare `adapt.toml` needs no ceremony, and
    /// checked when present.
    pub schema_version: Option<u32>,
    /// Which shell this recipe is about, and the namespace it was authored
    /// against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<Shell>,
    /// Variables to guarantee in the profile's managed env block.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Paths to guarantee outside the profile — the wallpaper-dir class.
    #[serde(default)]
    pub dirs: Vec<Dir>,
    /// The proposal vocabulary: one `[[resolution]]` per dispatched name. The
    /// table is singular in the document and plural in memory, which is what
    /// every other array of tables in this schema does.
    #[serde(default, rename = "resolution")]
    pub resolutions: Vec<Declared>,
    /// Namespaces that belong to another live shell and are never rewritten.
    #[serde(default)]
    pub foreign: Foreign,
    /// How to ask the running shell whether it is there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipc: Option<Ipc>,
}

/// The `[ipc]` table: the liveness probe this shell answers.
///
/// It exists because a process surviving a start is a weaker fact than a shell
/// answering a question. A `qs -c ii` that stays up for its grace window has
/// proved it did not exit; it has not proved it got past its own initialization,
/// which is where a broken QML file lands. A shell that exposes an IPC target
/// can be asked instead, and this is where the question is written down.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Ipc {
    /// A one-shot command that exits 0 only while the shell answers, and
    /// non-zero otherwise. Run as a shell reads it, from the operation's own
    /// environment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe: Option<String>,
}

/// The `[shell]` table.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Shell {
    /// The Quickshell config the recipe is about. A built-in recipe must name
    /// the shell it is filed under; the human tier inherits the profile's.
    pub name: Option<String>,
    /// The namespace the recipe was authored against — a pin for the drift
    /// check, never a source of truth.
    pub appid: Option<String>,
}

/// One `[[dirs]]` entry: a path to guarantee, and where a symlink at it should
/// point. With no `symlink_to` the path is guaranteed to be a directory.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Dir {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symlink_to: Option<String>,
}

/// The `[foreign]` table: names the engine must treat as another live shell's.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Foreign {
    #[serde(default)]
    pub entries: Vec<String>,
}

/// One `[[resolution]]` entry, exactly as declared. The trilemma — one of `to`,
/// `exec`, `drop` — is settled by [`Declared::decide`], so an entry that names
/// two sides is a *finding*, not a silent preference.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Declared {
    /// The join key: `ns:name` for one namespace, or the bare name to answer
    /// whichever namespace the dead name was dispatched under.
    pub dispatched: String,
    /// The target shortcut, with or without the appid.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// The command to run instead of dispatching.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exec: Option<String>,
    /// `true` to remove the statement that dispatches it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drop: Option<bool>,
    /// Options to add to the bind's own option table, alongside `to`.
    #[serde(default)]
    pub bind_options: Vec<String>,
}

/// What a resolution asks the pass to do. The kind is a closed set, because
/// every consumer downstream — the rewrite, the report, the panel — has to be
/// able to switch on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Re-point the dispatch at a shortcut the shell is proved to register.
    To {
        /// The target as declared, unprefixed or prefixed.
        target: String,
        /// Options to set on the bind, if the line has an option table.
        options: Vec<String>,
    },
    /// Replace the dispatch with a command.
    Exec {
        /// The command as a shell reads it, in whatever dialect the file it
        /// lands in wants.
        command: String,
    },
    /// Remove the statement that dispatches it.
    Drop,
}

impl Decision {
    /// The kind as the report names it.
    pub fn kind(&self) -> &'static str {
        match self {
            Decision::To { .. } => "to",
            Decision::Exec { .. } => "exec",
            Decision::Drop => "drop",
        }
    }
}

/// One resolution, joined: what it does, and under which name it was declared.
#[derive(Clone, Debug)]
pub struct Entry {
    /// The key as declared — `ns:name` or a bare name.
    pub dispatched: String,
    pub decision: Decision,
}

impl Declared {
    /// Which side of the trilemma this entry names, or why it names none.
    pub fn decide(&self) -> Result<Decision, String> {
        let mut sides: Vec<&str> = Vec::new();
        if self.to.as_deref().is_some_and(|to| !to.is_empty()) {
            sides.push("to");
        }
        if self
            .exec
            .as_deref()
            .is_some_and(|exec| !exec.trim().is_empty())
        {
            sides.push("exec");
        }
        if self.drop == Some(true) {
            sides.push("drop");
        }
        match sides.as_slice() {
            [one] => self.decision(one),
            [] if self.drop == Some(false) => Err(format!(
                "`{}` declares `drop = false`, which names no resolution at all; remove the \
                 entry or declare `to` or `exec`",
                self.dispatched
            )),
            [] => Err(format!(
                "`{}` declares none of `to`, `exec` or `drop`; a resolution is exactly one \
                 of the three",
                self.dispatched
            )),
            many => Err(format!(
                "`{}` declares {}; a resolution is exactly one of the three",
                self.dispatched,
                many.join(" and ")
            )),
        }
    }

    /// The decision for the one side an entry named.
    fn decision(&self, side: &str) -> Result<Decision, String> {
        if side != "to" && !self.bind_options.is_empty() {
            return Err(format!(
                "`bind_options` only means something alongside `to`, and `{}` names `{side}`",
                self.dispatched
            ));
        }
        match side {
            "to" => {
                if let Some(option) = self.bind_options.iter().find(|o| !is_identifier(o)) {
                    return Err(format!(
                        "`{option}` is not a bind option name (letters, digits and `_` only)"
                    ));
                }
                Ok(Decision::To {
                    target: self.to.clone().unwrap_or_default(),
                    options: self.bind_options.clone(),
                })
            }
            "exec" => Ok(Decision::Exec {
                command: self.exec.clone().unwrap_or_default(),
            }),
            _ => Ok(Decision::Drop),
        }
    }
}

// ------------------------------------------------------------------ the tiers

/// One loaded recipe file, with the lookup tables the pass joins against.
pub struct Tier {
    /// The precedence rank: `adapt`, `builtin` or `user`.
    pub name: &'static str,
    /// Where the document was read from, for the findings and the report.
    pub origin: String,
    /// Resolutions by fully qualified `ns:name`.
    by_entry: BTreeMap<String, Entry>,
    /// Resolutions by bare name — the join key the engine's proposals carry.
    by_name: BTreeMap<String, Entry>,
    env: BTreeMap<String, String>,
    dirs: BTreeMap<String, Dir>,
    foreign: BTreeSet<String>,
    appid: Option<String>,
    /// The `[ipc] probe` this tier declares, with the tier that asked for it.
    probe: Option<(&'static str, String)>,
}

impl Tier {
    /// Parses and validates one document, or explains why it says nothing.
    ///
    /// A document that will not parse, or that declares a version this build
    /// does not know, silences the whole tier — a recipe RiceSwap cannot read
    /// is not a recipe it may half-obey. An *entry* that will not validate is
    /// only itself: one bad `[[resolution]]` must not cost a tier its
    /// `[env]`, its `[[dirs]]` and every other decision.
    fn read(
        name: &'static str,
        path: &Path,
        raw: &str,
        findings: &mut Vec<String>,
    ) -> Option<Tier> {
        let origin = path.display().to_string();
        let recipe: Recipe = match toml::from_str(raw) {
            Ok(recipe) => recipe,
            Err(error) => {
                findings.push(format!("{origin} is not a valid recipe: {error}"));
                return None;
            }
        };
        if let Some(version) = recipe.schema_version
            && version != SCHEMA_VERSION
        {
            findings.push(format!(
                "{origin} declares `schema_version = {version}`, which this RiceSwap does not \
                 understand ({SCHEMA_VERSION}); it was not used"
            ));
            return None;
        }

        let mut by_entry = BTreeMap::new();
        let mut by_name = BTreeMap::new();
        for declared in &recipe.resolutions {
            // A key is a bare name or exactly one `appid:name`. Anything else —
            // a second colon, an empty half — would simply never match, so it
            // is a finding here rather than a resolution that never fires.
            let key = declared.dispatched.trim();
            let qualified = key.split(':').count() > 1;
            if key.is_empty() || (qualified && split_entry(key).is_none()) {
                findings.push(format!(
                    "{origin} has a [[resolution]] whose `dispatched` is not a name or an \
                     `appid:name`: `{}`",
                    declared.dispatched
                ));
                continue;
            }
            let decision = match declared.decide() {
                Ok(decision) => decision,
                Err(reason) => {
                    findings.push(format!("{origin} was not used: {reason}"));
                    continue;
                }
            };
            let entry = Entry {
                dispatched: key.to_string(),
                decision,
            };
            if qualified {
                by_entry.insert(key.to_string(), entry);
            } else {
                by_name.insert(key.to_string(), entry);
            }
        }

        let mut dirs = BTreeMap::new();
        for dir in recipe.dirs {
            if dir.path.trim().is_empty() {
                findings.push(format!("{origin} has a [[dirs]] entry with no `path`"));
                continue;
            }
            dirs.insert(dir.path.clone(), dir);
        }

        // The IPC probe is a *declared* question, so a tier that names one is
        // obeyed and a tier that names an empty or blank one is a finding: an
        // empty probe would be run on every switch and answer "the shell is
        // dead" about a shell nobody asked about.
        let probe = match recipe.ipc {
            Some(ipc) => match ipc.probe.map(|probe| probe.trim().to_string()) {
                Some(probe) if !probe.is_empty() => Some((name, probe)),
                _ => {
                    findings.push(format!(
                        "{origin} has an `[ipc]` table with no `probe` command; it names nothing \
                         to run"
                    ));
                    None
                }
            },
            None => None,
        };

        Some(Tier {
            name,
            origin,
            by_entry,
            by_name,
            env: recipe.env,
            dirs,
            foreign: recipe.foreign.entries.into_iter().collect(),
            appid: recipe.shell.and_then(|shell| shell.appid),
            probe,
        })
    }

    /// The recipe this tier resolves `entry` under, preferring a name it spelled
    /// with its namespace over the bare-name form.
    fn resolve(&self, entry: &str) -> Option<&Entry> {
        self.by_entry
            .get(entry)
            .or_else(|| self.by_name.get(name_of(entry)))
    }
}

/// What one tier decided for one name, and where it was read from.
pub struct Resolved<'a> {
    /// The tier that spoke: `adapt`, `builtin` or `user`.
    pub tier: &'static str,
    /// The file it was read from, named in every refusal so the user knows
    /// which recipe to edit.
    pub origin: &'a str,
    pub entry: &'a Entry,
}

/// Every tier that spoke, in the order a lookup walks them: human, built-in,
/// user.
pub struct Layers {
    tiers: Vec<Tier>,
}

impl Layers {
    /// Loads every tier that exists for this profile and shell: the human tier
    /// from `adapt.toml`, the built-in recipe compiled in for this shell, and
    /// the user tier from `<data-dir>/recipes/<shell>.toml`.
    ///
    /// Returns the catalog and the findings — the recipes that could not be
    /// read, the entries that named no resolution, the pins that contradict
    /// each other. A missing file is not a finding: most profiles have no
    /// `adapt.toml` and most shells no recipe, and saying so on every switch
    /// would be noise.
    pub fn load(profile: &Path, data: &Path, shell: &str) -> (Layers, Vec<String>) {
        let mut findings = Vec::new();
        let mut tiers = Vec::new();

        let adapt = profile.join(ADAPT_FILE);
        if let Some(tier) = read_if_present("adapt", &adapt, &mut findings) {
            tiers.push(tier);
        }

        // A built-in recipe is keyed by the shell it is about, and says so in
        // its own `[shell] name`. The two disagreeing is a mistake in a file
        // this pass cannot fix, so it is reported rather than served under a
        // name the recipe does not claim.
        if let Some((key, raw)) = BUILT_IN.iter().find(|(key, _)| *key == shell) {
            let origin = format!("the built-in `{key}` recipe");
            let declared = toml::from_str::<Recipe>(raw)
                .ok()
                .and_then(|recipe| recipe.shell)
                .and_then(|shell| shell.name);
            match declared {
                Some(name) if &name != key => findings.push(format!(
                    "{origin} declares `shell.name = \"{name}\"`; it was not used"
                )),
                _ => {
                    if let Some(tier) =
                        Tier::read("builtin", Path::new(&origin), raw, &mut findings)
                    {
                        tiers.push(tier);
                    }
                }
            }
        }

        let user = data.join(RECIPES_DIR).join(format!("{shell}.toml"));
        if let Some(tier) = read_if_present("user", &user, &mut findings) {
            tiers.push(tier);
        }

        (Layers { tiers }, findings)
    }

    /// The tiers that spoke, in precedence order — what the report names.
    pub fn names(&self) -> Vec<&'static str> {
        self.tiers.iter().map(|tier| tier.name).collect()
    }

    /// The decision for a dead `ns:name`, and the tier that made it. The first
    /// tier that speaks for the name wins; a name no tier speaks for keeps its
    /// place as a proposal.
    pub fn resolve(&self, entry: &str) -> Option<Resolved<'_>> {
        self.tiers.iter().find_map(|tier| {
            tier.resolve(entry).map(|entry| Resolved {
                tier: tier.name,
                origin: &tier.origin,
                entry,
            })
        })
    }

    /// The highest-precedence `[shell] appid` pin, with the tier that declared
    /// it — the fact the drift check compares against the derived registry.
    pub fn appid_pin(&self) -> Option<(&'static str, String)> {
        self.tiers
            .iter()
            .find_map(|tier| tier.appid.as_ref().map(|appid| (tier.name, appid.clone())))
    }

    /// Every foreign name any tier declared, unioned: a keep is a keep, and
    /// there is nothing to arbitrate between layers that all say "leave it".
    pub fn foreign(&self) -> BTreeSet<String> {
        self.tiers
            .iter()
            .flat_map(|tier| tier.foreign.iter().cloned())
            .collect()
    }

    /// The `[ipc] probe` the highest-precedence tier that declares one speaks
    /// for, and which tier it was — the same precedence as every other lookup,
    /// so a profile's own `adapt.toml` can correct a built-in probe by restating
    /// it. `None` when no tier declares a probe at all, which is the ordinary
    /// case: a shell with no IPC target has nothing to ask.
    pub fn ipc_probe(&self) -> Option<(&'static str, &str)> {
        self.tiers.iter().find_map(|tier| {
            tier.probe
                .as_ref()
                .map(|(name, probe)| (*name, probe.as_str()))
        })
    }

    /// The `[env]` variables every tier speaks for, higher tiers winning per
    /// name: `adapt.toml` can correct one value without restating the rest.
    ///
    /// The tiers are already in precedence order, so the first writer of a name
    /// is the one that keeps it — which is why this walks them forwards and a
    /// merge that walks them backwards would quietly invert the precedence.
    pub fn env(&self) -> BTreeMap<String, String> {
        let mut merged = BTreeMap::new();
        for tier in &self.tiers {
            for (name, value) in &tier.env {
                merged.entry(name.clone()).or_insert_with(|| value.clone());
            }
        }
        merged
    }

    /// The `[[dirs]]` entries every tier speaks for, higher tiers winning per
    /// path, in path order.
    pub fn dirs(&self) -> Vec<Dir> {
        let mut merged: BTreeMap<String, Dir> = BTreeMap::new();
        for tier in &self.tiers {
            for (path, dir) in &tier.dirs {
                merged.entry(path.clone()).or_insert_with(|| dir.clone());
            }
        }
        merged.into_values().collect()
    }
}

/// Reads a tier's file if it is there. An absent file is silence, not a finding.
fn read_if_present(name: &'static str, path: &Path, findings: &mut Vec<String>) -> Option<Tier> {
    match std::fs::read_to_string(path) {
        Ok(raw) => Tier::read(name, path, &raw, findings),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            findings.push(format!("cannot read {}: {error}", path.display()));
            None
        }
    }
}

// ------------------------------------------------------------ the env block

/// The managed block for `env`, as the file text: the marker lines and one
/// `hl.env(…)` per variable, in name order.
pub fn env_block(env: &BTreeMap<String, String>, home: &Path) -> String {
    let mut block = String::from(ENV_OPEN);
    block.push('\n');
    for (name, value) in env {
        block.push_str(&format!(
            "hl.env({}, {})\n",
            lua_string(name),
            lua_value(value, home)
        ));
    }
    block.push_str(ENV_CLOSE);
    block.push('\n');
    block
}

/// `existing` with the managed block installed, or — when it is already there —
/// with only what is between the markers replaced.
///
/// A block that is opened and never closed is refused rather than guessed at:
/// the file after an unterminated marker is the user's, and this layer does not
/// get to decide which of it is.
pub fn splice_env(existing: &str, block: &str) -> Result<String, String> {
    match block_region(existing)? {
        Some(region) => {
            let mut out = String::with_capacity(existing.len() + block.len());
            out.push_str(&existing[..region.0]);
            out.push_str(block);
            out.push_str(&existing[region.1..]);
            Ok(out)
        }
        None => {
            let mut out = String::with_capacity(existing.len() + block.len() + 2);
            out.push_str(existing);
            if !out.is_empty() {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
                // One blank line between the user's file and the block, so the
                // two are not read as one paragraph.
                if !out.ends_with("\n\n") {
                    out.push('\n');
                }
            }
            out.push_str(block);
            Ok(out)
        }
    }
}

/// The byte range the managed block occupies, from the opening marker's line
/// start to the line after the closing one.
///
/// `Ok(None)` is "no block here"; `Err` is "a block this layer must not touch",
/// which is a different answer and reaches the report as a warning. Two shapes
/// are refused rather than guessed at, and they are one refusal: a marker that
/// is opened and never closed, and a second opening marker — nested inside the
/// block or trailing after it. Either way the file holds something this layer
/// cannot tell it wrote, and writing over whichever region it found first would
/// leave the other half behind as stale `hl.env` lines.
fn block_region(existing: &str) -> Result<Option<(usize, usize)>, String> {
    let mut opens: Vec<(usize, usize)> = Vec::new();
    for (line, start) in lines_with_offsets(existing) {
        if line.trim() == ENV_OPEN {
            opens.push((start, existing[..start].matches('\n').count() + 1));
        }
    }
    let Some(&(start, line_number)) = opens.first() else {
        return Ok(None);
    };
    for (line, at) in lines_with_offsets(existing).skip_while(|(_, at)| *at <= start) {
        if line.trim() != ENV_CLOSE {
            continue;
        }
        if let Some(&(_, again)) = opens.get(1) {
            return Err(format!(
                "the managed block opens at line {line_number} and again at line {again} \
                 ({ENV_OPEN}); a file can only hold one, so it was not touched"
            ));
        }
        // The line carries its own newline, so the block ends where the closing
        // marker does — including on a file whose last line is the marker itself
        // and has none.
        return Ok(Some((start, at + line.len())));
    }
    Err(format!(
        "the managed block opens at line {line_number} ({ENV_OPEN}) and is never closed; \
         not touched"
    ))
}

/// Every line of `text` with its byte offset, trailing newline included.
fn lines_with_offsets(text: &str) -> impl Iterator<Item = (&str, usize)> + '_ {
    let mut at = 0;
    text.split_inclusive('\n').map(move |line| {
        let offset = at;
        at += line.len();
        (line, offset)
    })
}

/// The Lua expression for a declared value.
///
/// A value under the pass's `$HOME` is rendered the way a hand-written rice
/// renders it — `os.getenv("HOME") .. "/rest"` — because a profile is a thing
/// that travels between machines, and a block that hard-codes this user's home
/// would break on every other. Anything else is the literal string.
fn lua_value(value: &str, home: &Path) -> String {
    match under_home(value, home) {
        Some(rest) if rest.is_empty() => "os.getenv(\"HOME\")".to_string(),
        Some(rest) => format!("os.getenv(\"HOME\") .. {}", lua_string(&rest)),
        None => lua_string(value),
    }
}

/// The part of a declared path below `home`, as a Lua fragment would spell it:
/// `"/Wallpapers"`, with the separator the concatenation needs.
pub fn under_home(value: &str, home: &Path) -> Option<String> {
    let expanded = expand(home, value).ok()?;
    let rest = expanded.strip_prefix(home).ok()?;
    if rest.as_os_str().is_empty() {
        return Some(String::new());
    }
    Some(format!("/{}", rest.to_string_lossy()))
}

/// A `~`/`$HOME`-prefixed path resolved against the pass's home.
///
/// A path with a `..` component is refused before it is joined: `~/../etc` is
/// under `$HOME` lexically and is not under it at all, and the whitelist this
/// layer runs on is meant to be a shape a reader can check by eye.
pub fn expand(home: &Path, value: &str) -> Result<PathBuf, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("the path is empty".to_string());
    }
    if trimmed.split('/').any(|part| part == "..") {
        return Err(format!(
            "`{trimmed}` climbs out of the declared tree with `..`"
        ));
    }
    let relative = trimmed
        .strip_prefix("~/")
        .or_else(|| trimmed.strip_prefix("$HOME/"))
        .or_else(|| trimmed.strip_prefix("${HOME}/"))
        .or_else(|| trimmed.strip_prefix('~'))
        .unwrap_or(trimmed);
    if relative.is_empty() {
        return Err(format!(
            "`{trimmed}` is {} itself; only paths under it are declared",
            home.display()
        ));
    }
    let expanded = home.join(relative);
    if !expanded.starts_with(home) {
        return Err(format!(
            "`{trimmed}` resolves outside {}; only paths under it are declared",
            home.display()
        ));
    }
    Ok(expanded)
}

/// A Lua double-quoted string literal, with the text's own quotes escaped.
///
/// `\` is escaped too, which is what keeps a nested `tr '\\n'` in a recipe's
/// command meaning what the recipe author wrote rather than what one level of
/// unescaping turns it into. The pass borrows this to render a command into a
/// bind, so the two halves of a rewrite agree on what a literal looks like.
pub(crate) fn lua_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------- guarantee

/// What guaranteeing one declared path came to.
#[derive(Debug, PartialEq, Eq)]
pub enum Guaranteed {
    /// It did not exist and now does.
    Created(PathBuf),
    /// It already was what the recipe declares.
    AlreadyCorrect,
    /// The declaration is not one this layer will act on.
    Refused(String),
    /// The declaration is sound and the filesystem said no.
    Failed(String),
}

/// Guarantees one `[[dirs]]` entry: a directory that exists, or a symlink
/// pointing where the recipe says.
///
/// Nothing is ever replaced. A path that exists as something else — a file
/// where a directory is declared, a link pointing elsewhere, a real directory
/// where a link is declared — is refused, because the one thing a guarantee may
/// not do is destroy what is already there.
pub fn guarantee(home: &Path, dir: &Dir) -> Guaranteed {
    let Ok(path) = expand(home, &dir.path) else {
        return Guaranteed::Refused(refusal(&dir.path, "outside the home"));
    };
    match dir.symlink_to.as_deref() {
        None => ensure_dir(&path),
        Some(target) => {
            let Ok(target) = expand(home, target) else {
                return Guaranteed::Refused(format!(
                    "`{}` links to `{target}`, which is not a path this layer declares",
                    dir.path
                ));
            };
            ensure_link(&path, &target, &dir.path)
        }
    }
}

fn ensure_dir(path: &Path) -> Guaranteed {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => return Guaranteed::AlreadyCorrect,
        Ok(_) => {
            return Guaranteed::Refused(format!(
                "{} already exists and is not a directory",
                path.display()
            ));
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Guaranteed::Failed(format!("cannot look at {}: {error}", path.display()));
        }
        Err(_) => {}
    }
    let Some(parent) = path.parent() else {
        return Guaranteed::Failed(format!("{} has no parent directory", path.display()));
    };
    if let Err(error) = std::fs::create_dir_all(parent) {
        return Guaranteed::Failed(format!("cannot create {}: {error}", parent.display()));
    }
    match std::fs::create_dir(path) {
        Ok(()) => Guaranteed::Created(path.to_path_buf()),
        // Someone else won the race; the guarantee is the state, not the act.
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Guaranteed::AlreadyCorrect
        }
        Err(error) => Guaranteed::Failed(format!("cannot create {}: {error}", path.display())),
    }
}

fn ensure_link(path: &Path, target: &Path, declared: &str) -> Guaranteed {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return match std::fs::read_link(path) {
                    Ok(found) if found == target => Guaranteed::AlreadyCorrect,
                    Ok(found) => Guaranteed::Refused(format!(
                        "`{declared}` is a link to {}, not to {}",
                        found.display(),
                        target.display()
                    )),
                    Err(error) => Guaranteed::Failed(format!(
                        "cannot read the link at {}: {error}",
                        path.display()
                    )),
                };
            }
            return Guaranteed::Refused(format!(
                "`{declared}` already exists at {} and is not a link",
                path.display()
            ));
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Guaranteed::Failed(format!("cannot look at {}: {error}", path.display()));
        }
        Err(_) => {}
    }
    let Some(parent) = path.parent() else {
        return Guaranteed::Failed(format!("{} has no parent directory", path.display()));
    };
    if let Err(error) = std::fs::create_dir_all(parent) {
        return Guaranteed::Failed(format!("cannot create {}: {error}", parent.display()));
    }
    // Staged and renamed, the same discipline the config backups use: a crash
    // mid-write must not leave a half-made link where a directory belongs.
    let staged = path.with_extension("riceswap-staged");
    let _ = std::fs::remove_file(&staged);
    if let Err(error) = std::os::unix::fs::symlink(target, &staged) {
        return Guaranteed::Failed(format!("cannot create {}: {error}", staged.display()));
    }
    match std::fs::rename(&staged, path) {
        Ok(()) => Guaranteed::Created(path.to_path_buf()),
        Err(error) => {
            let _ = std::fs::remove_file(&staged);
            Guaranteed::Failed(format!("cannot link {}: {error}", path.display()))
        }
    }
}

fn refusal(declared: &str, why: &str) -> String {
    format!("`{declared}` is {why}; not touched")
}

fn is_identifier(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

// ------------------------------------------------------- dispatched entries
//
// The two halves of a dispatched `ns:name`, in one place for both tiers. The
// engine's scanner decides what a token *is* and a recipe decides what a name
// *means*, and both of those are questions about the same string, so the
// splitting is written once.

/// Whether a dispatched token is one `appid:name` with both halves filled.
pub(crate) fn split_entry(token: &str) -> Option<(&str, &str)> {
    let (appid, name) = token.split_once(':')?;
    (!appid.is_empty() && !name.is_empty() && !name.contains(':')).then_some((appid, name))
}

/// The name half of a dispatched `ns:name` — the join key a recipe resolves on.
pub(crate) fn name_of(entry: &str) -> &str {
    entry.split_once(':').map(|(_, name)| name).unwrap_or(entry)
}

/// The appid half of a dispatched `ns:name`, or nothing where there is none.
pub(crate) fn appid_of(entry: &str) -> &str {
    entry
        .split_once(':')
        .map(|(appid, _)| appid)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_resolution_names_exactly_one_side_or_is_a_finding() {
        let declared = |entry: &str| {
            toml::from_str::<Recipe>(&format!("[[resolution]]\n{entry}")).expect("parses")
        };

        let recipe = declared("dispatched = \"q:one\"\nto = \"one\"\n");
        assert!(matches!(
            recipe.resolutions[0].decide(),
            Ok(Decision::To { .. })
        ));

        let recipe = declared("dispatched = \"q:one\"\n");
        assert!(
            recipe.resolutions[0]
                .decide()
                .is_err_and(|why| why.contains("none of `to`, `exec` or `drop`")),
            "an entry that names no side says so"
        );

        let recipe = declared("dispatched = \"q:one\"\nto = \"one\"\nexec = \"true\"\n");
        assert!(
            recipe.resolutions[0]
                .decide()
                .is_err_and(|why| why.contains("exactly one of the three")),
            "two sides is a contradiction, not a preference"
        );

        let recipe = declared("dispatched = \"q:one\"\ndrop = false\n");
        assert!(
            recipe.resolutions[0]
                .decide()
                .is_err_and(|why| why.contains("names no resolution")),
        );
    }

    #[test]
    fn bind_options_only_belong_to_a_shortcut() {
        let with_options = |entry: &str| {
            toml::from_str::<Recipe>(&format!("[[resolution]]\n{entry}")).expect("parses")
        };

        let ok =
            with_options("dispatched = \"q:one\"\nto = \"one\"\nbind_options = [\"release\"]\n");
        assert_eq!(
            ok.resolutions[0].decide().expect("decides"),
            Decision::To {
                target: "one".to_string(),
                options: vec!["release".to_string()],
            }
        );

        let nonsense =
            with_options("dispatched = \"q:one\"\ndrop = true\nbind_options = [\"release\"]\n");
        assert!(
            nonsense.resolutions[0]
                .decide()
                .is_err_and(|why| why.contains("only means something alongside `to`")),
        );

        let unspellable = with_options(
            "dispatched = \"q:one\"\nto = \"one\"\nbind_options = [\"release = true\"]\n",
        );
        assert!(
            unspellable.resolutions[0]
                .decide()
                .is_err_and(|why| why.contains("not a bind option name")),
        );
    }

    /// The built-in recipes are compiled in, so a typo in one is a build-time
    /// fact this build can check rather than a user's switch discovering.
    #[test]
    fn every_built_in_recipe_parses_and_names_its_own_shell() {
        assert!(BUILT_IN.len() >= 2, "caelestia and ii ship");
        for (key, raw) in BUILT_IN {
            let mut findings = Vec::new();
            let tier = Tier::read("builtin", Path::new(key), raw, &mut findings)
                .unwrap_or_else(|| panic!("the built-in `{key}` recipe: {findings:?}"));
            assert!(
                findings.is_empty(),
                "the built-in `{key}` recipe: {findings:?}"
            );
            let recipe: Recipe = toml::from_str(raw).expect("parses");
            assert_eq!(
                recipe
                    .shell
                    .as_ref()
                    .and_then(|shell| shell.name.as_deref()),
                Some(*key),
                "`{key}` must name the shell it is filed under"
            );
            assert!(
                tier.appid.is_some(),
                "`{key}` pins the namespace it is about"
            );
        }
    }

    /// The caelestia codebook is the recorded hand-fix, so its shape is part of
    /// the contract: every donor intent the fixture dispatches has a decision,
    /// and the two that provably need none do not get invented ones.
    #[test]
    fn the_caelestia_codebook_answers_every_donor_intent_but_the_composite_one() {
        let mut findings = Vec::new();
        let tier = Tier::read(
            "builtin",
            Path::new("caelestia"),
            BUILT_IN
                .iter()
                .find(|(key, _)| *key == "caelestia")
                .expect("ships")
                .1,
            &mut findings,
        )
        .expect("parses");

        let mut kinds: BTreeMap<&str, BTreeMap<&str, usize>> = BTreeMap::new();
        for entry in tier.by_entry.values() {
            *kinds
                .entry(entry.decision.kind())
                .or_default()
                .entry(name_of(&entry.dispatched))
                .or_default() += 1;
        }
        assert_eq!(kinds["to"]["searchToggleRelease"], 1);
        assert_eq!(kinds["to"]["regionScreenshot"], 1);
        assert_eq!(kinds["to"]["sidebarLeftToggle"], 1);
        assert_eq!(kinds["exec"]["overviewClipboardToggle"], 1);
        assert_eq!(kinds["exec"]["regionOcr"], 1);
        assert_eq!(kinds["drop"]["workspaceNumber"], 1);
        assert_eq!(kinds["drop"]["barToggle"], 1);
        assert!(
            !kinds.values().any(|names| names.contains_key("lockFocus")),
            "the hypridle wake-and-lock composite is not a single dispatch resolution"
        );
        assert!(
            !kinds.values().any(|names| names.contains_key("lock")),
            "the exact-name move the engine proves needs no recipe entry"
        );
        assert!(
            tier.foreign.contains("quickshell:riceswap-toggle"),
            "the panel's own hotkey is guarded here too: {:?}",
            tier.foreign
        );
    }

    /// The precedence order is the contract three tiers hang on, and it is
    /// walked one name at a time.
    #[test]
    fn the_human_tier_outranks_the_built_in_which_outranks_the_user() {
        let root = tempfile::tempdir().expect("sandbox");
        let profile = root.path().join("profile");
        let data = root.path().join("data");
        std::fs::create_dir_all(profile.join(".config")).expect("profile tree");
        std::fs::create_dir_all(data.join(RECIPES_DIR)).expect("user tier");
        // The human tier and the user tier both answer `regionScreenshot`, in
        // opposite ways, and only the highest-precedence answer may win.
        std::fs::write(
            profile.join(ADAPT_FILE),
            concat!(
                "[env]\nSHARED = \"adapt\"\nONLY_ADAPT = \"a\"\n",
                "[[resolution]]\ndispatched = \"quickshell:regionScreenshot\"\n",
                "to = \"screenshotClip\"\n",
            ),
        )
        .expect("adapt");
        std::fs::write(
            data.join(RECIPES_DIR).join("caelestia.toml"),
            concat!(
                "schema_version = 1\n",
                "[shell]\nname = \"caelestia\"\nappid = \"caelestia\"\n",
                "[env]\nSHARED = \"user\"\nONLY_USER = \"u\"\n",
                "[[resolution]]\ndispatched = \"quickshell:regionScreenshot\"\n",
                "exec = \"user resolution\"\n",
                "[[resolution]]\ndispatched = \"userOnly\"\nexec = \"u\"\n",
            ),
        )
        .expect("user recipe");

        let (layers, findings) = Layers::load(&profile, &data, "caelestia");
        assert!(findings.is_empty(), "{findings:?}");
        assert_eq!(layers.names(), ["adapt", "builtin", "user"]);

        let found = layers
            .resolve("quickshell:regionScreenshot")
            .expect("resolves");
        assert_eq!(
            found.tier, "adapt",
            "the human tier answers a name the others also speak of"
        );
        assert!(
            matches!(found.entry.decision, Decision::To { .. }),
            "{:?}",
            found.entry.decision
        );

        assert_eq!(
            layers
                .resolve("quickshell:userOnly")
                .expect("resolves")
                .tier,
            "user",
            "a name only the user tier speaks of still speaks, under any namespace"
        );
        assert_eq!(
            layers
                .resolve("quickshell:cheatsheetToggle")
                .expect("resolves")
                .tier,
            "builtin",
            "and the built-in still answers its own names"
        );

        let env = layers.env();
        assert_eq!(env["CAELESTIA_WALLPAPERS_DIR"], "~/Wallpapers");
        assert_eq!(
            env["SHARED"], "adapt",
            "the human tier's value for a name two tiers speak of, per name"
        );
        assert_eq!(env["ONLY_ADAPT"], "a");
        assert_eq!(env["ONLY_USER"], "u");
        assert_eq!(
            layers.dirs().len(),
            1,
            "the built-in declares the one it guarantees"
        );
        assert_eq!(layers.appid_pin().map(|(tier, _)| tier), Some("builtin"));
    }

    /// A document that will not parse silences its own tier and says why. A
    /// document that parses but names a version this build does not know is
    /// refused the same way — a recipe RiceSwap cannot read is not one it may
    /// half-obey.
    #[test]
    fn a_recipe_that_cannot_be_read_is_reported_and_not_half_obeyed() {
        let root = tempfile::tempdir().expect("sandbox");
        let profile = root.path();
        let data = root.path().join("data");
        std::fs::write(
            profile.join(ADAPT_FILE),
            "schema_version = 1\n[[resolution]]\ndispatched = \"q:one\"\nto = \"one\"\ntypo = 1\n",
        )
        .expect("adapt");
        std::fs::create_dir_all(data.join(RECIPES_DIR)).expect("user tier");
        std::fs::write(
            data.join(RECIPES_DIR).join("caelestia.toml"),
            "schema_version = 7\n",
        )
        .expect("future recipe");

        let (layers, findings) = Layers::load(profile, &data, "caelestia");

        assert_eq!(
            layers.names(),
            ["builtin"],
            "only the recipe that parses is used"
        );
        assert!(
            findings
                .iter()
                .any(|why| why.contains("not a valid recipe") && why.contains("typo")),
            "{findings:?}"
        );
        assert!(
            findings
                .iter()
                .any(|why| why.contains("schema_version = 7") && why.contains("was not used")),
            "{findings:?}"
        );
    }

    /// One bad entry costs the entry, not the tier: a typo in a single
    /// resolution must not silently take the `[env]` block with it.
    #[test]
    fn one_invalid_entry_costs_only_that_entry() {
        let root = tempfile::tempdir().expect("sandbox");
        std::fs::write(
            root.path().join(ADAPT_FILE),
            concat!(
                "[env]\nCAELESTIA_WALLPAPERS_DIR = \"~/Wallpapers\"\n",
                "[[resolution]]\ndispatched = \"q:bad\"\n",
                "[[resolution]]\ndispatched = \"q:good\"\ndrop = true\n",
            ),
        )
        .expect("adapt");

        let (layers, findings) = Layers::load(root.path(), Path::new("/nonexistent"), "nothing");

        assert!(
            layers.resolve("q:bad").is_none(),
            "the bad entry says nothing"
        );
        assert!(
            layers.resolve("q:good").is_some(),
            "the good one still speaks"
        );
        assert_eq!(layers.env()["CAELESTIA_WALLPAPERS_DIR"], "~/Wallpapers");
        assert_eq!(findings.len(), 1, "{findings:?}");
    }

    /// A missing recipe is not a finding. Most profiles have no `adapt.toml`,
    /// most shells no built-in recipe, and a switch that announced an absent
    /// optional file on every run would be lying about something wrong.
    #[test]
    fn an_absent_recipe_is_silence() {
        let root = tempfile::tempdir().expect("sandbox");
        let (layers, findings) = Layers::load(root.path(), root.path(), "unknown-shell");

        assert!(layers.names().is_empty());
        assert!(findings.is_empty(), "{findings:?}");
    }

    /// The block is managed, not owned: the user's lines around it are carried
    /// through byte for byte, and a re-run with the same variables produces the
    /// same bytes.
    #[test]
    fn the_managed_block_is_spliced_around_whatever_the_user_wrote() {
        let home = Path::new("/home/tester");
        let env = BTreeMap::from([(
            "CAELESTIA_WALLPAPERS_DIR".to_string(),
            "~/Wallpapers".to_string(),
        )]);
        let block = env_block(&env, home);

        let fresh = splice_env("", &block).expect("into a new file");
        assert_eq!(
            fresh,
            concat!(
                "# >>> riceswap:adapt >>>\n",
                "hl.env(\"CAELESTIA_WALLPAPERS_DIR\", os.getenv(\"HOME\") .. \"/Wallpapers\")\n",
                "# <<< riceswap:adapt <<<\n",
            ),
            "the value travels: the profile names HOME, not this machine's path"
        );

        let kept = splice_env("hl.env(\"MINE\", \"1\")\n", &block).expect("alongside a user line");
        assert!(kept.starts_with("hl.env(\"MINE\", \"1\")\n\n"), "{kept}");
        assert!(
            kept.ends_with(&fresh),
            "the block is what a fresh run writes"
        );

        let replaced = splice_env(&kept, &block).expect("again");
        assert_eq!(replaced, kept, "a second pass changes nothing");

        let grown = splice_env(
            "hl.env(\"MINE\", \"1\")\n# >>> riceswap:adapt >>>\nhl.env(\"OLD\", \"1\")\n# <<< riceswap:adapt <<<\nhl.env(\"MINE\", \"2\")\n",
            &block,
        )
        .expect("over an old block");
        assert_eq!(
            grown,
            "hl.env(\"MINE\", \"1\")\n# >>> riceswap:adapt >>>\nhl.env(\"CAELESTIA_WALLPAPERS_DIR\", os.getenv(\"HOME\") .. \"/Wallpapers\")\n# <<< riceswap:adapt <<<\nhl.env(\"MINE\", \"2\")\n",
            "only what was between the markers moved"
        );

        assert!(
            splice_env(
                "# >>> riceswap:adapt >>>\nhl.env(\"MINE\", \"1\")\n",
                &block
            )
            .is_err_and(|why| why.contains("never closed")),
            "an unterminated block is refused, not guessed at"
        );

        // The other shape this layer cannot own: two blocks. Splicing the first
        // would leave the second as stale `hl.env` lines that still read as the
        // managed one, so the whole file is refused — the same answer, raised
        // the same way round, as an unterminated marker.
        let twice = format!("{block}{block}");
        assert!(
            splice_env(&twice, &block).is_err_and(|why| {
                why.contains("and again at line 4") && why.contains("can only hold one")
            }),
            "two blocks are refused: {twice:?}"
        );
        let nested = format!("{block}{ENV_OPEN}\nhl.env(\"MINE\", \"1\")\n{ENV_CLOSE}\n");
        assert!(
            splice_env(&nested, &block).is_err_and(|why| why.contains("and again at line 4")),
            "and so is one nested inside the first, which is the same file twice over: \
             {nested:?}"
        );
        assert!(
            splice_env(&format!("{ENV_CLOSE}\n{block}"), &block).is_ok(),
            "while a stray closing marker on its own is not a block, and refuses nothing"
        );
    }

    /// Tilde expansion is what makes a profile portable, and the whitelist is
    /// what makes the expansion safe to do at all.
    #[test]
    fn a_declared_path_is_expanded_inside_the_home_and_nowhere_else() {
        let home = Path::new("/home/tester");
        assert_eq!(
            expand(home, "~/Wallpapers").expect("expands"),
            Path::new("/home/tester/Wallpapers")
        );
        assert_eq!(
            expand(home, "$HOME/Pictures/Wallpapers").expect("expands"),
            Path::new("/home/tester/Pictures/Wallpapers")
        );
        assert_eq!(
            under_home("~/Wallpapers", home).as_deref(),
            Some("/Wallpapers")
        );
        assert_eq!(under_home("/etc/x", home), None);
        assert!(
            expand(home, "~/../etc").is_err_and(|why| why.contains("climbs out")),
            "lexically-under is not under"
        );
        assert!(expand(home, "/etc/passwd").is_err_and(|why| why.contains("outside")),);
        assert!(
            expand(home, "~").is_err_and(|why| why.contains("itself")),
            "the home directory is not a path under itself"
        );
        assert!(expand(home, "  ").is_err_and(|why| why.contains("empty")));
    }

    /// The guarantee is a state, not an act: whatever is already correct costs
    /// nothing, and whatever is in the way is never replaced.
    #[test]
    fn a_guarantee_creates_once_refuses_to_replace_and_is_quiet_the_second_time() {
        let home = tempfile::tempdir().expect("sandbox");
        let home = home.path();
        let wallpapers = Dir {
            path: "~/Wallpapers".to_string(),
            symlink_to: None,
        };
        let link = Dir {
            path: "~/Pictures/Wallpapers".to_string(),
            symlink_to: Some("~/Wallpapers".to_string()),
        };

        assert!(matches!(
            guarantee(home, &wallpapers),
            Guaranteed::Created(_)
        ));
        assert_eq!(
            guarantee(home, &wallpapers),
            Guaranteed::AlreadyCorrect,
            "an existing directory is the state the recipe declared"
        );
        assert!(matches!(guarantee(home, &link), Guaranteed::Created(_)));
        assert_eq!(
            std::fs::read_link(home.join("Pictures/Wallpapers")).expect("is a link"),
            home.join("Wallpapers"),
        );
        assert_eq!(
            guarantee(home, &link),
            Guaranteed::AlreadyCorrect,
            "an existing link to the declared target is the same state"
        );

        std::fs::create_dir(home.join("Notes")).expect("a real directory");
        assert!(
            matches!(
                guarantee(
                    home,
                    &Dir {
                        path: "~/Notes".to_string(),
                        symlink_to: Some("~/Wallpapers".to_string())
                    }
                ),
                Guaranteed::Refused(_)
            ),
            "a guarantee never destroys what is already there"
        );
        assert!(matches!(
            guarantee(
                home,
                &Dir {
                    path: "~/elsewhere/Wallpapers".to_string(),
                    symlink_to: Some("/etc".to_string())
                }
            ),
            Guaranteed::Refused(_)
        ));
    }
}
