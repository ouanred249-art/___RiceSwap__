//! Argument parsing for the ten operations.
//!
//! The GUI spawns one subprocess per operation, so the only interface here is
//! argv. Anything unrecognized becomes a message, never a panic.

use std::fmt;

/// The full operation surface, for error messages.
pub const USAGE: &str = "expected one of: \
     detect, snapshot <name> [--force], plan <target>, switch <target> [--aur-helper <helper>], \
     list, info <name>, delete <name> [--force], diff <a> <b>, wallpaper-import <path>, init";

/// One parsed operation invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    Detect,
    Snapshot {
        name: String,
        force: bool,
    },
    Plan {
        target: String,
    },
    /// `switch <target> [--aur-helper <helper>]` — the flag pins the AUR
    /// helper instead of runtime detection, the testing escape hatch the
    /// privilege-flow spec locks in.
    Switch {
        target: String,
        aur_helper: Option<String>,
    },
    List,
    Info {
        name: String,
    },
    Delete {
        name: String,
        force: bool,
    },
    Diff {
        a: String,
        b: String,
    },
    WallpaperImport {
        path: String,
    },
    Init,
}

impl Invocation {
    /// The operation name as it appears in the envelope, `state.json`, and
    /// progress lines.
    pub fn name(&self) -> &'static str {
        match self {
            Invocation::Detect => "detect",
            Invocation::Snapshot { .. } => "snapshot",
            Invocation::Plan { .. } => "plan",
            Invocation::Switch { .. } => "switch",
            Invocation::List => "list",
            Invocation::Info { .. } => "info",
            Invocation::Delete { .. } => "delete",
            Invocation::Diff { .. } => "diff",
            Invocation::WallpaperImport { .. } => "wallpaper-import",
            Invocation::Init => "init",
        }
    }

    /// The profile this operation acts on, when it acts on one.
    pub fn target(&self) -> Option<String> {
        match self {
            Invocation::Snapshot { name, .. } | Invocation::Info { name } => Some(name.clone()),
            Invocation::Plan { target } | Invocation::Switch { target, .. } => Some(target.clone()),
            Invocation::Delete { name, .. } => Some(name.clone()),
            Invocation::Diff { a, .. } => Some(a.clone()),
            Invocation::WallpaperImport { path } => Some(path.clone()),
            Invocation::Detect | Invocation::List | Invocation::Init => None,
        }
    }
}

/// Why argv did not name a valid invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgumentError(String);

impl fmt::Display for ArgumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Parses argv (already stripped of the program name).
pub fn parse(args: &[String]) -> Result<Invocation, ArgumentError> {
    let Some((operation, rest)) = args.split_first() else {
        return Err(ArgumentError(format!("no operation given; {USAGE}")));
    };
    match operation.as_str() {
        "detect" => no_args(operation, rest, Invocation::Detect),
        "list" => no_args(operation, rest, Invocation::List),
        "init" => no_args(operation, rest, Invocation::Init),
        "snapshot" => {
            let (force, positional) = split_force(operation, rest)?;
            let name = exactly_one(operation, "<name>", positional)?;
            Ok(Invocation::Snapshot { name, force })
        }
        "plan" => Ok(Invocation::Plan {
            target: one_positional(operation, "<target>", rest)?,
        }),
        "switch" => {
            let (aur_helper, positional) = split_aur_helper(operation, rest)?;
            let target = exactly_one(operation, "<target>", positional)?;
            Ok(Invocation::Switch { target, aur_helper })
        }
        "info" => Ok(Invocation::Info {
            name: one_positional(operation, "<name>", rest)?,
        }),
        "wallpaper-import" => Ok(Invocation::WallpaperImport {
            path: one_positional(operation, "<path>", rest)?,
        }),
        "diff" => {
            let (a, b) = two_positionals(operation, "<a> <b>", rest)?;
            Ok(Invocation::Diff { a, b })
        }
        "delete" => {
            let (force, positional) = split_force(operation, rest)?;
            let name = exactly_one(operation, "<name>", positional)?;
            Ok(Invocation::Delete { name, force })
        }
        other => Err(ArgumentError(format!(
            "unknown operation `{other}`; {USAGE}"
        ))),
    }
}

