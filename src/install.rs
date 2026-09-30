//! `riceswap install <git-url | local-path>` — acquisition, identity,
//! materialization (issue #37).
//!
//! One command takes a rice nobody here has ever seen and leaves a working
//! profile it has switched to. The pipeline is four steps in a fixed order, and
//! every one of them has a rule that exists because of the step after it:
//!
//! 0. **Source.** A git URL is cloned — in full, of the default branch, with the
//!    system `git` — into the cache at
//!    `sources/<repo-slug>/<commit-sha>/`; a local directory is used where it
//!    stands, read-only, and never copied into the cache. Both end as the same
//!    thing: a directory to read, a `source_url` for the manifest, and a
//!    `source_commit` when the tree could say one. A machine with no `git` is
//!    refused here, before a single directory exists.
//!
//! 1. **Identity.** The tree is read for the shell it carries: a
//!    `.config/quickshell/<name>/shell.qml` is the marker, because that file is
//!    what makes a Quickshell directory a *shell* rather than somebody's
//!    settings folder. Exactly one shell and the profile takes its name; more
//!    than one is refused with the candidates listed, because guessing installs
//!    the wrong desktop; none at all is refused outright, whatever else the tree
//!    carries. `--shell` picks the winner when the tree offers a choice.
//!
//! 1b. **Research, for a shell nothing here knows.** Only a rice no built-in
//!    recipe covers and no `adapt.toml` answers for, and only when the static
//!    parse of its own QML could not prove the registry: one `pi` run, offline,
//!    tool-stripped, against the acquired tree, and its validated answer saved
//!    as a *user-tier* recipe at `<data-dir>/recipes/<shell>.toml`. It runs
//!    here, before the profile exists, because the recipe has to be on disk
//!    before the adapt pass below loads the layers — and it can never fail the
//!    install: no `pi`, a hung run, a provider refusal, a model that will not
//!    answer in JSON are all a warning and the engine + declarations floor
//!    ([`crate::research`]).
//!
//! 2. **Materialize.** The same machinery `snapshot` uses, pointed at the
//!    acquired tree instead of `$HOME`: the detected config dirs and assets are
//!    mirrored into a new profile, the shared-hardware `source =` line is
//!    injected into the captured Hyprland config, wallpapers are imported into
//!    the shared layer, and the manifest is written with the real `source_url` /
//!    `source_commit`, a `[shell]` table naming the shell that was identified,
//!    the packages the research answer named merged into `[packages]`, and no
//!    screenshot — an acquired tree is not running.
//!
//! 3. **Adapt and switch.** The reconciliation engine runs over the new profile
//!    before anything goes live (the install-adapt trigger of #35), so the
//!    dispatch names the donor's configs speak are repaired against the shell
//!    that is about to own them; the existing `switch` then runs unchanged and
//!    re-runs the same pass idempotently, with the missing-package machinery
//!    riding along.
//!
//! Two rules make the whole thing safe to re-run and safe to stop halfway:
//!
//! **An existing profile refuses.** Installing is not updating, and a profile the
//! user has been living in is not something a muscle-memory command may clobber.
//! The one exception is the half-made profile: if `state.json` still names an
//! `install` that claimed *this* profile from *this* source and never finished,
//! the re-run is a resume. `state::StateStore::claim` is what writes that
//! record and what keeps it after a failure; a `switch` of the same profile
//! records nothing, so it never makes a profile look installable. The steps are
//! idempotent, so the resume clears the install's own profile and re-makes it
//! rather than trying to pick up where it stopped.
//!
//! **A refusal creates nothing.** Every refusal below the flip — no git, a failed
//! clone, a tree that is not a quickshell rice, an ambiguous identity, a profile
//! that already exists — carries one of the reason codes the spec froze for
//! exactly this, and leaves the cache, the store and the live desktop as they
//! were. A clone that fails unwinds its own staging directory rather than
//! leaving a half-clone under `sources/`.

use crate::detection::{self, IMAGE_EXTENSIONS, PackageScan};
use crate::envelope::{Emitter, Envelope};
use crate::operations::{self, Context};
use crate::profile::{Manifest, Service, Shell};
use crate::reconcile;
use crate::research::{self, Findings};
use crate::tools::{self, Tool};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The file that records which clone a slug currently holds, so a second
/// install of the same URL reads the cache instead of the network. It sits
/// beside the clones it points at, never inside one.
const HEAD_RECORD: &str = "head.json";

/// Where a clone is staged before its commit is known — a directory named by
/// nothing but the tool, so a crashed run leaves something a later run can
/// remove rather than something a later run mistakes for a cache entry.
const CLONE_STAGING: &str = "clone.staged";

/// The sha `HEAD_RECORD` falls back to. A commit is a fact, never a placeholder
/// dressed as one: if a clone could not say what it is at, the install refuses
/// instead of recording this.
const NO_COMMIT: &str = "";

