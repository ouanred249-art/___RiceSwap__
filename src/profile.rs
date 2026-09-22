//! The profile store.
//!
//! A profile is a directory under `~/.local/share/riceswap/profiles/<name>/`
//! mirroring the `$HOME` layout, plus the `profile.toml` manifest at its root.
//! The active profile *is* the `current` symlink
//! (`~/.local/share/riceswap/current` → a profile directory): the state lives on
//! the filesystem, so it cannot drift, and the GUI needs one `readlink`.
//! [`Store::flip`] is the activation primitive later tickets build on.
//!
//! Reading the store never shells out and never panics: an unreadable profile is
//! a message in the envelope, not a crash.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// The manifest version this build writes, and the one it migrates older
/// profiles up to.
pub const CURRENT_MANIFEST_VERSION: u32 = 1;

/// The manifest file at the root of every profile directory.
const MANIFEST_FILE: &str = "profile.toml";

/// A migration step: raw manifest text in, raw manifest text at the next
/// `manifest_version` out — or a message saying why not.
type Migration = fn(&str) -> Result<String, String>;

/// The forward-only migration chain: each entry migrates a document *from* the
/// named `manifest_version` to the next one, rewriting the raw TOML text so a
/// step keeps working on fields this build does not know about yet.
///
/// v1 ships one step. `0` is the pre-release marker: nothing about the document
/// changed when the format froze at `1`, so the step returns the text unchanged.
/// It is the extension point future structural migrations slot into, and the
/// loader rewrites the marker itself — a `0` profile therefore loads as `1`.
const MIGRATIONS: &[(u32, Migration)] = &[(0, migrate_0_to_1)];

fn migrate_0_to_1(raw: &str) -> Result<String, String> {
    Ok(raw.to_string())
}

/// The profile store rooted at a `$HOME`.
pub struct Store {
    home: PathBuf,
}

/// Which profile the `current` symlink names, and anything wrong with it.
///
/// `warning` is set when the symlink exists but does not name a profile in the
/// store, so a reader can say so instead of silently reporting "none".
pub struct Active {
    pub name: Option<String>,
    pub warning: Option<String>,
}

/// One profile as the store lists it.
pub struct Listed {
    pub name: String,
    pub manifest: Manifest,
}

impl Store {
    pub fn new(home: PathBuf) -> Store {
        Store { home }
    }

    /// `~/.local/share/riceswap`.
    pub fn data_dir(&self) -> PathBuf {
        self.home.join(".local").join("share").join("riceswap")
    }

    /// `~/.local/share/riceswap/profiles`.
    pub fn profiles_dir(&self) -> PathBuf {
        self.data_dir().join("profiles")
    }

    /// `~/.local/share/riceswap/profiles/<name>`.
    pub fn profile_dir(&self, name: &str) -> PathBuf {
        self.profiles_dir().join(name)
    }

    /// `~/.local/share/riceswap/current`, the activation primitive: flipping the
    /// active profile means pointing this symlink at another profile directory.
    pub fn current_link(&self) -> PathBuf {
        self.data_dir().join("current")
    }

    /// Reads the `current` symlink. A store with no symlink simply has no active
    /// profile; a symlink that leads nowhere is reported as a warning, because
    /// the GUI badges whichever profile this names and must not be lied to.
    pub fn active(&self) -> Active {
        let link = self.current_link();
        let Ok(target) = std::fs::read_link(&link) else {
            return Active {
                name: None,
                warning: None,
            };
        };
        let resolved = if target.is_absolute() {
            target
        } else {
            self.data_dir().join(target)
        };
        let profiles = self.profiles_dir();
        let inside = resolved
            .parent()
            .is_some_and(|parent| parent == profiles.as_path());
        let named = resolved.file_name().and_then(|name| name.to_str());
        let Some(name) = named.filter(|_| inside) else {
            return Active {
                name: None,
                warning: Some(format!(
                    "{} does not point at a profile in the store ({}); no profile is active",
                    link.display(),
                    resolved.display()
                )),
            };
        };
        if !self.profile_dir(name).is_dir() {
            return Active {
                name: None,
                warning: Some(format!(
                    "{} points at `{name}`, which is not in the profile store; \
                     no profile is active",
                    link.display()
                )),
            };
        }
        Active {
            name: Some(name.to_string()),
            warning: None,
        }
    }

