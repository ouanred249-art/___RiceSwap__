//! Snapshot pre-flight detection: what the desktop is made of.
//!
//! `detect` proposes the candidates the user confirms before `snapshot`
//! writes a profile, and `snapshot` re-runs the same scan to capture them:
//! config directories from the curated allowlist merged with a
//! format-agnostic scan of the Hyprland config (both the `hyprland.conf` and
//! `hyprland.lua` entrypoints, with lua `source` refs and `hl.exec_cmd`
//! startup lines included),
//! packages found by a binary-reference scan over the detected configs and
//! resolved through `pacman -Qo` (split official/AUR via `pacman -Qm`),
//! fonts, icons, and themes from the standard asset locations, and image
//! files in `~/Downloads` waiting to become wallpaper imports.
//!
//! Detection only reads the filesystem and shells out to `pacman` — it never
//! warns: a machine without pacman simply proposes no packages, and the tool
//! probes the envelope attaches report the rest.

use crate::profile::Service;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The file extensions a wallpaper candidate (and `wallpaper-import`)
/// carries. The extension is the contract: an image the user downloaded is
/// recognized by its name, and a non-image never becomes a candidate.
pub const IMAGE_EXTENSIONS: &[&str] = &[
    "avif", "bmp", "gif", "jpe", "jpeg", "jpg", "jxl", "png", "tif", "tiff", "webp",
];

/// The curated allowlist of known rice dirs under `~/.config/`, versioned
/// with releases: anything here is proposed whenever it exists on disk.
const CONFIG_ALLOWLIST: &[&str] = &[
    "ags",
    "bash",
    "cava",
    "dunst",
    "fish",
    "foot",
    "fontconfig",
    "fzf",
    "fuzzel",
    "gtk-3.0",
    "gtk-4.0",
    "hypr",
    "hypridle",
    "hyprlock",
    "hyprpaper",
    "kitty",
    "mako",
    "matugen",
    "nushell",
    "quickshell",
    "rofi",
    "starship",
    "swaync",
    "swww",
    "waybar",
    "wlogout",
    "wofi",
    "zsh",
];

/// The standard asset locations outside `~/.config` whose contents belong to
/// the rice: fonts, icons, themes, and color schemes are proposed the same
/// propose-and-confirm way config dirs are.
const ASSET_DIRS: &[&str] = &[
    ".local/share/fonts",
    ".local/share/icons",
    ".local/share/themes",
    ".local/share/color-schemes",
];

/// The lua module names a line requires: `require("hyprland.execs")` and the
/// `dofile`/`loadfile` spellings. Only a quoted, dot-separated module name
/// counts — a bare word in a comment or a string that is clearly a path is
/// left to `path_references`, which already handles it.
fn module_references(line: &str) -> Vec<String> {
    let mut modules = Vec::new();
    for marker in ["require", "dofile", "loadfile"] {
        let mut search = 0usize;
        while let Some(found) = line[search..].find(marker) {
            let index = search + found;
            search = index + marker.len();
            // A module name, not a longer identifier ending in the marker.
            if index > 0 {
                let previous = line.as_bytes()[index - 1];
                if previous.is_ascii_alphanumeric() || previous == b'_' || previous == b'.' {
                    continue;
                }
            }
            let rest = line[search..].trim_start();
            let Some(rest) = rest.strip_prefix('(').map(str::trim_start) else {
                continue;
            };
            let Some(rest) = rest.strip_prefix('"') else {
                continue;
            };
            let Some(end) = rest.find('"') else {
                continue;
            };
            let name = &rest[..end];
            // A path is a path: `dofile(HOME .. "/x.lua")` and friends are
            // already handled as references, and a module name has no
            // separator in it.
            if name.is_empty() || name.contains('/') || name.starts_with('.') {
                continue;
            }
            if !name.split('.').all(|part| !part.is_empty()) {
                continue;
            }
            modules.push(name.to_string());
        }
    }
    modules
}