/// Runs the whole pipeline, emitting exactly one envelope.
///
/// `shell` is the `--shell` override: `Some` picks between the shells an
/// acquired tree carries, `None` takes the only one there is and refuses a tree
/// that does not say.
pub fn install(context: &mut Context, source: &str, shell: Option<&str>) -> Envelope {
    let mut emitter = Emitter::new("install");
    let mut warnings: Vec<String> = Vec::new();

    // ---- step 0: the source.
    context.progress(&mut emitter, &format!("acquiring `{source}`"));
    let acquired = match acquire(context, source) {
        Ok(acquired) => acquired,
        Err(refusal) => return refusal.into_envelope(),
    };
    let source_word = if acquired.clone.is_some() {
        "a clone of it"
    } else {
        "the directory itself"
    };
    context.progress(
        &mut emitter,
        &format!("read {source_word} at {}", acquired.tree.display()),
    );

    // ---- step 1: the identity.
    let identity = match identify(&acquired.tree, shell) {
        Ok(identity) => identity,
        Err(refusal) => return refusal.into_envelope(),
    };
    let name = identity.shell.clone();
    if let Err(error) = context.store().validate_name(&name) {
        return Refusal::new("identify", None, error).into_envelope();
    }
    context.progress(
        &mut emitter,
        &format!("identified the rice as the `{name}` quickshell shell"),
    );

    // The exists-check, before anything is written. It is here and not after
    // materialization because the profile name only exists from this point on:
    // waiting until the copying began would mean the refusal came after the
    // tool had already taken the profile apart.
    let profile = context.store().profile_dir(&name);
    let claim = match claim_profile(context, &name, &acquired.url, &profile) {
        Ok(claim) => claim,
        Err(refusal) => return refusal.into_envelope(),
    };

    // The claim is recorded only now, once the install is actually going to
    // proceed: a refusal must leave `state.json` exactly as it found it. Written
    // before the exists-check it would mark a *refused* install as one that
    // "did not finish", and the next run would read its own refusal as
    // permission to re-make — and thereby clear — a profile it must never touch.
    // From here on, though, the entry is what tells a later run to resume if
    // this install dies before finishing.
    context.state().claim(&name, &acquired.url);
    context.state().write();
    if claim == Claim::Resumed {
        let note = format!(
            "resuming a half-finished install: `{}` was claimed by an install of {} that did \
             not finish, so it is re-made from scratch",
            name, acquired.url
        );
        emitter.warning(&note);
        warnings.push(note);
        context.progress(&mut emitter, &format!("resuming the install of `{name}`"));
    } else {
        context.progress(
            &mut emitter,
            &format!("creating profile directory for `{name}`"),
        );
    }

    let data_dir = context.store().data_dir();

    // ---- step 1b: the research tier, for a shell nothing here knows.
    //
    // It sits between identity and materialization for one reason: the recipe it
    // writes lives in the *user tier* (`<data-dir>/recipes/<shell>.toml`), and
    // the adapt pass in step 3a is the first thing that reads the layers. Write
    // the recipe after that pass and it would be read by the switch's own
    // re-run instead — one reconcile late, and the bind that made the install
    // worth running stays dead for one switch. The write-back is proven to
    // parse before it lands, so what step 3a loads is a recipe, not a file.
    context.progress(&mut emitter, &format!("researching the `{name}` shell"));
    let subject = research::Subject {
        home: context.home(),
        data: &data_dir,
        profile: &profile,
        tree: &acquired.tree,
        shell: &name,
    };
    let mut researched = research::run(&subject);
    researched.announce(&mut emitter, &mut warnings);
    researched.write_back(&subject, &mut emitter, &mut warnings);

    // ---- step 2: the profile itself.
    let materialized = match materialize(
        context,
        &name,
        &acquired,
        claim,
        &mut warnings,
        researched.answer().map(|answer| &answer.findings),
    ) {
        Ok(materialized) => materialized,
        Err(refusal) => return refusal.into_envelope(),
    };

    // ---- step 3a: adapt, on the profile's own files, before the first switch
    // installs them. The switch runs the same pass again and finds nothing left
    // to do; the report below is the one that names what was repaired.
    context.progress(
        &mut emitter,
        &format!("reconciling the dispatcher names in `{name}`"),
    );
    let report = reconcile::reconcile(
        &profile,
        &reconcile::Context::new(context.home(), &data_dir),
        Some(&name),
        &BTreeSet::new(),
    );
    announce(&mut emitter, &report, &mut warnings);

    // ---- step 3b: the ordinary switch, unchanged.
    context.progress(&mut emitter, &format!("switching to `{name}`"));
    let switched = operations::switch(context, &name, None);

    let facts = payload(
        &acquired,
        &identity,
        &materialized,
        claim,
        &report,
        Some(switched.data.clone()),
        &researched,
    );
    for warning in switched.warnings {
        warnings.push(warning);
    }

    // A switch that failed fails the install: the profile exists and is
    // reconciled, but the desktop did not land on it. The switch's own error,
    // `completed_steps` and report ride the envelope unchanged — it is the same
    // failure a standalone switch would report — and the resume hint points at
    // the command that resumes, because the half-made profile is exactly what
    // the next plain re-invocation of it is allowed to re-make.
    let mut data = facts;
    if !switched.ok {
        data["error"] = switched.data["error"].clone();
        data["phase"] = json!("switch");
        data["resume_hint"] = json!(format!(
            "re-run `riceswap install {}` to finish installing `{name}`",
            acquired.url
        ));
        return Envelope {
            ok: false,
            warnings,
            data,
        };
    }
    let mut envelope = Envelope::ok(data);
    envelope.warnings = warnings;
    envelope
}