fn no_args(
    operation: &str,
    rest: &[String],
    invocation: Invocation,
) -> Result<Invocation, ArgumentError> {
    match rest {
        [] => Ok(invocation),
        [extra, ..] => Err(ArgumentError(format!(
            "`{operation}` takes no arguments, got `{extra}`; usage: {operation}"
        ))),
    }
}

fn one_positional(
    operation: &str,
    placeholder: &str,
    rest: &[String],
) -> Result<String, ArgumentError> {
    let mut positional = Vec::new();
    for arg in rest {
        if arg.starts_with("--") {
            return Err(unknown_flag(operation, arg));
        }
        positional.push(arg.as_str());
    }
    exactly_one(operation, placeholder, positional)
}

fn exactly_one(
    operation: &str,
    placeholder: &str,
    positional: Vec<&str>,
) -> Result<String, ArgumentError> {
    match positional.as_slice() {
        [] => Err(ArgumentError(format!(
            "`{operation}` needs {placeholder}; usage: {operation} {placeholder}"
        ))),
        [value] => {
            if value.is_empty() {
                return Err(ArgumentError(format!(
                    "`{operation}` was given an empty {placeholder}"
                )));
            }
            Ok((*value).to_string())
        }
        [_, extra, ..] => Err(ArgumentError(format!(
            "`{operation}` takes one argument, got an extra `{extra}`; \
             usage: {operation} {placeholder}"
        ))),
    }
}

fn two_positionals(
    operation: &str,
    placeholder: &str,
    rest: &[String],
) -> Result<(String, String), ArgumentError> {
    let mut positional = Vec::new();
    for arg in rest {
        if arg.starts_with("--") {
            return Err(unknown_flag(operation, arg));
        }
        positional.push(arg.as_str());
    }
    match positional.as_slice() {
        [a, b] => {
            if a.is_empty() || b.is_empty() {
                return Err(ArgumentError(format!(
                    "`{operation}` was given an empty profile name"
                )));
            }
            Ok(((*a).to_string(), (*b).to_string()))
        }
        [] | [_] => Err(ArgumentError(format!(
            "`{operation}` needs two profiles; usage: {operation} {placeholder}"
        ))),
        [_, _, extra, ..] => Err(ArgumentError(format!(
            "`{operation}` takes two arguments, got an extra `{extra}`; \
             usage: {operation} {placeholder}"
        ))),
    }
}

/// Pulls `--force` out of the argument list wherever it appears.
fn split_force<'a>(
    operation: &str,
    rest: &'a [String],
) -> Result<(bool, Vec<&'a str>), ArgumentError> {
    let mut force = false;
    let mut positional = Vec::new();
    for arg in rest {
        if arg == "--force" {
            force = true;
        } else if arg.starts_with("--") {
            return Err(unknown_flag(operation, arg));
        } else {
            positional.push(arg.as_str());
        }
    }
    Ok((force, positional))
}

/// Pulls `--aur-helper <name>` out of `switch`'s argument list wherever it
/// appears, rejecting every other flag. The override pins the AUR helper the
/// privilege flow would otherwise detect at runtime.
fn split_aur_helper<'a>(
    operation: &str,
    rest: &'a [String],
) -> Result<(Option<String>, Vec<&'a str>), ArgumentError> {
    let mut helper = None;
    let mut positional = Vec::new();
    let mut arguments = rest.iter();
    while let Some(argument) = arguments.next() {
        if argument == "--aur-helper" {
            let usage = format!(
                "`{operation}` needs a helper name after `--aur-helper`; \
                 usage: {operation} <target> --aur-helper <helper>"
            );
            let Some(name) = arguments.next() else {
                return Err(ArgumentError(usage));
            };
            if name.starts_with("--") {
                return Err(ArgumentError(usage));
            }
            if helper.is_some() {
                return Err(ArgumentError(format!(
                    "`{operation}` was given `--aur-helper` more than once"
                )));
            }
            helper = Some(name.clone());
        } else if argument.starts_with("--") {
            return Err(unknown_flag(operation, argument));
        } else {
            positional.push(argument.as_str());
        }
    }
    Ok((helper, positional))
}

fn unknown_flag(operation: &str, flag: &str) -> ArgumentError {
    ArgumentError(format!(
        "`{operation}` does not take the flag `{flag}`; {USAGE}"
    ))
}
