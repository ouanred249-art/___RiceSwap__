//! The ten operations.
//!
//! The profile-store operations are real: `list` and `info` read manifests
//! through the store, and `switch` verifies its target then flips the `current`
//! symlink — the activation primitive later tickets build on. The rest are
//! honest stubs: they stream their real progress lines, probe the external
//! tools their real implementation will need, and return the real envelope
//! with placeholder data marked `"stub": true`. Snapshot pipelines, switch
//! sequences, and package operations arrive in later tickets.

use crate::cli::Invocation;
use crate::envelope::{Emitter, Envelope};
use crate::profile::Store;
use crate::state::StateStore;
use crate::tools::{self, Tool};
use serde_json::{Value, json};
use std::path::PathBuf;

/// The tools a package-touching operation depends on.
const PACKAGE_MANAGERS: &[Tool] = &[Tool::Pacman, Tool::Yay, Tool::Paru];

/// The tools the wallpapers layer is keyed to.
const WALLPAPER_TOOLS: &[Tool] = &[Tool::Hyprctl, Tool::Grim];

/// Everything an operation needs from its environment.
pub struct Context {
    store: Store,
    state: StateStore,
}

impl Context {
    /// Locates `$HOME`, opens the profile store, and loads the state document.
    fn new() -> Result<Context, Envelope> {
        let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) else {
            return Err(Envelope::failed(
                "cannot locate $HOME; RiceSwap needs it to find its state directory",
            ));
        };
        let store = Store::new(PathBuf::from(home));
        Ok(Context {
            state: StateStore::load(store.data_dir().join("state.json")),
            store,
        })
    }

    /// Emits a progress line and mirrors the step into `state.json`, so a panel
    /// reopened mid-operation still renders where the backend is.
    fn progress(&mut self, emitter: &mut Emitter, message: &str) {
        emitter.progress(message);
        self.state.set_step(emitter.step());
        self.state.write();
    }

    /// Mirrors whatever the `current` symlink names into `state.json`, so the
    /// active-profile badge always agrees with the filesystem.
    fn sync_active(&mut self) {
        let active = self.store.active();
        self.state.set_active_profile(active.name);
    }
}

/// Runs one parsed invocation, emitting exactly one envelope.
pub fn run(invocation: Invocation) {
    let mut context = match Context::new() {
        Ok(context) => context,
        Err(envelope) => return envelope.emit(),
    };
    context.state.begin(invocation.name(), invocation.target());
    context.sync_active();
    context.state.write();

    let envelope = dispatch(&invocation, &mut context);
    // Re-read after dispatch: `switch` flips `current`, and the document must
    // agree with the symlink it just wrote.
    context.sync_active();
    context.state.finish(envelope.ok, envelope.warnings.clone());
    context.state.write();
    envelope.emit();
}

fn dispatch(invocation: &Invocation, context: &mut Context) -> Envelope {
    match invocation {
        Invocation::Detect => detect(context),
        Invocation::Snapshot { name } => snapshot(context, name),
        Invocation::Plan { target } => plan(context, target),
        Invocation::Switch { target } => switch(context, target),
        Invocation::List => list(context),
        Invocation::Info { name } => info(context, name),
        Invocation::Delete { name, force } => delete(context, name, *force),
        Invocation::Diff { a, b } => diff(context, a, b),
        Invocation::WallpaperImport { path } => wallpaper_import(context, path),
        Invocation::Init => init(context),
    }
}

/// Attaches a probe of `needed` to `envelope`, plus a warning per unusable
/// tool. Every operation that will shell out reports the tools it depends on,
/// so a degraded machine is visible before any real work starts.
fn with_tools(envelope: &mut Envelope, needed: &[Tool]) {
    let report = tools::probe_all(needed);
    for (name, status) in &report {
        if !status.succeeded() {
            envelope
                .warnings
                .push(format!("{name} is not usable: {}", describe(status)));
        }
    }
    envelope.data["tools"] = json!(report);
}

/// The pre-flight scan behind `snapshot`: which config dirs, packages, and
/// assets are candidates, and which external tools are even present.
fn detect(context: &mut Context) -> Envelope {
    let mut emitter = Emitter::new("detect");
    context.progress(&mut emitter, "scanning candidate config directories");
    context.progress(&mut emitter, "scanning installed packages");
    context.progress(&mut emitter, "scanning wallpaper assets");

    let mut envelope = Envelope::ok(json!({
        "stub": true,
        "config_dirs": [],
        "packages": [],
        "assets": [],
    }));
    with_tools(&mut envelope, Tool::ALL);
    envelope
}

/// The pre-flight diff behind `switch`: what would change, and what is blocked.
fn plan(context: &mut Context, target: &str) -> Envelope {
    let mut emitter = Emitter::new("plan");
    context.progress(&mut emitter, &format!("reading profile `{target}`"));
    context.progress(&mut emitter, "computing package diff");
    context.progress(&mut emitter, "checking managed paths for conflicts");

    let mut envelope = Envelope::ok(json!({
        "stub": true,
        "target": target,
        "package_diff": [],
        "service_changes": [],
        "blocked_paths": [],
    }));
    with_tools(&mut envelope, PACKAGE_MANAGERS);
    envelope
}