// ---------------------------------------------------------------- the source

/// Where step 0 landed: the tree to read, the two facts the manifest records
/// about where it came from, and the cache entry it resolved to when it came
/// off the network.
struct Acquired {
    /// The manifest's `source_url`: the clone URL, or `file://<abs path>`.
    url: String,
    /// The tree's commit — a clone's HEAD, or a local checkout's. `None` when
    /// the directory is not a checkout, which is a fact and not a failure.
    commit: Option<String>,
    /// The directory the rest of the pipeline reads. Only ever read: a clone
    /// because it is this tool's cache, a local path because it is the user's.
    tree: PathBuf,
    /// The cache entry behind this tree, when there was one.
    clone: Option<Clone>,
}

/// One cache entry: `sources/<slug>/<commit-sha>/`. Assembled by the
/// acquisition, never read back — the record on disk is [`HeadRecord`].
struct Clone {
    /// The slug the entry's directory is named after.
    slug: String,
    /// The commit the clone is at — the directory name, and the manifest's
    /// `source_commit`.
    commit: String,
    /// The absolute path of the clone.
    path: PathBuf,
    /// Whether this install cloned it or found it already there.
    reused: bool,
}

/// What a slug directory records about the clone it currently holds.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
struct HeadRecord {
    /// The URL the clone came from. The cache is keyed by this string: two
    /// spellings of one repository are two caches, and neither is wrong.
    url: String,
    commit: String,
    path: PathBuf,
}

/// Step 0, dispatching on what the source is.
fn acquire(context: &Context, source: &str) -> Result<Acquired, Refusal> {
    if is_remote(source) {
        clone_source(context, source)
    } else {
        local_source(context, source)
    }
}

/// Whether a source is a remote to clone rather than a directory to read. The
/// spellings are the ones a dotfiles README actually uses: an `https://` URL,
/// an `ssh://` URL, the `git@host:path` shorthand, and `git://`.
fn is_remote(source: &str) -> bool {
    ["https://", "http://", "ssh://", "git://", "git@"]
        .iter()
        .any(|prefix| source.starts_with(prefix))
}

/// Clones `url` into the cache, or reads the clone the cache already holds.
///
/// The reuse rule, and why it is the honest one: the cache is keyed by the URL
/// as the user spelled it, and an install that finds an entry for that URL uses
/// the clone this tool already made. It does not ask the remote whether the
/// clone is current, because asking is a network round-trip and this operation
/// is specified to work offline — and because an answer would only ever produce
/// a *different* tree, which is not what re-installing the same URL means here.
/// Updating is its own operation (adopt / re-fetch), and it is not this one. So
/// the second install of a URL is a cache read, and the first is the only one
/// that clones.
fn clone_source(context: &Context, url: &str) -> Result<Acquired, Refusal> {
    let slug = slug_of(url)?;
    let sources = context.store().sources_dir();
    let repo = sources.join(&slug);

    // Git is probed before a directory is made: a machine without it must be
    // refused with the cache untouched, not with an empty `sources/` to clean up.
    let status = tools::probe(Tool::Git);
    if !status.succeeded() {
        return Err(Refusal::new(
            "acquire",
            Some("git-missing"),
            format!(
                "git is not usable ({}); installing {url} needs the system git to clone the rice",
                operations::describe(&status)
            ),
        )
        .with_fact("tools", json!({ "git": status })));
    }

    if let Some(entry) = cached(&repo, url) {
        return Ok(Acquired {
            url: url.to_string(),
            commit: Some(entry.commit.clone()),
            tree: entry.path.clone(),
            clone: Some(Clone {
                slug,
                commit: entry.commit,
                path: entry.path,
                reused: true,
            }),
        });
    }

    if let Err(error) = std::fs::create_dir_all(&repo) {
        return Err(Refusal::new(
            "acquire",
            None,
            format!("cannot open the source cache {}: {error}", repo.display()),
        ));
    }

    // The clone is staged beside its final home, so promoting it to
    // `sources/<slug>/<sha>/` is a rename inside one directory: a clone is
    // never *seen* under a commit name the tool has not read off it.
    let staging = repo.join(CLONE_STAGING);
    let _ = std::fs::remove_dir_all(&staging);
    if let Err(refusal) = run_git(&["clone", url, &staging.to_string_lossy()]) {
        unwind(&sources, &repo, &staging);
        return Err(refusal);
    }

    let commit = match head_commit(&staging) {
        Some(commit) => commit,
        None => {
            unwind(&sources, &repo, &staging);
            return Err(Refusal::new(
                "acquire",
                Some("clone-failed"),
                format!(
                    "the clone of {url} reports no HEAD commit, so there is nothing to key the \
                     cache on; {url} may not be a git repository"
                ),
            ));
        }
    };
    let destination = repo.join(&commit);
    let reused = destination.is_dir();
    if reused {
        // The same commit is already cached under this repo — a second repo
        // slug, or a re-run after the head record was lost. The clone just made
        // is redundant, and the cached one is the same bytes.
        let _ = std::fs::remove_dir_all(&staging);
    } else if let Err(error) = std::fs::rename(&staging, &destination) {
        unwind(&sources, &repo, &staging);
        return Err(Refusal::new(
            "acquire",
            Some("clone-failed"),
            format!(
                "cannot keep the clone of {url} at {}: {error}",
                destination.display()
            ),
        ));
    }

    let tree = destination;
    if let Err(error) = write_head(
        &repo,
        &HeadRecord {
            url: url.to_string(),
            commit: commit.clone(),
            path: tree.clone(),
        },
    ) {
        // The clone is on disk and readable; only the shortcut to it is missing,
        // so the next install would clone again. That is waste, not a failure,
        // and saying so is better than refusing over a cache index.
        return Err(Refusal::new(
            "acquire",
            None,
            format!("cannot record the cache entry for {url}: {error}"),
        ));
    }

    Ok(Acquired {
        url: url.to_string(),
        commit: Some(commit.clone()),
        tree: tree.clone(),
        clone: Some(Clone {
            slug,
            commit,
            path: tree,
            reused,
        }),
    })
}

