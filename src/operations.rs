//! The ten operations.
//!
//! The profile-store operations are real: `list` and `info` read manifests
//! through the store, and `switch` verifies its target then flips the `current`
//! symlink — the activation primitive later tickets build on. `init` is the
//! idempotent first-run bootstrap: it creates the shared layers, installs the
//! four bundled wallpapers, lifts the hardware configuration out of the live
//! Hyprland config into the shared hardware file, and copies the packaged GUI
//! config when there is one. `wallpaper-import` moves an image into the shared
//! wallpapers layer. `detect` and `snapshot` are the snapshot pipeline:
//! detection proposes the candidates, and a snapshot mirrors the confirmed
//! ones into a profile — manifest, screenshot, and the shared-hardware
//! `source =` line included. `plan` and `switch` are the switch core: the
//! plan computes the real diff, and the switch runs the locked 10-step
//! sequence — official package ops escalated through `pkexec pacman`, AUR
//! installs through the runtime-detected helper spawned inside the
//! floating-terminal wrapper. `delete` removes a profile from the store — refusing the active
//! one unless forced — and `diff` reports the package and config delta
//! between two profiles, the same computation `plan` uses.

use crate::cli::Invocation;
use crate::detection::{self, IMAGE_EXTENSIONS, PackageScan};
use crate::envelope::{Emitter, Envelope};
use crate::profile::{
    CURRENT_MANIFEST_VERSION, FileEntry, Manifest, Packages, ProfileInfo, RiceInfo, Service, Shell,
    Store,
};
use crate::state::StateStore;
use crate::tools::{self, Tool};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The tools a package-touching operation depends on.
const PACKAGE_MANAGERS: &[Tool] = &[Tool::Pacman, Tool::Yay, Tool::Paru];

/// The command prefix every official package op runs under: polkit escalation
/// over pacman, so installs and plain `-R` removals alike pass through the
/// system password dialog instead of a terminal. The AUR path has its own
/// prefix — the floating-terminal wrapper naming the detected helper — built
/// where the helper is known.
const OFFICIAL: &[&str] = &[Tool::PkExec.name(), Tool::Pacman.name()];

/// The tools a snapshot reports on: the package managers behind the
/// binary-reference scan and the package split, and `grim` behind the
/// profile screenshot.
const SNAPSHOT_TOOLS: &[Tool] = &[Tool::Pacman, Tool::Yay, Tool::Paru, Tool::Grim];

/// Where the package installs the bundled wallpapers. `init` prefers these and
/// falls back to the embedded copies below, so a standalone binary — or a test
/// sandbox — still ends up with all four defaults.
const PACKAGED_WALLPAPERS: &str = "/usr/share/riceswap/wallpapers";

/// Where the package installs the Quickshell GUI config. `init` copies it into
/// the user's config so users can customize their own copy; when it is absent
/// (headless, test sandbox, not packaged) there is simply nothing to copy.
const PACKAGED_GUI: &str = "/etc/xdg/quickshell/riceswap";

/// The four bundled default wallpapers, embedded so the defaults are always
/// installable. First run copies them from [`PACKAGED_WALLPAPERS`] when that
/// provides them; otherwise these bytes are written.
const BUNDLED_WALLPAPERS: &[(&str, &[u8])] = &[
    (
        "default-dawn.png",
        include_bytes!("../assets/wallpapers/default-dawn.png"),
    ),
    (
        "default-day.png",
        include_bytes!("../assets/wallpapers/default-day.png"),
    ),
    (
        "default-dusk.png",
        include_bytes!("../assets/wallpapers/default-dusk.png"),
    ),
    (
        "default-night.png",
        include_bytes!("../assets/wallpapers/default-night.png"),
    ),
];

/// The environment variables that configure GPU/hardware behaviour — the only
/// `env =` lines the lift takes. Theming and cursor variables belong to the
/// rice (`XCURSOR_SIZE`, `GTK_THEME`, ...), so they stay in `hyprland.conf`.
const GPU_ENV_KEYS: &[&str] = &[
    "LIBVA_DRIVER_NAME",
    "GBM_BACKEND",
    "__GLX_VENDOR_LIBRARY_NAME",
    "NVD_BACKEND",
    "WLR_NO_HARDWARE_CURSORS",
    "WLR_DRM_NO_ATOMIC",
    "AQ_DRM_DEVICES",
];

/// The shared hardware file's header: what the file is, who owns it, and how
/// the Hyprland config reaches it.
const HARDWARE_HEADER: &str = "\
# Shared hardware configuration - managed by RiceSwap.
# Monitors, input, and GPU environment lifted out of hyprland.conf so every
# profile shares one display setup. Edit here: hyprland.conf sources this file
# through ~/.config/hypr/riceswap/hardware.conf.";

/// The line `init` leaves at the end of the live Hyprland config once the
/// hardware blocks have been lifted out of it. Finding this line is how a
/// re-run knows the lift already happened and changes nothing.
const HARDWARE_SOURCE: &str = "source = ~/.config/hypr/riceswap/hardware.conf";