/// Resolves a lua module name to the file lua itself would load it from.
///
/// Lua resolves `require("hyprland.execs")` by searching the config tree, so
/// the module's dotted path is tried relative to the requiring file and then
/// under the roots a Hyprland lua config actually uses: the config dir itself,
/// and its `hypr` subdirectory (which is where `require("hyprland.execs")`
/// resolves from `~/.config/hypr/hyprland.lua`). `.lua` is the extension
/// unless the name already carries one.
///
/// The first path segment is also tried dropped: a config that lives in
/// `~/.config/hypr/` and spells its modules `require("hyprland.execs")` keeps
/// them in a `hyprland/` subdirectory of that same root, so the name resolves
/// there only once the leading segment is removed.
fn resolve_lua_module(home: &Path, requiring: &Path, module: &str) -> Option<PathBuf> {
    let parts: Vec<&str> = module.split('.').collect();
    let mut relatives = vec![module.replace('.', "/")];
    if parts.len() > 1 {
        relatives.push(parts[1..].join("/"));
    }
    let config = home.join(".config");
    let hypr = config.join("hypr");
    let mut roots = Vec::new();
    if let Some(parent) = requiring.parent() {
        roots.push(parent.to_path_buf());
    }
    roots.push(config);
    roots.push(hypr.clone());
    roots.push(hypr.join("hyprland"));
    for root in roots {
        for relative in &relatives {
            let stems = if Path::new(relative).extension().is_some() {
                vec![relative.clone()]
            } else {
                vec![format!("{relative}.lua"), format!("{relative}/init.lua")]
            };
            for stem in stems {
                let candidate = root.join(&stem);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

/// The `$HOME`-relative **files** a rice owns, which the directory allowlist
/// above can never propose because it only walks directories. A prompt or a
/// shell's rc file is a single file at a known name — `starship` keeps its
/// config in `.config/starship.toml`, not in a `.config/starship/` directory —
/// and a profile that captured the terminal but not the prompt is half a rice.
const CONFIG_FILE_ALLOWLIST: &[&str] = &[
    ".bash_profile",
    ".bashrc",
    ".profile",
    ".zprofile",
    ".zshenv",
    ".zshrc",
    ".config/fish/config.fish",
    ".config/starship.toml",
    ".config/bashrc",
    ".config/zsh/.zshrc",
];

/// The separators a reference or command token may be wrapped in. Splitting
/// on all of them is what makes the Hyprland scan format-agnostic: a path
/// surfaces the same way from `source = ~/...`, from JSON `"path": "~/..."`,
/// and from lua `dofile(HOME .. "/...")`.
const TOKEN_DELIMS: &[char] = &[
    ' ', '\t', '"', '\'', '`', '(', ')', ',', ';', '=', '{', '}', '[', ']', '<', '>', '|', '&',
    '\r',
];

/// The directives whose referenced file is configuration to read in turn —
/// Hyprland `source`, JSON/ini `include`, and the lua spellings.
const SOURCE_MARKERS: &[&str] = &[
    "source", "include", "import", "require", "dofile", "loadfile",
];

/// The file extensions that mark a referenced file as configuration worth
/// reading for further references, even when no source directive wraps it.
const CONFIG_EXTENSIONS: &[&str] = &[
    "conf", "config", "css", "ini", "json", "jsonc", "less", "lua", "sass", "scss", "toml", "xml",
    "yaml", "yml",
];

/// The line keys that introduce a command: the binary-reference scan reads
/// the first non-flag token after any of these as a candidate executable.
const COMMAND_MARKERS: &[&str] = &[
    "command",
    "cmd",
    "exec",
    "exec-once",
    "exec-shutdown",
    "launch",
    "on-click",
    "on-click-middle",
    "on-click-right",
    "on-scroll-down",
    "on-scroll-up",
    "on-update",
    "run",
    "spawn",
    "startup",
];

/// How deep the Hyprland reference scan follows referenced configs, and how
/// many files it will read — a rice's source chain is shallow, and anything
/// deeper is not a config tree RiceSwap should crawl.
const MAX_FOLLOW_DEPTH: usize = 8;
const MAX_FOLLOWED_FILES: usize = 64;

/// The size ceiling for a config file the scans will read: bigger files are
/// data, not configuration, and reading them would stall the operation.
const MAX_FILE_BYTES: u64 = 1_048_576;

/// How deep the per-directory walks (package scan, Downloads scan) recurse.
const MAX_SCAN_DEPTH: usize = 10;

/// What `scan_config` found: the proposed config directories (`$HOME`-relative,
/// sorted) and the services declared through `exec-once` lines in the
/// Hyprland source chain.
pub struct ConfigScan {
    pub dirs: Vec<String>,
    pub services: Vec<Service>,
}

/// What `scan_config`'s binary-reference scan resolved, split the way the
/// manifest's `[packages]` table splits it.
pub struct PackageScan {
    pub official: Vec<String>,
    pub aur: Vec<String>,
}

/// Proposes the config directories to capture: the allowlist merged with
/// whatever the Hyprland config references — dirs included, `source` chains
/// (lua files included) followed.
pub fn scan_config(home: &Path) -> ConfigScan {
    let mut dirs = BTreeSet::new();
    if let Ok(entries) = std::fs::read_dir(home.join(".config")) {
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !CONFIG_ALLOWLIST.contains(&name.as_str()) {
                continue;
            }
            if entry.path().is_dir() {
                for candidate in shell_candidates(&entry.path(), &name) {
                    insert_candidate(&mut dirs, candidate);
                }
            }
        }
    }
    // The single-file half of the allowlist, proposed the same way: present on
    // disk, so it belongs to the rice. `insert_candidate`, not `insert`: a raw
    // insert would let a file through beside the directory that already
    // contains it (`.config/fish` *and* `.config/fish/config.fish`), and a
    // manifest holding both a parent and its own child can never be switched
    // — the child blocks the parent, then destroys it.
    for relative in CONFIG_FILE_ALLOWLIST {
        if home.join(relative).is_file() {
            insert_candidate(&mut dirs, (*relative).to_string());
        }
    }
    let mut services = BTreeMap::new();
    follow_hyprland_refs(home, &mut dirs, &mut services);
    ConfigScan {
        dirs: dirs.into_iter().collect(),
        services: services.into_values().collect(),
    }
}

/// Resolves every command referenced in the detected configs to its owning
/// package via `pacman -Qo`, then splits the owners by `pacman -Qm`: what
/// `pacman -Qm` lists is AUR, the rest is official.
///
/// The config dir's own name counts as a reference too: `~/.config/matugen`
/// is matugen's config whether or not any file inside it spells the binary,
/// so the dir proposes its package the same way an `exec-once` line does —
/// the owner decides whether a binary of that name actually exists.
pub fn scan_packages(home: &Path, dirs: &[String]) -> PackageScan {
    let mut commands = BTreeSet::new();
    for dir in dirs {
        if let Some(tool) = tool_named(dir) {
            commands.insert(tool);
        }
        collect_commands(&home.join(dir), 0, &mut commands);
    }
    let mut owners = BTreeSet::new();
    for command in &commands {
        if let Some(package) = package_owning(home, command) {
            owners.insert(package);
        }
    }
    if owners.is_empty() {
        return PackageScan {
            official: Vec::new(),
            aur: Vec::new(),
        };
    }
    let foreign = foreign_packages();
    let (aur, official): (Vec<String>, Vec<String>) = owners
        .into_iter()
        .partition(|package| foreign.contains(package));
    PackageScan { official, aur }
}

/// The tool a detected config dir is named after: `.config/matugen` is
/// matugen's dir. A dir not under `.config` names no tool.
fn tool_named(dir: &str) -> Option<String> {
    let name = dir
        .strip_prefix(".config/")?
        .split('/')
        .next()
        .unwrap_or("");
    (!name.is_empty()).then(|| name.to_string())
}

/// The standard asset directories that exist under this `$HOME`, in
/// `$HOME`-relative form.
pub fn scan_assets(home: &Path) -> Vec<String> {
    ASSET_DIRS
        .iter()
        .filter(|relative| home.join(relative).is_dir())
        .map(|relative| (*relative).to_string())
        .collect()
}

/// The image files in `~/Downloads`, as absolute paths ready to be handed to
/// `wallpaper-import`.
pub fn scan_wallpapers(home: &Path) -> Vec<String> {
    let mut found = BTreeSet::new();
    collect_wallpapers(&home.join("Downloads"), 0, &mut found);
    found.into_iter().collect()
}

/// The seeds of the reference scan: the live Hyprland config, read first, then
/// every file it (transitively) references.
///
/// Both entrypoints are seeded, because a machine may use either or both: the
/// classic `hyprland.conf`, and the `hyprland.lua` that Hyprland 0.56+ loads when
/// a config is written in lua. Seeding only the `.conf` left a lua-configured
/// desktop entirely unscanned — no services, no referenced dirs.
fn follow_hyprland_refs(
    home: &Path,
    dirs: &mut BTreeSet<String>,
    services: &mut BTreeMap<String, Service>,
) {
    let hypr = home.join(".config").join("hypr");
    let mut queue: VecDeque<(PathBuf, usize)> = ["hyprland.conf", "hyprland.lua"]
        .iter()
        .map(|name| (hypr.join(name), 0usize))
        .collect();
    let mut visited: BTreeSet<PathBuf> = BTreeSet::new();
    while let Some((file, depth)) = queue.pop_front() {
        if visited.len() >= MAX_FOLLOWED_FILES {
            return;
        }
        let Ok(canonical) = std::fs::canonicalize(&file) else {
            continue;
        };
        if !visited.insert(canonical) {
            continue;
        }
        let Some(content) = read_text(&file) else {
            continue;
        };
        for line in content.lines() {
            if is_comment(line) {
                continue;
            }
            if let Some(service) = exec_once_service(line) {
                services.entry(service.name.clone()).or_insert(service);
            }
            // A lua `require("hyprland.execs")` names a module, not a path, so
            // `path_references` cannot see it. Lua resolves it against the
            // config tree, so the same referenced file is reached by resolving
            // the module name the way lua would — without this, a lua
            // entrypoint is a dead end and nothing below it is ever read.
            for module in module_references(line) {
                if depth < MAX_FOLLOW_DEPTH
                    && let Some(target) = resolve_lua_module(home, &file, &module)
                {
                    queue.push_back((target, depth + 1));
                }
            }
            for reference in path_references(line) {
                if let Some(candidate) = candidate_for(home, &reference) {
                    // A reference into a container of independent configs
                    // proposes the same per-config paths the allowlist would,
                    // so a Hyprland line cannot reintroduce the parent that
                    // `shell_candidates` just split up.
                    let name = candidate
                        .strip_prefix(".config/")
                        .and_then(|rest| rest.split('/').next());
                    if name == Some("quickshell") && candidate == ".config/quickshell" {
                        for each in shell_candidates(&home.join(&candidate), name.unwrap_or("")) {
                            insert_candidate(dirs, each);
                        }
                    } else {
                        insert_candidate(dirs, candidate);
                    }
                }
                if depth < MAX_FOLLOW_DEPTH
                    && (is_sourcey(line) || has_config_extension(&reference))
                {
                    let target = home.join(&reference);
                    if target.is_file() {
                        queue.push_back((target, depth + 1));
                    }
                }
            }
        }
    }
}

/// What a config directory proposes, when that directory is a container of
/// independently-switchable things rather than one rice's own files.
///
/// `~/.config/quickshell` is the case that matters: Quickshell keeps every
/// shell in one directory, each its own config named by `-c`. Capturing the
/// parent takes all of them at once, so the profile for one shell swallows
/// every other — and two shells can never both be profiles, because whichever
/// is captured second owns the path the first one needs. The same is true of
/// any directory whose immediate children are each a complete config.
///
/// A child that holds QML at its top level is one, so the children are
/// proposed instead of the parent. Anything else — a flat directory of this
/// rice's own files — is proposed as itself, exactly as before.
fn shell_candidates(dir: &Path, name: &str) -> Vec<String> {
    let name = name.to_owned();
    let mut children = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let Some(child) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !entry.path().is_dir() {
                continue;
            }
            let has_qml = std::fs::read_dir(entry.path()).is_ok_and(|files| {
                files.flatten().any(|file| {
                    file.file_name()
                        .to_str()
                        .is_some_and(|n| n.ends_with(".qml"))
                })
            });
            if has_qml {
                children.push(format!(".config/{name}/{child}"));
            }
        }
    }
    if children.is_empty() {
        return vec![format!(".config/{name}")];
    }
    children.sort();
    children
}

/// Adds a proposed directory unless an entry already covers it — a reference
/// into a directory that is already proposed changes nothing — or covers the
/// entries below it.
fn insert_candidate(dirs: &mut BTreeSet<String>, candidate: String) {
    if dirs.iter().any(|existing| covers(existing, &candidate)) {
        return;
    }
    dirs.retain(|existing| !covers(&candidate, existing));
    dirs.insert(candidate);
}

/// Whether `outer` is `inner` or a directory above it — the dedup rule that
/// keeps `.config/hypr` from being proposed again for every file inside it.
fn covers(outer: &str, inner: &str) -> bool {
    inner == outer
        || inner
            .strip_prefix(outer)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// The `$HOME`-relative directory a reference points at: the directory
/// itself when it exists, otherwise the nearest existing directory above it —
/// a reference to `~/.config/ricekit/theme.conf` proposes `.config/ricekit`.
/// A reference that climbs out to bare `.config` proposes nothing.
fn candidate_for(home: &Path, reference: &str) -> Option<String> {
    let full = home.join(reference);
    if full.is_dir() {
        return Some(reference.to_string());
    }
    let mut current = Path::new(reference);
    while let Some(parent) = current.parent() {
        let text = parent.to_string_lossy();
        if text.is_empty() || text == ".config" {
            return None;
        }
        if home.join(parent).is_dir() {
            return Some(text.into_owned());
        }
        current = parent;
    }
    None
}

/// Every `~/.config/...`-shaped path a line references, whatever syntax
/// wrapped it: `~/.config/`, `/home/you/.config/`, `$HOME/.config/`, and a
/// bare leading `.config/` all reduce to the same `$HOME`-relative form.
fn path_references(line: &str) -> Vec<String> {
    let mut found = Vec::new();
    for token in line.split(TOKEN_DELIMS) {
        let reference = if token.starts_with(".config/") {
            token
        } else if let Some(index) = token.find("/.config/") {
            &token[index + 1..]
        } else {
            continue;
        };
        let reference = reference.trim_end_matches('/');
        if reference.len() > ".config/".len() {
            found.push(reference.to_string());
        }
    }
    found
}

/// The service an `exec-once = <command>` line declares: the startup
/// command, with the `pkill <name>` stop the manifest defaults to. A path
/// rather than a bare command is a script, not a service.
///
/// `hl.exec_cmd("<command>")` — the lua-config equivalent, as a lua-configured
/// desktop writes it — declares the same thing and is read the same way.
fn exec_once_service(line: &str) -> Option<Service> {
    if let Some(service) = ini_exec_once_service(line) {
        return Some(service);
    }
    lua_exec_cmd_service(line)
}

/// The `exec-once = <command>` form: the marker must start a token of its own, so
/// `hl.myexec-once` never reads as one.
fn ini_exec_once_service(line: &str) -> Option<Service> {
    let marker = "exec-once";
    let index = line.find(marker)?;
    if index > 0 {
        let previous = line.as_bytes()[index - 1];
        if previous.is_ascii_alphanumeric() || previous == b'-' || previous == b'_' {
            return None;
        }
    }
    let rest = line[index + marker.len()..].trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    service_from_command(rest)
}

/// The `hl.exec_cmd("<command>")` form. The command is a single lua string, so
/// it is read up to the closing quote rather than by whitespace — a quoted
/// command is the only thing a lua config can pass here.
///
/// The marker is the `exec_cmd` of a method call, so the `.` in `hl.exec_cmd`
/// is part of the syntax and not a reason to skip the line; only a longer
/// identifier ending in the marker (`hl.myexec_cmd`) is.
fn lua_exec_cmd_service(line: &str) -> Option<Service> {
    let index = line.find("exec_cmd")?;
    if index > 0 {
        let previous = line.as_bytes()[index - 1];
        if previous.is_ascii_alphanumeric() || previous == b'-' || previous == b'_' {
            return None;
        }
    }
    let rest = line[index + "exec_cmd".len()..].trim_start();
    let rest = rest.strip_prefix('(')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    service_from_command(rest[..end].trim())
}

/// The `Service` a startup command declares: its own text as the start command,
/// with the `pkill <name>` stop the manifest defaults to. A path rather than a
/// bare command is a script, not a service.
fn service_from_command(rest: &str) -> Option<Service> {
    let comment = rest.find(" #").unwrap_or(rest.len());
    let rest = rest[..comment].trim();
    if rest.is_empty() {
        return None;
    }
    let first = rest.split_whitespace().next()?;
    let name = first.trim_end_matches(&[';', ',', '&'][..]);
    if name.is_empty() || name.contains('/') {
        return None;
    }
    Some(Service {
        name: name.to_string(),
        start: rest.to_string(),
        stop: format!("pkill {name}"),
    })
}

/// Whether a line's tokens carry any of `markers` — the source-directive
/// check that decides which referenced files are read in turn.
fn is_sourcey(line: &str) -> bool {
    line.split(TOKEN_DELIMS)
        .any(|token| is_marker(token, SOURCE_MARKERS))
}

/// Whether a token is one of `markers`, exactly or after a module prefix
/// (`awful.spawn` counts as `spawn`), case-insensitively.
fn is_marker(token: &str, markers: &[&str]) -> bool {
    let lowered = token.to_ascii_lowercase();
    let tail = lowered.rsplit('.').next().unwrap_or(&lowered);
    markers
        .iter()
        .any(|marker| lowered == *marker || tail == *marker)
}

fn has_config_extension(reference: &str) -> bool {
    Path::new(reference)
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            CONFIG_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        })
}

