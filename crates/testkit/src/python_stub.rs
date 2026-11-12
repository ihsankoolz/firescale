//! A stand-in for a virtual environment's `python`, used by tests.
//!
//! A test copies this program to `.venv/bin/python` (or
//! `.venv\Scripts\python.exe` on Windows) in a fixture project. When started
//! it appends "venv-python <arguments> in <name of the current folder>" to
//! the file named by `FSCALE_STUB_LOG`. When the current folder's name equals
//! `FSCALE_STUB_FAIL_IN`, it prints `FSCALE_STUB_MESSAGE` on stderr and exits
//! with code 1; otherwise it exits with code 0.

use std::env;
use std::fs::OpenOptions;
use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let folder = env::current_dir()
        .ok()
        .and_then(|dir| {
            dir.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    if let Some(log) = env::var_os("FSCALE_STUB_LOG") {
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log) {
            let args: Vec<String> = env::args().skip(1).collect();
            let _ = writeln!(file, "venv-python {} in {folder}", args.join(" "));
        }
    }
    if env::var("FSCALE_STUB_FAIL_IN").is_ok_and(|fail_in| fail_in == folder) {
        let message = env::var("FSCALE_STUB_MESSAGE").unwrap_or_else(|_| "failed".to_owned());
        eprintln!("{message}");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}
