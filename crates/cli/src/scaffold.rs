//! `fscale init`, `fscale adopt` and `fscale sync` (SPEC §15, ADR 0019).
//!
//! All three are the fix loop of `fscale fix`. They differ in where they
//! start from and in what they refuse:
//!
//! - `init` starts a new repo: `git init` if needed, a new config, the loop.
//! - `adopt` brings an existing repo in: a config if there is none, the loop.
//! - `sync` runs the loop again in a repo that already has a config.
//!
//! None of them has a template, and none of them overwrites a file. The one
//! file written here, the config, is created only when it does not exist.
//! Everything else is written by the fixes of the rules.

use std::fs;
use std::io::Write;
use std::path::Path;

use firescale_core::check::run_check;
use firescale_core::config::{
    find_root, load_config, starter_config, RuleSource, JSON_CONFIG, TS_CONFIG,
};
use firescale_core::fixloop::{LoopChange, LoopReport};
use firescale_core::git::{run_git, Git};
use firescale_core::model::Fixable;
use firescale_core::node::NodeHost;
use firescale_core::report::CheckReport;
use firescale_core::rule::Change;
use firescale_core::scratch::Scratch;
use firescale_core::{version, Error, Result};

use crate::{
    loop_failure, plan_fixes, plural, render_fixes, Args, Output, EXIT_OK, EXIT_VIOLATIONS,
};

/// The packs `init` turns on when `--packs` is not given (SPEC §16).
const INIT_PACKS: [&str; 2] = ["core", "agent-ready"];
/// The packs `adopt` turns on in a repo without a config. `agent-ready` is
/// opt-in for an existing repo (SPEC §6).
const ADOPT_PACKS: [&str; 1] = ["core"];
/// The heading under which the config file appears in the list of changes.
const SETUP: &str = "firescale";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Command {
    Init,
    Adopt,
    Sync,
}

impl Command {
    fn name(self) -> &'static str {
        match self {
            Self::Init => "init",
            Self::Adopt => "adopt",
            Self::Sync => "sync",
        }
    }
}

/// One run of the loop for one of the three commands.
struct Job<'a> {
    command: Command,
    root: &'a Path,
    /// Packs for the config to create. `None` when the repo has a config,
    /// which is then used as it is.
    new_config: Option<Vec<String>>,
    /// Work in a scratch copy and change nothing (`adopt --dry-run`, `sync --check`).
    dry_run: bool,
    /// Run `git init` first. Done after the config and the fix order have
    /// been validated, so that a refusal leaves nothing behind.
    git_init: bool,
    /// Lines to print before the list of changes.
    preface: Vec<String>,
}

fn has_config(root: &Path) -> bool {
    root.join(JSON_CONFIG).exists() || root.join(TS_CONFIG).exists()
}

/// Rejects arguments and options that the command does not take, so that a
/// mistyped safety flag is never silently ignored.
fn reject(command: Command, args: &Args, extra: &[String]) -> Result<()> {
    let name = command.name();
    if !extra.is_empty() {
        return Err(Error::usage(format!(
            "{name} takes no arguments, got \"{}\". It works on the current folder; use --cwd <dir> for another one.",
            extra.join(" ")
        )));
    }
    let not_here = |option: &str, home: &str| {
        Err(Error::usage(format!(
            "{name} does not take {option}. {home}"
        )))
    };
    if args.rules.is_some() {
        return not_here(
            "--rules",
            "It runs every rule that is on. Use `fscale fix --rules <glob>` to fix some rules only.",
        );
    }
    if args.dry_run && command != Command::Adopt {
        return not_here(
            "--dry-run",
            if command == Command::Sync {
                "Use `fscale sync --check` to see what would change."
            } else {
                "In an existing folder, use `fscale adopt --dry-run` to preview."
            },
        );
    }
    if args.check && command != Command::Sync {
        return not_here("--check", "It belongs to `fscale sync --check`.");
    }
    if args.force && command != Command::Init {
        return not_here("--force", "It belongs to `fscale init --force`.");
    }
    if args.packs.is_some() && command != Command::Init {
        return not_here(
            "--packs",
            "Packs are chosen with `fscale init --packs <list>`, or by editing \"packs\" in the firescale config.",
        );
    }
    Ok(())
}