    /// Points `current` at `profiles/<name>`: the flip later tickets build on.
    /// The symlink is staged and renamed into place, so a watcher never reads a
    /// half-flipped activation.
    pub fn flip(&self, name: &str) -> Result<(), String> {
        let profile = self.ensure_profile(name)?;
        let link = self.current_link();
        let Some(parent) = link.parent() else {
            return Err(format!("{} has no parent directory", link.display()));
        };
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        let staged = parent.join("current.staged");
        let _ = std::fs::remove_file(&staged);
        std::os::unix::fs::symlink(&profile, &staged)
            .map_err(|error| format!("cannot stage the `current` symlink: {error}"))?;
        if let Err(error) = std::fs::rename(&staged, &link) {
            let _ = std::fs::remove_file(&staged);
            return Err(format!(
                "cannot point {} at `{name}`: {error}",
                link.display()
            ));
        }
        Ok(())
    }

    /// Every profile in the store, in name order, with its manifest. A profile
    /// that cannot be read is a warning rather than a failure: one broken
    /// manifest must not blank the GUI's profile list.
    pub fn list(&self) -> (Vec<Listed>, Vec<String>) {
        let (names, mut warnings) = self.names();
        let mut listed = Vec::new();
        for name in names {
            match self.load(&name) {
                Ok(manifest) => listed.push(Listed { name, manifest }),
                Err(error) => warnings.push(format!("skipping profile `{name}`: {error}")),
            }
        }
        (listed, warnings)
    }

    /// Loads one profile's manifest, migrating it forward to the current
    /// version. Every failure is a message naming the file and the field.
    pub fn load(&self, name: &str) -> Result<Manifest, String> {
        let profile = self.ensure_profile(name)?;
        let path = profile.join(MANIFEST_FILE);
        let raw = std::fs::read_to_string(&path).map_err(|error| match error.kind() {
            ErrorKind::NotFound => {
                format!(
                    "profile `{name}` has no {MANIFEST_FILE} at {}",
                    path.display()
                )
            }
            _ => format!("cannot read {}: {error}", path.display()),
        })?;
        parse(&path, &raw)
    }

    /// Resolves `name` to a profile directory in the store, or explains why not:
    /// a name that could escape the store, or a profile that is not there (with
    /// what is).
    fn ensure_profile(&self, name: &str) -> Result<PathBuf, String> {
        if !valid_name(name) {
            return Err(format!(
                "`{name}` is not a valid profile name; profile names are directory \
                 names under {}, so they cannot be empty, contain `/`, or start with `.`",
                self.profiles_dir().display()
            ));
        }
        if !self.profile_dir(name).is_dir() {
            return Err(self.missing(name));
        }
        Ok(self.profile_dir(name))
    }

    /// The profile directory names in the store, plus anything unreadable.
    fn names(&self) -> (Vec<String>, Vec<String>) {
        let mut warnings = Vec::new();
        let entries = match std::fs::read_dir(self.profiles_dir()) {
            Ok(entries) => entries,
            // No store yet: an empty profile list, not a problem to report.
            Err(error) if error.kind() == ErrorKind::NotFound => return (Vec::new(), warnings),
            Err(error) => {
                warnings.push(format!(
                    "cannot read {}: {error}",
                    self.profiles_dir().display()
                ));
                return (Vec::new(), warnings);
            }
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    warnings.push(format!(
                        "cannot read an entry of {}: {error}",
                        self.profiles_dir().display()
                    ));
                    continue;
                }
            };
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                warnings.push(format!(
                    "{} has a name RiceSwap cannot read; skipped",
                    entry.path().display()
                ));
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            if !entry.path().is_dir() {
                warnings.push(format!(
                    "{} is not a profile directory; skipped",
                    entry.path().display()
                ));
                continue;
            }
            names.push(name);
        }
        names.sort();
        (names, warnings)
    }

    /// The failure for a name that is not in the store, naming what is.
    fn missing(&self, name: &str) -> String {
        let (names, _) = self.names();
        if names.is_empty() {
            format!(
                "no profile named `{name}`; the profile store at {} is empty",
                self.profiles_dir().display()
            )
        } else {
            format!(
                "no profile named `{name}`; the store holds: {}",
                names.join(", ")
            )
        }
    }
}

/// A profile name is a directory name: no separators, no hidden directories, and
/// nothing that could reach outside the store.
fn valid_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !name.contains('/') && !name.contains('\0')
}

/// Parses a manifest, migrating it forward to [`CURRENT_MANIFEST_VERSION`].
fn parse(path: &Path, raw: &str) -> Result<Manifest, String> {
    let probe: VersionProbe =
        toml::from_str(raw).map_err(|error| invalid(path, &error.to_string()))?;
    let declared = probe.manifest_version.ok_or_else(|| {
        format!(
            "{} is not a RiceSwap manifest: it has no `manifest_version` \
             (expected `manifest_version = {CURRENT_MANIFEST_VERSION}`)",
            path.display()
        )
    })?;
    let migrated = migrate(path, raw, declared)?;
    let mut manifest: Manifest =
        toml::from_str(&migrated).map_err(|error| invalid(path, &error.to_string()))?;
    // The load result is the migrated manifest: a profile that declared an older
    // version is reported at the current one.
    manifest.manifest_version = CURRENT_MANIFEST_VERSION;
    Ok(manifest)
}

