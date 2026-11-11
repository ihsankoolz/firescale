//! The `fscale` command.

use std::env;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use firescale_core::check::run_check;
use firescale_core::config::{find_root, load_config};
use firescale_core::explain::{explain_rule, find_rule};
use firescale_core::node::NodeHost;
use firescale_core::report::{format_human, format_json};
use firescale_core::select::select_rules;
use firescale_core::{version, Error, Result};

/// Success, and no enforced rule failed.
const EXIT_OK: u8 = 0;
/// An enforced rule has violations.
const EXIT_VIOLATIONS: u8 = 1;
/// Bad arguments, bad config, or an internal error.
const EXIT_USAGE: u8 = 2;

const HELP: &str = "fscale - knock the scale off a repo, and prove it's clean.

Usage:
  fscale check [--rules <glob>] [--json]   Run rules and report violations
  fscale explain <rule>                    Print what a rule checks and how to fix it
  fscale --version                         Print the firescale version
  fscale --help                            Print this help

Options:
  --rules <glob>   Only run matching rules, e.g. \"core/*\" (comma-separated)
  --json           Print the report as JSON (see docs/check-json.md)
  --cwd <dir>      Run as if started in <dir>

Exit codes:
  0  no enforced rule failed
  1  an enforced rule has violations
  2  bad usage, bad config, or an internal error
";

#[derive(Default)]
struct Args {
    version: bool,
    help: bool,
    json: bool,
    rules: Option<String>,
    cwd: Option<String>,
    positionals: Vec<String>,
}

fn parse_args(argv: impl Iterator<Item = String>) -> Result<Args> {
    let mut args = Args::default();
    let mut argv = argv.peekable();
    let mut only_positionals = false;
    while let Some(arg) = argv.next() {
        if only_positionals || !arg.starts_with('-') || arg == "-" {
            args.positionals.push(arg);
            continue;
        }
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) if name.starts_with("--") => {
                (name.to_owned(), Some(value.to_owned()))
            }
            _ => (arg.clone(), None),
        };
        let mut value = |name: &str| -> Result<String> {
            inline
                .clone()
                .or_else(|| argv.next())
                .ok_or_else(|| Error::usage(format!("option {name} needs a value.")))
        };
        match name.as_str() {
            "--" => only_positionals = true,
            "-v" | "--version" => args.version = true,
            "-h" | "--help" => args.help = true,
            "--json" => args.json = true,
            "--rules" => args.rules = Some(value("--rules <glob>")?),
            "--cwd" => args.cwd = Some(value("--cwd <dir>")?),
            _ => return Err(Error::usage(format!("unknown option \"{name}\"."))),
        }
    }
    Ok(args)
}

/// Text for stdout and stderr, plus the exit code.
struct Output {
    stdout: String,
    notes: Vec<String>,
    code: u8,
}

impl Output {
    fn ok(stdout: impl Into<String>) -> Self {
        Self {
            stdout: stdout.into(),
            notes: Vec::new(),
            code: EXIT_OK,
        }
    }
}

fn check(root: &std::path::Path, args: &Args, extra: &[String]) -> Result<Output> {
    if !extra.is_empty() {
        return Err(Error::usage(format!(
            "check takes no arguments, got \"{}\".",
            extra.join(" ")
        )));
    }
    let packs = firescale_rules::builtin_packs();
    let mut host = NodeHost::new(root);
    let config = load_config(root, &packs, &mut host)?;
    let selection = select_rules(config.rules, args.rules.as_deref())?;
    let report = run_check(root, &selection.rules, version(), &mut host)?;
    Ok(Output {
        stdout: if args.json {
            format_json(&report)
        } else {
            format_human(&report)
        },
        notes: selection.notes,
        code: if report.ok { EXIT_OK } else { EXIT_VIOLATIONS },
    })
}

fn explain(root: &std::path::Path, rest: &[String]) -> Result<Output> {
    let [query] = rest else {
        return Err(Error::usage(
            "explain needs exactly one rule id, for example: fscale explain core/no-committed-env",
        ));
    };
    let packs = firescale_rules::builtin_packs();
    let mut host = NodeHost::new(root);
    let config = load_config(root, &packs, &mut host)?;
    Ok(Output::ok(explain_rule(find_rule(&config, query)?)?))
}

fn run() -> Result<Output> {
    let args = parse_args(env::args().skip(1))?;
    let base = env::current_dir().map_err(|error| {
        Error::user(format!(
            "Could not read the current folder ({error}). Run fscale from a folder that exists."
        ))
    })?;
    let cwd = match &args.cwd {
        Some(dir) if PathBuf::from(dir).is_absolute() => PathBuf::from(dir),
        Some(dir) => base.join(dir),
        None => base,
    };
    if !cwd.is_dir() {
        return Err(Error::user(format!(
            "Folder \"{}\" does not exist. Check the --cwd value.",
            cwd.display()
        )));
    }

    if args.version {
        return Ok(Output::ok(format!("{}\n", version())));
    }
    let Some((command, rest)) = args.positionals.split_first() else {
        return Ok(Output::ok(HELP));
    };
    if args.help {
        return Ok(Output::ok(HELP));
    }
    let root = find_root(&cwd);
    match command.as_str() {
        "check" => check(&root, &args, rest),
        "explain" => explain(&root, rest),
        _ => Err(Error::usage(format!("unknown command \"{command}\"."))),
    }
}

fn main() -> ExitCode {
    let (stdout, stderr, code) = match run() {
        Ok(output) => {
            let notes: String = output.notes.iter().map(|note| format!("{note}\n")).collect();
            (output.stdout, notes, output.code)
        }
        Err(Error::Usage(message)) => (
            String::new(),
            format!("fscale: {message}\nRun `fscale --help` to see the available commands and options.\n"),
            EXIT_USAGE,
        ),
        Err(Error::User(message)) => (String::new(), format!("fscale: {message}\n"), EXIT_USAGE),
        Err(Error::Internal(message)) => (
            String::new(),
            format!(
                "fscale: unexpected error. Please report it at https://github.com/ihsankoolz/firescale/issues\n{message}\n"
            ),
            EXIT_USAGE,
        ),
    };
    // A closed pipe (for example `fscale check | head`) is not an error worth reporting.
    let _ = std::io::stderr().write_all(stderr.as_bytes());
    let _ = std::io::stdout().write_all(stdout.as_bytes());
    let _ = std::io::stdout().flush();
    ExitCode::from(code)
}
