//! The research tier (issue #38) — asking `pi` about a rice no recipe covers,
//! and keeping the answer as a user-tier recipe.
//!
//! #35's engine can prove what a shell *registers* and therefore repair a
//! dispatch that is the same name under another namespace. It cannot know what
//! a key *meant*, and a static parse cannot tell a registration built from a
//! variable from one that was never written. This module is the other half: when
//! a rice is unknown, one `pi` run is asked the questions the engine's own
//! survey raises, and the answer — validated, and only then — is written to
//! `<data-dir>/recipes/<shell>.toml`, the user tier [`recipe::Layers`] already
//! reads — so the adapt pass that runs a few lines later in the very same
//! install, and every switch after it, has a codebook.
//!
//! The file is never overwritten, and its presence *is* a reason to stay quiet:
//! a shell with a recipe here has already been researched, and the refresh path
//! is deleting that file. Re-asking would cost a model a run whose answer the
//! write-back would then refuse to install, which is the worst of both.
//!
//! The invocation is the contract in `docs/research/pi-invocation-contract.md`,
//! followed flag for flag:
//!
//! ```text
//! pi --offline -p --mode json --no-session --no-context-files --no-approve \
//!    --tools read,grep,find,ls --model <model> --thinking off -- "<brief>"
//! ```
//!
//! with the acquired tree as the working directory and stdin closed. The
//! tool allowlist is the sandbox: the model can read the clone and can neither
//! run a command nor write a byte, and `--no-context-files` keeps a rice's own
//! `AGENTS.md` from rewriting the brief.
//!
//! **Exit 0 is not success.** A provider that refuses a request at request time
//! (a free tier used from outside its own tool, say) leaves pi exiting 0 with a
//! perfectly well-formed stream whose final assistant message carries
//! `stopReason: "error"`. Success is [`StopReason::Stop`] on the *last* assistant
//! `message_end`, and nothing else counts.
//!
//! **Nothing here can fail an install.** Every failure below is a warning and
//! the engine + declarations floor (#29): no `pi`, a hung run, a provider
//! refusal, a model that will not answer in JSON — all of them cost the research
//! tier and nothing else, because a rice that installs without a codebook is a
//! working desktop with a few dead binds, and a rice that refuses to install is
//! a broken one.
//!
//! The budgets are configurable so the sandbox can test the timeout path in
//! milliseconds instead of minutes — see [`Config`].

use crate::detection::PackageScan;
use crate::envelope::Emitter;
use crate::recipe::{self, Recipe};
use crate::reconcile::{self, Survey};
use crate::tools::{self, Tool};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

/// The document `pi` is asked for, and the only shape this module accepts. The
/// version string is what lets a stale answer be refused rather than
/// half-understood.
///
/// The first block is the contract's own field list; the rest is what the
/// write-back needs beyond it — the `[[dirs]]`, `[[resolution]]`, `[foreign]`
/// and `[packages]` of a recipe — so one run produces both the codebook and the
/// package set the manifest merges.
pub const SCHEMA: &str = "riceswap.research.v1";

/// The model the tier asks for when nothing says otherwise.
///
/// This is the one provider the contract measured as dependable, and it is a
/// *documented default*, not a decision: `RICESWAP_PI_MODEL` overrides it, and
/// the honest cost story ("this costs you money unless you set the variable")
/// is in the install envelope's `research.model` either way.
pub const DEFAULT_MODEL: &str = "anthropic/claude-sonnet-4-5";

/// The model the bounded retry chain falls back to — the contract's chain is
/// "configured model, then a known-good one, then the floor".
const FALLBACK_MODEL: &str = "anthropic/claude-sonnet-4-5";

/// How long a run may take to say *anything* before it is killed. The contract
/// measured healthy runs emitting the session header in seconds and two free
/// providers producing zero bytes for ninety.
const DEFAULT_FIRST_BYTE: Duration = Duration::from_secs(45);

/// How long a whole run may take.
const DEFAULT_TOTAL: Duration = Duration::from_secs(300);

/// How long a run may stall mid-stream — no new record for this long is a hang
/// whatever the process thinks of itself.
const DEFAULT_STALL: Duration = Duration::from_secs(90);

/// How many models the chain tries before the floor. One attempt per model, one
/// repair pass for the whole run: the tier may not become the reason an install
/// takes five minutes.
const DEFAULT_ATTEMPTS: usize = 2;

/// The ceiling on the transcript handed to the repair pass. The repair only has
/// to lift JSON out of prose, and a clipped transcript is better than a second
/// run that also has to be waited for.
const MAX_TRANSCRIPT: usize = 4_000;

/// The probe's own budget. `pi auth check` is a local credentials read; if it
/// has not answered in this long something is wrong that a longer wait will not
/// fix.
const AUTH_BUDGET: Duration = Duration::from_secs(15);

/// How many dead names the brief names, and how many parse findings it quotes.
/// A rice whose configs dispatch sixty names produces a brief a model can no
/// longer hold in its head, and a question about sixty dead binds is a question
/// about the config file rather than about the shell. The rest is counted, so
/// the model knows the list is a sample and not the whole truth.
const MAX_BRIEF_NAMES: usize = 25;
const MAX_BRIEF_FINDINGS: usize = 10;

/// How many JSONL records one run may keep. The stream is drained either way —
/// a pipe nobody reads wedges the child — but a run that emits megabytes of
/// deltas is not a run whose *answer* is in its hundred-thousandth record, and
/// the total budget already bounds the time. Past the ceiling the records are
/// read and dropped, and the count reported is the ceiling.
const MAX_RECORDS: usize = 20_000;

/// Grace between `SIGTERM` and `SIGKILL` when a budget is blown. The contract
/// measured pi's own clean teardown as fast; this is the backstop for the run
/// that ignores the polite signal.
const TERM_GRACE: Duration = Duration::from_secs(2);

/// The fewest provable registrations a shell's own QML must offer before the
/// tier considers the static parse good enough to skip asking.
///
/// One is the honest floor and not a tuned number: a shell the parse proved
/// *something* about, with no parse finding left open, is a shell the engine
/// has a registry for, and the research brief would have nothing to add. A
/// registry of zero, or one the parse itself called uncertain, is exactly the
/// case this tier exists for.
const MIN_PROVEN_REGISTRATIONS: usize = 1;

/// What to send `pi`, and how long to wait for it.
///
/// Every field is read from the environment so the sandbox — and any user who
/// has a faster or cheaper model — can set it without a rebuild:
///
/// | variable | meaning | default |
/// |---|---|---|
/// | `RICESWAP_PI_MODEL` | the `--model` to ask for | [`DEFAULT_MODEL`] |
/// | `RICESWAP_PI_FIRST_BYTE_SECONDS` | budget for the first stdout line | 45 |
/// | `RICESWAP_PI_STALL_SECONDS` | budget between two records | 90 |
/// | `RICESWAP_PI_TOTAL_SECONDS` | budget for a whole run | 300 |
/// | `RICESWAP_PI_ATTEMPTS` | models to try before the floor | 2 |
///
/// A value that is not a number is ignored in favour of the default, because a
/// typo in an environment variable must not cost the tier its only budget.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub model: String,
    pub first_byte: Duration,
    pub stall: Duration,
    pub total: Duration,
    pub attempts: usize,
}

impl Config {
    /// The configuration this process was started with.
    pub fn from_env() -> Config {
        Config {
            model: std::env::var("RICESWAP_PI_MODEL")
                .ok()
                .map(|model| model.trim().to_string())
                .filter(|model| !model.is_empty())
                .unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            first_byte: seconds("RICESWAP_PI_FIRST_BYTE_SECONDS", DEFAULT_FIRST_BYTE),
            stall: seconds("RICESWAP_PI_STALL_SECONDS", DEFAULT_STALL),
            total: seconds("RICESWAP_PI_TOTAL_SECONDS", DEFAULT_TOTAL),
            attempts: std::env::var("RICESWAP_PI_ATTEMPTS")
                .ok()
                .and_then(|value| value.trim().parse::<usize>().ok())
                .filter(|attempts| *attempts > 0)
                .unwrap_or(DEFAULT_ATTEMPTS),
        }
    }

    /// The models the retry chain walks, in order, capped at `attempts`. The
    /// configured model first — a user who pinned one asked for it — and the
    /// contract's dependable fallback second, deduplicated so a configuration
    /// that *is* the fallback costs one attempt rather than two. With the
    /// default configuration that means the chain is one model long: the
    /// default is already the dependable one, so there is nothing to fall back
    /// *to*.
    fn chain(&self) -> Vec<String> {
        let mut models = vec![self.model.clone()];
        if !models.iter().any(|model| model == FALLBACK_MODEL) {
            models.push(FALLBACK_MODEL.to_string());
        }
        models.truncate(self.attempts.max(1));
        models
    }
}

fn seconds(variable: &str, default: Duration) -> Duration {
    std::env::var(variable)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .map(Duration::from_secs)
        .unwrap_or(default)
}

// ------------------------------------------------------------------ the brief

/// What this run is about: the tree to read, the profile the recipe will serve,
/// and the shell nobody has a recipe for.
pub struct Subject<'a> {
    /// The `$HOME` a declared path is bounded by.
    pub home: &'a Path,
    /// RiceSwap's data directory, holding the user tier.
    pub data: &'a Path,
    /// The profile being installed — consulted for the one tier that outranks
    /// everything: the human one.
    pub profile: &'a Path,
    /// The acquired tree: `pi`'s working directory, and the only thing it may
    /// read.
    pub tree: &'a Path,
    /// The Quickshell config the profile is about to own.
    pub shell: &'a str,
}