/// A local directory, used where it stands.
///
/// Nothing is copied: the user's own directory is the source of truth, it is
/// only ever read, and a rice being authored is exactly the case this exists
/// for. The `file://` form is the canonical path, so the manifest records where
/// the rice actually was even when the command was given a relative one.
fn local_source(_context: &Context, source: &str) -> Result<Acquired, Refusal> {
    let canonical = std::fs::canonicalize(source).map_err(|error| {
        Refusal::new(
            "acquire",
            None,
            format!(
                "cannot read {source}: {error}; install takes a git URL or a local directory \
                 holding a rice"
            ),
        )
    })?;
    if !canonical.is_dir() {
        return Err(Refusal::new(
            "acquire",
            None,
            format!("{source} is not a directory; install takes a git URL or a local directory"),
        ));
    }
    // The commit is the checkout's HEAD when the directory is a checkout. A
    // missing git and a directory that is not a checkout are the same answer —
    // `source_commit` is null — and neither of them is a reason to refuse a rice
    // the user is holding on disk.
    let commit = if tools::probe(Tool::Git).succeeded() {
        head_commit(&canonical).filter(|commit| commit.chars().all(|c| c.is_ascii_hexdigit()))
    } else {
        None
    };
    Ok(Acquired {
        url: format!("file://{}", canonical.display()),
        commit,
        tree: canonical,
        clone: None,
    })
}

/// The cache entry `repo` holds for `url`, when it holds one that is still
/// there and still inside `repo`.
///
/// The containment check is not decoration: `head.json` is a file on disk that a
/// user may edit, and a cache that could name an arbitrary directory would be a
/// way to make an install read — let alone install — anything on the machine.
fn cached(repo: &Path, url: &str) -> Option<HeadRecord> {
    let record = std::fs::read_to_string(repo.join(HEAD_RECORD)).ok()?;
    let record: HeadRecord = serde_json::from_str(&record).ok()?;
    if record.url != url || record.commit.is_empty() {
        return None;
    }
    let path = record.path.canonicalize().ok()?;
    if !path.starts_with(repo) || !path.is_dir() {
        return None;
    }
    Some(HeadRecord {
        url: record.url,
        commit: record.commit,
        path,
    })
}

/// Writes the cache index, atomically in the way everything else in this tool
/// writes: a temp file and a rename.
fn write_head(repo: &Path, record: &HeadRecord) -> Result<(), String> {
    let rendered = serde_json::to_string_pretty(record)
        .map_err(|error| format!("cannot render the cache index: {error}"))?;
    let temporary = repo.join(format!("{HEAD_RECORD}.tmp"));
    std::fs::write(&temporary, rendered)
        .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    std::fs::rename(&temporary, repo.join(HEAD_RECORD))
        .map_err(|error| format!("cannot write {}: {error}", repo.join(HEAD_RECORD).display()))
}