/// Runs the migration chain from `declared` up to the current version.
fn migrate(path: &Path, raw: &str, declared: i64) -> Result<String, String> {
    let current = i64::from(CURRENT_MANIFEST_VERSION);
    if declared > current {
        return Err(format!(
            "{} declares `manifest_version = {declared}`, which is newer than this \
             RiceSwap understands ({current}); update RiceSwap to open this profile",
            path.display()
        ));
    }
    // A version below zero is not a version this format ever wrote. Migrations
    // are forward-only, so there is nothing to convert it from.
    let Ok(mut version) = u32::try_from(declared) else {
        return Err(format!(
            "{} declares `manifest_version = {declared}`, which RiceSwap has never \
             written (the first version is 0); re-snapshot the rice to rebuild this profile",
            path.display()
        ));
    };
    let mut migrated = raw.to_string();
    while version < CURRENT_MANIFEST_VERSION {
        let Some((_, step)) = MIGRATIONS.iter().find(|(from, _)| *from == version) else {
            return Err(format!(
                "{} declares `manifest_version = {version}`, but this RiceSwap has no \
                 migration from {version} to {}; it cannot be loaded safely",
                path.display(),
                version + 1
            ));
        };
        migrated = step(&migrated).map_err(|error| {
            format!(
                "migrating {} from `manifest_version = {version}` failed: {error}",
                path.display()
            )
        })?;
        version += 1;
    }
    Ok(migrated)
}

fn invalid(path: &Path, error: &str) -> String {
    format!(
        "{} is not a valid RiceSwap manifest: {error}",
        path.display()
    )
}

/// Just enough of the document to read the version marker before the manifest
/// itself is validated, so an old profile is migrated before it is parsed.
#[derive(Deserialize)]
struct VersionProbe {
    manifest_version: Option<i64>,
}

/// A profile's `profile.toml`, at the version this build understands. Serializing
/// it yields the manifest JSON `list` and `info` report: the same shape as the
/// file, with every key present.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub manifest_version: u32,
    pub profile: ProfileInfo,
    pub packages: Packages,
    #[serde(default)]
    pub services: Vec<Service>,
    #[serde(default)]
    pub files: Vec<FileEntry>,
    #[serde(default)]
    pub rice_info: RiceInfo,
}

/// The `[profile]` table.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileInfo {
    pub name: String,
    pub description: String,
    pub created_at: String,
    pub updated_at: String,
    pub screenshot: String,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub source_commit: Option<String>,
}

/// The `[packages]` table, split by where the package comes from.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Packages {
    pub official: Vec<String>,
    pub aur: Vec<String>,
}

/// One `[[services]]` entry. `start` and `stop` are explicit: Hyprland's
/// `exec-once` does not re-run on reload, so switching needs both commands.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Service {
    pub name: String,
    pub start: String,
    pub stop: String,
}

/// One `[[files]]` entry: a `$HOME`-relative path the profile mirrors.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
    pub path: String,
    #[serde(default)]
    pub optional: bool,
}

/// The open `[rice_info]` table, kept as JSON so the GUI renders whatever keys
/// exist. TOML datetimes become their RFC 3339 strings: the marker TOML uses to
/// round-trip them is an implementation detail the GUI must never see.
#[derive(Debug, Default)]
pub struct RiceInfo(Map<String, Value>);

impl Serialize for RiceInfo {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RiceInfo {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let table = toml::Table::deserialize(deserializer)?;
        Ok(RiceInfo(
            table
                .into_iter()
                .map(|(key, value)| (key, json_value(value)))
                .collect(),
        ))
    }
}

/// One TOML value as JSON, so a table the manifest does not model still renders.
fn json_value(value: toml::Value) -> Value {
    match value {
        toml::Value::String(text) => Value::String(text),
        toml::Value::Integer(number) => Value::from(number),
        toml::Value::Float(number) => Value::from(number),
        toml::Value::Boolean(flag) => Value::from(flag),
        toml::Value::Datetime(stamp) => Value::String(stamp.to_string()),
        toml::Value::Array(items) => items.into_iter().map(json_value).collect(),
        toml::Value::Table(table) => Value::Object(
            table
                .into_iter()
                .map(|(key, value)| (key, json_value(value)))
                .collect(),
        ),
    }
}