/// Why the tier did not ask, when it did not ask.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum Skip {
    /// A built-in recipe already answers for this shell. Asking would be slower
    /// and less specific than the file that ships with the binary, and the
    /// built-in is the offline-deterministic answer by construction.
    Known { shell: String },
    /// The profile's own `adapt.toml` speaks for this shell — the human tier,
    /// which outranks the built-in, the user tier and every auto-move (#28).
    /// Researching on top of it would produce a second opinion nobody asked for.
    HumanTier { file: String },
    /// A user-tier recipe for this shell is already on disk. This is the tier's
    /// own answer, from a previous run, and re-asking because it is there would
    /// spend a model to write a file that is then kept for exactly the reason it
    /// is being skipped. The file is *evidence*, not a guess — which is why the
    /// refresh path is deleting it, not re-running the install.
    UserTier { file: String },
    /// The static parse was good enough: it proved registrations and had no
    /// finding left open, so the brief would have been a list of questions the
    /// engine can already answer.
    Confident { registered: usize },
}

impl Skip {
    /// The tier's own spelling, for the envelope and for the report a reader
    /// sees when nothing was asked.
    pub fn reason(&self) -> &'static str {
        match self {
            Skip::Known { .. } => "a built-in recipe already covers this shell",
            Skip::HumanTier { .. } => "the profile's own adapt.toml already answers for it",
            Skip::UserTier { .. } => {
                "a user-tier recipe for this shell already exists; delete it to research it again"
            }
            Skip::Confident { .. } => "the static parse of its QML was not low-confidence",
        }
    }
}

/// How a run ended.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Never asked: the shell is not unknown.
    Skipped(Skip),
    /// `pi` is not there, or not ready. The floor, with the probe's own words.
    Unavailable(String),
    /// Asked, and no usable answer came back. The floor, with the last reason.
    Floor(Failure),
    /// Asked, and answered with a document that validated. Boxed because the
    /// document is an order of magnitude larger than the other three arms, and
    /// an enum sized by its rarest case is a stack cost every caller pays.
    Answered(Box<Answer>),
}

/// A validated answer, and what it cost to get.
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    pub findings: Findings,
    /// The model the answer actually came from — the last one the chain tried.
    pub model: String,
    /// How many `pi` processes this run started, repair pass included.
    pub invocations: usize,
    /// Whether a repair pass was spent lifting JSON out of prose.
    pub repaired: bool,
    /// The final `message_end`'s `usage`, logged because every record carries
    /// one and the cost of this tier should never be a mystery.
    pub usage: Option<Value>,
}

/// A run that asked and did not get one. Every arm of the contract's failure
/// matrix lands here, and each carries the sentence a user can act on.
#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    /// Why the last attempt failed, in the contract's own terms.
    pub reason: String,
    /// The model the failure belongs to.
    pub model: String,
    pub invocations: usize,
    pub repaired: bool,
    /// How many JSONL records the killed run had produced, for the diagnostics
    /// a user can attach to a bug report.
    pub records: usize,
}

/// One research run: what the survey said, what the tier did about it, and how it
/// is written into the install envelope.
#[derive(Clone, Debug, PartialEq)]
pub struct Research {
    /// The shell the run was about, so every message this run emits can name it
    /// without the caller threading it through.
    pub shell: String,
    /// Where a recipe for this shell would live. Recorded whether or not
    /// anything was written, so the envelope names the file in every arm rather
    /// than only the happy one.
    pub recipe_path: PathBuf,
    pub survey: Survey,
    pub outcome: Outcome,
    pub config: Config,
    /// What became of the answer, once it has been offered to the user tier.
    /// `None` before [`Research::write_back`] runs, and forever after a run that
    /// had no answer to offer.
    write_back: Option<WriteBack>,
}

impl Research {
    /// The findings this run holds, or `None` when it holds none.
    ///
    /// Takes a reference so a caller can hold the `Research` and still name the
    /// document inside it — the install writes the recipe and merges the
    /// packages from the same answer, and neither should have to unwrap the
    /// outcome twice to get at it.
    pub fn answer(&self) -> Option<&Answer> {
        match &self.outcome {
            Outcome::Answered(answer) => Some(answer.as_ref()),
            _ => None,
        }
    }

    /// Streams the run's own warning, if it earned one, and folds the same text
    /// into the envelope.
    ///
    /// A skip is not a warning: not asking a rice that a built-in recipe
    /// already covers is the designed behaviour, not a loss. Everything else
    /// is — a research tier that quietly did nothing is a downgrade nobody was
    /// told about.
    pub fn announce(&self, emitter: &mut Emitter, warnings: &mut Vec<String>) {
        let note = match &self.outcome {
            Outcome::Skipped(_) => return,
            Outcome::Unavailable(reason) => format!(
                "the AI research tier is off for `{}`: {reason}; the install continues on the \
                 engine's proposals and the profile's own declarations",
                self.shell
            ),
            Outcome::Floor(failure) => format!(
                "the AI research tier produced no recipe for `{}`: {}; the install continues on \
                 the engine's proposals and the profile's own declarations",
                self.shell, failure.reason
            ),
            Outcome::Answered(_) => return,
        };
        emitter.warning(&note);
        warnings.push(note);
    }

    /// Offers this run's answer to the user tier, and says what became of it.
    ///
    /// Nothing happens without an answer, and nothing here can fail: the two
    /// refusals — a recipe that is already there, and a document that carries
    /// no fact the schema can hold — are warnings that name the file and the
    /// way out, because a user who cannot see why their codebook was not
    /// updated will read the file as current.
    pub fn write_back(
        &mut self,
        subject: &Subject,
        emitter: &mut Emitter,
        warnings: &mut Vec<String>,
    ) {
        let offered = match &self.outcome {
            Outcome::Answered(answer) => Some(write_back(subject, &answer.findings)),
            _ => None,
        };
        let Some((outcome, notes)) = offered else {
            return;
        };
        let said = format!("the research answer for `{}`", self.shell);
        for note in notes {
            let note = format!("{said}: {note}");
            emitter.warning(&note);
            warnings.push(note);
        }
        if outcome.written() {
            emitter.progress(&format!(
                "{said} became the user-tier recipe at {}",
                outcome.path().display()
            ));
        }
        let refusal = match &outcome {
            WriteBack::Kept(path) => format!(
                "the research answer for `{}` was not written: a recipe already exists at {},                  and a research run never overwrites one — delete it to research this shell again",
                self.shell,
                path.display()
            ),
            WriteBack::Declined(path, reason) => format!(
                "the research answer for `{}` was not written to {}: {reason}",
                self.shell,
                path.display()
            ),
            WriteBack::Written(_) => String::new(),
        };
        if !refusal.is_empty() {
            emitter.warning(&refusal);
            warnings.push(refusal);
        }
        self.write_back = Some(outcome);
    }

    /// The install envelope's `research` object: whether a question was asked,
    /// what it cost, what it said, and where the answer went.
    pub fn payload(&self) -> Value {
        let mut payload = json!({
            "asked": !matches!(self.outcome, Outcome::Skipped(_)),
            "model": self.config.model,
            "budgets_seconds": {
                "first_byte": self.config.first_byte.as_secs(),
                "stall": self.config.stall.as_secs(),
                "total": self.config.total.as_secs(),
            },
            "survey": self.survey,
        });
        match &self.outcome {
            Outcome::Skipped(skip) => {
                payload["skipped"] = json!(skip.reason());
                payload["invocations"] = json!(0);
            }
            Outcome::Unavailable(reason) => {
                payload["failure"] = json!(reason);
                payload["invocations"] = json!(0);
            }
            Outcome::Floor(failure) => {
                payload["failure"] = json!(failure.reason);
                // The model that failed is the one a user would change, so it is
                // reported rather than replaced by the configured one.
                payload["model"] = json!(failure.model);
                payload["invocations"] = json!(failure.invocations);
                payload["repaired"] = json!(failure.repaired);
                payload["partial_records"] = json!(failure.records);
            }
            Outcome::Answered(answer) => {
                payload["invocations"] = json!(answer.invocations);
                payload["repaired"] = json!(answer.repaired);
                payload["model"] = json!(answer.model);
                // Widened and rounded, because `confidence` is a 32-bit float
                // in the schema and `0.72` is not representable in one: an
                // envelope reporting `0.7200000286102295` of a model's own
                // self-assessment is noise where a number was meant. The
                // arithmetic is done in `f64`, because doing it in the answer's
                // own precision would round to the very digits it cannot hold.
                let confidence = (f64::from(answer.findings.confidence) * 1000.0).round() / 1000.0;
                payload["confidence"] = json!(confidence);
                payload["unknowns"] = json!(answer.findings.unknowns);
                payload["evidence"] = json!(answer.findings.evidence);
                payload["appid"] = json!(answer.findings.appid);
                payload["shell_kind"] = json!(answer.findings.shell);
                payload["usage"] = json!(answer.usage);
            }
        }
        // The recipe arm is present in every outcome, not only the one that
        // wrote a file: a consumer asking "did this shell get a recipe, and
        // where would it be" should not have to distinguish "no" from "the key
        // is missing", and the file it names is the one a user has to look at
        // or delete to re-research.
        payload["recipe"] = match &self.write_back {
            Some(write_back) => write_back.payload(),
            None => json!({
                "written": false,
                "path": self.recipe_path.display().to_string(),
                "kept_existing": false,
                "note": match &self.outcome {
                    Outcome::Skipped(_) => "the tier did not ask, so there was nothing to write",
                    _ => "no answer came back, so there was nothing to write",
                },
            }),
        };
        payload
    }
}