/// A full clone — no `--depth`, no `--branch` — of the default branch.
///
/// The clone is not shallow because dotfiles are tiny and both the manifest's
/// commit pin and every future update path need the history; it is not
/// branch-pinned because the recipe is "install the rice", and a rice's default
/// branch is what its author means by the repository.
fn run_git(args: &[&str]) -> Result<(), Refusal> {
    let failed = |detail: String| -> Refusal {
        Refusal::new(
            "acquire",
            Some("clone-failed"),
            format!("git {} failed: {detail}", args.join(" ")),
        )
    };
    match Command::new(Tool::Git.name()).args(args).output() {
        Err(error) => Err(failed(format!("cannot run git: {error}"))),
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => {
            let stderr = first_line(&output.stderr);
            let detail = match stderr {
                Some(line) => line,
                None => match output.status.code() {
                    Some(code) => format!("exited {code}"),
                    None => "was killed by a signal".to_string(),
                },
            };
            Err(failed(detail))
        }
    }
}

/// The HEAD commit of `tree`, as `git` reports it: the first line of
/// `git -C <tree> rev-parse HEAD`. Anything else — a directory that is not a
/// checkout, a git that is not there — is `None`.
fn head_commit(tree: &Path) -> Option<String> {
    let directory = tree.to_string_lossy().into_owned();
    let outcome = Command::new(Tool::Git.name())
        .args(["-C", &directory, "rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !outcome.status.success() {
        return None;
    }
    let commit = first_line(&outcome.stdout)?;
    let commit = commit.trim().to_string();
    (!commit.is_empty() && !commit.contains(char::is_whitespace) && commit != NO_COMMIT)
        .then_some(commit)
}

/// Undoes a clone that did not finish: the staged tree goes, and the directories
/// this run opened go with it when they are empty. A refusal that leaves an empty
/// `sources/` behind is a refusal that created something.
fn unwind(sources: &Path, repo: &Path, staging: &Path) {
    let _ = std::fs::remove_dir_all(staging);
    // Both of these fail harmlessly when the directory is not empty — a cache
    // that already holds clones is not this run's to delete.
    let _ = std::fs::remove_dir(repo);
    let _ = std::fs::remove_dir(sources);
}

/// The directory name a URL's repository is cached under: its last path segment,
/// minus a trailing `.git`, held to the same rule a profile name is held to — it
/// is a directory name inside `sources/`, and must not reach outside it.
fn slug_of(url: &str) -> Result<String, Refusal> {
    let trimmed = url.trim_end_matches('/');
    let last = trimmed.rsplit('/').next().unwrap_or(trimmed);
    // `git@host:path/to/dotfiles.git` has no `/` before the last segment's host.
    let last = last.rsplit(':').next().unwrap_or(last);
    let slug = last.strip_suffix(".git").unwrap_or(last);
    if !crate::profile::valid_name(slug) {
        return Err(Refusal::new(
            "acquire",
            None,
            format!(
                "cannot derive a cache directory name from {url}: `{slug}` is not one — a slug \
                 cannot be empty, contain `/`, or start with `.`"
            ),
        ));
    }
    Ok(slug.to_string())
}

// --------------------------------------------------------------- the identity

/// What the acquired tree turned out to be.
struct Identity {
    /// The shell that will own the profile — and therefore the profile's name.
    shell: String,
    /// Every shell the tree carried, in name order.
    candidates: Vec<String>,
    /// Quickshell config directories that carried no `shell.qml`, named in the
    /// refusal when nothing did: a tree one file short of a rice should say so.
    unmarked: Vec<String>,
    /// Whether the tree carries a Hyprland config at all.
    hypr: bool,
}

/// Step 1: what shell is this, and which one of them.
///
/// The hard rule is the refusal: a tree without a Quickshell shell is refused
/// even when it carries everything else a Hyprland rice has. A Hyprland config
/// beside the tree is reported as a fact — it is what most of these repos are —
/// but it is not a desktop RiceSwap can install, check, or later verify, and
/// guessing one would produce a profile whose shell nobody can reason about.
/// "Either signal suffices" is about markers (a `shell.qml` is the marker even
/// when the rest of the tree is unusual); the absence of a shell is the
/// unsupported-shell case, and when in doubt this refuses with the candidates
/// it found.
fn identify(tree: &Path, requested: Option<&str>) -> Result<Identity, Refusal> {
    let quickshell = tree.join(".config").join("quickshell");
    let mut candidates: Vec<String> = Vec::new();
    let mut unmarked: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&quickshell) {
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if !entry.path().is_dir() {
                continue;
            }
            // The marker is the entry point itself, matched case-sensitively:
            // `Shell.qml` is somebody's component, not a shell.
            if entry.path().join("shell.qml").is_file() {
                candidates.push(name);
            } else {
                unmarked.push(name);
            }
        }
    }
    candidates.sort();
    unmarked.sort();
    let hypr = tree.join(".config").join("hypr").is_dir();

    if candidates.is_empty() {
        return Err(not_a_rice(tree, &unmarked, hypr));
    }

    let shell = match requested {
        Some(requested) => {
            if !candidates.iter().any(|candidate| candidate == requested) {
                return Err(Refusal::new(
                    "identify",
                    None,
                    format!(
                        "--shell `{requested}` names no shell in {}; it carries: {}",
                        tree.display(),
                        candidates.join(", ")
                    ),
                )
                .with_fact("candidates", json!(candidates))
                .with_next("pass --shell with one of the shells the tree carries"));
            }
            requested.to_string()
        }
        None => match candidates.as_slice() {
            [only] => only.clone(),
            _ => {
                return Err(Refusal::new(
                    "identify",
                    Some("ambiguous-identity"),
                    format!(
                        "{} carries more than one quickshell shell: {}; pass --shell <name> to \
                         install the one you meant",
                        tree.display(),
                        candidates.join(", ")
                    ),
                )
                .with_fact("candidates", json!(candidates))
                .with_next("re-run with --shell <name>"));
            }
        },
    };

    Ok(Identity {
        shell,
        candidates,
        unmarked,
        hypr,
    })
}