/// Walks a detected config dir for text files and collects the first
/// non-flag token after every command marker — the binary-reference scan's
/// candidate set.
fn collect_commands(root: &Path, depth: usize, out: &mut BTreeSet<String>) {
    if depth > MAX_SCAN_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            collect_commands(&path, depth + 1, out);
        } else if metadata.is_file() {
            let Some(content) = read_text(&path) else {
                continue;
            };
            for line in content.lines() {
                if !is_comment(line) {
                    collect_line_commands(line, out);
                }
            }
        }
    }
}

fn collect_line_commands(line: &str, out: &mut BTreeSet<String>) {
    let tokens: Vec<&str> = line
        .split(TOKEN_DELIMS)
        .filter(|token| !token.is_empty())
        .collect();
    for (index, token) in tokens.iter().enumerate() {
        if !is_marker(token, COMMAND_MARKERS) {
            continue;
        }
        let Some(command) = tokens[index + 1..]
            .iter()
            .find(|next| !next.starts_with('-'))
        else {
            continue;
        };
        out.insert((*command).to_string());
    }
}

/// One candidate command as a `pacman -Qo` query: `~/`- and `$HOME`-paths
/// only when the file exists, PATH-resolved paths for installed commands,
/// and the bare command name otherwise — the owner stub (and pacman's own
/// error) decides from there.
fn resolve_query(home: &Path, command: &str) -> Option<String> {
    if let Some(rest) = command
        .strip_prefix("~/")
        .or_else(|| command.strip_prefix("$HOME/"))
    {
        let target = home.join(rest);
        return target.is_file().then(|| target.display().to_string());
    }
    if command.contains('/') {
        return None;
    }
    Some(
        which(command)
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| command.to_string()),
    )
}