// ----------------------------------------------------------------- the answers

/// One `riceswap.research.v1` document.
///
/// Deserialization is lenient on purpose: an answer carrying a key this build
/// has never heard of is still an answer, and refusing it over a field name
/// would make the tier brittle against every pi release. It is not lenient
/// about the two things that decide whether the document means anything: the
/// schema string, and the validation each field gets on the way into a recipe.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Findings {
    /// The format marker. Empty is tolerated (a model that dropped it is still
    /// telling us what we asked for); a *different* marker is refused.
    #[serde(default)]
    pub schema: String,
    /// The *kind* of shell the answer describes — `quickshell`, `hyprland`,
    /// `waybar`, `other`, the contract's own vocabulary. It is reported, and it
    /// is never the file's name: the recipe is filed under the Quickshell
    /// config the install identified, which is the thing every later pass looks
    /// it up by. A model calling the shell `quickshell` has not answered a
    /// different question, so this is a fact and not a check.
    pub shell: Option<String>,
    /// The namespace the shell registers under — the `[shell] appid` pin.
    pub appid: Option<String>,
    /// Variables that pick config or wallpaper paths.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Paths the shell reads outside its own config.
    #[serde(default)]
    pub dirs: Vec<AnswerDir>,
    /// Namespaces that belong to another live shell.
    #[serde(default)]
    pub foreign: Vec<String>,
    /// The codebook: one entry per dead dispatched name.
    #[serde(default)]
    pub resolutions: Vec<recipe::Declared>,
    /// The packages the shell needs, split the manifest's way.
    #[serde(default)]
    pub packages: AnswerPackages,
    /// How sure the answer is, `0.0`–`1.0`. Reported; never a gate — an
    /// unsure answer with evidence is more use than no answer.
    #[serde(default)]
    pub confidence: f32,
    /// The files and quotes the answer rests on.
    #[serde(default)]
    pub evidence: Vec<AnswerEvidence>,
    /// The questions that could not be answered. This list is the point of the
    /// exercise: a gap the model admits to is a gap a human can close.
    #[serde(default)]
    pub unknowns: Vec<String>,
    /// `name` → `appid`, the contract's registry view. Kept for the envelope;
    /// the recipe's `to` targets are what the adapt pass consumes.
    #[serde(default)]
    pub dispatcher_namespaces: BTreeMap<String, String>,
    /// The argv that starts the shell. The recipe schema has no launch
    /// recipe — the manifest's `[shell]` does — so this is reported, not written.
    #[serde(default)]
    pub start_cmd: Vec<String>,
    /// The argv that stops it. Reported, not written, for the same reason.
    #[serde(default)]
    pub stop_cmd: Vec<String>,
    /// The keybind scheme (`$mod`, `SUPER`, …). Reported, not written.
    pub keybind_namespace: Option<String>,
    /// Where wallpapers live. Reported, not written: the paths the recipe
    /// guarantees are the ones `[dirs]` and `[env]` name.
    pub wallpaper_dir: Option<String>,
    /// What the shell can do, in its own words. Reported, not written.
    #[serde(default)]
    pub capabilities: Vec<String>,
}

impl Findings {
    /// Whether this document carries anything the recipe schema can hold. An
    /// answer of pure `null`s is a failure to answer wearing a valid shape, and
    /// writing it would leave a file that says nothing while claiming the shell
    /// was researched.
    pub fn is_empty(&self) -> bool {
        self.env.is_empty()
            && self.dirs.is_empty()
            && self.resolutions.is_empty()
            && self.foreign.is_empty()
            && self.appid.is_none()
    }
}

/// One `dirs` entry in an answer.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct AnswerDir {
    pub path: String,
    pub symlink_to: Option<String>,
}

/// The `packages` table in an answer.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct AnswerPackages {
    #[serde(default)]
    pub official: Vec<String>,
    #[serde(default)]
    pub aur: Vec<String>,
}

/// One `evidence` entry in an answer.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct AnswerEvidence {
    pub file: String,
    #[serde(default)]
    pub quote: String,
}

// ------------------------------------------------------------------- the floor

/// Runs the tier for one install, and reports what it did.
///
/// Nothing in here can refuse. The worst outcome is [`Outcome::Floor`], which is
/// the engine + declarations behaviour an install had before this tier existed.
///
/// **This executes `pi`.** Every arm of the trigger that returns does so before
/// any process is spawned, which is what lets the unit tests drive them; an
/// outcome past the trigger reaches the probe and then the model, against
/// whatever `pi` is on the inherited `PATH`. Only the sandboxed integration
/// tests in `tests/research.rs` may take a subject that far — their `PATH` is
/// the stub, and a unit test's is the machine's.
pub fn run(subject: &Subject) -> Research {
    let config = Config::from_env();
    let survey = reconcile::survey(subject.tree, subject.shell);
    let shell = subject.shell.to_string();
    let done = |outcome| Research {
        shell: shell.clone(),
        recipe_path: recipe_path(subject.data, subject.shell),
        survey: survey.clone(),
        outcome,
        config: config.clone(),
        write_back: None,
    };

    // The trigger, in the order its reasons are worth reading. A shell a
    // built-in recipe covers needs no network and no model, and its brief would
    // be a worse version of a file that ships with the binary. A profile whose
    // human tier already answers needs nothing from a model either, and neither
    // does a shell this tier has already answered for — the write-back keeps its
    // file, so re-asking would buy a model a run it is not allowed to spend on
    // the result. What is left is the case the tier was built for: no recipe in
    // any tier, and a static parse that could not prove the registry.
    if let Some((key, _)) = recipe::BUILT_IN
        .iter()
        .find(|(key, _)| *key == subject.shell)
    {
        return done(Outcome::Skipped(Skip::Known {
            shell: (*key).to_string(),
        }));
    }
    let adapt = subject.profile.join(recipe::ADAPT_FILE);
    if adapt.is_file() {
        return done(Outcome::Skipped(Skip::HumanTier {
            file: adapt.display().to_string(),
        }));
    }
    // The user tier, by the same argument, and with the same shape: a recipe
    // there *is* this tier's answer for this shell, kept from a previous run, so
    // asking again would cost a model to produce a file the write-back then
    // refuses to replace. `is_file` rather than `exists`, because a directory at
    // that path is not a recipe and the write-back will say so if it is met.
    let user = recipe_path(subject.data, subject.shell);
    if user.is_file() {
        return done(Outcome::Skipped(Skip::UserTier {
            file: user.display().to_string(),
        }));
    }
    if survey.uncertain.is_empty() && survey.registered >= MIN_PROVEN_REGISTRATIONS {
        return done(Outcome::Skipped(Skip::Confident {
            registered: survey.registered,
        }));
    }

    // The probe, then the advisory auth check. Both are pure local reads: no
    // token is spent by either, and the auth check is treated as advisory
    // because "ready" only means a key is configured — the contract's own trap
    // is a provider that passes `auth check` and refuses every request.
    let probe = tools::probe(Tool::Pi);
    if !probe.succeeded() {
        return done(Outcome::Unavailable(format!(
            "pi is not usable ({})",
            crate::operations::describe(&probe)
        )));
    }
    if let Some(reason) = auth_is_not_ready(&config) {
        return done(Outcome::Unavailable(reason));
    }

    let brief = brief(subject, &survey);
    let models = config.chain();
    let mut tally = Tally::default();
    let mut last = "pi produced no answer".to_string();

    for (index, model) in models.iter().enumerate() {
        let attempt = match invoke(subject, &config, model, &brief, None) {
            Ok(attempt) => {
                tally.invocations += 1;
                tally.records = attempt.records;
                attempt
            }
            Err(abort) => {
                // A hang is terminal for the whole run, not a reason to try
                // another model: the budget that blew was the wall clock's, and
                // spending it twice is how an install stops being an install.
                last = abort.reason;
                tally.records = abort.records;
                break;
            }
        };

        // The refusal arm: exit 0, a well-formed stream, and an answer that says
        // it failed. Nothing downstream of here can repair that, so the chain
        // moves on to the next model.
        let Some(transcript) = attempt.answer else {
            last = attempt.failure;
            continue;
        };
        match extract(&transcript) {
            Ok(findings) => {
                return done(Outcome::Answered(Box::new(Answer {
                    findings,
                    model: model.clone(),
                    invocations: tally.invocations,
                    repaired: tally.repaired,
                    usage: attempt.usage,
                })));
            }
            Err(reason) => last = reason,
        }

        // One repair pass for the whole run, and only for a shape problem: a
        // provider that refused, or a stream that never came, is not something a
        // second, tool-less invocation fixes.
        if !tally.repaired {
            tally.repaired = true;
            match invoke(
                subject,
                &config,
                model,
                &repair(&transcript),
                Some(transcript.as_str()),
            ) {
                Ok(attempt) => {
                    tally.invocations += 1;
                    tally.records = attempt.records;
                    if let Some(text) = attempt.answer {
                        match extract(&text) {
                            Ok(findings) => {
                                return done(Outcome::Answered(Box::new(Answer {
                                    findings,
                                    model: model.clone(),
                                    invocations: tally.invocations,
                                    repaired: tally.repaired,
                                    usage: attempt.usage,
                                })));
                            }
                            Err(reason) => last = reason,
                        }
                    } else {
                        last = attempt.failure;
                    }
                }
                Err(abort) => {
                    last = abort.reason;
                    tally.records = abort.records;
                }
            }
        }

        if index + 1 >= models.len() {
            break;
        }
    }

    done(Outcome::Floor(Failure {
        reason: last,
        model: models
            .last()
            .cloned()
            .unwrap_or_else(|| config.model.clone()),
        invocations: tally.invocations,
        repaired: tally.repaired,
        records: tally.records,
    }))
}