/// The not-a-quickshell-rice refusal, saying what the tree does carry so the
/// user can see how close it is.
fn not_a_rice(tree: &Path, unmarked: &[String], hypr: bool) -> Refusal {
    let found = if unmarked.is_empty() {
        "it has no `.config/quickshell/` directory at all".to_string()
    } else {
        format!(
            "its quickshell config{} ({}) carry no `shell.qml`, which is the marker a \
             Quickshell shell is identified by",
            if unmarked.len() == 1 { " carries" } else { "s" },
            unmarked.join(", ")
        )
    };
    let message = format!(
        "{} is not a quickshell rice: {found}; RiceSwap installs quickshell desktops, whose \
         shell announces itself with `.config/quickshell/<name>/shell.qml`",
        tree.display()
    );
    let mut refusal = Refusal::new("identify", Some("not-a-quickshell-rice"), message)
        .with_fact("hypr_config", json!(hypr))
        .with_fact("quickshell_configs", json!(unmarked))
        .with_next(
            "install a rice whose shell is a Quickshell config with a `shell.qml` entry point",
        );
    if hypr {
        refusal = refusal.with_next(
            "the tree does carry a Hyprland config; RiceSwap installs the Quickshell shell that \
             config drives, and this one names none",
        );
    }
    refusal
}

// ------------------------------------------------------------ materialization

/// Which way the profile-exists gate went.
#[derive(PartialEq, Eq, Clone, Copy)]
enum Claim {
    /// Nothing was there: this install makes the profile.
    Fresh,
    /// A half-finished install of this very source left it: this install re-makes
    /// it, because every step below is idempotent.
    Resumed,
}

/// The exists-check, and the one condition under which it does not refuse.
///
/// The permission to overwrite is narrow and comes from outside the profile
/// directory: `state.json` must name an `install` that claimed *this* profile
/// from *this* source and never finished. A `switch` that touched the same
/// profile records no such claim, and neither does an install of a different
/// source — a half-written profile of someone else's rice is not this install's
/// to clear.
fn claim_profile(
    context: &mut Context,
    name: &str,
    url: &str,
    profile: &Path,
) -> Result<Claim, Refusal> {
    if std::fs::symlink_metadata(profile).is_err() {
        return Ok(Claim::Fresh);
    }
    let previous = context.state().interrupted();
    if let Some(operation) = previous {
        if operation.name == "install"
            && operation.target.as_deref() == Some(name)
            && operation.source.as_deref() == Some(url)
        {
            return Ok(Claim::Resumed);
        }
    }
    let other = previous.and_then(|operation| {
        if operation.name == "install" && operation.target.as_deref() == Some(name) {
            operation.source.clone()
        } else {
            None
        }
    });
    let message = match other {
        Some(source) => format!(
            "a profile already exists at {}, and the install that left it was of {source}, not \
             of {url}; an install never overwrites a profile it did not start",
            profile.display()
        ),
        None => format!(
            "a profile already exists at {}; an install refuses to overwrite one — delete it \
             with `riceswap delete {name} --force` first, or install another rice",
            profile.display()
        ),
    };
    Err(Refusal::new("materialize", Some("profile-exists"), message)
        .with_fact("profile", json!(name)))
}

/// What materialization produced.
struct Materialized {
    /// The manifest as written.
    manifest: Manifest,
    /// The `$HOME`-relative paths the profile actually mirrored.
    mirrored: Vec<String>,
    /// The packages the acquired tree's configs named, split the manifest's way.
    packages: PackageScan,
    /// Where the imported wallpapers landed in the shared layer.
    wallpapers: Vec<String>,
}