/// Writes a profile from confirmed selections.
fn snapshot(context: &mut Context, name: &str) -> Envelope {
    let mut emitter = Emitter::new("snapshot");
    context.progress(
        &mut emitter,
        &format!("creating profile directory for `{name}`"),
    );
    context.progress(&mut emitter, "collecting checked config paths");
    context.progress(&mut emitter, "collecting checked packages");

    let mut envelope = Envelope::ok(json!({
        "stub": true,
        "profile": name,
        "manifest_written": false,
        "checked_paths": [],
        "checked_packages": [],
    }));
    with_tools(&mut envelope, PACKAGE_MANAGERS);
    envelope
}

/// The switch sequence's first real step: verify the target exists, then flip
/// the `current` symlink. The package, config, and service steps between them
/// remain stubs until their tickets land.
fn switch(context: &mut Context, target: &str) -> Envelope {
    let mut emitter = Emitter::new("switch");
    context.progress(&mut emitter, &format!("verifying profile `{target}`"));
    if let Err(error) = context.store.load(target) {
        return Envelope::failed(error);
    }
    context.progress(&mut emitter, "installing missing packages");
    context.progress(&mut emitter, "linking managed config paths");
    context.progress(&mut emitter, "applying services and wallpaper");
    context.progress(&mut emitter, &format!("activating profile `{target}`"));
    if let Err(error) = context.store.flip(target) {
        return Envelope::failed(error);
    }

    let mut envelope = Envelope::ok(json!({
        "stub": true,
        "target": target,
        "completed_steps": 0,
        "resume_hint": format!("switch to `{target}` to restore"),
    }));
    with_tools(&mut envelope, Tool::ALL);
    envelope
}

/// Every profile in the store, with manifest data, plus whichever profile the
/// `current` symlink names. A broken manifest is a warning, never a blank list.
fn list(context: &mut Context) -> Envelope {
    let mut emitter = Emitter::new("list");
    context.progress(&mut emitter, "reading the profile store");

    let (profiles, mut warnings) = context.store.list();
    let active = context.store.active();
    let profiles: Vec<Value> = profiles
        .into_iter()
        .map(|listed| {
            json!({
                "name": listed.name,
                "manifest": serde_json::to_value(&listed.manifest)
                    .expect("manifest is always serializable"),
            })
        })
        .collect();
    if let Some(warning) = active.warning {
        warnings.push(warning);
    }
    let mut envelope = Envelope::ok(json!({
        "profiles": profiles,
        "active_profile": active.name,
    }));
    envelope.warnings.extend(warnings);
    envelope
}

/// One profile's full manifest. A profile that is missing, malformed, or from
/// an unknown future version is a failed envelope naming the file and field.
fn info(context: &mut Context, name: &str) -> Envelope {
    let mut emitter = Emitter::new("info");
    context.progress(&mut emitter, &format!("reading manifest for `{name}`"));

    match context.store.load(name) {
        Ok(manifest) => Envelope::ok(json!({
            "name": name,
            "manifest": serde_json::to_value(&manifest)
                .expect("manifest is always serializable"),
        })),
        Err(error) => Envelope::failed(error),
    }
}

/// Removes a profile, refusing the active one unless forced.
fn delete(context: &mut Context, name: &str, force: bool) -> Envelope {
    let mut emitter = Emitter::new("delete");
    context.progress(
        &mut emitter,
        &format!("checking `{name}` is not the active profile"),
    );
    context.progress(&mut emitter, &format!("removing profile `{name}`"));

    Envelope::ok(json!({
        "stub": true,
        "name": name,
        "force": force,
        "deleted": false,
    }))
}

/// The package and config delta between two profiles.
fn diff(context: &mut Context, a: &str, b: &str) -> Envelope {
    let mut emitter = Emitter::new("diff");
    context.progress(
        &mut emitter,
        &format!("reading manifests for `{a}` and `{b}`"),
    );
    context.progress(&mut emitter, "computing package and config delta");

    Envelope::ok(json!({
        "stub": true,
        "a": a,
        "b": b,
        "package_delta": [],
        "config_delta": [],
    }))
}

/// Moves an image into the shared wallpapers layer.
fn wallpaper_import(context: &mut Context, path: &str) -> Envelope {
    let mut emitter = Emitter::new("wallpaper-import");
    context.progress(
        &mut emitter,
        &format!("checking `{path}` is a readable image"),
    );
    context.progress(&mut emitter, "importing into the shared wallpapers layer");

    let mut envelope = Envelope::ok(json!({
        "stub": true,
        "source": path,
        "imported_to": Value::Null,
    }));
    with_tools(&mut envelope, WALLPAPER_TOOLS);
    envelope
}

/// Idempotent first-run bootstrap: shared layers, hardware extraction, bundled
/// wallpapers.
fn init(context: &mut Context) -> Envelope {
    let mut emitter = Emitter::new("init");
    context.progress(&mut emitter, "creating the shared riceswap layers");
    context.progress(&mut emitter, "extracting hardware information");
    context.progress(&mut emitter, "installing bundled wallpapers");

    context.state.set_initialized(true);

    let mut envelope = Envelope::ok(json!({
        "stub": true,
        "initialized": true,
        "created": [],
    }));
    with_tools(&mut envelope, Tool::ALL);
    envelope
}

/// One line describing why a tool is unusable.
fn describe(status: &tools::ToolStatus) -> String {
    match (&status.error, status.exit_code) {
        (Some(error), _) if !status.available => error.clone(),
        (Some(error), Some(code)) => format!("exited {code}: {error}"),
        (_, Some(code)) => format!("exited {code}"),
        _ => "no output".to_string(),
    }
}