/// The running count of what a run has spent, so the failure sentence and the
/// envelope cannot disagree about it.
#[derive(Default)]
struct Tally {
    invocations: usize,
    repaired: bool,
    records: usize,
}

/// The advisory auth check: `pi auth check --model <model> --json`.
///
/// Returns `Some(reason)` only when `pi` *says* it is not ready. Anything this
/// cannot read — an older pi without the subcommand, a help banner, a
/// credential helper that prints prose — is treated as "unknown, carry on",
/// because refusing the tier on the strength of an unparseable probe would be
/// the tool deciding it cannot ask a question it has not been refused.
fn auth_is_not_ready(config: &Config) -> Option<String> {
    let mut child = Command::new(Tool::Pi.name())
        .args(["auth", "check", "--model", &config.model, "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // The handle is taken before the wait, because `Child::wait` closes the
    // pipes: a probe that piped a banner nobody drains would otherwise wedge
    // on its own output.
    let raw = child.stdout.take().map(drain_all);
    // A probe that hangs is a probe that cannot answer. The check is a local
    // credentials read and the contract measured it at one to two seconds, so a
    // hung one is terminated and treated as unreadable — which the caller reads
    // as "unknown", and carries on past.
    let collected;
    let deadline = Instant::now() + AUTH_BUDGET;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                collected = raw.and_then(|raw| raw.recv_timeout(Duration::from_secs(2)).ok());
                break;
            }
            Ok(None) if Instant::now() >= deadline => {
                terminate(&mut child);
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return None,
        }
    }
    let parsed: Value = serde_json::from_str(collected?.trim()).ok()?;
    match parsed.get("status").and_then(Value::as_str) {
        Some("ready") => None,
        Some("not_ready") => {
            let provider = parsed
                .get("provider")
                .and_then(Value::as_str)
                .unwrap_or_else(|| config.model.split('/').next().unwrap_or(&config.model));
            let reason = parsed
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("no credentials configured");
            Some(format!(
                "pi reports no credentials for {provider} ({reason}); run `pi auth` to set one up"
            ))
        }
        _ => None,
    }
}

// ----------------------------------------------------------------- the request

/// One finished `pi` process: the answer text if it produced one, the failure
/// sentence if it did not, and the accounting either way.
struct Attempt {
    /// The final answer's text, or `None` when the run refused, hung short of
    /// one, or produced none — in which case `failure` says which.
    answer: Option<String>,
    failure: String,
    records: usize,
    usage: Option<Value>,
}

/// A run that could not be parsed at all — a hang, a kill, a spawn failure.
struct Abort {
    reason: String,
    records: usize,
}

/// Runs `pi` once and reads its stream under three budgets.
///
/// The child is spawned with stdout piped and drained by a reader thread that
/// sends one message per line: pi wedges if the pipe fills, so "collect it all
/// and look afterwards" is not available. The waiting side then enforces a
/// first-byte budget, a stall budget between records, and a total budget, and
/// `SIGTERM`s the child on whichever one runs out — the polite signal the
/// contract measured pi's own teardown to honour, with `SIGKILL` behind it for
/// the run that does not.
///
/// The budgets are the *backstop*, not the normal way a run ends. `agent_settled`
/// is the contract's "the true end of automatic work", so the read stops there
/// and whatever the process is doing afterwards is teardown this module
/// terminates rather than waits for: a pi that settles and then hangs — a
/// telemetry flush, a lock it cannot take — would otherwise hold the install
/// for the whole stall budget over a stream that is already complete.
fn invoke(
    subject: &Subject,
    config: &Config,
    model: &str,
    brief: &str,
    transcript: Option<&str>,
) -> Result<Attempt, Abort> {
    let mut command = Command::new(Tool::Pi.name());
    command
        .arg("--offline")
        .arg("-p")
        .arg("--mode")
        .arg("json")
        .arg("--no-session")
        .arg("--no-context-files")
        .arg("--no-approve");
    // The repair pass gets no tools at all: it is handed a transcript and asked
    // for JSON, and a model that can read the clone is a model that can decide
    // to go looking again.
    if transcript.is_none() {
        command.arg("--tools").arg("read,grep,find,ls");
    } else {
        command.arg("--no-tools");
    }
    command
        .arg("--model")
        .arg(model)
        .arg("--thinking")
        .arg("off")
        .arg("--")
        .arg(brief)
        .current_dir(subject.tree)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Err(Abort {
                reason: format!("cannot run pi: {error}"),
                records: 0,
            });
        }
    };
    let started = Instant::now();
    let lines = match child.stdout.take() {
        Some(stdout) => drain(stdout),
        None => {
            terminate(&mut child);
            return Err(Abort {
                reason: "pi produced no stdout to read".to_string(),
                records: 0,
            });
        }
    };
    // stderr is drained too, for the same reason: a chatty diagnostic filling an
    // unread pipe is a hang that looks like a hang.
    let stderr = child.stderr.take().map(drain_all);

    let mut records: Vec<String> = Vec::new();
    let mut usage: Option<Value> = None;
    let mut stopped: Option<String> = None;
    // Set by `agent_settled`, and the only reason to stop reading before the
    // pipe closes.
    let mut settled = false;
    loop {
        let budget = if records.is_empty() {
            config.first_byte.min(config.total)
        } else {
            config
                .stall
                .min(config.total.saturating_sub(started.elapsed()))
        };
        match lines.recv_timeout(budget) {
            Ok(Some(line)) => {
                if let Some(record) = StreamRecord::parse(&line) {
                    match record {
                        StreamRecord::Answer { stop, .. } => {
                            if stop == StopReason::Stop {
                                if records.len() < MAX_RECORDS {
                                    records.push(line);
                                }
                                // The last `stop` record's usage is the run's
                                // cost, which is the one the contract asks to be
                                // logged; a retry inside pi emits an earlier
                                // assistant message whose usage is not what the
                                // settled answer cost.
                                if let StreamRecord::Answer {
                                    usage: Some(used), ..
                                } = &record
                                {
                                    usage = Some(used.clone());
                                }
                            } else if stopped.is_none() {
                                // A refusal rides inside the stream: exit 0, a
                                // well-formed envelope, and an answer that says
                                // it failed. This is the line that decides it.
                                stopped = Some(record.failure_text());
                            }
                        }
                        StreamRecord::Settled => {
                            // The run is over by its own account. Everything
                            // after this is teardown, so the read stops here and
                            // the child is dealt with below.
                            settled = true;
                        }
                    }
                } else if records.len() < MAX_RECORDS {
                    records.push(line);
                }
                if settled {
                    break;
                }
            }
            Ok(None) => break,
            Err(RecvTimeoutError::Timeout) => {
                terminate(&mut child);
                return Err(Abort {
                    reason: if records.is_empty() {
                        format!(
                            "pi wrote nothing for {}s and was terminated; this provider is not \
                             answering",
                            config.first_byte.as_secs()
                        )
                    } else {
                        format!(
                            "pi stalled for {}s after {} records and was terminated",
                            config.stall.as_secs(),
                            records.len()
                        )
                    },
                    records: records.len(),
                });
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if started.elapsed() >= config.total {
            terminate(&mut child);
            return Err(Abort {
                reason: format!(
                    "pi ran past its {}s budget and was terminated",
                    config.total.as_secs()
                ),
                records: records.len(),
            });
        }
    }
    // A settled child is terminated rather than waited on. `terminate` polls
    // first, so a process that exited on its own is reaped and never signalled,
    // and one that lingers is stopped inside the SIGTERM/SIGKILL grace instead
    // of holding the install open. After the loop the stream is finished either
    // way, so the reap below turns the exit into a fact; both pipes were taken
    // before it, so the wait cannot deadlock on one.
    if settled {
        terminate(&mut child);
    }
    let exit = match child.wait() {
        Ok(status) => describe_exit(&status),
        Err(error) => format!("its status could not be read: {error}"),
    };
    let stderr = match stderr {
        Some(stderr) => stderr
            .recv_timeout(Duration::from_secs(2))
            .ok()
            .and_then(|text| first_line(text.as_bytes()))
            .unwrap_or_else(|| "nothing on stderr".to_string()),
        None => "stderr was not piped".to_string(),
    };

    let answer = match stopped {
        Some(_) => None,
        None => final_answer(&records),
    };
    let failure = stopped.unwrap_or_else(|| match &answer {
        Some(_) => String::new(),
        None if records.is_empty() => {
            format!("pi exited ({exit}) without a single JSONL record; stderr: {stderr}")
        }
        None => "pi's stream carried no assistant message_end with stopReason `stop`, so there \
                was no answer to read"
            .to_string(),
    });
    Ok(Attempt {
        answer,
        failure,
        records: records.len().min(MAX_RECORDS),
        usage,
    })
}

/// The final answer of a stream: the **last** assistant `message_end` whose
/// `stopReason` is `stop`, with its text blocks concatenated. Any later record
/// that is not an assistant `message_end` cannot unseat it, and an earlier one
/// cannot either — a retry inside pi emits a second assistant message, and the
/// one the run settled on is the last one.
fn final_answer(records: &[String]) -> Option<String> {
    let mut answer: Option<String> = None;
    for record in records {
        if let Some(StreamRecord::Answer { text, stop, .. }) = StreamRecord::parse(record)
            && stop == StopReason::Stop
        {
            answer = Some(text);
        }
    }
    answer
}

/// Terminates a child politely, then not so politely.
///
/// `SIGTERM` first because that is the signal pi's own cleanup is written for;
/// `SIGKILL` only if it has not gone by the grace period, so a run that ignores
/// the polite signal still cannot outlive the budget by much.
fn terminate(child: &mut Child) {
    let pid = child.id() as i32;
    // SAFETY: sending a signal to a pid this process owns, immediately after
    // having spawned it and before reaping it. A pid that has already exited
    // and been reaped is not ours to signal, and `kill` reports ESRCH rather
    // than doing anything.
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
    let deadline = Instant::now() + TERM_GRACE;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Reads a child's stdout one line at a time into a channel, so the waiter can
/// enforce budgets between records instead of between bytes. The reader thread
/// ends when the pipe closes, which is what a killed child causes.
fn drain<R: Read + Send + 'static>(stream: R) -> mpsc::Receiver<Option<String>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let reader = BufReader::new(stream);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            if sender.send(Some(line)).is_err() {
                break;
            }
        }
        let _ = sender.send(None);
    });
    receiver
}