/// Step 2: the snapshot machinery, pointed at the acquired tree.
///
/// Every part of this is the one `snapshot` runs — the same detection, the same
/// mirroring (symlinks dereferenced, so the profile holds real files), the same
/// shared-hardware `source =` line, the same manifest fields — with the four a
/// snapshot has no way to know filled in from what was acquired: the source URL,
/// the commit, the description, and the shell. There is no screenshot: an
/// acquired tree is not a running desktop.
fn materialize(
    context: &mut Context,
    name: &str,
    acquired: &Acquired,
    claim: Claim,
    warnings: &mut Vec<String>,
    researched: Option<&Findings>,
) -> Result<Materialized, Refusal> {
    let profile = context.store().profile_dir(name);
    // The collision rule is the snapshot's own: an existing profile refuses
    // unless it is cleared first. Only this install's own half-made profile is
    // ever cleared, and only when the journal says this install left it.
    if let Err(error) = operations::clear_for_overwrite(&profile, claim == Claim::Resumed) {
        return Err(Refusal::new("materialize", None, error));
    }
    if let Err(error) = std::fs::create_dir_all(&profile) {
        return Err(Refusal::new(
            "materialize",
            None,
            format!(
                "cannot create the profile directory {}: {error}",
                profile.display()
            ),
        ));
    }

    let tree = &acquired.tree;
    let config = detection::scan_config(tree);
    let mut packages = detection::scan_packages(tree, &config.dirs);
    // The research answer's packages join the scan's own, before the manifest
    // is written, so the switch that follows plans against the union: a shell
    // that needs a package nothing in its configs names (`quickshell` itself is
    // the usual one) is exactly the case a static scan cannot see and a brief
    // that asks "which packages does it need" can.
    if let Some(findings) = researched {
        for package in research::merge_packages(&mut packages, findings) {
            warnings.push(format!(
                "the research answer for `{name}` named a package: {package}"
            ));
        }
    }
    let mut selected = config.dirs.clone();
    selected.extend(detection::scan_assets(tree));
    selected.sort();
    selected.dedup();

    let mirrored = operations::mirror_selection(tree, &selected, &profile, warnings);

    let captured = profile.join(".config").join("hypr").join("hyprland.conf");
    if captured.is_file() {
        if let Err(error) = operations::inject_hardware_source(&captured) {
            return Err(Refusal::new("materialize", None, error));
        }
    }

    let wallpapers = import_wallpapers(context, tree, warnings);

    let manifest = install_manifest(name, &mirrored, &packages, config.services, acquired);
    if let Err(error) = context.store().save(name, &manifest) {
        return Err(Refusal::new("materialize", None, error));
    }

    Ok(Materialized {
        manifest,
        mirrored,
        packages,
        wallpapers,
    })
}

/// The `profile.toml` an install writes: the snapshot's document plus the facts
/// only an acquisition has — where it came from, at which commit, which shell it
/// is — and no screenshot, because there is no desktop to photograph.
fn install_manifest(
    name: &str,
    files: &[String],
    packages: &PackageScan,
    services: Vec<Service>,
    acquired: &Acquired,
) -> Manifest {
    // The same manifest a snapshot writes, with the four facts only an install
    // has: where it came from, the commit, the description, and the shell.
    operations::build_manifest_from(
        name,
        false,
        files,
        packages,
        services,
        operations::ManifestOrigin {
            description: format!("installed from {}", acquired.url),
            source_url: Some(acquired.url.clone()),
            source_commit: acquired.commit.clone(),
            shell: Some(shell_of(name)),
        },
    )
}

/// The `[shell]` table an installed profile records.
///
/// A shell is driven by two commands and a name, and no built-in recipe carries
/// them: the recipe schema's `[shell]` is the identity and the appid pin, not a
/// launch recipe. So this is the convention every shipped quickshell manifest
/// already uses — `qs -c <name>` starts the config the profile owns, and
/// `pkill qs` ends whichever one is running, which is what the switch needs to
/// swap one shell for another.
fn shell_of(name: &str) -> Shell {
    Shell {
        name: name.to_string(),
        start: format!("qs -c {name}"),
        stop: "pkill qs".to_string(),
    }
}

/// Imports the images the acquired tree carries into the shared wallpapers
/// layer.
///
/// The candidates are the ones `detect` has always proposed — the `Downloads`
/// folder — plus the `Wallpapers` directory the built-in caelestia recipe itself
/// declares as the path that shell reads, which is where a rice bundles its own.
/// They are **copied**, never moved: `wallpaper-import` moves because it takes
/// ownership of an image the user downloaded, while here the tree is either this
/// tool's cache or the user's own directory, and neither is a thing an install
/// may take bytes out of.
fn import_wallpapers(context: &Context, tree: &Path, warnings: &mut Vec<String>) -> Vec<String> {
    let mut candidates: Vec<PathBuf> = detection::scan_wallpapers(tree)
        .into_iter()
        .map(PathBuf::from)
        .collect();
    let bundled = tree.join("Wallpapers");
    if let Ok(entries) = std::fs::read_dir(&bundled) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_image = path
                .extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_ascii_lowercase)
                .is_some_and(|extension| IMAGE_EXTENSIONS.contains(&extension.as_str()));
            if is_image && path.is_file() {
                candidates.push(path);
            }
        }
    }
    if candidates.is_empty() {
        return Vec::new();
    }
    let layer = context.store().wallpapers_dir();
    if let Err(error) = std::fs::create_dir_all(&layer) {
        warnings.push(format!(
            "cannot open the shared wallpapers layer {}: {error}",
            layer.display()
        ));
        return Vec::new();
    }
    let mut imported = Vec::new();
    for candidate in candidates {
        if let Err(error) = operations::validate_image(&candidate) {
            warnings.push(format!(
                "skipped the wallpaper candidate {}: {error}",
                candidate.display()
            ));
            continue;
        }
        let Some(name) = candidate.file_name() else {
            continue;
        };
        let destination = operations::available_destination(&layer, &candidate, name);
        if let Err(error) = std::fs::copy(&candidate, &destination) {
            warnings.push(format!(
                "cannot copy {} into {}: {error}",
                candidate.display(),
                destination.display()
            ));
            continue;
        }
        imported.push(destination.display().to_string());
    }
    imported
}

