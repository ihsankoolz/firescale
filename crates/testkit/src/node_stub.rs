//! A stand-in for `node`, used by tests.
//!
//! When started it appends one line to the file named by the
//! `FSCALE_STUB_MARKER` environment variable, then exits with code 1. A test
//! puts a copy of this program on `PATH` under the name `node`: if the marker
//! file exists afterwards, something started "node".

use std::env;
use std::fs::OpenOptions;
use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    if let Some(marker) = env::var_os("FSCALE_STUB_MARKER") {
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(marker) {
            let args: Vec<String> = env::args().skip(1).collect();
            let _ = writeln!(file, "started {}", args.join(" "));
        }
    }
    ExitCode::from(1)
}