/// Everything an operation needs from its environment.
pub struct Context {
    home: PathBuf,
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
        let home = PathBuf::from(home);
        let store = Store::new(home.clone());
        Ok(Context {
            state: StateStore::load(store.data_dir().join("state.json")),
            home,
            store,
        })
    }

    /// `~/.local/share/riceswap/wallpapers`, the shared wallpaper layer.
    fn wallpapers_dir(&self) -> PathBuf {
        self.store.wallpapers_dir()
    }

    /// `~/.local/share/riceswap/hardware.conf`, the shared hardware file.
    fn hardware_file(&self) -> PathBuf {
        self.store.hardware_file()
    }

    /// `~/.config/hypr/hyprland.conf`, the live Hyprland config `init` lifts
    /// hardware configuration out of.
    fn hyprland_config(&self) -> PathBuf {
        self.home.join(".config").join("hypr").join("hyprland.conf")
    }

    /// `~/.config/hypr/riceswap/hardware.conf`, the permanent symlink through
    /// which every profile's Hyprland config inherits the hardware setup.
    fn hardware_link(&self) -> PathBuf {
        self.home
            .join(".config")
            .join("hypr")
            .join("riceswap")
            .join("hardware.conf")
    }

    /// `~/.config/quickshell/riceswap`, the user's customizable GUI copy.
    fn user_gui_dir(&self) -> PathBuf {
        self.home
            .join(".config")
            .join("quickshell")
            .join("riceswap")
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
        Invocation::Snapshot { name, force, only } => {
            snapshot(context, name, *force, only.as_deref())
        }
        Invocation::Plan { target } => plan(context, target),
        Invocation::Switch { target, aur_helper } => switch(context, target, aur_helper.as_deref()),
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
///
/// The proposal itself is [`detection`]'s — the same scan `snapshot` re-runs
/// to capture what the user confirmed — so `detect` on screen and `snapshot`
/// on disk can never disagree about what the desktop is made of.
fn detect(context: &mut Context) -> Envelope {
    let mut emitter = Emitter::new("detect");
    context.progress(&mut emitter, "scanning candidate config directories");
    let config = detection::scan_config(&context.home);
    context.progress(&mut emitter, "scanning installed packages");
    let packages = detection::scan_packages(&context.home, &config.dirs);
    context.progress(&mut emitter, "scanning assets and wallpaper candidates");
    let assets = detection::scan_assets(&context.home);
    let wallpapers = detection::scan_wallpapers(&context.home);

    let mut envelope = Envelope::ok(json!({
        "config_dirs": config.dirs,
        "packages": {
            "official": packages.official,
            "aur": packages.aur,
        },
        "assets": assets,
        "wallpapers": wallpapers,
    }));
    with_tools(&mut envelope, Tool::ALL);
    envelope
}

/// The pre-flight diff behind `switch`: what would change, and what is blocked.
fn plan(context: &mut Context, target: &str) -> Envelope {
    let mut emitter = Emitter::new("plan");
    context.progress(&mut emitter, &format!("reading profile `{target}`"));

    let target_manifest = match context.store.load(target) {
        Ok(manifest) => manifest,
        Err(error) => return Envelope::failed(error),
    };

    context.progress(&mut emitter, "computing package diff");

    let active = context.store.active();
    let active_manifest = active
        .name
        .as_ref()
        .and_then(|name| context.store.load(name).ok());

    let (install_official, install_aur, remove_official, remove_aur) =
        if let Some(ref current) = active_manifest {
            compute_package_diff(&current.packages, &target_manifest.packages)
        } else {
            // No active profile: install everything from target, remove nothing
            (
                target_manifest.packages.official.clone(),
                target_manifest.packages.aur.clone(),
                Vec::new(),
                Vec::new(),
            )
        };

    let (services_stop, services_start) = if let Some(ref current) = active_manifest {
        compute_service_changes(&current.services, &target_manifest.services)
    } else {
        (
            Vec::new(),
            target_manifest
                .services
                .iter()
                .map(|s| s.name.clone())
                .collect(),
        )
    };

    // Reported separately from the services for the reason
    // `compute_shell_change` gives: a shell swap is invisible to a service diff,
    // and a panel that cannot see it is a panel that cannot warn about the one
    // step that actually ends the old desktop.
    let (shell_stop, shell_start) = compute_shell_change(
        active_manifest.as_ref().and_then(|m| m.shell.as_ref()),
        target_manifest.shell.as_ref(),
    );

    context.progress(&mut emitter, "checking managed paths for conflicts");

    let (symlink_link, symlink_unlink) = if let Some(ref current) = active_manifest {
        compute_symlink_changes(&current.files, &target_manifest.files)
    } else {
        (
            target_manifest
                .files
                .iter()
                .map(|f| f.path.clone())
                .collect(),
            Vec::new(),
        )
    };

    let profile_dir = context.store.profile_dir(target);
    let classified = classify_blocked_paths(&context.home, &profile_dir, &symlink_link);
    let blocked_paths = blocked_paths_of(&classified);

    // The preview's removal lists are gated against the system-package floor
    // the same way the switch gates them, so the panel never offers a removal
    // the switch will decline — and the declined ones are named, because a
    // silent decline in a preview is a surprise at switch time.
    let (remove_official, mut protected) = partition_removals(&remove_official);
    let (remove_aur, aur_protected) = partition_removals(&remove_aur);
    protected.extend(aur_protected);
    protected.sort();

    let mut envelope = Envelope::ok(json!({
        "target": target,
        "package_diff": {
            "install": { "official": install_official, "aur": install_aur },
            "remove": { "official": remove_official, "aur": remove_aur },
        },
        // The diff above is manifest-to-manifest, so on a first switch it
        // names every package the profile declares even when the machine
        // already has it. This is the same set intersected with what pacman
        // says is actually absent — the honest count, and what a profile
        // imported from a rice's own dependency list is really asking for.
        "install_missing": {
            "official": missing_from_machine(&install_official),
            "aur": missing_from_machine(&install_aur),
        },
        // The floor in action: what the diff asked to remove and the switch
        // will decline to, because removing `glibc` or `coreutils` is not a
        // package change but a dead machine. Named here so the panel can show
        // it and the switch report can repeat it.
        "remove_protected": protected,
        "service_changes": { "stop": services_stop, "start": services_start },
        "shell_change": {
            "stop": shell_stop.map(|shell| shell.name.clone()),
            "start": shell_start.map(|shell| shell.name.clone()),
        },
        "symlink_changes": { "link": symlink_link, "unlink": symlink_unlink },
        // A real file at a target no longer stops the switch: the ones that
        // differ are preserved under the profile's `backups/` first, and the
        // report says which. The two lists partition `blocked_paths`.
        "backed_up_paths": classified.backed_up,
        "identical_paths": classified.identical,
        "blocked_paths": blocked_paths,
    }));
    with_tools(&mut envelope, PACKAGE_MANAGERS);
    envelope
}

/// Writes a profile from the desktop as it stands: the same candidates
/// `detect` proposes are captured — mirrored files, the manifest (official/AUR
/// package split, `exec-once` services with their `pkill` default stop,
/// auto-filled `rice_info`), the `grim` screenshot, and the shared-hardware
/// `source =` line injected exactly once into the captured Hyprland config.
///
/// A taken name refuses unless `force` clears it — the name and directory
/// stay, everything under them becomes the new capture's. The active profile
/// is never overwritten in place, force included: snapshotting over an active
/// profile forks the live desktop into the new profile instead, symlinks
/// dereferenced, and the `current` symlink untouched.
fn snapshot(context: &mut Context, name: &str, force: bool, only: Option<&[String]>) -> Envelope {
    let mut emitter = Emitter::new("snapshot");
    context.progress(&mut emitter, &format!("checking the name `{name}`"));
    if let Err(error) = context.store.validate_name(name) {
        return Envelope::failed(error);
    }
    let active = context.store.active();
    if active.name.as_deref() == Some(name) {
        return Envelope::failed(format!(
            "`{name}` is the active profile; RiceSwap never overwrites the active \
             profile in place — snapshot a new name to fork the desktop as it stands"
        ));
    }
    let profile = context.store.profile_dir(name);
    if let Err(error) = clear_for_overwrite(&profile, force) {
        return Envelope::failed(error);
    }
    let forked = active.name.is_some();
    if forked {
        context.progress(
            &mut emitter,
            &format!("forking the active desktop into `{name}`"),
        );
    }
    context.progress(
        &mut emitter,
        &format!("creating profile directory for `{name}`"),
    );
    if let Err(error) = std::fs::create_dir_all(&profile) {
        return Envelope::failed(format!(
            "cannot create the profile directory {}: {error}",
            profile.display()
        ));
    }

    context.progress(&mut emitter, "collecting checked config paths");
    let config = detection::scan_config(&context.home);
    context.progress(&mut emitter, "collecting checked packages");
    let packages = detection::scan_packages(&context.home, &config.dirs);
    context.progress(&mut emitter, "collecting fonts, icons, and themes");
    let mut selected = config.dirs.clone();
    selected.extend(detection::scan_assets(&context.home));
    selected.sort();
    selected.dedup();
    // A confirmed selection narrows the capture to exactly what the user kept
    // checked on screen. Without it every detected path is captured, which is
    // what a scripted snapshot means. Narrowing here, after the scan, means
    // `detect` on screen and `snapshot` on disk cannot disagree about what was
    // available — only about which parts of it were kept.
    if let Some(only) = only {
        let kept: BTreeSet<&str> = only.iter().map(String::as_str).collect();
        let dropped: Vec<String> = selected
            .iter()
            .filter(|path| !kept.contains(path.as_str()))
            .cloned()
            .collect();
        if !dropped.is_empty() {
            context.progress(
                &mut emitter,
                &format!(
                    "narrowing the capture to the {} confirmed paths",
                    only.len()
                ),
            );
        }
        selected.retain(|path| kept.contains(path.as_str()));
    }

    context.progress(&mut emitter, "copying the selected files into the profile");
    let mut warnings: Vec<String> = Vec::new();
    if let Some(warning) = active.warning {
        warnings.push(warning);
    }
    let mut stack = Vec::new();
    let mut mirrored = Vec::new();
    for relative in &selected {
        match mirror_into(&context.home, relative, &profile, &mut stack) {
            Ok(true) => mirrored.push(relative.clone()),
            // The selection vanished between scan and copy: nothing to mirror.
            Ok(false) => {}
            // One unreadable path must not take the capture down with it; the
            // envelope says what did not make it in.
            Err(error) => warnings.push(error),
        }
    }

    let captured = profile.join(".config").join("hypr").join("hyprland.conf");
    if captured.is_file() {
        context.progress(
            &mut emitter,
            "connecting the captured hyprland.conf to the hardware layer",
        );
        if let Err(error) = inject_hardware_source(&captured) {
            return Envelope::failed(error);
        }
    }

    context.progress(&mut emitter, "capturing the profile screenshot with grim");
    let screenshot = match capture_screenshot(&profile) {
        Ok(path) => Some(path),
        Err(error) => {
            warnings.push(format!("the profile screenshot was not captured: {error}"));
            None
        }
    };

    context.progress(&mut emitter, "writing profile.toml");
    let manifest = build_manifest(
        name,
        screenshot.is_some(),
        &mirrored,
        &packages,
        config.services,
    );
    if let Err(error) = context.store.save(name, &manifest) {
        return Envelope::failed(error);
    }
    context.progress(&mut emitter, &format!("profile `{name}` is ready"));

    let mut envelope = Envelope::ok(json!({
        "profile": name,
        "forked": forked,
        "manifest_written": true,
        "checked_paths": mirrored,
        "checked_packages": {
            "official": packages.official,
            "aur": packages.aur,
        },
        "screenshot": screenshot.map(|path| path.display().to_string()),
    }));
    envelope.warnings.extend(warnings);
    with_tools(&mut envelope, SNAPSHOT_TOOLS);
    envelope
}

/// The collision rule: an existing profile directory refuses unless `force`
/// clears it first — the name and the directory stay, everything under them
/// is the new capture's to write. A missing profile is simply `Ok`.
fn clear_for_overwrite(profile: &Path, force: bool) -> Result<(), String> {
    match std::fs::symlink_metadata(profile) {
        Ok(_) if !force => Err(format!(
            "a profile already exists at {}; snapshot with --force to overwrite it",
            profile.display()
        )),
        Ok(metadata) => {
            let removed = if metadata.is_dir() && !metadata.file_type().is_symlink() {
                std::fs::remove_dir_all(profile)
            } else {
                std::fs::remove_file(profile)
            };
            removed.map_err(|error| {
                format!(
                    "cannot clear the existing profile at {}: {error}",
                    profile.display()
                )
            })
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "cannot inspect the profile directory {}: {error}",
            profile.display()
        )),
    }
}

/// Copies one `$HOME`-relative selection into the profile, preserving the
/// `$HOME`-relative structure. Symlinks are dereferenced along the way: the
/// profile holds real files whether the live path is one or points into the
/// active profile — which is what makes snapshotting over an active profile
/// a fork rather than a self-copy. `Ok(false)` means the source was not
/// there to mirror.
///
/// `stack` holds the directories currently being copied, by canonical path,
/// so a symlink back up the tree ends the recursion instead of chasing it.
fn mirror_into(
    home: &Path,
    relative: &str,
    profile: &Path,
    stack: &mut Vec<PathBuf>,
) -> Result<bool, String> {
    let source = home.join(relative);
    let metadata = match std::fs::metadata(&source) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("cannot inspect {}: {error}", source.display())),
    };
    let destination = profile.join(relative);
    if metadata.is_dir() {
        mirror_dir(&source, &destination, stack)?;
        return Ok(true);
    }
    if !metadata.is_file() {
        // A fifo or socket has no bytes a profile could hold.
        return Ok(false);
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    std::fs::copy(&source, &destination)
        .map(|_| true)
        .map_err(|error| {
            format!(
                "cannot copy {} to {}: {error}",
                source.display(),
                destination.display()
            )
        })
}

/// Mirrors one directory tree, guarding the recursion against symlinks that
/// lead back into a directory already being copied.
fn mirror_dir(source: &Path, destination: &Path, stack: &mut Vec<PathBuf>) -> Result<(), String> {
    let canonical = std::fs::canonicalize(source)
        .map_err(|error| format!("cannot resolve {}: {error}", source.display()))?;
    if stack.contains(&canonical) {
        return Ok(());
    }
    stack.push(canonical);
    let copied = copy_entries(source, destination, stack);
    stack.pop();
    copied
}

/// Creates the destination directory and copies every file under `source`
/// into it, recursing into subdirectories. Entries that cannot be inspected —
/// a broken symlink, a socket — are skipped: they hold nothing to mirror.
fn copy_entries(source: &Path, destination: &Path, stack: &mut Vec<PathBuf>) -> Result<(), String> {
    std::fs::create_dir_all(destination)
        .map_err(|error| format!("cannot create {}: {error}", destination.display()))?;
    let entries = std::fs::read_dir(source)
        .map_err(|error| format!("cannot read {}: {error}", source.display()))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("cannot read an entry of {}: {error}", source.display()))?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        let Ok(metadata) = std::fs::metadata(&from) else {
            continue;
        };
        if metadata.is_dir() {
            mirror_dir(&from, &to, stack)?;
        } else if metadata.is_file() {
            std::fs::copy(&from, &to).map_err(|error| {
                format!(
                    "cannot copy {} to {}: {error}",
                    from.display(),
                    to.display()
                )
            })?;
        }
    }
    Ok(())
}

/// Appends the shared-hardware `source =` line to the captured Hyprland
/// config, exactly once: a config that already sources the hardware file —
/// the live one after `init`, or one a previous capture already injected
/// into — is left verbatim, so a forced re-snapshot can never duplicate the
/// line. Switch time never comes back here: profiles are written once, at
/// snapshot time.
fn inject_hardware_source(captured: &Path) -> Result<(), String> {
    let content = std::fs::read_to_string(captured)
        .map_err(|error| format!("cannot read {}: {error}", captured.display()))?;
    if content.lines().any(sources_hardware) {
        return Ok(());
    }
    let mut rewritten = content;
    if !rewritten.is_empty() && !rewritten.ends_with('\n') {
        rewritten.push('\n');
    }
    if !rewritten.is_empty() {
        rewritten.push('\n');
    }
    rewritten.push_str(HARDWARE_SOURCE);
    rewritten.push('\n');
    std::fs::write(captured, rewritten).map_err(|error| {
        format!(
            "cannot connect {} to the shared hardware file: {error}",
            captured.display()
        )
    })
}

/// The profile preview: `grim <profile>/screenshot.png`. A machine without a
/// working grim still gets its profile — the caller turns the failure into a
/// warning instead of losing the capture over a preview image.
fn capture_screenshot(profile: &Path) -> Result<PathBuf, String> {
    let target = profile.join("screenshot.png");
    let output = Command::new("grim")
        .arg(&target)
        .output()
        .map_err(|error| format!("cannot run grim: {error}"))?;
    if output.status.success() {
        return Ok(target);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| match output.status.code() {
            Some(code) => format!("grim exited with code {code}"),
            None => "grim was killed by a signal".to_string(),
        }))
}