// ------------------------------------------------------------------- envelope

/// Announces a reconciliation pass, through the switch's own mouth, so a repair
/// reads the same whether install made it or the switch did.
fn announce(emitter: &mut Emitter, report: &reconcile::Report, warnings: &mut Vec<String>) {
    operations::announce_reconcile(emitter, report, warnings);
}

/// The install envelope's `data`: what was acquired, what it turned out to be,
/// what was written, what the engine made of it, and what the switch did.
fn payload(
    acquired: &Acquired,
    identity: &Identity,
    materialized: &Materialized,
    claim: Claim,
    report: &reconcile::Report,
    switch: Option<Value>,
    researched: &research::Research,
) -> Value {
    let clone = acquired.clone.as_ref().map(|clone| {
        json!({
            "slug": clone.slug,
            "commit": clone.commit,
            "path": clone.path.display().to_string(),
            "reused": clone.reused,
        })
    });
    json!({
        "profile": identity.shell,
        "shell": identity.shell,
        "resumed": claim == Claim::Resumed,
        "source": {
            "url": acquired.url,
            "commit": acquired.commit,
            "tree": acquired.tree.display().to_string(),
            "cache": clone,
        },
        "identity": {
            "candidates": identity.candidates,
            "selected": identity.shell,
            "hypr": identity.hypr,
            "quickshell_configs_without_marker": identity.unmarked,
        },
        "research": researched.payload(),
        "manifest_written": true,
        "checked_paths": materialized.mirrored,
        "checked_packages": {
            "official": materialized.packages.official,
            "aur": materialized.packages.aur,
        },
        "wallpapers": materialized.wallpapers,
        "services": materialized.manifest.services.iter().map(|s| s.name.clone()).collect::<Vec<String>>(),
        "reconcile": report,
        "switch": switch,
        "tools": { "git": tools::probe(Tool::Git) },
    })
}

/// A refusal: what stopped the install, in which phase, under which of the reason
/// codes the spec froze, and what the user can do about it.
///
/// Nothing is created by a refusal — that is the property of this whole phase of
/// the pipeline — so these carry no partial state, only the facts the panel needs
/// to render the next step.
struct Refusal {
    phase: &'static str,
    /// One of the frozen acquisition codes, when the refusal has one. A refusal
    /// that is really a bad argument (`--shell` naming no candidate, a source
    /// that is not there) carries none: the message is its contract, exactly as
    /// it is for a bad flag.
    reason_code: Option<&'static str>,
    message: String,
    facts: Map<String, Value>,
    next: Option<String>,
}

impl Refusal {
    fn new(
        phase: &'static str,
        reason_code: Option<&'static str>,
        message: impl Into<String>,
    ) -> Refusal {
        Refusal {
            phase,
            reason_code,
            message: message.into(),
            facts: Map::new(),
            next: None,
        }
    }

    /// Adds a fact beside the message — the candidates, the shell list, the
    /// probe that decided the refusal.
    fn with_fact(mut self, key: &str, value: Value) -> Refusal {
        self.facts.insert(key.to_string(), value);
        self
    }

    /// What the user can do about this refusal.
    fn with_next(mut self, next: impl Into<String>) -> Refusal {
        self.next = Some(next.into());
        self
    }

    /// The failed envelope, in the frozen shape: `ok: false`, the message in
    /// place of `data` — with the diagnosis payload beside it.
    fn into_envelope(self) -> Envelope {
        let mut data = Map::new();
        data.insert("error".to_string(), Value::String(self.message));
        data.insert("phase".to_string(), Value::String(self.phase.to_string()));
        if let Some(code) = self.reason_code {
            data.insert("reason_code".to_string(), Value::String(code.to_string()));
        }
        if let Some(next) = self.next {
            data.insert("next".to_string(), Value::String(next));
        }
        for (key, value) in self.facts {
            data.insert(key, value);
        }
        Envelope {
            ok: false,
            warnings: Vec::new(),
            data: Value::Object(data),
        }
    }
}

/// First non-empty line of a stream, trimmed and capped so one chatty tool cannot
/// bloat a refusal.
fn first_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(200).collect())
}
