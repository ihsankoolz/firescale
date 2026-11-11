//! The `fscale` command.

use std::env;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use firescale_core::affected::{changed_files, narrow_report, Changed};
use firescale_core::check::run_check;
use firescale_core::config::{find_root, load_config, RuleSource};
use firescale_core::diff::unified_diff;
use firescale_core::explain::{explain_rule, find_rule};
use firescale_core::facts::{detect_facts, format_facts_human, format_facts_json};
use firescale_core::node::NodeHost;
use firescale_core::report::{format_human, format_json};
use firescale_core::rule::{fix_rule_with, Change};
use firescale_core::sarif::format_sarif;
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
  fscale check [options]       Run rules and report violations
  fscale fix [options]         Apply deterministic fixes
  fscale explain <rule>        Print what a rule checks and how to fix it
  fscale facts [--json]        Print detected repo facts
  fscale --version             Print the firescale version
  fscale --help                Print this help

Options:
  --rules <glob>   check, fix: only matching rules, e.g. \"core/*\" (comma-separated)
  --json           check, facts: print JSON (see docs/check-json.md, docs/facts.md)
  --sarif          check: print SARIF 2.1.0 (see docs/sarif.md)
  --affected       check: only report files that changed, and projects that depend on them
  --base <ref>     With --affected: also count commits since the merge base with <ref>
  --no-cache       check: run every rule again, ignoring .firescale/cache
  --dry-run        fix: print what would change as a diff, and change nothing
  --cwd <dir>      Run as if started in <dir>

Exit codes:
  0  no enforced rule failed
  1  an enforced rule has violations
  2  bad usage, bad config, or an internal error
";

#[derive(Debug, Default)]
struct Args {
    version: bool,
    help: bool,
    json: bool,
    sarif: bool,
    affected: bool,
    base: Option<String>,
    no_cache: bool,
    dry_run: bool,
    rules: Option<String>,
    cwd: Option<String>,
    positionals: Vec<String>,
}

/// Parses the command line. Follows the conventions the CLI has always had:
/// `--name value` or `--name=value`, grouped short flags (`-vh`), and `--` to
/// end the options.
fn parse_args(argv: impl Iterator<Item = String>) -> Result<Args> {
    let mut args = Args::default();
    let mut argv = argv;
    let mut only_positionals = false;
    while let Some(arg) = argv.next() {
        if only_positionals || !arg.starts_with('-') || arg == "-" {
            args.positionals.push(arg);
            continue;
        }
        if arg == "--" {
            only_positionals = true;
            continue;
        }
        if !arg.starts_with("--") {
            for flag in arg.chars().skip(1) {
                match flag {
                    'v' => args.version = true,
                    'h' => args.help = true,
                    _ => return Err(Error::usage(format!("unknown option \"-{flag}\"."))),
                }
            }
            continue;
        }
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) => (name.to_owned(), Some(value.to_owned())),
            None => (arg.clone(), None),
        };
        match name.as_str() {
            "--version" | "--help" | "--json" | "--sarif" | "--no-cache" | "--dry-run"
            | "--affected" => {
                if inline.is_some() {
                    return Err(Error::usage(format!(
                        "option {name} does not take a value."
                    )));
                }
                match name.as_str() {
                    "--version" => args.version = true,
                    "--help" => args.help = true,
                    "--no-cache" => args.no_cache = true,
                    "--dry-run" => args.dry_run = true,
                    "--sarif" => args.sarif = true,
                    "--affected" => args.affected = true,
                    _ => args.json = true,
                }
            }
            "--rules" | "--cwd" | "--base" => {
                let value = match inline.or_else(|| argv.next()) {
                    Some(value) if arg.contains('=') || !value.starts_with('-') => value,
                    Some(value) => {
                        return Err(Error::usage(format!(
                            "option {name} needs a value, but got \"{value}\". To pass a value that starts with \"-\", write {name}=<value>."
                        )));
                    }
                    None => return Err(Error::usage(format!("option {name} needs a value."))),
                };
                match name.as_str() {
                    "--rules" => args.rules = Some(value),
                    "--base" => args.base = Some(value),
                    _ => args.cwd = Some(value),
                }
            }
            _ => return Err(Error::usage(format!("unknown option \"{name}\"."))),
        }
    }
    Ok(args)
}

/// Removes `.` and `..` segments without touching the filesystem, so that
/// walking up with `Path::parent` really goes to the parent folder.
fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component);
                }
            }
            other => normalized.push(other),
        }
    }
    normalized
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

