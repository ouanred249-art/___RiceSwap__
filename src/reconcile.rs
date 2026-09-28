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

use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

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
    /// Dispatched names nothing answers to and the engine could not move: the
    /// dead set that survives the pass, each needing a recipe to resolve.
    pub dead_names: Vec<String>,
    /// Every dead name with the resolution the engine could prove for it — the
    /// exact-name move that was applied, or `null` for "no counterpart" — and
    /// the lines that dispatch it. This is the proposal vocabulary the recipe
    /// tier (#28) consumes verbatim: one entry per dead name, carrying the
    /// per-line detail that tier resolves against.
    pub proposals: Vec<Proposal>,
    /// Config files the engine rewrote, as `$HOME`-relative paths.
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
    /// backup that could not be written.
    pub warnings: Vec<String>,
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
/// owns. `keeps` is the escape hatch the recipe tier fills (#28); it is empty
/// here, and a kept entry behaves exactly like a foreign one: never rewritten,
/// never reported dead.
pub fn reconcile(profile: &Path, shell: Option<&str>, keeps: &BTreeSet<String>) -> Report {
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
    // dead set, the proposals' line sites and the rewrite all come out of the
    // one `Scan`, which is kept for the rewrite below rather than recomputed.
    // (`scan_externals` re-reads the configs afterwards, for the exec lines it
    // needs — a separate, read-only sweep over the same files.)
    let mut moved: BTreeMap<String, String> = BTreeMap::new();
    let mut dead: BTreeMap<String, Vec<ProposalSite>> = BTreeMap::new();
    let mut scans: Vec<Scan> = Vec::with_capacity(configs.len());
    let mut contents: Vec<Option<String>> = Vec::with_capacity(configs.len());
    for path in &configs {
        let Ok(text) = std::fs::read_to_string(path) else {
            report
                .warnings
                .push(format!("cannot read {}: skipped", relative(path)));
            scans.push(Scan::default());
            contents.push(None);
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
        for site in &scan.sites {
            let entry = site.entry();
            if keeps.contains(&entry) || FOREIGN.contains(&entry.as_str()) {
                continue;
            }
            if registry.holds(&site.appid, &site.name) {
                continue; // Resolves as it stands: nothing to fix, nothing to say.
            }
            if let Some(appid) = registry.provable_appid(&site.name) {
                moved.insert(entry.clone(), format!("{appid}:{}", site.name));
            }
            dead.entry(entry).or_default().push(ProposalSite {
                file: relative(path),
                line: site.line,
            });
        }
        scans.push(scan);
        contents.push(Some(text));
    }

    // The proposal vocabulary, in a stable order: every dead name with the move
    // that was applied (or the honest absence of one) and every line that
    // dispatches it.
    report.proposals = dead
        .iter()
        .map(|(entry, sites)| Proposal {
            dispatched: entry.clone(),
            name: name_of(entry).to_string(),
            target: moved.get(entry).cloned(),
            sites: sites.clone(),
        })
        .collect();
    report.dead_names = dead
        .iter()
        .filter(|(entry, _)| !moved.contains_key(*entry))
        .map(|(entry, _)| entry.clone())
        .collect();

    for ((path, content), scan) in configs.iter().zip(&contents).zip(&scans) {
        let Some(text) = content else {
            continue; // Already reported above.
        };
        let rewritten = rewrite(text, scan, &moved);
        if rewritten == *text {
            continue; // Nothing to change: what keeps a clean profile untouched.
        }
        let stored = store_backup(&backups, path, text.as_bytes());
        if let Err(error) = &stored {
            report
                .warnings
                .push(format!("{}: {error}; left as it was", relative(path)));
            continue;
        }
        if let Err(error) = std::fs::write(path, rewritten) {
            report
                .warnings
                .push(format!("cannot rewrite {}: {error}", relative(path)));
            continue;
        }
        report.files_written.push(relative(path));
        if let Ok(stored) = stored {
            report.backups.push(stored);
        }
    }

    report.externals = scan_externals(profile, &hypr, &configs, &registry, keeps);
    report
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
        }
        if matches!(bytes.get(cursor), Some(b'"') | Some(b'\'')) {
            cursor += 1;
        }
        let token_start = cursor;
        while bytes.get(cursor).is_some_and(|byte| is_token(*byte)) {
            cursor += 1;
        }
        let token = &text[token_start..cursor];
        match split_entry(token) {
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

/// Applies the proved moves and leaves everything else byte for byte.
fn rewrite(text: &str, scan: &Scan, moved: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for site in &scan.sites {
        let Some(target) = moved.get(&site.entry()) else {
            continue;
        };
        out.push_str(&text[cursor..site.start]);
        out.push_str(target);
        cursor = site.end;
    }
    out.push_str(&text[cursor..]);
    out
}

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
                        && !registry.holds(site_appid(entry), site_name(entry))
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

/// Splits a scanned token into its `appid:name` halves, if that is what it is.
fn split_entry(token: &str) -> Option<(&str, &str)> {
    let (appid, name) = token.split_once(':')?;
    (!appid.is_empty() && !name.is_empty() && !name.contains(':')).then_some((appid, name))
}

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn is_token(byte: u8) -> bool {
    // The colon is in because the token being read *is* an `appid:name` pair;
    // [`split_entry`] is what decides whether what came out is one.
    is_word(byte) || matches!(byte, b'-' | b'.' | b'/' | b'+' | b':')
}

fn site_appid(entry: &str) -> &str {
    entry
        .split_once(':')
        .map(|(appid, _)| appid)
        .unwrap_or_default()
}

fn site_name(entry: &str) -> &str {
    entry.split_once(':').map(|(_, name)| name).unwrap_or(entry)
}

fn name_of(entry: &str) -> &str {
    site_name(entry)
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

    fn read(root: &Path, relative: &str) -> String {
        std::fs::read_to_string(root.join(relative))
            .unwrap_or_else(|error| panic!("read {}: {error}", root.join(relative).display()))
    }

    fn none() -> BTreeSet<String> {
        BTreeSet::new()
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
        let report = reconcile(profile.path(), Some("demo"), &none());

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

        let nameless = reconcile(empty.path(), None, &none());
        assert!(nameless.files_written.is_empty());
        assert!(
            nameless
                .skipped
                .as_deref()
                .is_some_and(|why| why.contains("names no shell")),
            "{nameless:?}"
        );

        let treeless = reconcile(empty.path(), Some("demo"), &none());
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
        let mut moved = BTreeMap::new();
        moved.insert("quickshell:lock".to_string(), "caelestia:lock".to_string());

        assert_eq!(
            rewrite(text, &scan, &moved),
            "hl.bind(\"SUPER + L\", hl.dsp.global(\"caelestia:lock\"), { description = \"Lock\" })\n"
        );
    }

    // ----------------------------------------------------------------- golden

    /// The donor graft, against the real caelestia QML: the caelestia profile
    /// as it was *before* the hand-fix, still dispatching the donor vocabulary.
    ///
    /// The hand-fix invented `quickshell:searchToggleRelease` →
    /// `caelestia:launcher` and `quickshell:regionScreenshot` →
    /// `caelestia:screenshotClip`, and pointed hypridle's wake command at
    /// `caelestia:lock`. Only the last of those is the same name the registry
    /// registers, so only the last of those is applied; the other two ship as
    /// proposals for the recipe tier.
    #[test]
    fn the_donor_graft_moves_only_what_the_registry_proves() {
        let donor = fixture("donor");
        let before = read(donor.path(), ".config/hypr/hypridle.conf");

        let report = reconcile(donor.path(), Some("caelestia"), &none());

        // (a) the exact-name move, and only the file holding it.
        assert_eq!(report.appid.as_deref(), Some("caelestia"));
        assert_eq!(report.registered, 22);
        assert_eq!(report.files_written, [".config/hypr/hypridle.conf"]);
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
            "lockFocus has no counterpart and is left exactly as it was: {after}"
        );
        assert_ne!(before, after);

        // (b) the fuzzy names are untouched and reported.
        let keybinds = read(donor.path(), ".config/hypr/hyprland/keybinds.lua");
        assert!(
            keybinds.contains(r#"hl.dsp.global("quickshell:regionScreenshot")"#),
            "caelestia registers no `regionScreenshot`; the bind is not the engine's to move"
        );
        assert!(keybinds.contains(r#"hl.dsp.global("quickshell:searchToggleRelease")"#));
        for dead in [
            "quickshell:regionScreenshot",
            "quickshell:searchToggleRelease",
        ] {
            let proposal = report
                .proposals
                .iter()
                .find(|proposal| proposal.dispatched == dead)
                .unwrap_or_else(|| panic!("{dead} must be proposed: {:?}", report.proposals));
            assert_eq!(proposal.name, dead.split_once(':').expect("split").1);
            assert_eq!(proposal.target, None, "no counterpart, so no rewrite");
            assert!(report.dead_names.contains(&dead.to_string()));
        }

        // A proposal names the lines, not just the name. `regionScreenshot` is
        // dispatched on two binds in two different files, and the recipe tier
        // resolves per line — one dead name, two decisions, and both have to be
        // visible in the report or the recipe is written against half the binds.
        let proposal = |dead: &str| {
            report
                .proposals
                .iter()
                .find(|proposal| proposal.dispatched == dead)
                .unwrap_or_else(|| panic!("{dead} must be proposed"))
        };
        assert_eq!(
            proposal("quickshell:regionScreenshot").sites,
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
        assert_eq!(
            proposal("quickshell:overviewWorkspacesToggle").sites,
            [
                ProposalSite {
                    file: ".config/hypr/custom/general.lua".to_string(),
                    line: 28,
                },
                ProposalSite {
                    file: ".config/hypr/custom/general.lua".to_string(),
                    line: 35,
                },
                ProposalSite {
                    file: ".config/hypr/hyprland/keybinds.lua".to_string(),
                    line: 22,
                },
            ],
            "two gesture handlers and the SUPER+Tab bind — the same name in two \
             files, which is exactly the case a per-name entry cannot describe"
        );
        assert_eq!(
            proposal("quickshell:lock").sites,
            [ProposalSite {
                file: ".config/hypr/hypridle.conf".to_string(),
                line: 1,
            }],
            "the line the applied move rewrote"
        );

        // The one move the engine did make is proposed with the target it
        // applied, which is the vocabulary the recipe tier consumes.
        let moved = report
            .proposals
            .iter()
            .find(|proposal| proposal.dispatched == "quickshell:lock")
            .expect("lock is dead and moved");
        assert_eq!(moved.target.as_deref(), Some("caelestia:lock"));
        assert!(!report.dead_names.contains(&"quickshell:lock".to_string()));

        // (c) the panel's own shortcut survives: foreign, and not even reported.
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
    }

    /// The invariant the engine exists to establish, checked on the file it
    /// wrote rather than on what it intended: after the pass, the only
    /// dispatches left unresolved are the ones the report named.
    #[test]
    fn every_dispatch_resolves_after_the_rewrite_but_the_reported_ones() {
        let donor = fixture("donor");
        let report = reconcile(donor.path(), Some("caelestia"), &none());
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
    /// already-repaired profile writes no file and makes no backup, and every
    /// byte on disk — modification times included — is what the first pass left.
    #[test]
    fn a_second_pass_over_a_repaired_profile_writes_nothing() {
        let donor = fixture("donor");
        let first = reconcile(donor.path(), Some("caelestia"), &none());
        assert_eq!(
            first.files_written.len(),
            1,
            "the first pass has work to do"
        );

        let before = stamp(donor.path());
        let second = reconcile(donor.path(), Some("caelestia"), &none());
        let after = stamp(donor.path());

        assert!(second.files_written.is_empty(), "{second:?}");
        assert!(second.backups.is_empty(), "no second backup: {second:?}");
        assert_eq!(before, after, "not one byte or timestamp moved");
        assert_eq!(
            first.dead_names, second.dead_names,
            "the unresolvable names are still unresolvable, and that is the whole report"
        );
    }

    /// The regression guard. ii's configs dispatch `quickshell:*` and ii
    /// registers those very names, so the correct answer is to change nothing
    /// at all — no rewrites, no backups, not a byte moved anywhere in the tree.
    #[test]
    fn iis_own_profile_comes_out_untouched() {
        let ii = fixture("ii");
        let before = stamp(ii.path());

        let report = reconcile(ii.path(), Some("ii"), &none());

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

        let report = reconcile(profile.path(), Some("demo"), &none());

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
        let report = reconcile(donor.path(), Some("caelestia"), &none());
        let stored = &report.backups[0];
        let name = stored.strip_prefix("backups/").expect("profile-relative");
        let kept = read(donor.path(), stored);
        assert_eq!(
            kept,
            read(donor.path(), ".config/hypr/hypridle.conf")
                .replace("caelestia:lock", "quickshell:lock")
                .replacen("quickshell:lockFocus", "quickshell:lockFocus", 1),
            "the backup holds the bytes as they were, not as they became"
        );
        assert!(name.ends_with(".hypridle.conf"), "{stored}");
        assert!(donor.path().join(stored).is_file());
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

        let report = reconcile(profile.path(), Some("demo"), &none());

        assert!(report.dead_names.is_empty(), "{report:?}");
        assert!(report.proposals.is_empty(), "{report:?}");
        assert_eq!(before, stamp(profile.path()));
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