/// Reads a stream to its end on a thread of its own, handing the text back
/// through a receiver the caller can wait on with a budget of its own. Used for
/// stderr and for the auth probe's one line of JSON.
fn drain_all<R: Read + Send + 'static>(mut stream: R) -> mpsc::Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stream.read_to_string(&mut text);
        let _ = sender.send(text);
    });
    receiver
}

// ------------------------------------------------------------------ the stream

/// Why a run stopped: the `stopReason` of the final assistant message, which is
/// the contract's definition of success.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StopReason {
    Stop,
    Error,
    Aborted,
    Other,
}

impl StopReason {
    fn parse(reason: Option<&str>) -> StopReason {
        match reason {
            Some("stop") => StopReason::Stop,
            Some("error") => StopReason::Error,
            Some("aborted") => StopReason::Aborted,
            _ => StopReason::Other,
        }
    }

    fn kind(self) -> &'static str {
        match self {
            StopReason::Stop => "stop",
            StopReason::Error => "error",
            StopReason::Aborted => "aborted",
            StopReason::Other => "other",
        }
    }
}

/// The one JSONL record type this parser cares about, plus the one it uses to
/// know the run is over.
#[derive(Clone, Debug, PartialEq)]
enum StreamRecord {
    Answer {
        text: String,
        stop: StopReason,
        error: Option<String>,
        usage: Option<Value>,
    },
    Settled,
}

impl StreamRecord {
    /// Parses one line. `None` means "not a record this parser uses" — a
    /// `message_update` delta, a `tool_execution_start`, or a line that is not
    /// JSON at all, which pi's forward-compatible stream is allowed to grow.
    fn parse(line: &str) -> Option<StreamRecord> {
        let value: Value = serde_json::from_str(line.trim()).ok()?;
        match value.get("type").and_then(Value::as_str)? {
            "agent_settled" => Some(StreamRecord::Settled),
            "message_end" => {
                let message = value.get("message")?;
                if message.get("role").and_then(Value::as_str) != Some("assistant") {
                    return None;
                }
                let mut text = String::new();
                if let Some(blocks) = message.get("content").and_then(Value::as_array) {
                    for block in blocks {
                        if block.get("type").and_then(Value::as_str) == Some("text")
                            && let Some(piece) = block.get("text").and_then(Value::as_str)
                        {
                            text.push_str(piece);
                        }
                    }
                }
                Some(StreamRecord::Answer {
                    text,
                    stop: StopReason::parse(message.get("stopReason").and_then(Value::as_str)),
                    error: message
                        .get("errorMessage")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    usage: message.get("usage").cloned(),
                })
            }
            _ => None,
        }
    }

    /// The sentence a refusal becomes: pi's own `errorMessage` when it gave
    /// one, which carries the provider's verbatim words (`403: {"type":
    /// "FreeTierError", …}`) and is the only place the real reason exists.
    fn failure_text(&self) -> String {
        match self {
            StreamRecord::Answer {
                stop, error, text, ..
            } => {
                let detail = error
                    .clone()
                    .filter(|error| !error.trim().is_empty())
                    .unwrap_or_else(|| {
                        if text.trim().is_empty() {
                            "no error message".to_string()
                        } else {
                            first_line(text.as_bytes())
                                .unwrap_or_else(|| "no error message".to_string())
                        }
                    });
                format!(
                    "pi's run ended with stopReason `{}`: {}",
                    stop.kind(),
                    detail
                )
            }
            StreamRecord::Settled => "pi's run ended without an answer".to_string(),
        }
    }
}

// -------------------------------------------------------------------- parsing

/// Turns an answer's text into a validated findings document.
///
/// Two steps, both of the contract's: the first balanced `{ … }` in the text
/// (models fence their JSON, prefix it with a sentence, or both), and then the
/// document itself. A failure here is a *repairable* failure, which is why it
/// returns a sentence rather than an error type — the caller decides whether to
/// spend the one repair pass.
fn extract(answer: &str) -> Result<Findings, String> {
    let object = first_object(answer)
        .ok_or_else(|| "the answer carried no JSON object at all, only prose".to_string())?;
    let findings: Findings = serde_json::from_str(object).map_err(|error| {
        format!("the answer's JSON object is not a `{SCHEMA}` findings document: {error}")
    })?;
    if !findings.schema.is_empty() && findings.schema != SCHEMA {
        return Err(format!(
            "the answer declares `schema = \"{}\"`, which is not `{SCHEMA}`; a stale shape is \
             refused rather than half-read",
            findings.schema
        ));
    }
    Ok(findings)
}

/// The first balanced `{ … }` in `text`, string-aware.
///
/// A JSON object contains braces inside strings — a quote of a QML block, a
/// regex, a path — so the scan tracks string state and escapes rather than
/// counting characters, and a fence the model wrapped the object in is simply
/// skipped over as prose.
fn first_object(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'{' {
            index += 1;
            continue;
        }
        let mut depth = 0usize;
        let mut quoted = false;
        let mut escaped = false;
        let mut cursor = index;
        while cursor < bytes.len() {
            let byte = bytes[cursor];
            if quoted {
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    quoted = false;
                }
            } else {
                match byte {
                    b'"' => quoted = true,
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(&text[index..=cursor]);
                        }
                    }
                    _ => {}
                }
            }
            cursor += 1;
        }
        // An unbalanced opening brace: nothing after it can be trusted either,
        // so the scan stops rather than restarting inside a broken object.
        return None;
    }
    None
}

// ------------------------------------------------------------------- the brief

/// The prompt, built from the engine's own survey of the tree.
///
/// It is the contract's brief with two things this pipeline can actually supply:
/// the parse findings, and the names the configs dispatch that nothing in the
/// tree answers for. A question the engine has already answered is not asked,
/// and a name the engine found dead is named, with the file and line it is on.
fn brief(subject: &Subject, survey: &Survey) -> String {
    let mut brief = String::new();
    brief.push_str(
        "You are researching an unknown desktop rice to make RiceSwap able to switch it. The \
         repo is already cloned at ",
    );
    brief.push_str(&subject.tree.display().to_string());
    brief.push_str(
        ". Only read files inside it (docs, config, sources); you cannot run commands or write \
         files.\n\nWhat the static parse of this tree could prove about the shell `",
    );
    brief.push_str(subject.shell);
    brief.push_str("`: ");
    match survey.skipped.as_deref() {
        Some(reason) => {
            brief.push_str("nothing — ");
            brief.push_str(reason);
            brief.push('.');
        }
        None => {
            brief.push_str(&format!(
                "it registers {} shortcut name(s) under appid `{}`, and the following \
                 dispatched names are answered by nothing in the tree",
                survey.registered,
                survey.appid.as_deref().unwrap_or(reconcile::DEFAULT_APPID)
            ));
            for dead in survey.dead.iter().take(MAX_BRIEF_NAMES) {
                brief.push_str(&format!(
                    "\n- `{}` at {}:{}",
                    dead.dispatched, dead.file, dead.line
                ));
            }
            if survey.dead.len() > MAX_BRIEF_NAMES {
                brief.push_str(&format!(
                    "\n- … and {} more dispatched names nothing in the tree answers for",
                    survey.dead.len() - MAX_BRIEF_NAMES
                ));
            }
            if !survey.uncertain.is_empty() {
                brief.push_str("\n\nOpen questions the parse could not answer:");
                for finding in survey.uncertain.iter().take(MAX_BRIEF_FINDINGS) {
                    brief.push_str("\n- ");
                    brief.push_str(finding);
                }
                if survey.uncertain.len() > MAX_BRIEF_FINDINGS {
                    brief.push_str(&format!(
                        "\n- … and {} more, all of them unresolvable by a static parse",
                        survey.uncertain.len() - MAX_BRIEF_FINDINGS
                    ));
                }
            }
        }
    }
    brief.push_str(
        "\n\nAnswer these specific questions from the repo's docs and config: 1. Which appid does \
         it register Hyprland GlobalShortcuts / layer-shell namespaces under? 2. Which env vars \
         pick config or wallpaper paths, and what values should be set? 3. Which keybind \
         namespace does it use ($mod, SUPER, …) and which dispatcher names does it register? 4. \
         Which packages does it need? 5. Which dispatched names above are the *same intent* as \
         one of its own shortcuts, and which are better dropped? A `to` target must be a name \
         this shell itself registers; never invent one.\n\nReturn ONLY one JSON object, no prose, \
         no code fences, matching exactly:\n",
    );
    brief.push_str(&schema_illustration());
    brief.push_str(
        "\nUse null / [] / \"\" rather than inventing values; put every gap in \"unknowns\". Every \
         `evidence` quote must be copied from a file in the tree, and `file` must be the path \
         relative to the root of the tree.",
    );
    brief
}