/// A command's executable on `PATH`, if this machine has one.
fn which(command: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(command);
        let metadata = std::fs::metadata(&candidate).ok()?;
        (metadata.is_file() && metadata.permissions().mode() & 0o111 != 0).then_some(candidate)
    })
}

/// The package that owns a referenced binary, via `pacman -Qo`. An unowned
/// (or unresolvable) command simply has no package: detection never warns.
fn package_owning(home: &Path, command: &str) -> Option<String> {
    let query = resolve_query(home, command)?;
    let output = Command::new("pacman")
        .arg("-Qo")
        .arg(&query)
        .env("LC_ALL", "C")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines().find_map(|line| {
        line.split_once(" is owned by ")
            .and_then(|(_, owned)| owned.split_whitespace().next())
            .map(str::to_string)
    })
}

/// Everything `pacman -Qm` lists as foreign — the AUR side of the split.
/// A pacman that cannot answer yields no foreign packages rather than a
/// warning: the tool probes already report whether pacman is usable.
fn foreign_packages() -> BTreeSet<String> {
    let Ok(output) = Command::new("pacman")
        .arg("-Qm")
        .env("LC_ALL", "C")
        .output()
    else {
        return BTreeSet::new();
    };
    if !output.status.success() {
        return BTreeSet::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_whitespace().next().map(str::to_string))
        .collect()
}

/// The image files under `directory`, walking subdirectories to a bounded
/// depth — `~/Downloads` trees can be deep, but wallpapers sit near the top.
fn collect_wallpapers(directory: &Path, depth: usize, found: &mut BTreeSet<String>) {
    if depth > 4 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            collect_wallpapers(&path, depth + 1, found);
        } else if metadata.is_file() && is_image(&path) {
            found.insert(path.display().to_string());
        }
    }
}

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase)
        .as_deref()
        .is_some_and(|extension| IMAGE_EXTENSIONS.contains(&extension))
}

/// A commented-out line: `#` (hypr, ini), `//` (jsonc, lua), `--` (lua).
/// Commented references are not live configuration and never become
/// candidates, services, or commands.
fn is_comment(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with('#') || trimmed.starts_with("//") || trimmed.starts_with("--")
}

/// A config file's text, or nothing: too big or holding NUL bytes means it
/// is not text configuration worth scanning.
fn read_text(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}
