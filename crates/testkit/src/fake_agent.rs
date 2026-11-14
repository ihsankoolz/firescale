//! A stand-in agent for the guarded runner's tests (M27).
//!
//! - `fscale-fake-agent --version` prints a version, unless the scenario
//!   says `"installed": false` (then it exits 127).
//! - `fscale-fake-agent auth status` exits 0, or 1 when the scenario says
//!   `"authenticated": false`.
//! - `fscale-fake-agent run <request.json>` reads the request, appends it to
//!   the file `FSCALE_FAKE_AGENT_LOG` names (with its working folder and
//!   the git environment it was given), writes the scenario's `write` files
//!   (relative to its working folder; `null` deletes), runs its `commands`,
//!   sleeps `sleepSecs`, and prints `result` as JSON.
//!
//! The scenario is the JSON file `FSCALE_FAKE_AGENT_SCENARIO` names.

use std::io::Write;
use std::process::{Command, ExitCode};

fn scenario() -> serde_json::Value {
    std::env::var("FSCALE_FAKE_AGENT_SCENARIO")
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let scenario = scenario();
    let flag = |name: &str| scenario.get(name).and_then(serde_json::Value::as_bool);
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["--version"] => {
            if flag("installed") == Some(false) {
                return ExitCode::from(127);
            }
            println!("fake-agent 1.0.0");
            ExitCode::SUCCESS
        }
        ["auth", "status"] => {
            if flag("authenticated") == Some(false) {
                println!("{{\"loggedIn\":false}}");
                return ExitCode::from(1);
            }
            println!("{{\"loggedIn\":true}}");
            ExitCode::SUCCESS
        }
        ["run", file] => run(&scenario, std::fs::read_to_string(file).unwrap_or_default()),
        _ => {
            eprintln!("usage: fscale-fake-agent --version | auth status | run <request.json>");
            ExitCode::from(2)
        }
    }
}

fn run(scenario: &serde_json::Value, input: String) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_default();
    if let Ok(log) = std::env::var("FSCALE_FAKE_AGENT_LOG") {
        let request: serde_json::Value =
            serde_json::from_str(&input).unwrap_or(serde_json::Value::Null);
        let entry = serde_json::json!({
            "request": request,
            "cwd": cwd.to_string_lossy(),
            "gitConfigKey": std::env::var("GIT_CONFIG_KEY_0").ok(),
        });
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
        {
            let _ = writeln!(file, "{entry}");
        }
    }
    if let Some(writes) = scenario.get("write").and_then(serde_json::Value::as_object) {
        for (path, content) in writes {
            // An absolute path is used as it is: a test of a run that
            // writes outside its worktree.
            let target = if std::path::Path::new(path).is_absolute() {
                std::path::PathBuf::from(path)
            } else {
                path.split('/')
                    .fold(cwd.clone(), |dir, part| dir.join(part))
            };
            match content.as_str() {
                Some(text) => {
                    if let Some(parent) = target.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let _ = std::fs::write(&target, text);
                }
                None => {
                    let _ = std::fs::remove_file(&target);
                }
            }
        }
    }
    if let Some(commands) = scenario
        .get("commands")
        .and_then(serde_json::Value::as_array)
    {
        for command in commands.iter().filter_map(serde_json::Value::as_array) {
            let words: Vec<&str> = command
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect();
            if let Some((program, rest)) = words.split_first() {
                let _ = Command::new(program).args(rest).current_dir(&cwd).output();
            }
        }
    }
    if let Some(secs) = scenario
        .get("sleepSecs")
        .and_then(serde_json::Value::as_u64)
    {
        std::thread::sleep(std::time::Duration::from_secs(secs));
    }
    let result = scenario
        .get("result")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({ "ok": true }));
    println!("{result}");
    let code = scenario
        .get("exit")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}
