//! The ten operations.
//!
//! The profile-store operations are real: `list` and `info` read manifests
//! through the store, and `switch` verifies its target then flips the `current`
//! symlink — the activation primitive later tickets build on. `init` is the
//! idempotent first-run bootstrap: it creates the shared layers, installs the
//! four bundled wallpapers, lifts the hardware configuration out of the live
//! Hyprland config into the shared hardware file, and copies the packaged GUI
//! config when there is one. `wallpaper-import` moves an image into the shared
//! wallpapers layer. The remaining operations are honest stubs: they stream
//! their real progress lines, probe the external tools their real
//! implementation will need, and return the real envelope with placeholder data
//! marked `"stub": true`. Snapshot pipelines, switch sequences, and package
//! operations arrive in later tickets.

use crate::cli::Invocation;
use crate::envelope::{Emitter, Envelope};
use crate::profile::Store;
use crate::state::StateStore;
use crate::tools::{self, Tool};
use serde_json::{Value, json};
use std::ffi::OsStr;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};

/// The tools a package-touching operation depends on.
const PACKAGE_MANAGERS: &[Tool] = &[Tool::Pacman, Tool::Yay, Tool::Paru];

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

/// The file extensions `wallpaper-import` accepts. The extension is the
/// contract: an image the user downloaded is recognized by its name, and a
/// non-image never reaches the wallpaper layer.
const IMAGE_EXTENSIONS: &[&str] = &[
    "avif", "bmp", "gif", "jpe", "jpeg", "jpg", "jxl", "png", "tif", "tiff", "webp",
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

/// One line describing why a tool is unusable.
fn describe(status: &tools::ToolStatus) -> String {
    match (&status.error, status.exit_code) {
        (Some(error), _) if !status.available => error.clone(),
        (Some(error), Some(code)) => format!("exited {code}: {error}"),
        (_, Some(code)) => format!("exited {code}"),
        _ => "no output".to_string(),
    }
}
