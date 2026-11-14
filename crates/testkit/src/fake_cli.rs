//! A stand-in for an agent's own CLI (`claude`, `codex`) in the adapter
//! tests (M28): copied into a folder on the PATH under the agent's name, it
//! appends one JSON line to the file `FSCALE_FAKE_CLI_LOG` names (its
//! arguments, its standard input and its working folder), prints the file
//! `FSCALE_FAKE_CLI_OUTPUT` names (or nothing), and exits with
//! `FSCALE_FAKE_CLI_EXIT` (0 by default). With `FSCALE_FAKE_CLI_AUTH=0`,
//! `auth status` and `login status` exit 1; with `FSCALE_FAKE_CLI_BROKEN=1`,
//! `--version` exits 127. `FSCALE_FAKE_CLI_WRITE` names a
//! JSON object of files to write, relative to its working folder, as an
//! agent's edits.

use std::io::{Read, Write};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let status =
        matches!(args.as_slice(), [a, b] if (a == "auth" || a == "login") && b == "status");
    let mut input = String::new();
    if !status && !args.iter().any(|a| a == "--version") {
        let _ = std::io::stdin().read_to_string(&mut input);
    }
    if let Ok(log) = std::env::var("FSCALE_FAKE_CLI_LOG") {
        let entry = serde_json::json!({
            "args": args,
            "stdin": input,
            "cwd": std::env::current_dir().unwrap_or_default().to_string_lossy(),
        });
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
        {
            let _ = writeln!(file, "{entry}");
        }
    }
    if args.iter().any(|a| a == "--version") {
        // A broken install: tests never leave the agent off the PATH, as a
        // real agent installed on the machine would then be found.
        if std::env::var("FSCALE_FAKE_CLI_BROKEN").as_deref() == Ok("1") {
            return ExitCode::from(127);
        }
        println!("9.9.9 (stand-in)");
        return ExitCode::SUCCESS;
    }
    if status {
        let ok = std::env::var("FSCALE_FAKE_CLI_AUTH").as_deref() != Ok("0");
        println!("{}", if ok { "Logged in" } else { "Not logged in" });
        return if ok {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(1)
        };
    }
    if let Some(writes) = std::env::var("FSCALE_FAKE_CLI_WRITE")
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
    {
        let cwd = std::env::current_dir().unwrap_or_default();
        for (path, content) in writes.as_object().into_iter().flatten() {
            let target = path
                .split('/')
                .fold(cwd.clone(), |dir, part| dir.join(part));
            if let Some(parent) = target.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&target, content.as_str().unwrap_or_default());
        }
    }
    if let Some(text) = std::env::var("FSCALE_FAKE_CLI_OUTPUT")
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
    {
        print!("{text}");
    }
    let code = std::env::var("FSCALE_FAKE_CLI_EXIT")
        .ok()
        .and_then(|c| c.parse::<u8>().ok())
        .unwrap_or(0);
    ExitCode::from(code)
}