/// The `profile.toml` a snapshot writes: the locked schema at the current
/// version, the mirrored `$HOME`-relative paths as `files` entries (never
/// `optional` — that flag is the user's to hand-add), the official/AUR
/// package split, the `exec-once` services with their `pkill` default stop,
/// and the `rice_info` the desktop revealed.
fn build_manifest(
    name: &str,
    has_screenshot: bool,
    files: &[String],
    packages: &PackageScan,
    services: Vec<Service>,
) -> Manifest {
    let now = now_rfc3339();
    Manifest {
        manifest_version: CURRENT_MANIFEST_VERSION,
        profile: ProfileInfo {
            name: name.to_string(),
            description: String::new(),
            created_at: now.clone(),
            updated_at: now,
            screenshot: if has_screenshot {
                "screenshot.png".to_string()
            } else {
                String::new()
            },
            source_url: None,
            source_commit: None,
        },
        packages: Packages {
            official: packages.official.clone(),
            aur: packages.aur.clone(),
        },
        services,
        // A snapshot records the shell it found running, so a later switch can
        // recognise it and end it. `detect` reads it off the environment the
        // same way it reads the services out of `exec-once`.
        shell: None,
        files: files
            .iter()
            .map(|path| FileEntry {
                path: path.clone(),
                optional: false,
            })
            .collect(),
        rice_info: auto_rice_info(files, packages),
    }
}

/// The `rice_info` entries the desktop reveals at snapshot time: the bar,
/// terminal, and color scheme that showed up among the captured paths and
/// packages. Nothing detected means no key — the GUI renders the table as it
/// finds it.
fn auto_rice_info(files: &[String], packages: &PackageScan) -> RiceInfo {
    const BARS: &[&str] = &["waybar", "ags", "quickshell"];
    const TERMINALS: &[&str] = &["kitty", "foot", "fish", "alacritty", "wezterm", "ghostty"];
    const COLOR_SCHEMES: &[&str] = &["matugen", "pywal"];

    // A captured path counts as the candidate's own config when it is that
    // config dir or anything inside it: a profile owning
    // `.config/quickshell/caelestia` is a quickshell desktop exactly as much as
    // one owning `.config/quickshell` outright, and an exact match would
    // silently drop the attribution for every nested capture.
    let present = |candidate: &str| {
        let dir = format!(".config/{candidate}");
        let nested = format!("{dir}/");
        files
            .iter()
            .any(|path| path.as_str() == dir || path.starts_with(&nested))
            || packages
                .official
                .iter()
                .chain(&packages.aur)
                .any(|package| package == candidate || package.ends_with(&format!("-{candidate}")))
    };
    let mut info = RiceInfo::default();
    for (key, candidates) in [
        ("bar", BARS),
        ("terminal", TERMINALS),
        ("colors", COLOR_SCHEMES),
    ] {
        if let Some(found) = candidates.iter().find(|candidate| present(candidate)) {
            info.insert(key, *found);
        }
    }
    info
}

/// The current instant as an RFC 3339 UTC timestamp — the manifest's
/// `created_at`/`updated_at` format, with no date crate to get there.
fn now_rfc3339() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let (hours, minutes, secs) = (seconds % 86_400 / 3600, seconds % 3600 / 60, seconds % 60);
    let (year, month, day) = civil_from_days((seconds / 86_400) as i64);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{secs:02}Z")
}

/// Days since the Unix epoch as a civil (year, month, day): Howard Hinnant's
/// `civil_from_days`, exact for every date the timestamp range can hold.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = (shifted - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// The switch sequence's heart: verify the target, compute the plan, check for
/// blocked paths and — when the plan installs AUR packages — a usable AUR
/// helper, then run the fixed idempotent sequence: flip `current` → stop A's
/// services → flip symlinks → install-first package ops (official via
/// `pkexec pacman`, AUR via the detected helper inside the floating-terminal
/// wrapper; on conflict remove the conflicting A-package and retry; then
/// remove A-unique with plain `pkexec pacman -R`, logging "kept" when
/// refused) → `hyprctl reload` → start B's services (failure = warning).
/// Failures produce `ok: false` with `completed_steps` + `resume_hint`.
/// SIGTERM cancels at the next step boundary.
///
/// `aur_helper` is the `--aur-helper` override: it pins the helper instead of
/// runtime detection (yay before paru).
fn switch(context: &mut Context, target: &str, aur_helper: Option<&str>) -> Envelope {
    let mut emitter = Emitter::new("switch");

    // Set up SIGTERM handler
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancelled_flag = Arc::clone(&cancelled);
    let _ = signal_hook::flag::register(signal_hook::consts::SIGTERM, cancelled_flag);

    context.progress(&mut emitter, &format!("verifying profile `{target}`"));

    let target_manifest = match context.store.load(target) {
        Ok(manifest) => manifest,
        Err(error) => return Envelope::failed(error),
    };

    context.progress(&mut emitter, "computing the switch plan");

    let active = context.store.active();
    let active_manifest = active
        .name
        .as_ref()
        .and_then(|name| context.store.load(name).ok());

    let (install_official, install_aur, remove_official, remove_aur) =
        if let Some(ref current) = active_manifest {
            compute_package_diff(&current.packages, &target_manifest.packages)
        } else {
            (
                target_manifest.packages.official.clone(),
                target_manifest.packages.aur.clone(),
                Vec::new(),
                Vec::new(),
            )
        };
    // The diff above is manifest-to-manifest, so with no active profile it
    // reports every declared package as an install — including the ones the
    // machine already has. Installing those anyway is not merely wasteful: each
    // `pkexec pacman -S` is its own polkit prompt, so a profile declaring 22
    // present packages asks for the password 22 times and the switch dies
    // looking like a credential failure. The machine is the other half of the
    // question, and `plan` already asks it — the switch asks it too.
    let install_official = missing_from_machine(&install_official);
    let install_aur = missing_from_machine(&install_aur);

    // Removals are gated against the system-package floor, and it matters far
    // more than the install gate above. The diff is "packages the leaving
    // profile had that the arriving one does not", which on two real shells
    // reads `glibc` as removable and raised a polkit prompt the user answered
    // correctly — only for pacman to decline the *request*. Installing a present
    // package is waste; attempting to remove a foundation package is a dead
    // machine, so it is never even asked for. What the floor withholds is
    // named here and in the report, so the decline is not silent.
    let (remove_official, mut protected) = partition_removals(&remove_official);
    let (remove_aur, aur_protected) = partition_removals(&remove_aur);
    protected.extend(aur_protected);
    protected.sort();
    let mut warnings = Vec::new();
    if !protected.is_empty() {
        let note = format!(
            "kept: {} (system packages are never removed by a switch)",
            protected.join(", ")
        );
        emitter.warning(&note);
        warnings.push(note);
    }

    let (services_stop, services_start) = if let Some(ref current) = active_manifest {
        compute_service_changes(&current.services, &target_manifest.services)
    } else {
        (
            Vec::new(),
            target_manifest
                .services
                .iter()
                .map(|s| s.name.clone())
                .collect(),
        )
    };

    // The shell is computed apart from the services, because a shell change is
    // invisible to a service diff: both profiles record a `qs` service running
    // `qs -c $qsConfig`, so the names match, the strings match, and the diff
    // concludes nothing happened. It did not stop the old shell.
    let (shell_stop, shell_start) = compute_shell_change(
        active_manifest.as_ref().and_then(|m| m.shell.as_ref()),
        target_manifest.shell.as_ref(),
    );

    let (symlink_link, symlink_unlink) = if let Some(ref current) = active_manifest {
        compute_symlink_changes(&current.files, &target_manifest.files)
    } else {
        (
            target_manifest
                .files
                .iter()
                .map(|f| f.path.clone())
                .collect(),
            Vec::new(),
        )
    };

    // A real file at a target is the user's own, and replacing it must not lose
    // it. Files that match the profile byte for byte carry nothing the profile
    // does not already hold; the rest are copied under the target profile's
    // `backups/` here, before `current` flips, so a failure part-way through
    // leaves the live tree exactly as it was.
    let profile_dir = context.store.profile_dir(target);
    let classified = classify_blocked_paths(&context.home, &profile_dir, &symlink_link);
    let backups_dir = context.store.backups_dir(target);
    let mut backed_up = Vec::new();
    for relative in &classified.backed_up {
        let stored = match backup_managed_file(&backups_dir, relative, &context.home.join(relative))
        {
            Ok(stored) => stored,
            Err(error) => {
                return Envelope {
                    ok: false,
                    data: json!({
                        "error": format!("cannot preserve your `{relative}` before switching: {error}"),
                        "completed_steps": 2,
                        "resume_hint": format!("switch to `{target}` to restore"),
                    }),
                    warnings: Vec::new(),
                };
            }
        };
        emitter.warning(&format!("kept your `{relative}` at {}", stored.display()));
        backed_up.push(relative.clone());
    }

    // AUR installs need a helper before anything is touched: with AUR packages
    // in the plan and none usable on PATH, refuse here — the error names the
    // packages that would be left uninstalled, and `current` never flips.
    let aur_helper = if install_aur.is_empty() {
        None
    } else {
        match detect_aur_helper(aur_helper) {
            Some(helper) => Some(helper),
            None => {
                let looked = match aur_helper {
                    Some(helper) => {
                        format!("the AUR helper `{helper}` (--aur-helper) does not answer on PATH")
                    }
                    None => "no usable AUR helper on PATH (install `yay` or `paru`, or pass \
                             `--aur-helper <helper>`)"
                        .to_string(),
                };
                return package_failure(
                    target,
                    2,
                    Vec::new(),
                    format!(
                        "{looked}; cannot install the AUR packages: {}",
                        install_aur.join(", ")
                    ),
                );
            }
        }
    };

    let mut completed_steps = 2;
    let mut installed = Vec::new();
    let mut removed = Vec::new();
    let mut kept = Vec::new();
    let mut conflict_removed = Vec::new();
    let mut linked = Vec::new();
    let mut unlinked = Vec::new();
    let mut preserved = Vec::new();
    let mut services_stopped = Vec::new();
    let mut services_started = Vec::new();
    let mut shell_stopped = None;
    let mut shell_started = None;
    let mut reloaded = false;

    // Step 3: flip `current` symlink
    context.progress(&mut emitter, &format!("activating profile `{target}`"));
    if let Err(error) = context.store.flip(target) {
        return Envelope {
            ok: false,
            data: json!({
                "error": error,
                "completed_steps": completed_steps,
                "resume_hint": format!("switch to `{target}` to restore"),
            }),
            warnings,
        };
    }
    completed_steps += 1;

    if cancelled.load(Ordering::Relaxed) {
        return Envelope {
            ok: false,
            data: json!({
                "error": "operation cancelled",
                "completed_steps": completed_steps,
                "resume_hint": format!("switch to `{target}` to restore"),
            }),
            warnings,
        };
    }

    // Step 4: stop A's services
    context.progress(&mut emitter, "stopping old services");
    if let Some(ref current) = active_manifest {
        for service in &current.services {
            if services_stop.contains(&service.name) {
                let _ = Command::new("sh").arg("-c").arg(&service.stop).output();
                services_stopped.push(service.name.clone());
            }
        }
    }
    // The shell stops here too, and for the same reason the other services do:
    // it belongs to the profile being left. It is driven by its own `[shell]`
    // table rather than the service list because the service list cannot tell
    // two shells apart — see `compute_shell_change`.
    if let Some(shell) = shell_stop {
        let _ = Command::new("sh").arg("-c").arg(&shell.stop).output();
        shell_stopped = Some(shell.name.clone());
    }
    completed_steps += 1;

    if cancelled.load(Ordering::Relaxed) {
        return Envelope {
            ok: false,
            data: json!({
                "error": "operation cancelled",
                "completed_steps": completed_steps,
                "resume_hint": format!("switch to `{target}` to restore"),
            }),
            warnings,
        };
    }

    // Step 5: flip symlinks
    context.progress(&mut emitter, "linking managed config paths");
    // A path the old profile managed and the new one does not used to be
    // deleted. That is the one place the switch could still lose data: the
    // whole promise is that you can go back, and on the way back the old
    // profile's own files are exactly the ones the new profile stopped claiming.
    // So the path is moved into the *leaving* profile's `backups/` before the
    // link is replaced, and the report names where it went.
    for relative in &symlink_unlink {
        let live = context.home.join(relative);
        if let Some(name) = active.name.as_ref() {
            let backups = context.store.backups_dir(name);
            if let Err(error) = preserve_managed_path(&backups, relative, &live) {
                return package_failure(
                    target,
                    completed_steps,
                    warnings,
                    format!("cannot preserve the path `{relative}`: {error}"),
                );
            }
        } else if let Err(error) = clear_managed_path(&live) {
            return package_failure(
                target,
                completed_steps,
                warnings,
                format!("cannot unlink the managed path `{relative}`: {error}"),
            );
        }
        unlinked.push(relative.clone());
        preserved.push(relative.clone());
    }
    let profile_dir = context.store.profile_dir(target);
    for relative in &symlink_link {
        let link = context.home.join(relative);
        let target_path = profile_dir.join(relative);
        if let Err(error) = link_managed_path(&link, &target_path) {
            return package_failure(
                target,
                completed_steps,
                warnings,
                format!("cannot link the managed path `{relative}`: {error}"),
            );
        }
        linked.push(relative.clone());
    }
    completed_steps += 1;

    if cancelled.load(Ordering::Relaxed) {
        return Envelope {
            ok: false,
            data: json!({
                "error": "operation cancelled",
                "completed_steps": completed_steps,
                "resume_hint": format!("switch to `{target}` to restore"),
            }),
            warnings,
        };
    }

    // Step 6-7: install-first package ops — official through `pkexec pacman`,
    // AUR through the detected helper inside the floating-terminal wrapper.
    context.progress(&mut emitter, "applying package changes");

    // Install official packages
    if let Err(error) = install_packages(
        OFFICIAL,
        "pkexec pacman",
        &install_official,
        &mut installed,
        &mut removed,
        &mut conflict_removed,
    ) {
        return package_failure(target, completed_steps, warnings, error);
    }

    // Install AUR packages — the helper was pinned or detected pre-flight.
    let mut aur_output = Vec::new();
    if let Some(helper) = &aur_helper {
        let actor = format!("the AUR helper `{helper}` in the floating terminal");
        let prefix = [Tool::FloatTerminal.name(), helper.as_str()];
        match install_packages(
            &prefix,
            &actor,
            &install_aur,
            &mut installed,
            &mut removed,
            &mut conflict_removed,
        ) {
            Err(error) => return package_failure(target, completed_steps, warnings, error),
            Ok(lines) => aur_output = lines,
        }
    }

    // Remove A-unique packages (plain -R, never -Rs/-Rdd)
    for package in remove_official.iter().chain(&remove_aur) {
        if conflict_removed.contains(package) {
            continue; // Already removed during conflict resolution
        }
        match remove_package(package) {
            Ok(true) => removed.push(package.clone()),
            // "still needed" refusal: kept, warned, the switch goes on.
            Ok(false) => {
                kept.push(package.clone());
                let note = format!("kept: {package} (still needed)");
                emitter.warning(&note);
                warnings.push(note);
            }
            // A polkit denial or dead wrapper on the removal: stop the
            // package ops and report, per the failure model.
            Err(error) => return package_failure(target, completed_steps, warnings, error),
        }
    }
    completed_steps += 2;

    if cancelled.load(Ordering::Relaxed) {
        return Envelope {
            ok: false,
            data: json!({
                "error": "operation cancelled",
                "completed_steps": completed_steps,
                "resume_hint": format!("switch to `{target}` to restore"),
            }),
            warnings,
        };
    }

    // Step 8: hyprctl reload
    context.progress(&mut emitter, "reloading Hyprland");
    let reload_output = Command::new("hyprctl").arg("reload").output();
    if reload_output.map(|o| o.status.success()).unwrap_or(false) {
        reloaded = true;
    }
    completed_steps += 1;

    if cancelled.load(Ordering::Relaxed) {
        return Envelope {
            ok: false,
            data: json!({
                "error": "operation cancelled",
                "completed_steps": completed_steps,
                "resume_hint": format!("switch to `{target}` to restore"),
            }),
            warnings,
        };
    }

    // Step 9-10: start B's services
    context.progress(&mut emitter, "starting new services");
    for service in &target_manifest.services {
        if services_start.contains(&service.name) {
            // A service that is still running after the grace window is up —
            // `qs`, `wl-paste --watch` and friends are daemons that never
            // exit. Waiting for one is waiting forever, so the switch gives
            // each start only the window and reads survival as success.
            match start_daemonized(&service.start) {
                Ok(()) => services_started.push(service.name.clone()),
                Err(detail) => {
                    let note = format!("service `{}` failed to start: {}", service.name, detail);
                    emitter.warning(&note);
                    warnings.push(note);
                }
            }
        }
    }
    // The target's shell starts last, after the reload, so it comes up against
    // the config the reload has already installed. A shell that starts too early
    // reads the old config and keeps it — the failure this ordering exists to
    // prevent.
    if let Some(shell) = shell_start {
        // A shell that will not start is a warning, not a failure: the other
        // services, the packages and the links all succeeded, and the config is
        // correct for the next login. Failing the switch here would report a
        // broken profile for a shell that is merely not up yet. The start is
        // daemonised for the same reason the services' are: `qs -c ii` is the
        // desktop itself, and it never exits — waiting for it to is waiting
        // for a switch that has already succeeded to never finish.
        match start_daemonized(&shell.start) {
            Ok(()) => shell_started = Some(shell.name.clone()),
            Err(detail) => {
                let note = format!("shell `{}` failed to start: {detail}", shell.name);
                emitter.warning(&note);
                warnings.push(note);
            }
        }
    }
    completed_steps += 2;

    let mut envelope = Envelope::ok(json!({
        "target": target,
        "completed_steps": completed_steps,
        "resume_hint": format!("switch to `{target}` to restore"),
        "report": {
            "installed": installed,
            "removed": removed,
            "kept": kept,
            "conflict_removed": conflict_removed,
            "aur_output": aur_output,
            "linked": linked,
            "unlinked": unlinked,
            "preserved": preserved,
            "backed_up": backed_up,
            "protected": protected,
            "services_stopped": services_stopped,
            "services_started": services_started,
            "shell_stopped": shell_stopped,
            "shell_started": shell_started,
            "reloaded": reloaded,
        },
    }));
    envelope.warnings = warnings;
    with_tools(&mut envelope, Tool::ALL);
    envelope
}