fn check(root: &Path, args: &Args, extra: &[String]) -> Result<Output> {
    if !extra.is_empty() {
        return Err(Error::usage(format!(
            "check takes no arguments, got \"{}\".",
            extra.join(" ")
        )));
    }
    if args.base.is_some() && !args.affected {
        return Err(Error::usage(
            "--base only works together with --affected. Add --affected.",
        ));
    }
    if args.json && args.sarif {
        return Err(Error::usage(
            "--json and --sarif cannot be used together. Pass only one of them.",
        ));
    }
    let packs = firescale_rules::builtin_packs();
    let mut host = NodeHost::new(root);
    let config = load_config(root, &packs, &mut host)?;
    let selection = select_rules(config.rules, args.rules.as_deref())?;
    let outcome = run_check(root, &selection.rules, version(), &mut host, !args.no_cache)?;
    let mut report = outcome.report;
    let mut notes = selection.notes;
    if args.affected {
        let changed = changed_files(root, args.base.as_deref())?;
        if changed == Changed::Everything {
            notes.push(
                "note: --affected found no commit to compare with, so every file counts as changed."
                    .to_owned(),
            );
        }
        narrow_report(&mut report, &detect_facts(root)?, &changed);
    }
    notes.extend(outcome.warnings);
    // FSCALE_LOG=cache shows, per rule, whether its result came from the cache.
    if env::var("FSCALE_LOG").is_ok_and(|value| value.split(',').any(|part| part == "cache")) {
        notes.extend(
            outcome
                .cache
                .iter()
                .map(|(id, outcome)| format!("cache: {} {id}", outcome.as_str())),
        );
    }
    Ok(Output {
        stdout: if args.json {
            format_json(&report)
        } else if args.sarif {
            format_sarif(&report)
        } else {
            format_human(&report)
        },
        notes,
        code: if report.ok { EXIT_OK } else { EXIT_VIOLATIONS },
    })
}