/// The schema, spelled out as a JSON example. Kept next to the brief because a
/// schema that is described in two places is a schema that drifts.
fn schema_illustration() -> String {
    json!({
        "schema": SCHEMA,
        "shell": subject_shell_placeholder(),
        "appid": "<GlobalShortcuts appid or null>",
        "env": { "<VAR>": "<value or ~-template>" },
        "dirs": [{ "path": "~/Wallpapers", "symlink_to": Value::Null }],
        "foreign": ["<ns:name belonging to another live shell>"],
        "resolutions": [
            { "dispatched": "quickshell:regionScreenshot", "to": "<a name this shell registers>" },
            { "dispatched": "quickshell:barToggle", "drop": true },
            { "dispatched": "quickshell:screenshotSave", "exec": "grim -g \"$HOME/pics\" - | wl-copy" }
        ],
        "packages": { "official": ["<package>"], "aur": [] },
        "dispatcher_namespaces": { "<name>": "<ns>" },
        "start_cmd": ["<argv>"],
        "stop_cmd": ["<argv>"],
        "keybind_namespace": "<$mod or SUPER>",
        "wallpaper_dir": "<path or null>",
        "capabilities": ["<what this shell can do, in its own words>"],
        "confidence": 0.0,
        "evidence": [{ "file": "<tree-relative path>", "quote": "<short excerpt>" }],
        "unknowns": ["<question you could not answer>"]
    })
    .to_string()
}

fn subject_shell_placeholder() -> &'static str {
    "<hyprland|quickshell|waybar|other>"
}

/// The repair prompt: the same contract, with the previous answer in hand and no
/// tools to go looking with.
fn repair(transcript: &str) -> String {
    let mut prompt = String::from(
        "Extract the JSON object from the text below and return only that object, with no prose \
         and no code fences. It must match this shape exactly:\n",
    );
    prompt.push_str(&schema_illustration());
    prompt.push_str("\n\nText to extract from:\n");
    prompt.push_str(&clip(transcript, MAX_TRANSCRIPT));
    prompt
}

/// Clips a string to `limit` characters, marking that it was clipped rather than
/// pretending the text ended there.
fn clip(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let head: String = text.chars().take(limit).collect();
    format!("{head}\n[… {limit} characters of the original omitted …]")
}

// ---------------------------------------------------------------- the write-back

/// What happened to the recipe the run produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteBack {
    /// Written to this path, and proven to parse.
    Written(PathBuf),
    /// A recipe was already there and was kept. Research never overwrites one:
    /// a user who has edited `recipes/<shell>.toml` is the authority on that
    /// file, and an install that silently replaced it would be a worse bug than
    /// a stale codebook.
    ///
    /// The trigger already skips a shell whose recipe is on disk, so reaching
    /// this arm means the file appeared between the trigger's `is_file` and this
    /// `create_new` — a race, and the one case where the guarantee has to be
    /// enforced rather than assumed.
    Kept(PathBuf),
    /// The path the write would have used, with the reason nothing was written:
    /// an answer the schema cannot hold, or a document that does not survive a
    /// round trip.
    Declined(PathBuf, String),
}

impl WriteBack {
    /// The recipe envelope.
    pub fn payload(&self) -> Value {
        match self {
            WriteBack::Written(path) => json!({
                "written": true,
                "path": path.display().to_string(),
                "kept_existing": false,
            }),
            WriteBack::Kept(path) => json!({
                "written": false,
                "path": path.display().to_string(),
                "kept_existing": true,
                "note": "a recipe was already there; research never overwrites one — delete it to \
                         research this shell again",
            }),
            WriteBack::Declined(path, reason) => json!({
                "written": false,
                "path": path.display().to_string(),
                "kept_existing": false,
                "note": reason,
            }),
        }
    }

    /// Whether a recipe exists on disk because of this run.
    pub fn written(&self) -> bool {
        matches!(self, WriteBack::Written(_))
    }

    /// The file this run used, whatever it decided about it — named in every
    /// message, so a user always knows which file to look at or delete.
    pub fn path(&self) -> &Path {
        match self {
            WriteBack::Written(path) | WriteBack::Kept(path) | WriteBack::Declined(path, _) => path,
        }
    }
}

/// The user tier's file for `shell`: `<data-dir>/recipes/<shell>.toml`.
pub fn recipe_path(data: &Path, shell: &str) -> PathBuf {
    data.join(recipe::RECIPES_DIR).join(format!("{shell}.toml"))
}

/// Turns a validated answer into a recipe document, and says what it had to
/// leave out.
///
/// The filtering here is the tier's only power, so it is the place the checks
/// belong: an env variable that is not a variable name, a path that does not
/// land under this pass's `$HOME`, a resolution that names two of the three
/// sides of the trilemma. Each of those is dropped and named — the alternative,
/// writing them and letting a later pass refuse them, would leave a file whose
/// own reader has to decide what it meant.
pub fn document(subject: &Subject, findings: &Findings) -> (Recipe, Vec<String>) {
    let mut notes = Vec::new();
    let mut env = BTreeMap::new();
    for (name, value) in &findings.env {
        if !identifier(name) {
            notes.push(format!(
                "`{name}` is not an environment variable name, so it was not written into the \
                 recipe"
            ));
            continue;
        }
        if value.trim().is_empty() {
            notes.push(format!(
                "`{name}` was answered with an empty value, which is not a value; put the gap in \
                 `unknowns` instead"
            ));
            continue;
        }
        env.insert(name.clone(), value.clone());
    }

    let mut dirs = Vec::new();
    for dir in &findings.dirs {
        match recipe::under_home(&dir.path, subject.home) {
            Some(_) => {}
            None => {
                notes.push(format!(
                    "`{}` does not land under this pass's `$HOME`, and a recipe may only name \
                     paths inside it; not written",
                    dir.path
                ));
                continue;
            }
        }
        let symlink_to = match dir.symlink_to.as_deref() {
            None => None,
            Some(target) => match recipe::under_home(target, subject.home) {
                Some(_) => Some(target.to_string()),
                None => {
                    notes.push(format!(
                        "`{}` would have symlinked to `{target}`, which is outside this pass's \
                         `$HOME`; the entry was written as a plain directory guarantee instead",
                        dir.path
                    ));
                    None
                }
            },
        };
        dirs.push(recipe::Dir {
            path: dir.path.clone(),
            symlink_to,
        });
    }

    let mut resolutions = Vec::new();
    for declared in &findings.resolutions {
        match declared.decide() {
            Ok(_) => resolutions.push(declared.clone()),
            Err(reason) => notes.push(format!("a resolution was not written: {reason}")),
        }
    }

    let mut foreign: BTreeSet<String> = BTreeSet::new();
    for entry in &findings.foreign {
        let entry = entry.trim();
        if entry.is_empty() || !entry.contains(':') {
            notes.push(format!(
                "`{entry}` was offered as another live shell's namespace but is not an \
                 `appid:name`, so it was not written"
            ));
            continue;
        }
        foreign.insert(entry.to_string());
    }

    let recipe = Recipe {
        schema_version: Some(recipe::SCHEMA_VERSION),
        shell: Some(recipe::Shell {
            name: Some(subject.shell.to_string()),
            appid: findings
                .appid
                .clone()
                .filter(|appid| !appid.trim().is_empty()),
        }),
        env,
        dirs,
        resolutions,
        foreign: recipe::Foreign {
            entries: foreign.into_iter().collect(),
        },
        // A research run derives facts, and an IPC probe is a *declaration* —
        // it is somebody's claim about a command that shell answers. Nothing
        // here establishes that claim, so the write-back declares none and a
        // profile that wants one has to say so in its own `adapt.toml`.
        ipc: None,
        // And no probe script, for a stronger version of the same reason: a
        // probe drives *this* session with synthetic input and screenshots and
        // asserts on what it sees. A research run is a `pi` invocation in a
        // directory; it has no session, and a probe it invented would be a
        // test written for a machine nobody ran it on.
        probes: Vec::new(),
    };
    (recipe, notes)
}

/// Writes `document` to the user tier, or explains why it did not.
///
/// Three rules, in this order:
///
/// * **Never overwrite.** The file is created with `create_new`, so an existing
///   recipe is a fact the filesystem reports rather than a window this code
///   could race. A file that is already there is kept, and the run says so.
/// * **Round-trip before writing.** The rendered TOML is parsed back with the
///   same [`Recipe`] the adapt pass will use. A document that renders but does
///   not parse is a file no pass could read, and it must never reach the disk.
/// * **Round-trip after writing.** The bytes on disk are read back and parsed
///   again; a failure there removes the file, so what is left behind is either
///   a recipe that parses or nothing at all.
pub fn write_back(subject: &Subject, findings: &Findings) -> (WriteBack, Vec<String>) {
    let (document, notes) = document(subject, findings);
    if findings.is_empty() {
        let path = recipe_path(subject.data, subject.shell);
        return (
            WriteBack::Declined(
                path,
                "the answer carried no fact the recipe schema can hold (no env, no dirs, no \
                 resolutions, no appid), so nothing was written; its gaps are in `unknowns`"
                    .to_string(),
            ),
            notes,
        );
    }
    match write_document(subject.data, subject.shell, &document) {
        Ok(outcome) => (outcome, notes),
        Err(reason) => {
            let path = recipe_path(subject.data, subject.shell);
            (WriteBack::Declined(path, reason), notes)
        }
    }
}

