//! The NDJSON stream every invocation writes to stdout.
//!
//! Frozen contract (the QML panel reads this): progress lines and warning
//! lines first — warnings the moment they happen, so a consumer renders
//! them inline instead of waiting — then exactly one final line carrying
//! the `{ok, warnings, data}` envelope. The envelope's own shape never
//! changes: `data` still carries every warning when the operation ends.

use serde::Serialize;
use serde_json::{Value, json};
use std::io::Write;

/// The final line of every invocation, successful or not.
#[derive(Debug, Serialize)]
pub struct Envelope {
    pub ok: bool,
    pub warnings: Vec<String>,
    pub data: Value,
}

impl Envelope {
    /// Success: `data` carries the operation's payload.
    pub fn ok(data: Value) -> Envelope {
        Envelope {
            ok: true,
            warnings: Vec::new(),
            data,
        }
    }

    /// Failure: `ok: false` with the message in place of `data`. Every error
    /// path lands here instead of unwinding, so a bad invocation can never
    /// become a crash.
    pub fn failed(message: impl Into<String>) -> Envelope {
        Envelope {
            ok: false,
            warnings: Vec::new(),
            data: json!({ "error": message.into() }),
        }
    }

    /// Writes the final line.
    pub fn emit(&self) {
        write_line(&serde_json::to_string(self).expect("envelope is always serializable"));
    }
}

/// Streams the progress lines that precede the envelope.
pub struct Emitter {
    operation: &'static str,
    step: u32,
}

impl Emitter {
    pub fn new(operation: &'static str) -> Emitter {
        Emitter { operation, step: 0 }
    }

    /// Emits one progress line. `step` counts the lines emitted so far, so a
    /// consumer reopening mid-operation can tell how far along it is.
    pub fn progress(&mut self, message: &str) {
        self.step += 1;
        let line = json!({
            "progress": {
                "operation": self.operation,
                "step": self.step,
                "message": message,
            }
        });
        write_line(&line.to_string());
    }

    /// Streams one warning line at the moment the warning happens — before
    /// the final envelope — so the panel can render it inline while the
    /// operation still runs. It carries the step in progress and does not
    /// advance it: a warning belongs to the step that produced it. The
    /// caller still folds the same text into `Envelope::warnings`, whose
    /// shape is frozen.
    pub fn warning(&self, message: &str) {
        let line = json!({
            "warning": {
                "operation": self.operation,
                "step": self.step,
                "message": message,
            }
        });
        write_line(&line.to_string());
    }

    /// Progress lines emitted so far.
    pub fn step(&self) -> u32 {
        self.step
    }
}

/// Writes one NDJSON line. Write errors are deliberately ignored: the consumer
/// may have closed the pipe, and a closed pipe must not become a panic.
fn write_line(line: &str) {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}