/// Installs a batch of packages with `prefix -S --noconfirm <pkg>` — the
/// prefix being the privilege path: `pkexec pacman` for official packages,
/// the floating-terminal wrapper naming the AUR helper for AUR ones — handling
/// the install-first conflict fallback: when the installer reports a conflict
/// against a package still in the A-set, that conflicting A-package is removed
/// with plain `pkexec pacman -R` first and the install retried — exactly the
/// spec's locked sequence.
///
/// Returns the stdout the transactions printed — the AUR helper's output the
/// switch report carries. A polkit denial, a dead wrapper, or any refusal that
/// is not the declared-conflict dance is an error naming the actor, the
/// package, and what the transaction said: the failure model's "stop package
/// ops, report".
fn install_packages(
    prefix: &[&str],
    actor: &str,
    packages: &[String],
    installed: &mut Vec<String>,
    removed: &mut Vec<String>,
    conflict_removed: &mut Vec<String>,
) -> Result<Vec<String>, String> {
    let mut output_lines = Vec::new();
    for package in packages {
        let output = run_command(prefix, &["-S", "--noconfirm", package])
            .map_err(|error| format!("cannot run {actor}: {error}"))?;
        if output.status.code() == Some(2) {
            // Conflict detected: remove conflicting package and retry
            let stderr = String::from_utf8_lossy(&output.stderr);
            let Some(conflicting) = extract_conflicting_package(&stderr) else {
                return Err(format!(
                    "{actor} failed to install `{package}`: {}",
                    failure_detail(&output)
                ));
            };
            // Best-effort: if the removal cannot happen, the retry below
            // reports the conflict that is still standing.
            let _ = run_command(OFFICIAL, &["-R", "--noconfirm", &conflicting]);
            conflict_removed.push(conflicting.clone());
            removed.push(conflicting);
            let retry = run_command(prefix, &["-S", "--noconfirm", package])
                .map_err(|error| format!("cannot run {actor}: {error}"))?;
            if retry.status.success() {
                installed.push(package.clone());
                output_lines.extend(stdout_lines(&retry.stdout));
            } else {
                return Err(format!(
                    "{actor} failed to install `{package}` after removing the \
                     conflicting package: {}",
                    failure_detail(&retry)
                ));
            }
        } else if output.status.success() {
            installed.push(package.clone());
            output_lines.extend(stdout_lines(&output.stdout));
        } else {
            return Err(format!(
                "{actor} failed to install `{package}`: {}",
                failure_detail(&output)
            ));
        }
    }
    Ok(output_lines)
}

/// Packages no switch may remove, whatever the manifests say.
///
/// The package diff is manifest-to-manifest: "packages the leaving profile had
/// that the arriving one does not". It is a statement about two rice configs,
/// not evidence that a package is expendable. A profile that captured the
/// machine it was written on carries that machine's foundations in its manifest
/// — `glibc`, `gcc-libs`, `coreutils`, the init system — because the config
/// referenced something that needed them, not because the rice installed them.
/// The moment a *different* profile happens not to list `glibc`, the diff reads
/// it as something to remove, and `pkexec pacman -R glibc` is not a package
/// change; it is a dead system. This is exactly what happened on a return
/// switch between two shells whose manifests disagreed about `glibc`.
///
/// `remove_package` already keeps a package pacman refuses ("breaks
/// dependency"), so on a healthy system pacman would have saved `glibc` from
/// itself. But a polkit *denial* surfaces as a hard error, not a keep — so the
/// dialog this profile produced aborted the switch before pacman could refuse.
/// Declining to even raise the transaction for a foundation package is what
/// makes the return path safe without a password. The list is the floor, not a
/// filter one machine can adjust.
///
/// The floor holds two kinds. First, the system itself: `glibc`, `bash`,
/// `pacman` — pacman's own dependency guard would mostly refuse these, but a
/// guard that answers after a polkit round-trip is a guard that can abort the
/// switch instead of saving the package. Second, the session RiceSwap runs
/// inside: `hyprland`, `quickshell`. For these there is no guard to rely on —
/// nothing declares a pacman-level dependency on the compositor, so
/// `pacman -R hyprland` succeeds and takes the live desktop, the reload of
/// step 8, and the panel itself down with it. A rice can only exist on top of
/// the session, never as a replacement for it.
const SYSTEM_PACKAGES: &[&str] = &[
    // The system.
    "filesystem",
    "glibc",
    "gcc-libs",
    "gcc",
    "binutils",
    "coreutils",
    "bash",
    "sh",
    "systemd",
    "systemd-libs",
    "systemd-sysvcompat",
    "dbus",
    "util-linux",
    "pacman",
    "linux",
    "linux-api-headers",
    "linux-firmware",
    "base",
    "base-devel",
    // The session.
    "hyprland",
    "hyprland-git",
    "quickshell",
    "quickshell-git",
    "quickshell-nightly",
];