/// Splits and validates `--packs`. Only built-in packs can be named: a custom
/// pack is a file, and a new repo has none yet.
fn parse_packs(list: &str) -> Result<Vec<String>> {
    let builtin = firescale_rules::builtin_packs();
    let available: Vec<&str> = builtin.iter().map(|pack| pack.name).collect();
    let mut packs: Vec<String> = Vec::new();
    for name in list
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        if !available.contains(&name) {
            return Err(Error::usage(format!(
                "--packs: unknown rule pack \"{name}\". Available packs: {}. Add a custom pack to \"packs\" in {JSON_CONFIG} after init.",
                available.join(", ")
            )));
        }
        if !packs.iter().any(|pack| pack == name) {
            packs.push(name.to_owned());
        }
    }
    if packs.is_empty() {
        return Err(Error::usage(format!(
            "--packs needs at least one pack, for example --packs core. Available packs: {}.",
            available.join(", ")
        )));
    }
    Ok(packs)
}

/// The names in `dir`, sorted, without git's own folder.
fn entries(dir: &Path) -> Result<Vec<String>> {
    let failed = |error: std::io::Error| {
        Error::user(format!(
            "Could not list the folder {} ({error}). Check its permissions.",
            dir.display()
        ))
    };
    let mut names = Vec::new();
    for entry in fs::read_dir(dir).map_err(failed)? {
        let name = entry.map_err(failed)?.file_name();
        let name = name.to_string_lossy().into_owned();
        if name != ".git" {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

/// `fscale init`: a new repo from an empty folder.
///
/// `cwd` is the folder itself, not a repo root found by walking up: `init`
/// makes a root.
pub(crate) fn init(cwd: &Path, args: &Args, extra: &[String]) -> Result<Output> {
    reject(Command::Init, args, extra)?;
    let configured = has_config(cwd);
    if configured {
        if args.packs.is_some() {
            return Err(Error::usage(
                "this folder already has a firescale config, so --packs would be ignored. Edit \"packs\" in the config, then run `fscale sync`.",
            ));
        }
    } else if !args.force {
        // A folder inside another repo belongs to that repo: the files
        // would land in a subfolder of it and be staged in its index.
        let outer = find_root(cwd);
        if outer != cwd {
            let (what, instead) = if has_config(&outer) {
                (
                    "firescale repo",
                    "Run `fscale sync` there to repair that repo",
                )
            } else {
                ("git repo", "Run `fscale adopt` there to bring that repo in")
            };
            return Err(Error::user(format!(
                "This folder is inside the {what} at {}, so `fscale init` will not start a second one here. {instead}, or pass --force to make this folder a firescale repo of its own.",
                shown(&outer)
            )));
        }
        let existing = entries(cwd)?;
        if !existing.is_empty() {
            let shown: Vec<&str> = existing.iter().take(3).map(String::as_str).collect();
            let more = if existing.len() > shown.len() {
                ", ..."
            } else {
                ""
            };
            return Err(Error::user(format!(
                "This folder is not empty ({}: {}{more}) and is not a firescale repo, so `fscale init` will not touch it.\n\nFor a project that already exists, use adopt. It only adds what is missing and never overwrites a file:\n  fscale adopt --dry-run   preview the changes\n  fscale adopt             apply them\n\nTo run init here anyway, pass --force. It adds the same files as adopt, with init's packs.",
                if existing.len() == 1 {
                    "1 entry".to_owned()
                } else {
                    format!("{} entries", existing.len())
                },
                shown.join(", ")
            )));
        }
    }
    let packs = match &args.packs {
        Some(list) => parse_packs(list)?,
        None => INIT_PACKS.iter().map(|pack| (*pack).to_owned()).collect(),
    };

    let mut preface = Vec::new();
    let git_init = !Git::new(cwd).is_repo()?;
    if configured {
        preface
            .push("This folder already has a firescale config. It was kept as it is.".to_owned());
    }
    run(Job {
        command: Command::Init,
        root: cwd,
        new_config: (!configured).then_some(packs),
        dry_run: false,
        git_init,
        preface,
    })
}

/// `fscale adopt`: the loop on a repo that exists. Adds what is missing.
pub(crate) fn adopt(root: &Path, args: &Args, extra: &[String]) -> Result<Output> {
    reject(Command::Adopt, args, extra)?;
    let new_config =
        (!has_config(root)).then(|| ADOPT_PACKS.iter().map(|pack| (*pack).to_owned()).collect());
    run(Job {
        command: Command::Adopt,
        root,
        new_config,
        dry_run: args.dry_run,
        git_init: false,
        preface: Vec::new(),
    })
}

/// `fscale sync`: the loop again, to repair drift. `--check` only reports.
pub(crate) fn sync(root: &Path, args: &Args, extra: &[String]) -> Result<Output> {
    reject(Command::Sync, args, extra)?;
    if !has_config(root) {
        return Err(Error::user(format!(
            "No firescale config found in {}, so there is nothing to sync with. Run `fscale adopt` in a project that already exists, or `fscale init` in an empty folder.",
            shown(root)
        )));
    }
    run(Job {
        command: Command::Sync,
        root,
        new_config: None,
        dry_run: args.check,
        git_init: false,
        preface: Vec::new(),
    })
}

/// A path for output: forward slashes on every OS.
fn shown(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

/// Creates the config file. Fails when a file is already there: `create_new`
/// makes "never overwrite" a property of the call, not of a check before it.
fn create_config(root: &Path, text: &str) -> Result<()> {
    let failed = |error: std::io::Error| {
        Error::user(format!(
            "Could not create {JSON_CONFIG} ({error}). Check the folder's permissions, and that no such file exists already."
        ))
    };
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join(JSON_CONFIG))
        .map_err(failed)?;
    file.write_all(text.as_bytes()).map_err(failed)
}

fn fix_hint(fixable: Fixable) -> &'static str {
    match fixable {
        Fixable::Deterministic => "deterministic fix available",
        Fixable::Agent => "agent-fixable",
        Fixable::Manual => "manual fix",
    }
}

/// The part of the output that says what the loop could not do: violations
/// that need an agent or a person, and the notes of the fixes that ran.
fn render_left(
    check: &CheckReport,
    report: &LoopReport,
    dry_run: bool,
    unlisted: &[String],
) -> String {
    let mut out = String::new();
    let count = check.diagnostics.len();
    if count == 0 {
        out.push_str(if dry_run {
            "Left for you afterwards: no violations.\n"
        } else {
            "Left for you: no violations.\n"
        });
    } else {
        out.push_str(&format!(
            "Left for you{}: {} that a fix cannot repair. A person or an agent has to.\n",
            if dry_run { " afterwards" } else { "" },
            plural(count, "violation")
        ));
        for rule in check.rules.iter().filter(|rule| rule.violations > 0) {
            out.push_str(&format!("  {}\n", rule.id));
            for diagnostic in check.diagnostics.iter().filter(|d| d.rule_id == rule.id) {
                let line = diagnostic
                    .range
                    .map_or_else(String::new, |range| format!(":{}", range.start_line));
                out.push_str(&format!(
                    "    {}{line}: {} [{}]\n",
                    diagnostic.file,
                    diagnostic.message,
                    fix_hint(diagnostic.fixable)
                ));
            }
            out.push_str(&format!("    How to fix: fscale explain {}\n", rule.id));
        }
    }
    if !unlisted.is_empty() {
        out.push_str(&format!(
            "  Not listed: what custom rules would still report ({}). A dry run cannot check them; `fscale check` after the real run shows it.\n",
            unlisted.join(", ")
        ));
    }
    // A note is a fix telling a person what to look at in a file it made.
    let mut notes: Vec<(&str, &str)> = Vec::new();
    for (rule, note) in &report.notes {
        if !notes.contains(&(rule.as_str(), note.as_str())) {
            notes.push((rule, note));
        }
    }
    if !notes.is_empty() {
        out.push_str(if dry_run {
            "\nTo review afterwards:\n"
        } else {
            "\nTo review:\n"
        });
        for (rule, note) in notes {
            out.push_str(&format!("  {rule}: {note}\n"));
        }
    }
    out
}

fn run(job: Job<'_>) -> Result<Output> {
    let Job {
        command,
        root,
        new_config,
        dry_run,
        git_init,
        mut preface,
    } = job;
    let name = format!("fscale {}", command.name());
    let packs = firescale_rules::builtin_packs();
    let mut host = NodeHost::new(root);

    // Everything that can be wrong with the config or the rules' order is
    // found here, before a single file is written.
    let (config, new_config_text) = match &new_config {
        Some(names) => {
            let (text, config) = starter_config(names, &packs)?;
            (config, Some(text))
        }
        None => (load_config(root, &packs, &mut host)?, None),
    };
    let plan = plan_fixes(config.rules, &packs, None)?;

    // The folder everything below happens in. It can be an ancestor of the
    // folder the command was started in, so it is always named.
    preface.insert(0, format!("Repo root: {}", shown(root)));
    if git_init {
        let result = run_git(root, &["init", "--quiet"])?;
        if result.code != 0 {
            return Err(Error::user(format!(
                "git init failed: {}. Fix the git error above, then run `{name}` again.",
                result.stderr.trim()
            )));
        }
        preface.push("Initialised a git repository (git init).".to_owned());
    }

    let scratch = if dry_run {
        Some(Scratch::copy_of(root)?)
    } else {
        None
    };
    let work_root = scratch.as_ref().map_or(root, Scratch::root);

    let mut setup: Vec<LoopChange> = Vec::new();
    if let Some(text) = new_config_text {
        create_config(work_root, &text)?;
        setup.push(LoopChange {
            pass: 0,
            rule: SETUP.to_owned(),
            change: Change::Write {
                file: JSON_CONFIG.to_owned(),
                before: None,
                after: text,
            },
        });
    }
    let with_setup = |mut report: LoopReport| {
        report.changes.splice(0..0, setup.iter().cloned());
        report
    };

    let report = match plan.run(work_root, &mut host) {
        Ok(report) => with_setup(report),
        Err(mut failure) => {
            failure.report = with_setup(failure.report);
            // What happened before the loop is part of what happened.
            let error = loop_failure(&failure, dry_run, &name);
            return Err(Error::user(format!(
                "{}\n\n{}",
                preface.join("\n"),
                error.message()
            )));
        }
    };

    // What is left: one more check, on the result. In a dry run the result is
    // in the scratch copy, where custom rules cannot be checked (the Node host
    // reads the real repo), so they are named instead of listed.
    let notes = plan.selection.notes;
    let (checked, skipped): (Vec<_>, Vec<_>) = plan
        .selection
        .rules
        .into_iter()
        .partition(|rule| !dry_run || matches!(rule.source, RuleSource::Builtin(_)));
    let unlisted: Vec<String> = skipped.into_iter().map(|rule| rule.meta.id).collect();
    let check = run_check(work_root, &checked, version(), &mut host, false)?.report;

    let done = render_fixes(&report, dry_run, false);
    let mut out = String::new();
    for line in &preface {
        out.push_str(&format!("{line}\n"));
    }
    out.push('\n');
    out.push_str(&done.text);
    let changes = plural(done.changes, "change");
    let headline = match (done.changes, dry_run, command) {
        (0, true, Command::Sync) => "In sync: `fscale sync` would change nothing.".to_owned(),
        (0, true, _) => format!("Dry run: `{name}` would change nothing."),
        (0, false, _) => "Nothing to change: every deterministic fix is already in place.".to_owned(),
        (_, true, Command::Sync) => format!(
            "Out of sync: `fscale sync` would make {changes}. Nothing was changed. Run `fscale sync` to apply."
        ),
        (_, true, _) => {
            format!("Dry run: {changes}. Nothing was changed. Run `{name}` to apply.")
        }
        (_, false, _) => format!("Done: {changes}."),
    };
    out.push_str(&format!("{headline}\n\n"));
    out.push_str(&render_left(&check, &report, dry_run, &unlisted));
    if !dry_run && done.changes > 0 {
        out.push_str(
            "\nNext: review the changes with `git status` and `git diff`, then commit them.\n",
        );
    }
    Ok(Output {
        stdout: out,
        notes,
        code: if dry_run && command == Command::Sync && done.changes > 0 {
            EXIT_VIOLATIONS
        } else {
            EXIT_OK
        },
    })
}