fn plural(count: usize, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

/// `fscale fix`: applies the deterministic fix of every selected rule.
fn fix(root: &Path, args: &Args, extra: &[String]) -> Result<Output> {
    if !extra.is_empty() {
        return Err(Error::usage(format!(
            "fix takes no arguments, got \"{}\".",
            extra.join(" ")
        )));
    }
    let packs = firescale_rules::builtin_packs();
    let mut host = NodeHost::new(root);
    let config = load_config(root, &packs, &mut host)?;
    let selection = select_rules(config.rules, args.rules.as_deref())?;
    let notes = selection.notes;

    let mut out = String::new();
    let (mut total, mut rules_changed) = (0, 0);
    for rule in &selection.rules {
        let attempt = match &rule.source {
            RuleSource::Builtin(builtin) => {
                fix_rule_with(*builtin, root, &rule.options, args.dry_run)
            }
            // A custom rule's fix runs in the Node host, through the SDK.
            RuleSource::Custom { pack, .. } if rule.meta.has_fix => {
                host.fix_rule(pack, &rule.meta.id, &rule.options, args.dry_run)
            }
            RuleSource::Custom { .. } => continue,
        };
        // A failure must not hide what earlier rules already changed.
        let outcome = match attempt {
            Ok(outcome) => outcome,
            Err(error) if total > 0 && !args.dry_run => {
                return Err(Error::user(format!(
                    "{}\n\nThe fix of {} failed after these changes had already been made:\n\n{}\nReview them with `git status` and `git diff`, then run `fscale fix` again.",
                    error.message(),
                    rule.meta.id,
                    out.trim_end()
                )));
            }
            Err(error) => return Err(error),
        };
        if outcome.changes.is_empty() {
            continue;
        }
        total += outcome.changes.len();
        rules_changed += 1;
        out.push_str(&format!("{}\n", rule.meta.id));
        let mut diffs = String::new();
        for change in &outcome.changes {
            match change {
                Change::Untrack { file } => {
                    let verb = if args.dry_run {
                        "would untrack"
                    } else {
                        "untracked"
                    };
                    out.push_str(&format!("  {verb} {file} (the file stays on disk)\n"));
                }
                Change::Write {
                    file,
                    before,
                    after,
                } => {
                    let verb = match (args.dry_run, before.is_some()) {
                        (true, true) => "would edit",
                        (true, false) => "would create",
                        (false, true) => "edited",
                        (false, false) => "created",
                    };
                    out.push_str(&format!("  {verb} {file}\n"));
                    if args.dry_run {
                        diffs.push_str(&unified_diff(file, before.as_deref(), after));
                    }
                }
            }
        }
        // Notes describe what was done, so they only make sense after a real fix.
        if !args.dry_run {
            for note in &outcome.result.notes {
                out.push_str(&format!("  note: {note}\n"));
            }
        }
        if !diffs.is_empty() {
            out.push_str(&format!("\n{diffs}"));
        }
        out.push('\n');
    }

    if total == 0 {
        out.push_str("Nothing to fix: no deterministic fix has anything left to do.\n");
    } else if args.dry_run {
        out.push_str(&format!(
            "Dry run: {} in {}. Nothing was changed. Run `fscale fix` to apply.\n",
            plural(total, "change"),
            plural(rules_changed, "rule")
        ));
    } else {
        out.push_str(&format!(
            "Fixed: {} in {}. Run `fscale check` to see what is left.\n",
            plural(total, "change"),
            plural(rules_changed, "rule")
        ));
    }
    Ok(Output {
        stdout: out,
        notes,
        code: EXIT_OK,
    })
}

fn explain(root: &Path, rest: &[String]) -> Result<Output> {
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

/// `fscale facts`: detected stack and project graph. Needs no config and no Node.
fn facts(root: &Path, args: &Args, extra: &[String]) -> Result<Output> {
    if !extra.is_empty() {
        return Err(Error::usage(format!(
            "facts takes no arguments, got \"{}\".",
            extra.join(" ")
        )));
    }
    let facts = detect_facts(root)?;
    Ok(Output::ok(if args.json {
        format_facts_json(&facts)
    } else {
        format_facts_human(&facts)
    }))
}

fn run() -> Result<Output> {
    let args = parse_args(env::args().skip(1))?;
    let base = env::current_dir().map_err(|error| {
        Error::user(format!(
            "Could not read the current folder ({error}). Run fscale from a folder that exists."
        ))
    })?;
    let cwd = normalize(&match &args.cwd {
        Some(dir) if Path::new(dir).is_absolute() => PathBuf::from(dir),
        Some(dir) => base.join(dir),
        None => base,
    });
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
        "fix" => fix(&root, &args, rest),
        "explain" => explain(&root, rest),
        "facts" => facts(&root, &args, rest),
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

#[cfg(test)]
mod tests {
    use super::{normalize, parse_args, Args};
    use std::path::Path;

    fn parse(argv: &[&str]) -> Result<Args, String> {
        parse_args(argv.iter().map(|arg| (*arg).to_owned())).map_err(|error| error.to_string())
    }

    #[test]
    fn parses_commands_flags_and_values() {
        let args = parse(&["check", "--json", "--rules", "core/*", "--cwd=some dir"]).unwrap();
        assert_eq!(args.positionals, ["check"]);
        assert!(args.json);
        assert_eq!(args.rules.as_deref(), Some("core/*"));
        assert_eq!(args.cwd.as_deref(), Some("some dir"));
    }

    #[test]
    fn accepts_short_flags_alone_or_grouped() {
        assert!(parse(&["-v"]).unwrap().version);
        assert!(parse(&["-h"]).unwrap().help);
        let both = parse(&["-vh"]).unwrap();
        assert!(both.version && both.help);
        assert_eq!(parse(&["-x"]).unwrap_err(), "unknown option \"-x\".");
    }

    #[test]
    fn double_dash_ends_the_options() {
        let args = parse(&["explain", "--", "--json"]).unwrap();
        assert_eq!(args.positionals, ["explain", "--json"]);
        assert!(!args.json);
    }

    #[test]
    fn rejects_unknown_options_and_misused_values() {
        assert_eq!(
            parse(&["--nope"]).unwrap_err(),
            "unknown option \"--nope\"."
        );
        assert_eq!(
            parse(&["--json=1"]).unwrap_err(),
            "option --json does not take a value."
        );
        assert_eq!(
            parse(&["--version=x"]).unwrap_err(),
            "option --version does not take a value."
        );
        assert_eq!(
            parse(&["check", "--rules"]).unwrap_err(),
            "option --rules needs a value."
        );
        assert!(parse(&["check", "--rules", "--json"])
            .unwrap_err()
            .contains("needs a value, but got \"--json\""));
    }

    #[test]
    fn an_inline_value_may_start_with_a_dash() {
        assert_eq!(
            parse(&["check", "--rules=-x"]).unwrap().rules.as_deref(),
            Some("-x")
        );
    }

    #[test]
    fn normalize_removes_dot_segments() {
        assert_eq!(normalize(Path::new("a/b/../c/./d")), Path::new("a/c/d"));
        assert_eq!(normalize(Path::new("a/b/..")), Path::new("a"));
        assert_eq!(normalize(Path::new("../a")), Path::new("../a"));
    }
}