/// Whether `package` is one no profile gets to remove.
fn is_system_package(package: &str) -> bool {
    SYSTEM_PACKAGES.contains(&package)
}

/// Splits `removals` into the ones a switch may act on and the foundation
/// packages it must not, preserving the caller's order on the act list. The
/// protected list is what the report names so a decline is never silent.
fn partition_removals(removals: &[String]) -> (Vec<String>, Vec<String>) {
    let mut actionable = Vec::new();
    let mut protected = Vec::new();
    for package in removals {
        if is_system_package(package) {
            protected.push(package.clone());
        } else {
            actionable.push(package.clone());
        }
    }
    (actionable, protected)
}

/// Removes one package with plain `pkexec pacman -R` — never `-Rs`/`-Rdd` —
/// for official and AUR packages alike: pacman is the removal authority
/// either way. `Ok(false)` is the dependency refusal the caller logs as kept;
/// anything else (a polkit denial, a spawn failure) is an error, so a denial
/// on a removal surfaces exactly like one on an install.
fn remove_package(package: &str) -> Result<bool, String> {
    let output = run_command(OFFICIAL, &["-R", "--noconfirm", package])
        .map_err(|error| format!("cannot run pkexec pacman: {error}"))?;
    if output.status.success() {
        return Ok(true);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("breaks dependency") || stderr.contains("could not satisfy dependencies") {
        return Ok(false);
    }
    Err(format!(
        "pkexec pacman failed to remove `{package}`: {}",
        failure_detail(&output)
    ))
}

/// Runs one privilege-path command and waits for it: `prefix` first (`pkexec
/// pacman`, or the floating-terminal wrapper naming the helper), then `args`.
fn run_command(prefix: &[&str], args: &[&str]) -> std::io::Result<Output> {
    let (program, escalated) = prefix
        .split_first()
        .ok_or_else(|| std::io::Error::new(ErrorKind::InvalidInput, "empty command prefix"))?;
    Command::new(program).args(escalated).args(args).output()
}

/// What a failed transaction said — or how it died when it said nothing — so
/// every package failure reaches the envelope with a reason.
fn failure_detail(output: &Output) -> String {
    describe_failure(
        String::from_utf8_lossy(&output.stderr).trim(),
        output.status,
    )
}

/// Why a command that ran and exited badly is reported: its own stderr if it
/// printed one, else the bare facts of how it died (code or signal), so every
/// failure reaches the envelope with a reason.
fn describe_failure(stderr: &str, status: ExitStatus) -> String {
    if !stderr.is_empty() {
        return stderr.to_string();
    }
    match status.code() {
        Some(code) => format!("exit code {code}, with no error output"),
        None => "killed by a signal".to_string(),
    }
}

/// How long a service or shell gets to prove it will not die on its own. A
/// daemon — `qs -c ii`, `wl-paste --watch` — never exits, so the switch must
/// not wait for it to; but a start that is *going* to fail almost always
/// fails within its first moments (no binary, no display, bad config), and
/// the grace window gives those failures their stderr back instead of
/// reporting a success the desktop will not show.
const START_GRACE: Duration = Duration::from_millis(700);

/// Runs one `start` command and reports whether to count it as started, in a
/// bounded time no matter what the command does.
///
/// `sh -c` plus a blocking `.output()` was the old shape, and it hangs:
/// Quickshell *is* the desktop, it does not exit, and a switch that waits for
/// the shell to exit waits forever — at step 10, with the session already
/// killed at step 4. Instead the command runs with a stdout and stdin of
/// `/dev/null` and its stderr pointed at a capture file, and the child is
/// watched without blocking for [`START_GRACE`]: an early exit is read as
/// success or failure with the command's own error text from the file, and a
/// command still alive when the window closes is a daemon coming up. The file
/// is then deleted: a surviving daemon keeps writing into the unlinked inode,
/// which is harmless, where a pipe it inherited would hand it `SIGPIPE` the
/// moment the switch exited.
///
/// The returned `Err` string is the failure's text: the captured stderr when
/// the command said one, else the bare facts of how it died.
fn start_daemonized(start: &str) -> Result<(), String> {
    let capture = std::env::temp_dir().join(format!(
        "riceswap-start-{}-{:x}.log",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0),
    ));
    let quoted = format!("'{}'", capture.display().to_string().replace('\'', r"'\''"));
    let mut child = match Command::new("sh")
        .arg("-c")
        // `exec` replaces the shell with the command, so what the grace window
        // watches is the daemon itself, not a shell waiting behind it; the
        // `2>` is applied before the exec and is what makes the daemon's own
        // error text recoverable. Every descriptor the command would otherwise
        // inherit is /dev/null or the capture file — nothing it holds open can
        // keep the switch's stdout reader waiting after the switch has exited.
        .arg(format!("exec {start} 2>{quoted}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => return Err(format!("cannot run `{start}`: {error}")),
    };

    // `try_wait` never blocks, so the window is polled to its deadline; a
    // process that exits on its own is seen on the very next poll.
    let deadline = Instant::now() + START_GRACE;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(error) => {
                let _ = std::fs::remove_file(&capture);
                return Err(format!("cannot watch `{start}`: {error}"));
            }
        }
        if Instant::now() >= deadline {
            break None;
        }
        std::thread::sleep(Duration::from_millis(25));
    };

    let outcome = match status {
        // Still running at the deadline: a daemon is up. Already exited 0:
        // a one-shot service did its job. Either way, started.
        None => Ok(()),
        Some(exit) if exit.success() => Ok(()),
        Some(exit) => {
            let stderr = std::fs::read_to_string(&capture).unwrap_or_default();
            Err(describe_failure(stderr.trim(), exit))
        }
    };
    let _ = std::fs::remove_file(&capture);
    outcome
}