/// Renders, proves, and creates the recipe file.
fn write_document(data: &Path, shell: &str, document: &Recipe) -> Result<WriteBack, String> {
    let path = recipe_path(data, shell);
    let rendered = render(document);
    // The proof that matters: what we are about to write is a document the pass
    // that will read it can parse. A `Recipe` that only serializes is a recipe
    // that is silently ignored, and silence is the one outcome worse than a
    // refusal.
    toml::from_str::<Recipe>(&rendered).map_err(|error| {
        format!("the recipe did not survive a round trip, so nothing was written: {error}")
    })?;

    let directory = path.parent().expect("the recipe path has a parent");
    std::fs::create_dir_all(directory).map_err(|error| {
        format!(
            "cannot open the recipes directory {}: {error}",
            directory.display()
        )
    })?;
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Ok(WriteBack::Kept(path));
        }
        Err(error) => {
            return Err(format!("cannot write {}: {error}", path.display()));
        }
    };
    use std::io::Write;
    if let Err(error) = file.write_all(rendered.as_bytes()) {
        let _ = std::fs::remove_file(&path);
        return Err(format!("cannot write {}: {error}", path.display()));
    }
    drop(file);

    // The second proof, on the bytes that are actually there.
    match std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read back {}: {error}", path.display()))
        .and_then(|raw| {
            toml::from_str::<Recipe>(&raw).map_err(|error| {
                format!(
                    "{} does not parse after being written: {error}",
                    path.display()
                )
            })
        }) {
        Ok(_) => Ok(WriteBack::Written(path)),
        Err(reason) => {
            let _ = std::fs::remove_file(&path);
            Err(format!(
                "{reason}; the file was removed rather than left unreadable"
            ))
        }
    }
}

/// The file's text: a header that says where it came from, then the document.
fn render(document: &Recipe) -> String {
    let body = toml::to_string_pretty(document)
        .unwrap_or_else(|error| panic!("a recipe document always renders: {error}"));
    format!(
        "# RiceSwap recipe — written by the research tier (issue #38), from a `pi` run against \
         this shell's own tree.\n\
         #\n\
         # This is the user tier: it is read as authority over nothing but itself, and it is \
         never overwritten. To have it researched again, delete this file and re-install. To \
         correct it by hand, edit it — the human tier (`adapt.toml`) outranks it, and a recipe \
         the runtime cannot read is reported rather than half-obeyed.\n\
         #\n\
         # The answer it was written from reported: {}{}.\n\n{body}",
        document
            .shell
            .as_ref()
            .and_then(|shell| shell.name.clone())
            .unwrap_or_default(),
        document
            .shell
            .as_ref()
            .and_then(|shell| shell.appid.clone())
            .map(|appid| format!(" (appid `{appid}`)"))
            .unwrap_or_default(),
    )
}

// ------------------------------------------------------------------- packages

/// Merges the packages an answer named into the manifest's own scan.
///
/// The names are filtered hard — letters, digits and the four characters a
/// package name is allowed to carry — because these strings end up inside
/// `pacman` transactions, and a value that came out of a model's answer is
/// exactly the kind of string that must be checked before it is a command
/// argument. Anything filtered out is returned rather than dropped in silence,
/// and every returned entry says what happened to it, so the install's warning
/// reads the same for a package it merged and one it refused.
pub fn merge_packages(scan: &mut PackageScan, findings: &Findings) -> Vec<String> {
    let mut added = Vec::new();
    for (names, package) in [
        (&mut scan.official, &findings.packages.official),
        (&mut scan.aur, &findings.packages.aur),
    ] {
        for candidate in package {
            if !is_package_name(candidate) {
                added.push(format!("{candidate} (rejected: not a package name)"));
                continue;
            }
            let package = candidate.trim().to_string();
            if !names.contains(&package) {
                names.push(package.clone());
                added.push(format!("{package} (merged into the profile's manifest)"));
            }
        }
        names.sort();
        names.dedup();
    }
    added
}

/// A package name: no whitespace, no shell metacharacter, nothing that could
/// turn a transaction argument into a second argument.
fn is_package_name(candidate: &str) -> bool {
    !candidate.is_empty()
        && candidate.len() <= 128
        && candidate
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '+' | '-'))
}

/// An environment-variable name, by the same rule the recipe schema's own bind
/// options are held to.
fn identifier(text: &str) -> bool {
    !text.is_empty()
        && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !text.starts_with(|c: char| c.is_ascii_digit())
}

// ------------------------------------------------------------------ utilities

/// First non-empty line, trimmed and capped — the diagnostics discipline the
/// rest of the tool uses.
fn first_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(200).collect())
}

