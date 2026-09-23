//! Probing the external tools the operations shell out to.
//!
//! No operation performs real work yet, but each one does declare the external
//! tools its real implementation will need. Probing them is the only live
//! plumbing the scaffold keeps, and it is strictly read-only: every tool is
//! asked for its version and nothing else.

use serde::Serialize;
use std::collections::BTreeMap;
use std::process::Command;

/// An external tool the backend shells out to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Pacman,
    Yay,
    Paru,
    Hyprctl,
    Grim,
    /// The polkit executor every official package op runs under.
    PkExec,
    /// The floating-terminal wrapper the AUR helper is spawned inside, so its
    /// password prompt works without a terminal of our own.
    FloatTerminal,
}

impl Tool {
    /// Every tool the backend knows about.
    pub const ALL: &'static [Tool] = &[
        Tool::Pacman,
        Tool::Yay,
        Tool::Paru,
        Tool::Hyprctl,
        Tool::Grim,
        Tool::PkExec,
        Tool::FloatTerminal,
    ];

    /// The executable name looked up on `PATH`.
    pub const fn name(self) -> &'static str {
        match self {
            Tool::Pacman => "pacman",
            Tool::Yay => "yay",
            Tool::Paru => "paru",
            Tool::Hyprctl => "hyprctl",
            Tool::Grim => "grim",
            Tool::PkExec => "pkexec",
            Tool::FloatTerminal => "riceswap-float",
        }
    }
}

/// What the version probe learned about one tool.
#[derive(Debug, Serialize)]
pub struct ToolStatus {
    /// The executable was found on `PATH` and ran.
    pub available: bool,
    /// Its exit status, or `None` when it never ran.
    pub exit_code: Option<i32>,
    /// First line of stdout when it exited cleanly.
    pub version: Option<String>,
    /// Spawn error, or first line of stderr when it exited non-zero.
    pub error: Option<String>,
}

impl ToolStatus {
    /// The tool answered cleanly.
    pub fn succeeded(&self) -> bool {
        self.available && self.exit_code == Some(0)
    }
}

/// Probe results keyed by tool name, in a stable order.
pub type ToolReport = BTreeMap<&'static str, ToolStatus>;

/// Probes every tool in `tools`.
pub fn probe_all(tools: &[Tool]) -> ToolReport {
    tools
        .iter()
        .map(|tool| (tool.name(), probe(*tool)))
        .collect()
}

fn probe(tool: Tool) -> ToolStatus {
    match Command::new(tool.name()).arg("--version").output() {
        Err(error) => ToolStatus {
            available: false,
            exit_code: None,
            version: None,
            error: Some(error.to_string()),
        },
        Ok(output) => {
            let clean = output.status.success();
            ToolStatus {
                available: true,
                exit_code: output.status.code(),
                version: if clean {
                    first_line(&output.stdout)
                } else {
                    None
                },
                error: first_line(&output.stderr),
            }
        }
    }
}

/// First non-empty line of a stream, trimmed and length-capped so one chatty
/// tool cannot bloat the envelope.
fn first_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(200).collect())
}