/// The non-empty lines a transaction printed — the AUR helper's output that
/// flows into the switch report.
fn stdout_lines(stdout: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// The AUR helper the switch installs with: the `--aur-helper` override when
/// given — the testing escape hatch the spec locks in — else the first of
/// `yay`, `paru` that answers its version probe on PATH, yay before paru.
/// Detection means "answers cleanly", not merely present: a helper too broken
/// to report its version must reach the clear no-helper error up front instead
/// of dying halfway through a transaction.
fn detect_aur_helper(override_helper: Option<&str>) -> Option<String> {
    match override_helper {
        Some(helper) => usable_helper(helper).then(|| helper.to_string()),
        None => [Tool::Yay.name(), Tool::Paru.name()]
            .into_iter()
            .find(|candidate| usable_helper(candidate))
            .map(str::to_string),
    }
}

/// Whether `helper` is on `PATH` and answers `--version` with success.
fn usable_helper(helper: &str) -> bool {
    Command::new(helper)
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

/// The failure model behind every package-step stop: `ok: false` carrying the
/// error, what had completed, and the re-switch hint — no auto-rollback; every
/// step is idempotent, so the same switch resumes the work.
fn package_failure(
    target: &str,
    completed_steps: i32,
    warnings: Vec<String>,
    error: String,
) -> Envelope {
    Envelope {
        ok: false,
        data: json!({
            "error": error,
            "completed_steps": completed_steps,
            "resume_hint": format!("switch to `{target}` to restore"),
        }),
        warnings,
    }
}

/// Extracts the conflicting package name from pacman conflict stderr
fn extract_conflicting_package(stderr: &str) -> Option<String> {
    for line in stderr.lines() {
        if !line.contains("are in conflict") {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        let position = parts.iter().position(|&p| p == "::")?;
        if position + 1 < parts.len() {
            return Some(parts[position + 1].to_string());
        }
    }
    None
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

/// Removes a profile directory from the store.
///
/// The active profile refuses without `--force`: deleting the profile the
/// `current` symlink points at would strand the activation. The force flag
/// completes it and clears the active record, so `state.json` stops naming a
/// profile that no longer exists. A profile that is not in the store is a
/// clean failure naming it — a deletion never guesses. Filesystem-only: no
/// external tool is consulted.
fn delete(context: &mut Context, name: &str, force: bool) -> Envelope {
    let mut emitter = Emitter::new("delete");
    context.progress(
        &mut emitter,
        &format!("checking `{name}` is not the active profile"),
    );

    let directory = match context.store.ensure_profile_dir(name) {
        Ok(directory) => directory,
        Err(error) => return Envelope::failed(error),
    };

    let active = context.store.active();
    let is_active = active.name.as_deref() == Some(name);
    if is_active && !force {
        return Envelope::failed(format!(
            "`{name}` is the active profile; delete refuses the active profile \
             unless forced — switch away first, or pass --force to delete it \
             anyway and clear the activation"
        ));
    }

    context.progress(&mut emitter, &format!("removing profile `{name}`"));
    if let Err(error) = std::fs::remove_dir_all(&directory) {
        return Envelope::failed(format!("cannot remove {}: {error}", directory.display()));
    }

    // Deleting the active profile clears the activation record: state.json
    // must stop naming a profile the store no longer holds.
    if is_active {
        context.state.set_active_profile(None);
    }

    Envelope::ok(json!({
        "name": name,
        "force": force,
        "deleted": true,
    }))
}

/// The package and config delta between two profiles, for browsing.
///
/// The same computation `plan` runs for a switch, exposed with neither
/// profile active: packages split official/AUR into adds and removes,
/// managed paths into the paths `b` gains and the paths `a` gains back. No
/// external tool is consulted — a delta is pure manifest arithmetic.
fn diff(context: &mut Context, a: &str, b: &str) -> Envelope {
    let mut emitter = Emitter::new("diff");
    context.progress(
        &mut emitter,
        &format!("reading manifests for `{a}` and `{b}`"),
    );

    let manifest_a = match context.store.load(a) {
        Ok(manifest) => manifest,
        Err(error) => return Envelope::failed(error),
    };
    let manifest_b = match context.store.load(b) {
        Ok(manifest) => manifest,
        Err(error) => return Envelope::failed(error),
    };

    context.progress(&mut emitter, "computing package and config delta");
    // `a` is the current side, `b` the target: the adds are what moving from
    // `a` to `b` gains, exactly as `plan` computes them for a switch.
    let (added_official, added_aur, removed_official, removed_aur) =
        compute_package_diff(&manifest_a.packages, &manifest_b.packages);
    let (config_added, config_removed) =
        compute_symlink_changes(&manifest_a.files, &manifest_b.files);

    // Filesystem-only: a delta is pure manifest arithmetic, so no tool probe.
    Envelope::ok(json!({
        "a": a,
        "b": b,
        "package_delta": {
            "added": { "official": added_official, "aur": added_aur },
            "removed": { "official": removed_official, "aur": removed_aur },
        },
        "config_delta": {
            "added": config_added,
            "removed": config_removed,
        },
    }))
}

/// Moves an image into the shared wallpapers layer.
///
/// A path that does not exist, is not a file, or does not carry an image
/// extension refuses through a failed envelope naming the file — a non-image
/// never reaches the layer. A valid image is moved (renamed, or copied across
/// filesystems) into `~/.local/share/riceswap/wallpapers/`, never silently
/// overwriting a different wallpaper that is already there.
fn wallpaper_import(context: &mut Context, path: &str) -> Envelope {
    let mut emitter = Emitter::new("wallpaper-import");
    context.progress(
        &mut emitter,
        &format!("checking `{path}` is a readable image"),
    );

    let source = PathBuf::from(path);
    if let Err(error) = validate_image(&source) {
        return Envelope::failed(error);
    }
    context.progress(&mut emitter, "importing into the shared wallpapers layer");

    let wallpapers = context.wallpapers_dir();
    if let Err(error) = std::fs::create_dir_all(&wallpapers) {
        return Envelope::failed(format!(
            "cannot open the shared wallpapers layer {}: {error}",
            wallpapers.display()
        ));
    }
    let Some(name) = source.file_name() else {
        return Envelope::failed(format!(
            "{} has no file name to import under",
            source.display()
        ));
    };
    let destination = available_destination(&wallpapers, &source, name);
    if let Err(error) = move_image(&source, &destination) {
        return Envelope::failed(error);
    }

    // Moving a file needs no external tools, so nothing is probed: a broken
    // Hyprland or a missing screenshot tool has no bearing on an import.
    Envelope::ok(json!({
        "source": path,
        "imported_to": destination.display().to_string(),
    }))
}

/// Verifies `source` is an existing, readable image file. Every failure is a
/// message naming the path, so the refusal explains itself.
fn validate_image(source: &Path) -> Result<(), String> {
    let metadata = std::fs::metadata(source).map_err(|error| match error.kind() {
        ErrorKind::NotFound => format!(
            "{} does not exist; wallpaper-import needs a path to an image file",
            source.display()
        ),
        _ => format!("cannot read {}: {error}", source.display()),
    })?;
    if !metadata.is_file() {
        return Err(format!(
            "{} is not a file; wallpaper-import needs one image file",
            source.display()
        ));
    }
    let extension = source
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase);
    if !extension
        .as_deref()
        .is_some_and(|extension| IMAGE_EXTENSIONS.contains(&extension))
    {
        return Err(format!(
            "{} is not an image RiceSwap can import; expected an image file \
             (one of: {})",
            source.display(),
            IMAGE_EXTENSIONS.join(", ")
        ));
    }
    // The extension is only the claim; the leading bytes are the proof. A
    // script or text payload wearing `photo.png` is refused right here.
    let mut header = [0u8; 16];
    let read = {
        let mut file = std::fs::File::open(source)
            .map_err(|error| format!("cannot read {}: {error}", source.display()))?;
        file.read(&mut header)
            .map_err(|error| format!("cannot read {}: {error}", source.display()))?
    };
    if !has_image_signature(&header[..read]) {
        return Err(format!(
            "{} is not an image RiceSwap can import; its contents are not a \
             recognized image format",
            source.display()
        ));
    }
    Ok(())
}

/// Whether the file's leading bytes are a recognized image signature: PNG,
/// JPEG, GIF, BMP, WEBP, AVIF, TIFF, or JXL. This is what refuses a
/// non-image that merely bears an image extension.
fn has_image_signature(header: &[u8]) -> bool {
    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF];
    const TIFF_LITTLE: &[u8] = &[0x49, 0x49, 0x2A, 0x00];
    const TIFF_BIG: &[u8] = &[0x4D, 0x4D, 0x00, 0x2A];
    const JXL_CODESTREAM: &[u8] = &[0xFF, 0x0A];
    const JXL_CONTAINER: &[u8] = b"\x00\x00\x00\x0cJXL \x0d\x0a\x87\x0a";
    const AVIF_BRANDS: &[&[u8]] = &[b"avif", b"avis", b"mif1", b"msf1"];

    if header.starts_with(PNG)
        || header.starts_with(JPEG)
        || header.starts_with(b"GIF8")
        || header.starts_with(b"BM")
        || header.starts_with(TIFF_LITTLE)
        || header.starts_with(TIFF_BIG)
        || header.starts_with(JXL_CODESTREAM)
        || header.starts_with(JXL_CONTAINER)
    {
        return true;
    }
    // WEBP: `RIFF` .... `WEBP`.
    if header.len() >= 12 && header.starts_with(b"RIFF") && &header[8..12] == b"WEBP" {
        return true;
    }
    // AVIF: an ISO BMFF `ftyp` box with an image brand.
    header.len() >= 12 && &header[4..8] == b"ftyp" && AVIF_BRANDS.contains(&&header[8..12])
}

/// Where the image should land: its own file name when free (or already the
/// same image, so the move is a no-op overwrite), otherwise a numbered name —
/// importing must never destroy a wallpaper already in the layer.
fn available_destination(wallpapers: &Path, source: &Path, name: &OsStr) -> PathBuf {
    let destination = wallpapers.join(name);
    if !destination.exists() || same_contents(source, &destination) {
        return destination;
    }
    let stem = source
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "wallpaper".to_string());
    let extension = source
        .extension()
        .map(|extension| extension.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut index = 1;
    loop {
        let candidate_name = if extension.is_empty() {
            format!("{stem}-{index}")
        } else {
            format!("{stem}-{index}.{extension}")
        };
        let candidate = wallpapers.join(candidate_name);
        if !candidate.exists() || same_contents(source, &candidate) {
            return candidate;
        }
        index += 1;
    }
}

/// Whether two files hold the same bytes. A failed read is treated as
/// different: the caller then picks another name rather than risk a loss.
fn same_contents(a: &Path, b: &Path) -> bool {
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

/// Moves `source` onto `destination`, falling back to copy-then-remove when
/// the two sit on different filesystems (e.g. `~/Downloads` on another disk).
fn move_image(source: &Path, destination: &Path) -> Result<(), String> {
    match std::fs::rename(source, destination) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::CrossesDevices => {
            std::fs::copy(source, destination).map_err(|error| {
                format!(
                    "cannot copy {} into {}: {error}",
                    source.display(),
                    destination.display()
                )
            })?;
            std::fs::remove_file(source).map_err(|error| {
                format!(
                    "imported {}, but cannot remove the original: {error}",
                    source.display()
                )
            })
        }
        Err(error) => Err(format!(
            "cannot move {} into {}: {error}",
            source.display(),
            destination.display()
        )),
    }
}

/// Idempotent first-run bootstrap: shared layers, bundled wallpapers,
/// hardware extraction, packaged GUI config.
///
/// Every step is safe to repeat: directories that exist are kept, bundled
/// wallpapers that exist are preserved, the hardware lift only runs while the
/// live Hyprland config has not yet been pointed at the shared file, and the
/// GUI copy only fills in files the user does not already have. A second run
/// therefore reports `created: []` and changes nothing. `state.json` ends with
/// `initialized: true`.
fn init(context: &mut Context) -> Envelope {
    let mut emitter = Emitter::new("init");
    let mut created: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    context.progress(&mut emitter, "creating the shared riceswap layers");
    if let Err(error) = ensure_dir(&context.store.profiles_dir(), &mut created) {
        return Envelope::failed(error);
    }
    if let Err(error) = ensure_dir(&context.wallpapers_dir(), &mut created) {
        return Envelope::failed(error);
    }

    context.progress(&mut emitter, "installing the bundled default wallpapers");
    if let Err(error) = install_bundled_wallpapers(&context.wallpapers_dir(), &mut created) {
        return Envelope::failed(error);
    }

    context.progress(
        &mut emitter,
        "lifting hardware configuration out of hyprland.conf",
    );
    match lift_hardware(context, &mut created) {
        Ok(lifted) => warnings.extend(lifted),
        Err(error) => return Envelope::failed(error),
    }

    context.progress(&mut emitter, "installing the packaged GUI config");
    copy_packaged_gui(&context.user_gui_dir(), &mut created, &mut warnings);

    context.progress(&mut emitter, "the shared layers are ready");
    context.state.set_initialized(true);

    // Bootstrapping is filesystem work: no external tool is consulted, so a
    // machine missing optional tools (an AUR helper, grim) is not warned
    // about — the bootstrap itself was clean.
    let mut envelope = Envelope::ok(json!({
        "initialized": true,
        "created": created,
    }));
    envelope.warnings.extend(warnings);
    envelope
}

/// Creates the directory `path` when missing, recording it in `created`.
/// An existing file where a layer belongs is an error, never a silent skip.
fn ensure_dir(path: &Path, created: &mut Vec<String>) -> Result<(), String> {
    if path.is_dir() {
        return Ok(());
    }
    if path.exists() {
        return Err(format!("{} exists and is not a directory", path.display()));
    }
    std::fs::create_dir_all(path)
        .map_err(|error| format!("cannot create {}: {error}", path.display()))?;
    created.push(path.display().to_string());
    Ok(())
}

/// Installs the four bundled defaults into the wallpaper layer: from the
/// packaged location when it provides them, from the embedded copies
/// otherwise. Existing files are preserved — re-running changes nothing.
fn install_bundled_wallpapers(wallpapers: &Path, created: &mut Vec<String>) -> Result<(), String> {
    let packaged = Path::new(PACKAGED_WALLPAPERS);
    for &(name, bytes) in BUNDLED_WALLPAPERS {
        let destination = wallpapers.join(name);
        if destination.exists() {
            continue;
        }
        let from_packaged = packaged.join(name);
        if !(from_packaged.is_file() && std::fs::copy(&from_packaged, &destination).is_ok()) {
            // The packaged copy was absent or failed part-way: the embedded
            // bytes are the guarantee that all four defaults always exist.
            let _ = std::fs::remove_file(&destination);
            std::fs::write(&destination, bytes).map_err(|error| {
                format!(
                    "cannot install the bundled wallpaper {}: {error}",
                    destination.display()
                )
            })?;
        }
        created.push(destination.display().to_string());
    }
    Ok(())
}

/// Lifts `monitor=`, GPU `env =`, and `input { ... }` configuration out of
/// the live Hyprland config into the shared hardware file, then leaves the
/// `source =` line behind — even when there was nothing to lift — so the live
/// config inherits the hardware layer through the permanent symlink. When the
/// live config already sources the shared file, the lift has happened and
/// this changes nothing — the idempotency of a second run.
///
/// Returns warnings for anything skipped (an unreadable live config), or an
/// error for anything that should exist but could not be written.
fn lift_hardware(context: &Context, created: &mut Vec<String>) -> Result<Vec<String>, String> {
    let mut warnings = Vec::new();
    let live = context.hyprland_config();
    match std::fs::read_to_string(&live) {
        Ok(config) => {
            if !config.lines().any(sources_hardware) {
                let (remaining, lifted) = extract_hardware(&config);
                if !lifted.is_empty() {
                    let mut content = String::from(HARDWARE_HEADER);
                    content.push_str("\n\n");
                    for line in &lifted {
                        content.push_str(line);
                        content.push('\n');
                    }
                    write_new(&context.hardware_file(), &content, created)?;
                }
                // Whether or not anything was lifted, the live config ends up
                // sourcing the shared hardware file: a config without hardware
                // directives still belongs to the hardware layer, and its
                // future edits there must reach every profile.
                let mut rewritten = remaining.join("\n");
                if !rewritten.is_empty() {
                    rewritten.push_str("\n\n");
                }
                rewritten.push_str(HARDWARE_SOURCE);
                rewritten.push('\n');
                std::fs::write(&live, rewritten).map_err(|error| {
                    format!(
                        "cannot connect {} to the shared hardware file: {error}",
                        live.display()
                    )
                })?;
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            // A machine that has never configured Hyprland: there is nothing
            // to lift, but the shared file and symlink still come into being.
        }
        Err(error) => warnings.push(format!(
            "cannot read {}: {error}; the hardware configuration was not lifted",
            live.display()
        )),
    }

    let hardware = context.hardware_file();
    if !hardware.exists() {
        write_new(&hardware, &format!("{HARDWARE_HEADER}\n"), created)?;
    }
    ensure_hardware_link(context, created)?;
    Ok(warnings)
}

/// Writes `content` to `path`, recording the path in `created` when it is a
/// new file. Any failure names the file: the hardware layer is not optional.
fn write_new(path: &Path, content: &str, created: &mut Vec<String>) -> Result<(), String> {
    let fresh = !path.exists();
    std::fs::write(path, content)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    if fresh {
        created.push(path.display().to_string());
    }
    Ok(())
}

/// Creates the permanent hardware symlink under the Hyprland config dir, or
/// verifies it already points at the shared hardware file. A symlink left
/// pointing elsewhere is repointed; a regular file squatting on the path is an
/// error naming it, because silently deleting a user's file is not RiceSwap's
/// place.
fn ensure_hardware_link(context: &Context, created: &mut Vec<String>) -> Result<(), String> {
    let link = context.hardware_link();
    let target = context.hardware_file();
    if link.is_symlink() {
        match std::fs::read_link(&link) {
            Ok(existing) if existing == target => return Ok(()),
            Ok(_) => std::fs::remove_file(&link).map_err(|error| {
                format!(
                    "cannot repoint the hardware symlink {}: {error}",
                    link.display()
                )
            })?,
            Err(error) => {
                return Err(format!(
                    "cannot read the hardware symlink {}: {error}",
                    link.display()
                ));
            }
        }
    } else if link.exists() {
        return Err(format!(
            "{} exists and is not the RiceSwap hardware symlink; move it aside \
             and re-run init",
            link.display()
        ));
    }
    let Some(parent) = link.parent() else {
        return Err(format!("{} has no parent directory", link.display()));
    };
    ensure_dir(parent, created)?;
    std::os::unix::fs::symlink(&target, &link).map_err(|error| {
        format!(
            "cannot create the hardware symlink {}: {error}",
            link.display()
        )
    })?;
    created.push(link.display().to_string());
    Ok(())
}

/// Whether a config line already sources the hardware layer — the marker that
/// the lift happened, and that a re-run must change nothing.
fn sources_hardware(line: &str) -> bool {
    let Some(rest) = line.trim_start().strip_prefix("source") else {
        return false;
    };
    let rest = rest.trim_start();
    rest.starts_with('=') && rest.contains("hypr/riceswap/hardware.conf")
}

/// What a top-level line configures, for the hardware lift.
enum Hardware {
    /// A one-line directive: `monitor=...`, `env = ...`.
    Directive,
    /// A brace block: `input { ... }`.
    Block,
}

/// The value after `name =` / `name=`, when the line is a directive for
/// `name` — and only then: `envvar = x` is not an `env` directive.
fn directive_value<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(name)?.trim_start();
    Some(rest.strip_prefix('=')?.trim_start())
}

/// `Some(kind)` for the lines that belong in the shared hardware file:
/// `monitor=` display rules, GPU/driver `env =` variables, and the `input`
/// block. Comments, theming env, and everything rice-specific stay put.
fn hardware_line(line: &str) -> Option<Hardware> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return None;
    }
    if directive_value(trimmed, "monitor").is_some() {
        return Some(Hardware::Directive);
    }
    if let Some(value) = directive_value(trimmed, "env") {
        let key = value.split(',').next().unwrap_or(value).trim();
        return GPU_ENV_KEYS.contains(&key).then_some(Hardware::Directive);
    }
    let rest = trimmed.strip_prefix("input")?.trim_start();
    (rest.starts_with('{') || rest.is_empty()).then_some(Hardware::Block)
}