/// How a child ended, in the words a user can act on.
///
/// pi's own documented exits are worth reading rather than paraphrasing: a
/// startup failure is exit 1 with an empty stdout and `Error: Model … not
/// found` on stderr, which is a configuration mistake and not a provider
/// refusal, and the two deserve different advice. A nonzero exit is named as
/// such and the stderr line rides along; SIGTERM is called out because that is
/// what this module does to a hung run.
fn describe_exit(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(0) => "exit 0".to_string(),
        Some(code) => format!("exit {code}"),
        None => "it was killed by a signal".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn findings(raw: &str) -> Findings {
        extract(raw).expect("the document parses")
    }

    /// A complete findings document, in the shape the brief asks for.
    const ANSWER: &str = r#"{
        "schema": "riceswap.research.v1",
        "shell": "demo",
        "appid": "demo",
        "env": { "DEMO_WALLPAPERS": "~/Pictures/Wallpapers" },
        "dirs": [ { "path": "~/Pictures/Wallpapers" } ],
        "foreign": [ "quickshell:riceswap-toggle" ],
        "resolutions": [ { "dispatched": "quickshell:regionScreenshot", "to": "screenshotClip" } ],
        "packages": { "official": [ "quickshell" ], "aur": [] },
        "confidence": 0.8,
        "evidence": [ { "file": "README.md", "quote": "needs quickshell" } ],
        "unknowns": [ "where the launcher keeps its config" ]
    }"#;

    fn subject() -> Subject<'static> {
        Subject {
            home: Path::new("/home/tester"),
            data: Path::new("/data"),
            profile: Path::new("/data/profiles/demo"),
            tree: Path::new("/data/sources/demo"),
            shell: "demo",
        }
    }

    #[test]
    fn the_first_balanced_object_is_found_through_prose_fences_and_inner_braces() {
        let answer =
            format!("Here is what I found.\n```json\n{ANSWER}\n```\nThe gaps are in unknowns.");
        let parsed = findings(&answer);
        assert_eq!(parsed.schema, SCHEMA);
        assert_eq!(parsed.confidence, 0.8);

        // A brace inside a string is not a nesting level: without string
        // awareness the first `}` inside a quote would end the object early.
        let nested = r#"{"schema":"riceswap.research.v1","evidence":[{"file":"a","quote":"if (x) { y() }"}]}"#;
        assert_eq!(
            findings(nested).evidence[0].quote,
            "if (x) { y() }",
            "a brace inside a string must not end the object"
        );
    }

    #[test]
    fn prose_with_no_object_is_a_repairable_failure_not_a_parse() {
        let error = extract("I could not find anything about this rice.").unwrap_err();
        assert!(error.contains("no JSON object"), "{error}");
    }

    #[test]
    fn a_stale_schema_is_refused_rather_than_half_read() {
        let stale = r#"{"schema":"riceswap.research.v0","env":{"A":"b"}}"#;
        let error = extract(stale).unwrap_err();
        assert!(error.contains("riceswap.research.v0"), "{error}");
    }

    #[test]
    fn only_the_last_assistant_message_that_stopped_is_the_answer() {
        let first = json!({
            "type": "message_end",
            "message": { "role": "assistant", "stopReason": "stop",
                         "content": [{ "type": "text", "text": "{\"first\":true}" }] }
        });
        let second = json!({
            "type": "message_end",
            "message": { "role": "assistant", "stopReason": "stop",
                         "content": [{ "type": "text", "text": ANSWER }] }
        });
        let records = vec![
            first.to_string(),
            json!({ "type": "message_update", "usage": { "input": 1 } }).to_string(),
            second.to_string(),
            json!({ "type": "agent_settled" }).to_string(),
        ];
        let answer = final_answer(&records).expect("there is an answer");
        assert_eq!(extract(&answer).unwrap().appid.as_deref(), Some("demo"));
    }

    #[test]
    fn exit_zero_with_an_error_in_the_stream_is_a_failure_carrying_the_provider_words() {
        // The contract's own smoke run, byte for its shape: a well-formed
        // stream, exit 0, and a refusal at the end of it.
        let stream = json!({
            "type": "message_end",
            "message": {
                "role": "assistant",
                "stopReason": "error",
                "errorMessage": "403: {\"type\":\"FreeTierError\"}",
                "content": []
            }
        });
        let record = StreamRecord::parse(&stream.to_string()).expect("an assistant message_end");
        assert!(matches!(
            record,
            StreamRecord::Answer {
                stop: StopReason::Error,
                ..
            }
        ));
        let reason = record.failure_text();
        assert!(reason.contains("stopReason `error`"), "{reason}");
        assert!(reason.contains("FreeTierError"), "{reason}");
    }

    #[test]
    fn a_message_end_that_is_not_an_assistant_one_is_not_the_answer() {
        let line = json!({
            "type": "message_end",
            "message": { "role": "user", "stopReason": "stop",
                         "content": [{ "type": "text", "text": "the brief" }] }
        });
        assert!(StreamRecord::parse(&line.to_string()).is_none());
    }

    #[test]
    fn a_line_this_parser_does_not_use_is_skipped_rather_than_fatal() {
        assert!(StreamRecord::parse("this is not a record").is_none());
        assert!(StreamRecord::parse(r#"{"type":"tool_execution_start"}"#).is_none());
    }

    #[test]
    fn an_answered_document_becomes_a_recipe_that_renders_and_parses_back() {
        let (document, notes) = document(&subject(), &findings(ANSWER));
        assert!(
            notes.is_empty(),
            "an honest answer needs no notes: {notes:?}"
        );
        assert_eq!(document.schema_version, Some(recipe::SCHEMA_VERSION));
        assert_eq!(
            document.shell.as_ref().unwrap().name.as_deref(),
            Some("demo")
        );
        assert_eq!(
            document.shell.as_ref().unwrap().appid.as_deref(),
            Some("demo")
        );
        assert_eq!(
            document.env.get("DEMO_WALLPAPERS").map(String::as_str),
            Some("~/Pictures/Wallpapers")
        );
        assert_eq!(document.dirs.len(), 1);
        assert_eq!(document.resolutions.len(), 1);
        assert_eq!(document.foreign.entries, vec!["quickshell:riceswap-toggle"]);

        // The proof the write-back also makes: what this renders is a document
        // the pass that reads recipes can parse back.
        let rendered = toml::to_string_pretty(&document).expect("a recipe renders");
        let round: recipe::Recipe = toml::from_str(&rendered).expect("and parses back");
        assert_eq!(round.resolutions.len(), 1);
    }

    #[test]
    fn a_path_outside_home_or_a_variable_that_is_not_one_is_lost_exactly() {
        let raw = r#"{
            "schema": "riceswap.research.v1",
            "env": { "GOOD_ONE": "~/x", "not a name": "~/y", "EMPTY": "" },
            "dirs": [ { "path": "~/ok" }, { "path": "/etc" }, { "path": "~/../etc" } ],
            "resolutions": [ { "dispatched": "a", "to": "b", "drop": true } ]
        }"#;
        let (document, notes) = document(&subject(), &findings(raw));
        assert_eq!(document.env.keys().collect::<Vec<_>>(), vec!["GOOD_ONE"]);
        assert_eq!(document.dirs.len(), 1, "{:?}", document.dirs);
        assert_eq!(document.dirs[0].path, "~/ok");
        assert!(
            document.resolutions.is_empty(),
            "a resolution naming two sides is dropped, not obeyed"
        );
        // One note per thing left out, so a user can act on every one: two env
        // entries, two paths, one resolution.
        assert_eq!(notes.len(), 5, "{notes:?}");
    }

    #[test]
    fn a_package_name_that_could_become_a_second_argument_never_reaches_the_manifest() {
        let mut scan = PackageScan {
            official: vec!["waybar".to_string()],
            aur: Vec::new(),
        };
        let raw = r#"{
            "schema": "riceswap.research.v1",
            "appid": "demo",
            "packages": { "official": [ "quickshell", "waybar", "evil; rm -rf /", "" ], "aur": [] }
        }"#;
        let reported = merge_packages(&mut scan, &findings(raw));
        assert_eq!(scan.official, vec!["quickshell", "waybar"]);
        assert_eq!(
            reported,
            vec![
                "quickshell (merged into the profile's manifest)",
                "evil; rm -rf / (rejected: not a package name)",
                " (rejected: not a package name)",
            ],
            "each reported entry says what became of it, refused ones included"
        );
    }

    #[test]
    fn an_answer_of_nothing_but_nulls_carries_no_recipe() {
        let raw = r#"{
            "schema": "riceswap.research.v1",
            "shell": "demo", "appid": null, "env": {}, "dirs": [],
            "resolutions": [], "foreign": [], "packages": { "official": [], "aur": [] },
            "confidence": 0.0, "evidence": [], "unknowns": ["everything"]
        }"#;
        assert!(findings(raw).is_empty());
    }

    #[test]
    fn the_retry_chain_deduplicates_a_model_that_is_already_the_fallback() {
        let budget = Config {
            model: FALLBACK_MODEL.to_string(),
            first_byte: Duration::from_secs(1),
            stall: Duration::from_secs(1),
            total: Duration::from_secs(1),
            attempts: 2,
        };
        assert_eq!(budget.chain(), vec![FALLBACK_MODEL.to_string()]);

        let pinned = Config {
            model: "kios/grok-4.6-free".to_string(),
            ..budget.clone()
        };
        assert_eq!(pinned.chain(), vec!["kios/grok-4.6-free", FALLBACK_MODEL]);

        let single = Config {
            attempts: 1,
            ..pinned
        };
        assert_eq!(single.chain(), vec!["kios/grok-4.6-free"]);
    }

    #[test]
    fn the_repair_prompt_asks_for_json_alone_and_says_when_it_clipped() {
        let prompt = repair(&format!("{ANSWER} {}", "x".repeat(MAX_TRANSCRIPT)));
        assert!(prompt.contains("no code fences"), "{prompt}");
        assert!(
            prompt.contains("characters of the original omitted"),
            "a transcript over the ceiling says it was clipped, rather than ending as if it had"
        );
        assert!(
            prompt.len() < MAX_TRANSCRIPT + 4_000,
            "the prompt stays bounded"
        );
    }

    /// The trigger's middle arm, which no install can reach: a fresh install's
    /// profile does not exist yet, so the only way a profile holds an
    /// `adapt.toml` at research time is a half-finished install being resumed
    /// with one already there. The human tier outranks the built-in, the user
    /// tier and every auto-move (#28), so a model must not be asked over it.
    #[test]
    fn a_profile_that_already_has_an_adapt_toml_is_not_researched() {
        let root = tempfile::tempdir().expect("a temp dir");
        let profile = root.path().join("profiles").join("demo");
        std::fs::create_dir_all(&profile).expect("make the profile");
        std::fs::write(profile.join(recipe::ADAPT_FILE), "schema_version = 1\n")
            .expect("write an adapt.toml");
        let tree = root.path().join("tree");
        std::fs::create_dir_all(tree.join(".config/quickshell/demo")).expect("a shell");
        std::fs::write(
            tree.join(".config/quickshell/demo/shell.qml"),
            "ShellRoot {}\n",
        )
        .expect("the marker");

        let subject = Subject {
            home: root.path(),
            data: root.path(),
            profile: &profile,
            tree: &tree,
            shell: "demo",
        };
        let run = run(&subject);
        assert!(
            matches!(run.outcome, Outcome::Skipped(Skip::HumanTier { .. })),
            "{:?}",
            run.outcome
        );
        // And the run is over before any probe: nothing was executed, and the
        // file is exactly where the user left it.
        assert_eq!(
            std::fs::read_to_string(profile.join(recipe::ADAPT_FILE)).expect("read it back"),
            "schema_version = 1\n"
        );
        assert_eq!(run.payload()["invocations"], json!(0));
    }

    /// The skip that makes "the same shell is never researched twice" true.
    ///
    /// Only the skip is exercised here, and that is deliberate: a subject whose
    /// trigger *is* met carries on to the `pi` probe, which spawns whatever `pi`
    /// is on `PATH` — in a unit test that is the host's real one, not the stub.
    /// The other half (no recipe on disk means the tier does ask) is proved in
    /// `tests/research.rs`, where `PATH` is the sandbox.
    #[test]
    fn a_user_tier_recipe_on_disk_is_a_previous_runs_answer_and_a_reason_not_to_ask() {
        let root = tempfile::tempdir().expect("a temp dir");
        let data = root.path();
        let tree = root.path().join("tree");
        let qml = tree.join(".config/quickshell/demo");
        std::fs::create_dir_all(&qml).expect("a shell");
        std::fs::write(qml.join("shell.qml"), "ShellRoot {}\n").expect("the marker");
        // A registration built at runtime, so the parse is uncertain and the
        // confidence arm of the trigger would otherwise say yes.
        std::fs::write(
            qml.join("Launchers.qml"),
            "Scope {\n    GlobalShortcut {\n        name: built\n    }\n}\n",
        )
        .expect("a dynamic registration");

        let profile = root.path().join("profiles").join("demo");
        std::fs::create_dir_all(&profile).expect("the profile");
        let path = recipe_path(data, "demo");
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("the recipes dir");
        std::fs::write(&path, "schema_version = 1\n").expect("a previous answer");

        let run = run(&Subject {
            home: root.path(),
            data,
            profile: &profile,
            tree: &tree,
            shell: "demo",
        });
        assert!(
            matches!(run.outcome, Outcome::Skipped(Skip::UserTier { .. })),
            "{:?}",
            run.outcome
        );
        assert_eq!(run.payload()["invocations"], json!(0));
        assert_eq!(
            run.payload()["skipped"],
            json!(
                "a user-tier recipe for this shell already exists; delete it to research it \
                  again"
            )
        );
        // The skip reads the file and never writes it: that is the other half of
        // the same promise, and it is the half a stale answer depends on.
        assert_eq!(
            std::fs::read_to_string(&path).expect("read it back"),
            "schema_version = 1\n"
        );
    }

    #[test]
    fn a_written_recipe_is_never_overwritten_and_the_second_run_says_so() {
        let root = tempfile::tempdir().expect("a temp dir");
        let data = root.path();
        let (document, _) = document(&subject(), &findings(ANSWER));

        let first = write_document(data, "demo", &document).expect("the first write lands");
        assert!(first.written());
        let before = std::fs::read_to_string(recipe_path(data, "demo")).expect("read it back");

        let second = write_document(data, "demo", &document).expect("the second is not a failure");
        assert!(matches!(second, WriteBack::Kept(_)));
        assert_eq!(
            std::fs::read_to_string(recipe_path(data, "demo")).expect("still there"),
            before,
            "a research run never rewrites a recipe that is already there"
        );
    }
}
