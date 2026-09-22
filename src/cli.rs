//! Argument parsing for the ten operations.
//!
//! The GUI spawns one subprocess per operation, so the only interface here is
//! argv. Anything unrecognized becomes a message, never a panic.

use std::fmt;

/// The full operation surface, for error messages.
pub const USAGE: &str = "expected one of: \
     detect, snapshot <name>, plan <target>, switch <target>, list, info <name>, \
     delete <name> [--force], diff <a> <b>, wallpaper-import <path>, init";

/// One parsed operation invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    Detect,
    Snapshot { name: String },
    Plan { target: String },
    Switch { target: String },
    List,
    Info { name: String },
    Delete { name: String, force: bool },
    Diff { a: String, b: String },
    WallpaperImport { path: String },
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
            Invocation::Snapshot { name } | Invocation::Info { name } => Some(name.clone()),
            Invocation::Plan { target } | Invocation::Switch { target } => Some(target.clone()),
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
        "snapshot" => Ok(Invocation::Snapshot {
            name: one_positional(operation, "<name>", rest)?,
        }),
        "plan" => Ok(Invocation::Plan {
            target: one_positional(operation, "<target>", rest)?,
        }),
        "switch" => Ok(Invocation::Switch {
            target: one_positional(operation, "<target>", rest)?,
        }),
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

fn unknown_flag(operation: &str, flag: &str) -> ArgumentError {
    ArgumentError(format!(
        "`{operation}` does not take the flag `{flag}`; {USAGE}"
    ))
}
