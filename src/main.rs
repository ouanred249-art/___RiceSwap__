//! RiceSwap backend.
//!
//! One process per operation. The GUI spawns this binary, reads the streamed
//! NDJSON progress lines off stdout, and takes the final line as the
//! `{ok, warnings, data}` envelope. The process always exits 0: a malformed
//! invocation is a reportable result, not a crash.

mod cli;
mod detection;
mod envelope;
mod operations;
mod profile;
mod state;
mod tools;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match cli::parse(&args) {
        Ok(invocation) => operations::run(invocation),
        Err(error) => envelope::Envelope::failed(error.to_string()).emit(),
    }
}