/// Splits a config into what stays in `hyprland.conf` and the hardware lines
/// to lift, preserving both verbatim (block indentation included) so the
/// lifted configuration parses exactly as it did in place.
fn extract_hardware(config: &str) -> (Vec<&str>, Vec<&str>) {
    let mut remaining = Vec::new();
    let mut lifted = Vec::new();
    let mut lines = config.lines();
    while let Some(line) = lines.next() {
        match hardware_line(line) {
            None => remaining.push(line),
            Some(Hardware::Directive) => lifted.push(line),
            Some(Hardware::Block) => {
                lifted.push(line);
                let mut depth = 0;
                let mut braced = false;
                count_braces(line, &mut depth, &mut braced);
                // Keep taking lines until the block closes. An `input` with its
                // brace on a later line has not opened yet, so it keeps going.
                while !(braced && depth <= 0) {
                    let Some(next) = lines.next() else {
                        break;
                    };
                    count_braces(next, &mut depth, &mut braced);
                    lifted.push(next);
                }
            }
        }
    }
    (remaining, lifted)
}

/// Folds one line's braces into the running block depth: `depth` moves with
/// every `{`/`}`, and `braced` records that any brace was seen at all — the
/// signal that a block has opened (and so can close).
fn count_braces(line: &str, depth: &mut i32, braced: &mut bool) {
    for character in line.chars() {
        match character {
            '{' => {
                *depth += 1;
                *braced = true;
            }
            '}' => {
                *depth -= 1;
                *braced = true;
            }
            _ => {}
        }
    }
}

/// Copies the packaged Quickshell GUI config into the user's config, filling
/// in only files the user does not already have — their customizations
/// survive every re-run. An absent packaged config is normal (nothing to copy),
/// not a failure; a present-but-unreadable one is a warning, because the GUI
/// layer must not take the whole bootstrap down with it.
fn copy_packaged_gui(user_gui: &Path, created: &mut Vec<String>, warnings: &mut Vec<String>) {
    let packaged = Path::new(PACKAGED_GUI);
    if !packaged.is_dir() {
        return;
    }
    if let Err(error) = copy_missing(packaged, user_gui, created) {
        warnings.push(format!(
            "cannot install the packaged GUI config into {}: {error}",
            user_gui.display()
        ));
    }
}

/// Recursively copies `source` into `destination`, creating directories and
/// copying only files that are not there yet.
fn copy_missing(
    source: &Path,
    destination: &Path,
    created: &mut Vec<String>,
) -> Result<(), String> {
    ensure_dir(destination, created)?;
    let entries = std::fs::read_dir(source)
        .map_err(|error| format!("cannot read {}: {error}", source.display()))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("cannot read an entry of {}: {error}", source.display()))?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|error| format!("cannot inspect {}: {error}", from.display()))?;
        if file_type.is_dir() {
            copy_missing(&from, &to, created)?;
        } else if !to.exists() {
            std::fs::copy(&from, &to)
                .map_err(|error| format!("cannot copy {}: {error}", from.display()))?;
            created.push(to.display().to_string());
        }
    }
    Ok(())
}

/// Computes package diff: (install_official, install_aur, remove_official, remove_aur)
fn compute_package_diff(
    current: &Packages,
    target: &Packages,
) -> (Vec<String>, Vec<String>, Vec<String>, Vec<String>) {
    let install_official: Vec<String> = target
        .official
        .iter()
        .filter(|p| !current.official.contains(p))
        .cloned()
        .collect();
    let install_aur: Vec<String> = target
        .aur
        .iter()
        .filter(|p| !current.aur.contains(p))
        .cloned()
        .collect();
    let remove_official: Vec<String> = current
        .official
        .iter()
        .filter(|p| !target.official.contains(p))
        .cloned()
        .collect();
    let remove_aur: Vec<String> = current
        .aur
        .iter()
        .filter(|p| !target.aur.contains(p))
        .cloned()
        .collect();
    (install_official, install_aur, remove_official, remove_aur)
}

/// The shell change a switch implies: (stop, start).
///
/// The rule is name equality, because the name is the only thing that says
/// "same shell". Two profiles both carrying a `qs` service whose command is the
/// identical string `qs -c $qsConfig` are two different shells wearing one
/// service name; comparing services alone says nothing changed and leaves the
/// old shell running under the new one's config. Comparing the shells says
/// `ii` and `caelestia` differ, which is the truth.
///
/// `stop` is the shell to end, `start` the one to begin. Each is `None` when
/// that half is not called for.
fn compute_shell_change<'a>(
    current: Option<&'a Shell>,
    target: Option<&'a Shell>,
) -> (Option<&'a Shell>, Option<&'a Shell>) {
    match (current, target) {
        // No active profile: nothing is known to be running, so the target's
        // shell starts — the same call the service diff makes with no `current`.
        (None, target) => (None, target),
        // The target claims no shell. Ending the old one would leave a desktop
        // with none, so it stays: a profile that does not name a shell has no
        // opinion about the one running.
        (Some(_), None) => (None, None),
        (Some(current), Some(target)) if current.name == target.name => (None, None),
        (Some(current), Some(target)) => (Some(current), Some(target)),
    }
}

/// Computes service changes: (stop, start)
fn compute_service_changes(current: &[Service], target: &[Service]) -> (Vec<String>, Vec<String>) {
    let current_names: std::collections::HashSet<&str> =
        current.iter().map(|s| s.name.as_str()).collect();
    let target_names: std::collections::HashSet<&str> =
        target.iter().map(|s| s.name.as_str()).collect();

    let stop: Vec<String> = current
        .iter()
        .filter(|s| !target_names.contains(s.name.as_str()))
        .map(|s| s.name.clone())
        .collect();
    let start: Vec<String> = target
        .iter()
        .filter(|s| !current_names.contains(s.name.as_str()))
        .map(|s| s.name.clone())
        .collect();
    (stop, start)
}

/// Computes symlink changes: (link, unlink). `link` is B's managed paths (the
/// full set, so a re-run re-pointing an existing link is idempotent); `unlink`
/// is A's paths B no longer manages.
fn compute_symlink_changes(
    current: &[FileEntry],
    target: &[FileEntry],
) -> (Vec<String>, Vec<String>) {
    let target_paths: std::collections::HashSet<&str> =
        target.iter().map(|f| f.path.as_str()).collect();

    let mut link: Vec<String> = target.iter().map(|f| f.path.clone()).collect();
    link.sort();

    let unlink: Vec<String> = current
        .iter()
        .filter(|f| !target_paths.contains(f.path.as_str()))
        .map(|f| f.path.clone())
        .collect();
    (link, unlink)
}

/// The packages in `wanted` that this machine does not have, per `pacman -Qq`.
///
/// `plan` reports the raw manifest difference, which on a first switch is every
/// package the target declares. Intersecting with what is actually installed
/// is what turns "this profile declares 20 packages" into "this machine is
/// missing 5", which is the number worth putting in front of someone before
/// they authorise a switch.
///
/// The query runs under plain `pacman`, never the `OFFICIAL` prefix: `plan` is
/// a preview, and a preview that asks for a password is not a preview. A query
/// pacman cannot answer leaves the set alone — the manifest difference stays
/// the conservative answer.
fn missing_from_machine(wanted: &[String]) -> Vec<String> {
    if wanted.is_empty() {
        return Vec::new();
    }
    let Ok(output) = run_command(&[Tool::Pacman.name()], &["-Qq"]) else {
        return wanted.to_vec();
    };
    if !output.status.success() {
        return wanted.to_vec();
    }
    let listing = String::from_utf8_lossy(&output.stdout).into_owned();
    let installed: std::collections::HashSet<&str> = listing
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    wanted
        .iter()
        .filter(|package| !installed.contains(package.as_str()))
        .cloned()
        .collect()
}

/// The real files standing at the paths a switch would link, split by what the
/// switch would do about each one.
struct BlockedPaths {
    /// Byte-for-byte the same as the profile's own copy: replacing it loses
    /// nothing, so the switch says nothing and links it.
    identical: Vec<String>,
    /// Genuinely different: the switch copies these into the target profile's
    /// `backups/` before it replaces them, and the report names each one.
    backed_up: Vec<String>,
}

/// Classifies the real files sitting at the paths a switch would link.
///
/// A real file at a link target used to abort the whole switch, which made any
/// profile un-switchable on a machine that had never switched — and every real
/// machine has a `.bashrc` and a `.zshrc`. Nothing is clobbered either way: a
/// file that matches the profile byte for byte carries no information the
/// profile does not already hold, and one that differs is preserved under
/// `backups/` first.
fn classify_blocked_paths(home: &Path, profile: &Path, paths: &[String]) -> BlockedPaths {
    let mut identical = Vec::new();
    let mut backed_up = Vec::new();
    for relative in paths {
        let live = home.join(relative);
        let Ok(metadata) = std::fs::symlink_metadata(&live) else {
            continue; // Absent: nothing stands in the way.
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            continue; // Already managed, or a directory this handles elsewhere.
        }
        let (Ok(mine), Ok(theirs)) = (std::fs::read(&live), std::fs::read(profile.join(relative)))
        else {
            // The profile has no copy to compare against, so the difference is
            // unprovable: treat it as something worth keeping.
            backed_up.push(relative.clone());
            continue;
        };
        if mine == theirs {
            identical.push(relative.clone());
        } else {
            backed_up.push(relative.clone());
        }
    }
    BlockedPaths {
        identical,
        backed_up,
    }
}

/// Every real file at a link target, whatever the switch would do with it: the
/// blocked set `plan` reports, in path order.
fn blocked_paths_of(classified: &BlockedPaths) -> Vec<String> {
    let mut all: Vec<String> = classified
        .identical
        .iter()
        .chain(&classified.backed_up)
        .cloned()
        .collect();
    all.sort();
    all
}

/// Moves a path the new profile no longer manages into the *leaving* profile's
/// `backups/`, so the switch gives the path back instead of deleting it.
///
/// The bytes already exist — inside the leaving profile, which is what the path
/// is currently a symlink to — so what is being preserved is the right to have
/// them back at this path without a second profile copy. A symlink is recreated
/// under the backup name pointing at its own profile copy; a real file or
/// directory is moved, recursively, so a profile that was edited in place since
/// it was captured is preserved as edited rather than as the stale capture.
///
/// `now_rfc3339` names the collision suffix, so returning to a profile twice
/// keeps both generations.
fn preserve_managed_path(backups: &Path, relative: &str, live: &Path) -> Result<(), String> {
    let Ok(metadata) = std::fs::symlink_metadata(live) else {
        return Ok(()); // Absent: nothing to preserve, nothing to unlink.
    };
    let mut destination = backups.join(relative);
    let parent = destination
        .parent()
        .ok_or_else(|| format!("the backup path for `{relative}` has no parent directory"))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;

    if destination.exists() || std::fs::symlink_metadata(&destination).is_ok() {
        let stem = destination
            .file_stem()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| relative.to_string());
        let extension = destination
            .extension()
            .map(|name| format!(".{}", name.to_string_lossy()))
            .unwrap_or_default();
        destination = parent.join(format!("{stem}.{}.bak{extension}", now_rfc3339()));
    }

    if metadata.file_type().is_symlink() {
        // A link to the leaving profile's own copy: recreating it under
        // `backups/` keeps those bytes reachable, and a plain `remove_file`
        // would have discarded the pointer to them.
        std::os::unix::fs::symlink(
            std::fs::read_link(live)
                .map_err(|error| format!("cannot read the link at {}: {error}", live.display()))?,
            &destination,
        )
        .map_err(|error| {
            format!(
                "cannot store the link at {}: {error}",
                destination.display()
            )
        })?;
        return std::fs::remove_file(live)
            .map_err(|error| format!("cannot remove {}: {error}", live.display()));
    }

    if metadata.is_dir() {
        return move_directory(&destination, live);
    }
    std::fs::rename(live, &destination).map_err(|error| {
        format!(
            "cannot move {} to {}: {error}",
            live.display(),
            destination.display()
        )
    })
}

/// Moves a directory and everything under it, creating the destination as it
/// goes, so a half-finished move leaves the source intact rather than a
/// truncated tree in both places.
fn move_directory(destination: &Path, source: &Path) -> Result<(), String> {
    let mut entries: Vec<_> = std::fs::read_dir(source)
        .map_err(|error| format!("cannot read {}: {error}", source.display()))?
        .collect::<Result<_, _>>()
        .map_err(|error| format!("cannot read {}: {error}", source.display()))?;
    entries.sort_by_key(|entry| entry.file_name());
    std::fs::create_dir_all(destination)
        .map_err(|error| format!("cannot create {}: {error}", destination.display()))?;
    for entry in entries {
        let from = entry.path();
        let to = destination.join(entry.file_name());
        let metadata = std::fs::symlink_metadata(&from)
            .map_err(|error| format!("cannot inspect {}: {error}", from.display()))?;
        if metadata.file_type().is_symlink() {
            std::os::unix::fs::symlink(
                std::fs::read_link(&from).map_err(|error| {
                    format!("cannot read the link at {}: {error}", from.display())
                })?,
                &to,
            )
            .map_err(|error| format!("cannot store the link at {}: {error}", to.display()))?;
            std::fs::remove_file(&from)
                .map_err(|error| format!("cannot remove {}: {error}", from.display()))?;
        } else if metadata.is_dir() {
            move_directory(&to, &from)?;
        } else {
            std::fs::rename(&from, &to).map_err(|error| {
                format!(
                    "cannot move {} to {}: {error}",
                    from.display(),
                    to.display()
                )
            })?;
        }
    }
    std::fs::remove_dir(source)
        .map_err(|error| format!("cannot remove {}: {error}", source.display()))
}

/// Copies a real file the switch is about to replace into the target profile's
/// `backups/`, so the bytes survive the switch.
///
/// The copy is staged beside its destination and renamed into place, the same
/// staged-then-rename primitive `Store::flip` and `link_managed_path` use, so a
/// crash mid-write cannot leave a truncated file where the user's config used to
/// be. `now_rfc3339` names a collision suffix, so a second switch over the same
/// path keeps both copies instead of overwriting the first.
fn backup_managed_file(backups: &Path, relative: &str, live: &Path) -> Result<PathBuf, String> {
    let mut destination = backups.join(relative);
    let parent = destination
        .parent()
        .ok_or_else(|| format!("the backup path for `{relative}` has no parent directory"))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;

    if destination.exists() {
        let stem = destination
            .file_stem()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| relative.to_string());
        let extension = destination
            .extension()
            .map(|name| format!(".{}", name.to_string_lossy()))
            .unwrap_or_default();
        // Two switches over the same path, minutes apart: keep both.
        destination = parent.join(format!("{stem}.{}.bak{extension}", now_rfc3339()));
    }

    let staged = staged_backup_path(&destination);
    let _ = std::fs::remove_file(&staged);
    std::fs::copy(live, &staged).map_err(|error| {
        format!(
            "cannot copy {} to {}: {error}",
            live.display(),
            staged.display()
        )
    })?;
    if let Err(error) = std::fs::rename(&staged, &destination) {
        let _ = std::fs::remove_file(&staged);
        return Err(format!(
            "cannot store the backup at {}: {error}",
            destination.display()
        ));
    }
    Ok(destination)
}

/// The scratch name a backup is written under before it is renamed into place.
fn staged_backup_path(destination: &Path) -> PathBuf {
    let mut name = destination.as_os_str().to_os_string();
    name.push(".riceswap-staged");
    PathBuf::from(name)
}

/// Clears a managed path the active profile owned and the target no longer does,
/// so the switch leaves nothing of the old rice behind.
///
/// A symlink is simply unlinked. A real **directory** is the profile's own copy of
/// a `$HOME` path — the shape every captured config dir has — and is removed in
/// full: the live tree is mid-switch, and leaving a half-old config in place is
/// worse than replacing it. A real **file** is user data that no profile claims, so
/// it refuses rather than deleting it. Anything that cannot be inspected is left
/// alone: an absent path is already clear.
fn clear_managed_path(path: &Path) -> Result<(), String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
    };
    if metadata.file_type().is_symlink() || metadata.is_file() {
        return std::fs::remove_file(path)
            .map_err(|error| format!("cannot remove {}: {error}", path.display()));
    }
    if metadata.is_dir() {
        return std::fs::remove_dir_all(path)
            .map_err(|error| format!("cannot remove {}: {error}", path.display()));
    }
    // A fifo or socket holds nothing a config switch needs to clear.
    Ok(())
}

/// Points `link` at the target profile's copy of a managed path.
///
/// The link is staged beside its destination and renamed into place, the same
/// staged-then-rename primitive `Store::flip` uses for the `current` symlink, so a
/// watcher never observes a half-linked config. The destination is cleared first:
/// a path the old profile left as a real directory has to be replaced wholesale,
/// which `remove_file` alone cannot do.
fn link_managed_path(link: &Path, target: &Path) -> Result<(), String> {
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    clear_managed_path(link)?;
    let staged = staged_link_path(link);
    let _ = std::fs::remove_file(&staged);
    std::os::unix::fs::symlink(target, &staged)
        .map_err(|error| format!("cannot create {}: {error}", staged.display()))?;
    if let Err(error) = std::fs::rename(&staged, link) {
        let _ = std::fs::remove_file(&staged);
        return Err(format!(
            "cannot point {} at the profile: {error}",
            link.display()
        ));
    }
    Ok(())
}

/// The staging path a managed link is built at before being renamed onto its
/// destination: the destination with a `.riceswap-staged` suffix, in the same
/// directory so the rename stays within one filesystem.
fn staged_link_path(link: &Path) -> PathBuf {
    let mut name = link.as_os_str().to_os_string();
    name.push(".riceswap-staged");
    PathBuf::from(name)
}

fn describe(status: &tools::ToolStatus) -> String {
    match (&status.error, status.exit_code) {
        (Some(error), _) if !status.available => error.clone(),
        (Some(error), Some(code)) => format!("exited {code}: {error}"),
        (_, Some(code)) => format!("exited {code}"),
        _ => "no output".to_string(),
    }
}
