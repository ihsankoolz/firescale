//! The `fscale` command.

mod agentfix;
mod scaffold;

use std::collections::BTreeSet;
use std::env;
use std::io::{IsTerminal, Write};
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use firescale_core::affected::{changed_files, narrow_report, Changed, RanRule};
use firescale_core::blame::{
    changed_since, choose_base, compare, violation_id, BaseChoice, BaseReport, BaseRun,
};
use firescale_core::check::run_check;
use firescale_core::config::{
    find_root, load_config, load_doctor_settings, load_verify_config, ConfiguredRule, RuleSource,
    JSON_CONFIG, TS_CONFIG,
};
use firescale_core::configkey;
use firescale_core::diff::unified_diff;
use firescale_core::doctor::{
    add_checks, format_doctor_human, format_doctor_json, run_doctor_with_plan, ThisMachine,
};
use firescale_core::explain::{explain_rule, find_rule};
use firescale_core::facts::{detect_facts, format_facts_human, format_facts_json};
use firescale_core::fixloop::{
    plan_order, run_fix_loop, LoopFailure, LoopReport, LoopRule, MAX_PASSES,
};
use firescale_core::graph::{
    affected_except, build_graph, format_graph_human, format_graph_json, Graph,
};
use firescale_core::hooks::{
    block_summary, check_path, check_start, claude_hook_timeout, decide, default_bin,
    has_fscale_script, hook_command, hook_file_changes, hook_timeout_for, install_hook_with,
    installed_prefixes, launch_note, max_blocks_from_env, may_start, no_download, not_downloaded,
    not_started, parse_hook_input, pinned_answer, pinned_version, probe_line, reply_with,
    start_verdict, timeout_seconds, Agent, HookFileChanges, Installed, KeptTimeout, AGENTS,
    CLAUDE_DEFAULT_TIMEOUT, MAX_HOOK_TIMEOUT,
};
use firescale_core::model::Status;
use firescale_core::node::NodeHost;
use firescale_core::process::{install_stop_handler, EXIT_STOPPED, STOP};
use firescale_core::progress::{format_duration, format_limit, heartbeat_from_env, Progress};
use firescale_core::report::{format_human, format_json, CheckReport};
use firescale_core::rule::{fix_rule_with, BuiltinPack, Change, FixOutcome};
use firescale_core::runs::RunRecord;
use firescale_core::sarif::format_sarif;
use firescale_core::scratch::Scratch;
use firescale_core::select::{select_rules, Selection};
use firescale_core::sharedcache::{CiRun, SharedCache};
use firescale_core::stepcache::{self, CacheSettings, StepCache};
use firescale_core::steptimes::StepTimes;
use firescale_core::tamper;
use firescale_core::verify::add_tamper;
use firescale_core::verify::{
    check_project_folders, format_budget_plan, format_verify_human, format_verify_json,
    format_verify_plan, limit_to_affected, plan_verify, planned_times, run_verify_with,
    stopped_in_check, CheckStep, Limit, RunOptions, VerifyPlan, VerifyReport, VerifyResult,
    DEFAULT_BASE_BUDGET, STOPPED_IN_CHECK, TAIL_LINES,
};
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
  fscale fix --agent <name> [--rule <id>]
                               Hand one rule's remaining violations to an agent, with guardrails
  fscale init [--packs <list>] Start a repo in an empty folder: config, then every fix
  fscale adopt [--dry-run]     Bring an existing repo in: add what is missing, overwrite nothing
  fscale sync [--check]        Repair drift: run the fixes again
  fscale verify [options]      Check, then lint, typecheck and test: the definition of done
  fscale tamper [options]      Only the tamper guard: what the change weakens, against the base
  fscale ledger [--json]       Sum the agent fix runs: runs, pass rate and cost, per rule and agent
  fscale explain <rule>        Print what a rule checks and how to fix it
  fscale facts [--json]        Print detected repo facts
  fscale graph [--explain]     Print the project graph; --explain says where each edge comes from
  fscale doctor [--json]       Check this machine: tools, env variables, dependencies, services,
                               agent hooks
  fscale add <generator> --json <input> [--dry-run]
                               Scaffold new files from JSON input (package, rule)
  fscale schema [<generator>]  Print a generator's input JSON Schema, or list the generators
  fscale hooks install         Make an agent run verify before it may stop (--agent, --all)
  fscale --version [--json]    Print the firescale version (--json: with its name)
  fscale --help                Print this help

Options:
  --rules <glob>   check, fix: only matching rules, e.g. \"core/*\" (comma-separated)
  --json           check, verify, facts, graph, doctor, ledger, --version: print JSON (see docs/)
  --sarif          check: print SARIF 2.1.0 (see docs/sarif.md)
  --affected       check, verify: only what changed, and projects that depend on it
  --base <ref>     With --affected: also count commits since the merge base with <ref>;
                   verify, tamper: compare with the merge base with <ref>
  --pr-base        verify, tamper, in GitHub Actions: the base from the pull request
                   event GitHub gives, never from refs in the clone
  --head <commit>  tamper: compare <commit> (read from git, nothing checked out)
                   instead of the working tree
  --approvals <file>
                   tamper --pr-base, in a pull_request_target run only: the pull
                   request's reviews from GitHub; a changed snapshot or expected
                   value then needs an approving review of its head commit
  --explain        graph: show where each edge comes from
  --no-cache       check: run every rule again, ignoring .firescale/cache;
                   verify: run every step again (passes are still stored)
  --dry-run        fix, adopt, add: print what would change as a diff, and change nothing;
                   verify: print what would run (with --affected, and why), run nothing
  --packs <list>   init: rule packs to turn on, comma-separated (default: core,agent-ready)
  --force          init: run in a folder that is not empty (nothing is overwritten)
  --check          sync: change nothing, and exit 1 when sync would change something
  --agent <name>   hooks: claude, codex or cursor; fix: the agent to run (claude, codex,
                   or fake, the stand-in the tests use)
  --rule <id>      fix --agent: the rule to fix (default: the first with violations a
                   deterministic fix cannot repair)
  --all            hooks install: every supported agent
  --bin <command>  hooks install: how the hook starts fscale (default: the repo's own
                   fscale package script, else the firescale it installs, else
                   npx --yes firescale@<this version>)
  --timeout <time> hooks install: the time limit written on Claude Code's stop hook
                   (30m, 1h 30m; default: from the steps' kept times, at least 10m)
  --jobs <n>       verify: run up to <n> projects at once (default: 1)
  --budget <time>  verify, not in CI and not in the stop hook: do not start a step that
                   took longer than <time> (30m, 90s, 1h 30m) when it last ran; it is
                   reported as NOT RUN, and the run is PARTIAL, never OK
  --shared-cache <dir>
                   verify, in GitHub Actions only: use step results that CI on the
                   default branch kept in <dir>, and keep them there on a push to it
  --progress       verify: print progress on stderr even when it is not a terminal
  --no-progress    verify: print no progress, even in a terminal
  --cwd <dir>      Run as if started in <dir>

Exit codes:
  0    no enforced rule failed
  1    an enforced rule has violations, verify failed, sync --check found drift,
       or doctor found a problem
  2    bad usage, bad config, or an internal error
  130  verify was stopped by Ctrl+C or a termination signal
";

#[derive(Debug, Default, Clone)]
struct Args {
    version: bool,
    help: bool,
    json: bool,
    sarif: bool,
    affected: bool,
    explain: bool,
    base: Option<String>,
    no_cache: bool,
    dry_run: bool,
    all: bool,
    force: bool,
    check: bool,
    packs: Option<String>,
    rules: Option<String>,
    agent: Option<String>,
    bin: Option<String>,
    cwd: Option<String>,
    /// `--progress` (true) or `--no-progress` (false).
    progress: Option<bool>,
    jobs: Option<String>,
    /// `--budget <time>`: verify leaves out steps known to take longer.
    budget: Option<String>,
    /// `--timeout <time>`: hooks install, Claude Code's stop hook limit.
    timeout: Option<String>,
    shared_cache: Option<String>,
    /// `--pr-base`: the base from GitHub's pull request event (ADR 0045).
    pr_base: bool,
    /// `--head <commit>`: tamper compares this commit, not the working tree.
    head: Option<String>,
    /// `--approvals <file>`: the pull request's reviews from GitHub (M26).
    approvals: Option<String>,
    /// `--rule <id>`: fix --agent works on this rule (M27).
    rule: Option<String>,
    positionals: Vec<String>,
}

/// Every long option that takes no value.
const FLAG_OPTIONS: &[&str] = &[
    "--version",
    "--help",
    "--json",
    "--sarif",
    "--no-cache",
    "--dry-run",
    "--affected",
    "--all",
    "--force",
    "--check",
    "--progress",
    "--no-progress",
    "--explain",
    "--pr-base",
];

/// Every long option that takes a value (`--name value` or `--name=value`).
/// [`parse_args`] knows an option only when it is in one of these two lists,
/// and [`positionals_of`] uses this one to skip the values, so an option
/// added here is handled by both.
const VALUE_OPTIONS: &[&str] = &[
    "--rules",
    "--cwd",
    "--base",
    "--agent",
    "--bin",
    "--packs",
    "--jobs",
    "--budget",
    "--timeout",
    "--shared-cache",
    "--head",
    "--approvals",
    "--rule",
];

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
        if FLAG_OPTIONS.contains(&name.as_str()) {
            if inline.is_some() {
                return Err(Error::usage(format!(
                    "option {name} does not take a value."
                )));
            }
            match name.as_str() {
                "--version" => args.version = true,
                "--help" => args.help = true,
                "--json" => args.json = true,
                "--no-cache" => args.no_cache = true,
                "--dry-run" => args.dry_run = true,
                "--sarif" => args.sarif = true,
                "--affected" => args.affected = true,
                "--explain" => args.explain = true,
                "--pr-base" => args.pr_base = true,
                "--all" => args.all = true,
                "--force" => args.force = true,
                "--check" => args.check = true,
                "--progress" => args.progress = Some(true),
                "--no-progress" => args.progress = Some(false),
                _ => return Err(not_stored(&name)),
            }
        } else if VALUE_OPTIONS.contains(&name.as_str()) {
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
                "--cwd" => args.cwd = Some(value),
                "--base" => args.base = Some(value),
                "--agent" => args.agent = Some(value),
                "--bin" => args.bin = Some(value),
                "--timeout" => args.timeout = Some(value),
                "--packs" => args.packs = Some(value),
                "--jobs" => args.jobs = Some(value),
                "--budget" => args.budget = Some(value),
                "--shared-cache" => args.shared_cache = Some(value),
                "--head" => args.head = Some(value),
                "--approvals" => args.approvals = Some(value),
                "--rule" => args.rule = Some(value),
                _ => return Err(not_stored(&name)),
            }
        } else {
            return Err(Error::usage(format!("unknown option \"{name}\".")));
        }
    }
    Ok(args)
}

/// An option that is in [`FLAG_OPTIONS`] or [`VALUE_OPTIONS`] but that
/// [`parse_args`] does not store: a mistake in fscale, caught by the tests.
fn not_stored(name: &str) -> Error {
    Error::internal(format!(
        "option {name} is listed but not read. Run fscale without it."
    ))
}

/// The words of a command line that are neither options nor the values of
/// options: the command and its arguments. Reads as [`parse_args`] does, but
/// never fails, so it also works on a command line `parse_args` refuses.
fn positionals_of(argv: &[String]) -> Vec<&str> {
    let mut words = Vec::new();
    let mut argv = argv.iter();
    let mut only_positionals = false;
    while let Some(arg) = argv.next() {
        if only_positionals || !arg.starts_with('-') || arg == "-" {
            words.push(arg.as_str());
        } else if arg == "--" {
            only_positionals = true;
        } else if VALUE_OPTIONS.contains(&arg.as_str()) {
            // `--name value`: the next word is the value, whatever it says.
            // `--name=value` is one word and is not in the list.
            argv.next();
        }
    }
    words
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
pub(crate) struct Output {
    pub(crate) stdout: String,
    pub(crate) notes: Vec<String>,
    pub(crate) code: u8,
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

/// What `--affected` needs of the rules that ran.
fn ran_rules<'a>(rules: &'a [ConfiguredRule<'_>]) -> Vec<RanRule<'a>> {
    rules
        .iter()
        .map(|rule| RanRule {
            meta: &rule.meta,
            custom: matches!(rule.source, RuleSource::Custom { .. }),
        })
        .collect()
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
        match &changed {
            Changed::Everything => notes.push(
                "note: --affected found no commit to compare with, so every file counts as changed."
                    .to_owned(),
            ),
            Changed::Shallow(note) => notes.push(format!(
                "note: --affected: {note}. Every file counts as changed."
            )),
            Changed::Files(_) => {}
        }
        narrow_report(
            &mut report,
            &detect_facts(root)?,
            &changed,
            &ran_rules(&selection.rules),
        );
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

pub(crate) fn plural(count: usize, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

/// One rule's part of a fix run: its change lines, and in a dry run the diffs.
pub(crate) struct RenderedFixes {
    pub(crate) text: String,
    pub(crate) changes: usize,
    pub(crate) rules: usize,
}

/// Renders what the loop changed, grouped by rule in the order the rules
/// first changed something. A file a rule wrote in several passes is listed
/// once, with the content from before its first write and after its last.
pub(crate) fn render_fixes(report: &LoopReport, dry_run: bool, show_notes: bool) -> RenderedFixes {
    let mut rules: Vec<&str> = Vec::new();
    for change in &report.changes {
        if !rules.contains(&change.rule.as_str()) {
            rules.push(&change.rule);
        }
    }
    let mut text = String::new();
    let mut total = 0;
    for rule in &rules {
        text.push_str(&format!("{rule}\n"));
        let mut diffs = String::new();
        let mut seen: Vec<(bool, &str)> = Vec::new();
        let own = || report.changes.iter().filter(|change| change.rule == *rule);
        for entry in own() {
            match &entry.change {
                Change::Untrack { file } => {
                    if seen.contains(&(false, file.as_str())) {
                        continue;
                    }
                    seen.push((false, file));
                    let verb = if dry_run {
                        "would untrack"
                    } else {
                        "untracked"
                    };
                    text.push_str(&format!("  {verb} {file} (the file stays on disk)\n"));
                }
                // Staging goes with the file's "created" line.
                Change::Track { .. } => {}
                Change::Write { file, before, .. } => {
                    if seen.contains(&(true, file.as_str())) {
                        continue;
                    }
                    seen.push((true, file));
                    let verb = match (dry_run, before.is_some()) {
                        (true, true) => "would edit",
                        (true, false) => "would create",
                        (false, true) => "edited",
                        (false, false) => "created",
                    };
                    // A fix may add the file it made to the git index.
                    let staged = own().any(|other| {
                        matches!(&other.change, Change::Track { file: tracked } if tracked == file)
                    });
                    let staged = match (staged, dry_run) {
                        (false, _) => "",
                        (true, true) => " (and stage it with git add)",
                        (true, false) => " (staged with git add)",
                    };
                    text.push_str(&format!("  {verb} {file}{staged}\n"));
                    if dry_run {
                        let last = own()
                            .filter_map(|later| match &later.change {
                                Change::Write {
                                    file: other, after, ..
                                } if other == file => Some(after),
                                _ => None,
                            })
                            .next_back();
                        if let Some(after) = last {
                            diffs.push_str(&unified_diff(file, before.as_deref(), after));
                        }
                    }
                }
            }
        }
        total += seen.len();
        // Notes describe what was done, so they only make sense after a real fix.
        if show_notes {
            let mut shown: Vec<&str> = Vec::new();
            for (_, note) in report.notes.iter().filter(|(owner, _)| owner == rule) {
                if !shown.contains(&note.as_str()) {
                    shown.push(note);
                    text.push_str(&format!("  note: {note}\n"));
                }
            }
        }
        if !diffs.is_empty() {
            text.push_str(&format!("\n{diffs}"));
        }
        text.push('\n');
    }
    RenderedFixes {
        text,
        changes: total,
        rules: rules.len(),
    }
}

/// The fix loop, planned: the rules that take part and the order of their fixes.
pub(crate) struct FixPlan<'a> {
    pub(crate) selection: Selection<'a>,
    selected: Vec<LoopRule>,
    /// Indices into `selected` of the rules that have a fix, in run order.
    with_fix: Vec<usize>,
}

/// Validates the rules' `after` declarations and works out the fix order.
/// Bad declarations are found here, before anything is written, over every
/// rule that is on, whatever `rules_glob` selects.
pub(crate) fn plan_fixes<'a>(
    rules: Vec<ConfiguredRule<'a>>,
    packs: &[BuiltinPack],
    rules_glob: Option<&str>,
) -> Result<FixPlan<'a>> {
    let loop_rule = |meta: &firescale_core::model::RuleMeta| LoopRule {
        id: meta.id.clone(),
        after: meta.after.clone(),
    };
    let known: BTreeSet<String> = packs
        .iter()
        .flat_map(|pack| pack.rules.iter().map(|rule| rule.meta().id))
        .chain(rules.iter().map(|rule| rule.meta.id.clone()))
        .collect();
    let enabled: Vec<LoopRule> = rules
        .iter()
        .filter(|rule| rule.status != Status::Off)
        .map(|rule| loop_rule(&rule.meta))
        .collect();
    plan_order(&enabled, &known)?;

    let selection = select_rules(rules, rules_glob)?;
    let selected: Vec<LoopRule> = selection
        .rules
        .iter()
        .map(|rule| loop_rule(&rule.meta))
        .collect();
    let order = plan_order(&selected, &known)?;
    // Rules without a fix take no part in the loop.
    let with_fix = order
        .into_iter()
        .filter(|index| selection.rules[*index].meta.has_fix)
        .collect();
    Ok(FixPlan {
        selection,
        selected,
        with_fix,
    })
}

impl FixPlan<'_> {
    pub(crate) fn has_fixes(&self) -> bool {
        !self.with_fix.is_empty()
    }

    /// Runs the loop in `work_root`: the repo, or a scratch copy of it.
    pub(crate) fn run(
        &self,
        work_root: &Path,
        host: &mut NodeHost,
    ) -> std::result::Result<LoopReport, LoopFailure> {
        let with_fix: Vec<&LoopRule> = self
            .with_fix
            .iter()
            .map(|index| &self.selected[*index])
            .collect();
        run_fix_loop(&with_fix, MAX_PASSES, |entry| {
            let Some(rule) = self
                .selection
                .rules
                .iter()
                .find(|rule| rule.meta.id == entry.id)
            else {
                return Ok(FixOutcome::default());
            };
            match &rule.source {
                RuleSource::Builtin(builtin) => {
                    fix_rule_with(*builtin, work_root, &rule.options, false)
                }
                // A custom rule's fix runs in the Node host, through the SDK.
                RuleSource::Custom { pack, .. } => {
                    host.fix_rule(work_root, pack, &rule.meta.id, &rule.options, false)
                }
            }
        })
    }
}

/// The error for a loop that stopped. It must not hide what earlier fixes
/// already changed. `command` is what the person ran, such as `fscale fix`.
pub(crate) fn loop_failure(failure: &LoopFailure, dry_run: bool, command: &str) -> Error {
    if failure.report.changes.is_empty() {
        return Error::user(failure.error.message());
    }
    let done = render_fixes(&failure.report, dry_run, !dry_run);
    let (lead, next) = if dry_run {
        (
            "This was a dry run, so nothing was changed. It had got this far:".to_owned(),
            "Fix the problem above, then run the dry run again.".to_owned(),
        )
    } else {
        (
            "The fix stopped after these changes had already been made:".to_owned(),
            format!("Review them with `git status` and `git diff`, then run `{command}` again."),
        )
    };
    Error::user(format!(
        "{}\n\n{lead}\n\n{}\n\n{next}",
        failure.error.message(),
        done.text.trim_end()
    ))
}

/// `fscale fix`: runs the deterministic fixes in dependency order, pass after
/// pass, until nothing is left to fix (ADR 0018).
fn fix(root: &Path, args: &Args, extra: &[String]) -> Result<Output> {
    if !extra.is_empty() {
        return Err(Error::usage(format!(
            "fix takes no arguments, got \"{}\".",
            extra.join(" ")
        )));
    }
    // One rule's violations handed to an agent, with guardrails (M27).
    if let Some(agent) = &args.agent {
        if args.dry_run {
            return Err(Error::usage(
                "fix --agent has no --dry-run: its run happens in a worktree of its own and is rolled back unless it passes.",
            ));
        }
        return agentfix::fix_with_agent(root, args, agent);
    }
    if args.rule.is_some() {
        return Err(Error::usage(
            "--rule goes with --agent. To fix some rules deterministically, use --rules <glob>.",
        ));
    }
    let packs = firescale_rules::builtin_packs();
    let mut host = NodeHost::new(root);
    let config = load_config(root, &packs, &mut host)?;
    let plan = plan_fixes(config.rules, &packs, args.rules.as_deref())?;

    // A dry run is the real loop in a scratch copy of the repo, so every fix
    // sees what the fixes before it wrote, and the repo itself is not touched.
    let scratch = if args.dry_run && plan.has_fixes() {
        Some(Scratch::copy_of(root)?)
    } else {
        None
    };
    let work_root = scratch.as_ref().map_or(root, Scratch::root);

    let report = plan
        .run(work_root, &mut host)
        .map_err(|failure| loop_failure(&failure, args.dry_run, "fscale fix"))?;

    let done = render_fixes(&report, args.dry_run, !args.dry_run);
    let mut out = done.text;
    if done.changes == 0 {
        out.push_str("Nothing to fix: no deterministic fix has anything left to do.\n");
    } else if args.dry_run {
        out.push_str(&format!(
            "Dry run: {} in {}. Nothing was changed. Run `fscale fix` to apply.\n",
            plural(done.changes, "change"),
            plural(done.rules, "rule")
        ));
    } else {
        out.push_str(&format!(
            "Fixed: {} in {}. Run `fscale check` to see what is left.\n",
            plural(done.changes, "change"),
            plural(done.rules, "rule")
        ));
    }
    Ok(Output {
        stdout: out,
        notes: plan.selection.notes,
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
    let rule = find_rule(&config, query).map_err(|error| {
        // A rule of a built-in pack that is not on gets a pointer to the pack.
        let pack = packs.iter().find(|pack| {
            !config.packs.iter().any(|enabled| enabled == pack.name)
                && pack.rules.iter().any(|rule| rule.meta().id == *query)
        });
        match pack {
            Some(pack) => Error::user(format!(
                "Rule \"{query}\" is in the \"{}\" pack, which is not on. Add \"{}\" to \"packs\" in your firescale config, then run this again.",
                pack.name, pack.name
            )),
            None => error,
        }
    })?;
    Ok(Output::ok(explain_rule(rule)?))
}

/// `fscale facts`: detected stack and project graph. Needs no config and no Node.
fn facts(root: &Path, args: &Args, extra: &[String]) -> Result<Output> {
    if !extra.is_empty() {
        return Err(Error::usage(format!(
            "facts takes no arguments, got \"{}\".",
            extra.join(" ")
        )));
    }
    let mut facts = detect_facts(root)?;
    let graph = build_graph(root, &facts);
    facts.warnings.extend(graph.warnings.iter().cloned());
    facts.graph = Some(graph);
    Ok(Output::ok(if args.json {
        format_facts_json(&facts)
    } else {
        format_facts_human(&facts)
    }))
}

/// `fscale graph`: the projects, shared folders and files, and the edges
/// between them (ADR 0032). Needs no config and no Node.
fn graph(root: &Path, args: &Args, extra: &[String]) -> Result<Output> {
    if !extra.is_empty() {
        return Err(Error::usage(format!(
            "graph takes no arguments, got \"{}\".",
            extra.join(" ")
        )));
    }
    if args.json && args.explain {
        return Err(Error::usage(
            "--explain is for people; --json always carries every edge's sources. Use one of them.",
        ));
    }
    let facts = detect_facts(root)?;
    let graph = build_graph(root, &facts);
    Ok(Output::ok(if args.json {
        format_graph_json(&graph)
    } else {
        format_graph_human(&graph, args.explain)
    }))
}

/// `fscale schema [<generator>]` (ADR 0028).
fn schema(root: &Path, rest: &[String]) -> Result<Output> {
    let mut host = NodeHost::new(root);
    match rest {
        [] => {
            let list = host.list_generators()?;
            let mut out =
                String::from("Generators (fscale schema <name> prints one's input schema):\n");
            for generator in list.as_array().into_iter().flatten() {
                let name = generator
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let description = generator
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                out.push_str(&format!("  {name:<10} {description}\n"));
            }
            Ok(Output::ok(out))
        }
        [name] => {
            let schema = host.generator_schema(name)?;
            let text = serde_json::to_string_pretty(&schema).unwrap_or_default();
            Ok(Output::ok(format!("{text}\n")))
        }
        _ => Err(Error::usage(
            "schema takes at most one generator name, for example: fscale schema package",
        )),
    }
}

/// `fscale add <generator> --json <input> [--dry-run]` (ADR 0028). The
/// generator runs in Node and returns files; fscale writes them only when
/// none of them exists yet.
fn add(root: &Path, args: &Args, rest: &[String]) -> Result<Output> {
    let ([name, input], true) = (rest, args.json) else {
        return Err(Error::usage(
            "add needs a generator and its input: fscale add <generator> --json '<input>'. Run `fscale schema` to list the generators and `fscale schema <generator>` for the input.",
        ));
    };
    let input: serde_json::Value = serde_json::from_str(input).map_err(|error| {
        Error::user(format!(
            "The --json input is not valid JSON ({error}). Pass one JSON object, for example --json '{{\"name\": \"utils\"}}'."
        ))
    })?;
    let mut host = NodeHost::new(root);
    let output = host.run_generator(name, &input)?;
    let files: Vec<(String, String)> = output
        .get("files")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|file| {
            Some((
                file.get("path")?.as_str()?.to_owned(),
                file.get("content")?.as_str()?.to_owned(),
            ))
        })
        .collect();
    let notes: Vec<String> = output
        .get("notes")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|note| note.as_str().map(str::to_owned))
        .collect();
    // Never overwrite: refuse before anything is written.
    let mut taken = Vec::new();
    for (file, _) in &files {
        firescale_core::rule::validate_relative(file)?;
        if file.contains('\\') {
            return Err(Error::internal(format!(
                "The generator returned the path \"{file}\" with a backslash. Please report this."
            )));
        }
        let target = file
            .split('/')
            .fold(root.to_path_buf(), |path, part| path.join(part));
        if target.symlink_metadata().is_ok() {
            taken.push(file.clone());
            continue;
        }
        // A folder on the way that is a file (or a link) stops the run too.
        let parts: Vec<&str> = file.split('/').collect();
        let mut folder = root.to_path_buf();
        for part in &parts[..parts.len().saturating_sub(1)] {
            folder.push(part);
            if folder.symlink_metadata().is_ok_and(|meta| !meta.is_dir()) {
                let shown = folder
                    .strip_prefix(root)
                    .unwrap_or(&folder)
                    .to_string_lossy()
                    .replace('\\', "/");
                taken.push(format!("{shown} (not a folder)"));
                break;
            }
        }
    }
    if !taken.is_empty() {
        return Err(Error::user(format!(
            "fscale add {name} would write over {}, so nothing was written. Choose another name or folder in the input, or move the existing file.",
            taken.join(", ")
        )));
    }
    let mut out = String::new();
    if args.dry_run {
        for (file, content) in &files {
            out.push_str(&unified_diff(file, None, content));
        }
        out.push_str(&format!(
            "\nDry run: fscale add {name} would create {} file{}. Nothing was written.\n",
            files.len(),
            if files.len() == 1 { "" } else { "s" }
        ));
    } else {
        let mut created: Vec<PathBuf> = Vec::new();
        for (file, content) in &files {
            let target = file
                .split('/')
                .fold(root.to_path_buf(), |path, part| path.join(part));
            let write = || -> std::io::Result<()> {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&target)
                    .and_then(|mut handle| handle.write_all(content.as_bytes()))
            };
            if let Err(error) = write() {
                // All or nothing: take back the files this run created.
                for done in &created {
                    let _ = std::fs::remove_file(done);
                }
                return Err(Error::user(format!(
                    "Could not create {file} ({error}), so the files created before it were removed again. Check the folder's permissions and that nothing is in the way, then run fscale add again."
                )));
            }
            created.push(target);
            out.push_str(&format!("created {file}\n"));
        }
    }
    if !notes.is_empty() {
        out.push_str("\nNext:\n");
        for note in &notes {
            out.push_str(&format!("  {note}\n"));
        }
    }
    if !args.dry_run {
        out.push_str("\nCheck the result with `fscale check`.\n");
    }
    Ok(Output::ok(out))
}

fn doctor(root: &Path, args: &Args, extra: &[String]) -> Result<Output> {
    if !extra.is_empty() {
        return Err(Error::usage(format!(
            "doctor takes no arguments, got \"{}\".",
            extra.join(" ")
        )));
    }
    let facts = detect_facts(root)?;
    let settings = load_doctor_settings(root)?;
    // The plan a full verify would run, built as --dry-run builds it: each
    // step's program is checked as the step would start it (ADR 0085).
    let plan =
        load_verify_config(root).map(|config| plan_verify(root, &facts, &config, false, None));
    let mut report = run_doctor_with_plan(root, &facts, &ThisMachine, settings, plan.as_ref().ok());
    // Without the plan nothing says which programs verify's steps need:
    // say so, rather than check nothing in silence (review of ADR 0085).
    if let Err(error) = &plan {
        let ts = root.join(firescale_core::config::TS_CONFIG).exists();
        let (detail, fix) = if ts {
            (
                "doctor does not run firescale.config.ts (it is code), so the verify plan is not known and the programs of verify's steps were not checked.".to_owned(),
                "keep the config in firescale.config.json to have them checked".to_owned(),
            )
        } else {
            (
                format!(
                    "the verify plan could not be built, so the programs of verify's steps were not checked: {}",
                    error.message()
                ),
                "fix the firescale config, then run fscale doctor again; fscale verify --dry-run shows the plan".to_owned(),
            )
        };
        add_checks(
            &mut report,
            vec![firescale_core::doctor::Finding {
                section: "tools",
                project: None,
                subject: "verify plan".to_owned(),
                status: firescale_core::doctor::Status::Warning,
                detail,
                fix: Some(fix),
            }],
        );
    }
    add_checks(&mut report, hook_findings(root));
    add_checks(&mut report, hook_limit_findings(root));
    let text = if args.json {
        format_doctor_json(&report)
    } else {
        format_doctor_human(&report)
    };
    Ok(Output {
        stdout: text,
        notes: Vec::new(),
        code: u8::from(!report.ok),
    })
}

/// What the summary shows when the check step fails: the violations of
/// enforced rules only, capped like any other step's output.
fn enforced_violation_lines(report: &CheckReport) -> Vec<String> {
    let mut lines = Vec::new();
    for rule in report
        .rules
        .iter()
        .filter(|rule| rule.status == Status::Enforced && rule.violations > 0)
    {
        for diagnostic in report.diagnostics.iter().filter(|d| d.rule_id == rule.id) {
            let line = diagnostic
                .range
                .map_or_else(String::new, |range| format!(":{}", range.start_line));
            lines.push(format!(
                "{}  {}{line}  {}",
                rule.id, diagnostic.file, diagnostic.message
            ));
        }
    }
    let total = lines.len();
    if total > TAIL_LINES {
        lines.truncate(TAIL_LINES - 1);
        lines.push(format!(
            "... and {} more. Run `fscale check` to see them all.",
            total - (TAIL_LINES - 1)
        ));
    }
    lines
}

/// The identity of every enforced violation, for comparing with the base
/// commit's (ADR 0037).
fn violation_ids(report: &CheckReport) -> Vec<String> {
    report
        .rules
        .iter()
        .filter(|rule| rule.status == Status::Enforced && rule.violations > 0)
        .flat_map(|rule| {
            report
                .diagnostics
                .iter()
                .filter(move |d| d.rule_id == rule.id)
                .map(|d| violation_id(&d.rule_id, &d.file, &d.message))
        })
        .collect()
}

/// The value of `--jobs`: a whole number of at least 1.
fn jobs(args: &Args) -> Result<usize> {
    match &args.jobs {
        None => Ok(1),
        Some(value) => match value.trim().parse::<usize>() {
            Ok(jobs) if jobs >= 1 => Ok(jobs),
            _ => Err(Error::usage(format!(
                "--jobs needs a whole number of 1 or more, got \"{value}\"."
            ))),
        },
    }
}

/// The value of `--budget`: a time as the config's time limits are written
/// (`30m`, `90s`, `1h 30m`), or a number of seconds; longer than zero.
fn budget(args: &Args) -> Result<Option<Duration>> {
    let Some(value) = &args.budget else {
        return Ok(None);
    };
    let text = value.trim();
    let seconds = if !text.is_empty() && text.chars().all(|c| c.is_ascii_digit()) {
        text.parse::<u64>().ok()
    } else {
        firescale_core::config::time_text_seconds(text)
    };
    match seconds {
        Some(seconds) if seconds > 0 => Ok(Some(Duration::from_secs(seconds))),
        _ => Err(Error::usage(format!(
            "--budget needs a time longer than zero, such as \"30m\", \"90s\" or \"1h 30m\", or a number of seconds; got \"{value}\"."
        ))),
    }
}

/// Why this is a CI run, if it is one: `GITHUB_ACTIONS` is `true` (what the
/// shared cache of CI goes by, ADR 0041), or `CI` is set and not empty (what
/// verify's own commands go by, ADR 0024).
fn ci_run(read: &dyn Fn(&str) -> Option<String>) -> Option<&'static str> {
    if read("GITHUB_ACTIONS").as_deref() == Some("true") {
        Some("GITHUB_ACTIONS is true in the environment")
    } else if read("CI").is_some_and(|value| !value.is_empty()) {
        Some("CI is set in the environment")
    } else {
        None
    }
}

/// `--budget` leaves steps out, so it is refused where a run stands for the
/// whole repo: in CI, where a partial run would pass (exit code 0) for a
/// whole one (ADR 0057). The stop hook refuses it in `hooks`.
fn refuse_budget_in_ci(args: &Args, read: &dyn Fn(&str) -> Option<String>) -> Result<()> {
    match ci_run(read).filter(|_| args.budget.is_some()) {
        Some(why) => {
            // `CI=false` and `CI=0` are how people say "not CI" on their
            // own machine, and they count as CI here: say how to get out.
            let own_machine = read("CI")
                .filter(|value| {
                    read("GITHUB_ACTIONS").as_deref() != Some("true")
                        && (value.eq_ignore_ascii_case("false") || value == "0")
                })
                .map_or_else(String::new, |value| {
                    format!(
                        " CI is \"{value}\" here, and any value that is not empty counts as CI: on your own machine, unset CI."
                    )
                });
            Err(Error::usage(format!(
                "--budget cannot be used in CI ({why}): it leaves slow steps out, so the run would pass without them. Run fscale verify without --budget there; to bound a slow step in CI, set its time limit in the firescale config (verify.<step>.timeout).{own_machine}"
            )))
        }
        None => Ok(()),
    }
}

/// The commands verify will run. With `--affected`, the units that no change
/// reaches are marked, using the project graph (ADR 0032). `changed` is
/// computed here when the caller has not done so already.
fn verify_plan(
    root: &Path,
    args: &Args,
    facts: &firescale_core::facts::RepoFacts,
    verify_config: &firescale_core::verify::VerifyConfig,
    changed: Option<Changed>,
) -> Result<(VerifyPlan, Option<Graph>)> {
    let base = args.base.as_deref();
    let mut plan = plan_verify(root, facts, verify_config, args.affected, base);
    firescale_core::verify::check_project_inputs(verify_config, &plan)?;
    if !args.affected {
        return Ok((plan, None));
    }
    let changed = match changed {
        Some(changed) => changed,
        None => changed_files(root, base)?,
    };
    let graph = build_graph(root, facts);
    // The firescale.config.json is compared with the base's, setting by
    // setting, instead of counting as a file that affects everything
    // (ADR 0058). Whether git lists it as changed is not asked: it lists a
    // config it ignores nowhere, and its settings count all the same.
    let config_effect =
        configkey::effect_for_affected(root, base, &changed, facts, verify_config, &plan);
    // As a file, the config changed when git says so. A config that git
    // ignores is in no commit to compare its bytes with: it changed when it
    // is not as the last verify run that passed saw it (or none did).
    let config_listed = matches!(&changed, Changed::Files(files) if files.contains(JSON_CONFIG));
    let ignored_config_changed = !config_listed
        && matches!(&changed, Changed::Files(_))
        && configkey::ignored_config(root)
            .is_some_and(|digest| stepcache::ignored_config_changed(root, &digest));
    // An agent hook file that changed only inside its hook entries, and
    // that no project reads by its path, reaches only the units whose key
    // holds the root. A change outside the hook entries (Claude Code gives
    // the `env` of its settings to every command it starts) reaches every
    // project (ADR 0064).
    let hook_changes = match &changed {
        Changed::Files(files) => hook_file_changes(root, base, files, &graph),
        Changed::Everything | Changed::Shallow(_) => HookFileChanges::default(),
    };
    let mut set = match &config_effect {
        Some(effect) => {
            let mut settled = hook_changes.settled.clone();
            settled.push(JSON_CONFIG);
            let mut set = affected_except(&graph, &changed, &settled);
            if set.everything.is_none() && effect.everything.is_some() {
                set.everything.clone_from(&effect.everything);
                set.everything_why =
                    Some(firescale_core::graph::Why::ConfigSetting { steps: Vec::new() });
            }
            set
        }
        None => affected_except(&graph, &changed, &hook_changes.settled),
    };
    if let Some(first) = hook_changes.outside.first() {
        set.everything_why = Some(firescale_core::graph::Why::AgentHookFile {
            file: first.to_string(),
        });
        set.everything = Some(format!(
            "{first} changed outside the stop hook entry firescale writes, and what is there reaches every step (Claude Code gives its env to every command it starts, and another hook can set that env)"
        ));
    }
    let mut notes = Vec::new();
    match &changed {
        Changed::Everything => notes.push(
            "--affected found no commit to compare with (not a git repo, or no commit yet), so every project ran.".to_owned(),
        ),
        Changed::Shallow(note) => notes.push(format!("--affected: {note}. Every project ran.")),
        Changed::Files(files) if files.is_empty() && base.is_none() => {
            set.everything = Some(
                "nothing changed since the last commit and no --base was given, so there is nothing to compare with".to_owned(),
            );
            set.everything_why = Some(firescale_core::graph::Why::NothingToCompare);
            notes.push(
                "--affected found no uncommitted change and no --base was given, so there was nothing to compare with and every project ran. Pass --base <ref> (for example --base origin/main) to compare with a branch.".to_owned(),
            );
        }
        Changed::Files(_) => {}
    }
    if let Some(first) = set.inert_files.first() {
        let count = set.inert_files.len();
        let (files, verb, run, them) = if count == 1 {
            (
                format!("1 changed file ({first})"),
                "belongs",
                "it runs",
                "it",
            )
        } else {
            (
                format!("{count} changed files ({first} and {} more)", count - 1),
                "belong",
                "they run",
                "them",
            )
        };
        notes.push(format!(
            "{files} {verb} to no project and cannot change a build or a test (Markdown, text, a licence, docs/, or an image, PDF, archive or media file), so {run} nothing; the check still covers {them}."
        ));
    }
    limit_to_affected(&mut plan, facts, &graph, &set, base);
    if let Some(effect) = &config_effect {
        effect.mark(&mut plan);
        if config_listed {
            // As a file, the config reaches only the units whose key holds
            // the repo root, where a step can read it (`prettier --check .`).
            let read_as_a_file =
                configkey::mark_file_readers(&mut plan, &graph, configkey::READ_AS_A_FILE_REASON);
            // Not when another changed file runs every project (the config
            // renamed to `firescale.config.ts`, say): then something runs.
            if effect.is_empty() && !read_as_a_file && set.everything.is_none() {
                notes.push(configkey::NOTHING_TO_RUN_NOTE.to_owned());
            }
        }
    }
    if ignored_config_changed {
        configkey::mark_file_readers(&mut plan, &graph, configkey::IGNORED_READ_AS_A_FILE_REASON);
    }
    if let [first, rest @ ..] = hook_changes.settled.as_slice() {
        let named = match rest.len() {
            0 => (*first).to_owned(),
            more => format!("{first} and {more} more"),
        };
        let reason = format!(
            "{named} changed, and these steps can read every file of the repo, the agent hook files among them"
        );
        let read = configkey::mark_file_readers(&mut plan, &graph, &reason);
        if !read && set.everything.is_none() {
            notes.push(if rest.is_empty() {
                format!("1 changed file ({first}) is an agent hook file, which only a step that can read every file of the repo could read, and no such step runs here, so it runs nothing; the check still covers it.")
            } else {
                format!("{} changed files ({first} and {} more) are agent hook files, which only a step that can read every file of the repo could read, and no such step runs here, so they run nothing; the check still covers them.", rest.len() + 1, rest.len())
            });
        }
    }
    if let Some(affected) = plan.affected.as_mut() {
        affected.notes = notes;
    }
    Ok((plan, Some(graph)))
}

/// `fscale verify --dry-run`: prints the plan and runs nothing, not even the
/// check. No scripts, no installs.
fn verify_dry_run(root: &Path, args: &Args) -> Result<Output> {
    let verify_config = load_verify_config(root)?;
    check_project_folders(root, &verify_config)?;
    let facts = detect_facts(root)?;
    let (plan, _) = verify_plan(root, args, &facts, &verify_config, None)?;
    if args.json {
        // Only with --affected (ADR 0079): the affected plan, and nothing else.
        budget(args)?;
        let text = firescale_core::verify::format_affected_plan_json(&plan)
            .ok_or_else(|| Error::internal("The affected plan is missing. Please report this."))?;
        return Ok(Output::ok(text));
    }
    let check_command = if args.affected {
        "fscale check --affected"
    } else {
        "fscale check"
    };
    let mut text = format_verify_plan(&plan, check_command);
    // With a budget the plan also names the steps a run would not start.
    if let Some(budget) = budget(args)? {
        text.push_str(&format_budget_plan(
            &plan,
            &verify_config,
            &StepTimes::new(root),
            budget,
        ));
    }
    Ok(Output::ok(text))
}

/// The base's tests on the head's code for a tamper report made outside a
/// verify run (`fix --agent`'s referee, ADR 0089): the plan of a full
/// verify of `root`, no cache, the base budget. Anything that keeps the
/// plan from being made leaves the report as it is.
pub(crate) fn old_tests_for(
    root: &Path,
    base: &firescale_core::blame::BaseCommit,
    changed: &std::collections::BTreeSet<String>,
    report: &mut tamper::TamperReport,
) {
    let packs = firescale_rules::builtin_packs();
    let mut host = NodeHost::new(root);
    let Ok(config) = load_config(root, &packs, &mut host) else {
        return;
    };
    let Ok(facts) = detect_facts(root) else {
        return;
    };
    let Ok((plan, _)) = verify_plan(root, &Args::default(), &facts, &config.verify, None) else {
        return;
    };
    let times = StepTimes::new(root);
    let options = RunOptions {
        stop: &STOP,
        progress: None,
        jobs: 1,
        config: &config.verify,
        heartbeat: heartbeat_from_env(),
        record: None,
        cache: None,
        every_step: true,
        deadline: None,
        base_run: false,
        times: Some(&times),
        budget: None,
    };
    firescale_core::oldtests::judge(root, base, changed, report, &plan, &options, None, &|_| {});
}

/// Seconds kept free before the stop hook's limit, to answer the agent.
const HOOK_MARGIN_SECS: f64 = 30.0;

/// Runs check and then the planned lint, typecheck and test commands.
/// `progress` turns on the live lines on stderr.
/// How a verify run is used: by a person or CI, or by an agent's stop hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerifyMode {
    Full,
    /// The stop hook (M21): only the projects the change reaches, measured
    /// from the base commit, and a base run for failures only.
    Hook,
}

/// The command that repeats verify's check step.
fn check_command(args: &Args) -> String {
    match (&args.affected, &args.base) {
        (true, Some(base)) => format!("fscale check --affected --base {base}"),
        (true, None) => "fscale check --affected".to_owned(),
        (false, _) => "fscale check".to_owned(),
    }
}

fn verify_report(
    root: &Path,
    args: &Args,
    progress: bool,
    mode: VerifyMode,
    hook_deadline: Option<Instant>,
) -> Result<(VerifyReport, Vec<String>)> {
    // Blame-aware verify (ADR 0037): with a base commit, every step runs and
    // each failure is compared with the base.
    let base = choose_base(root, args.base.as_deref())?;
    // The stop hook runs only what the change reaches: the projects of the
    // files changed since the base commit (or, with no base, since the last
    // commit), and what depends on them (ADR 0039).
    let mut hook_args = args.clone();
    // With no base there is nothing to measure committed work against, so
    // the hook runs everything, as a full verify does.
    if mode == VerifyMode::Hook {
        if let BaseChoice::Compare(base) = &base {
            hook_args.affected = true;
            hook_args.base = Some(base.commit.clone());
        }
    }
    let args = &hook_args;
    let jobs = jobs(args)?;
    let budget = budget(args)?;
    // Ctrl+C and termination signals stop the commands, not fscale at once.
    install_stop_handler();
    let progress = progress.then(Progress::stderr);
    let say = |text: &str| {
        if let Some(progress) = &progress {
            progress.line(text);
        }
    };
    // No lock: a second run never waits. It says what else is running.
    let (record, others) = RunRecord::start(root);
    for note in others {
        let _ = writeln!(std::io::stderr(), "fscale: {note}");
    }
    let packs = firescale_rules::builtin_packs();
    let mut host = NodeHost::new(root);
    let config = load_config(root, &packs, &mut host)?;
    let verify_config = config.verify.clone();
    check_project_folders(root, &verify_config)?;
    let selection = select_rules(config.rules, None)?;
    // The command that repeats the check: with --base when the run used
    // one, so it reports what this run reported.
    let check_command = check_command(args);
    let check_command = check_command.as_str();
    say(&format!("check: started: {check_command}"));
    let started = Instant::now();
    let outcome = run_check(root, &selection.rules, version(), &mut host, !args.no_cache)?;
    let mut report = outcome.report;
    let facts = detect_facts(root)?;
    let changed = if args.affected {
        let changed = changed_files(root, args.base.as_deref())?;
        narrow_report(&mut report, &facts, &changed, &ran_rules(&selection.rules));
        Some(changed)
    } else {
        None
    };
    say(&format!(
        "check: {} after {}",
        if report.ok { "passed" } else { "FAILED" },
        format_duration(started.elapsed())
    ));
    let check = CheckStep {
        ok: report.ok,
        detail: format!(
            "{}, {}",
            plural(report.summary.rules, "rule"),
            if report.summary.enforced_violations == 0 {
                "no enforced violations".to_owned()
            } else {
                plural(report.summary.enforced_violations, "enforced violation")
            }
        ),
        output: enforced_violation_lines(&report),
        duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        command: check_command.to_owned(),
        violations: violation_ids(&report),
    };
    let head_violations = check.violations.clone();
    // An ignored config, as it is before any step runs: written down at the
    // end if nothing failed, for the next `--affected` run to compare with.
    let ignored_config = configkey::ignored_config(root);
    let (plan, graph) = verify_plan(root, args, &facts, &verify_config, changed)?;
    // The step cache (ADR 0036) needs the graph only for projects below the
    // root: the root's own steps cover the whole repo.
    let graph = graph.or_else(|| {
        plan.projects
            .iter()
            .any(|project| project.kind.is_some() && project.root != ".")
            .then(|| build_graph(root, &facts))
    });
    let read_env = |name: &str| firescale_core::stepcache::process_env(name);
    // In GitHub Actions every key also holds the workflow file, and the
    // shared cache of CI is read and, on the default branch, written
    // (ADR 0041). Laptops never read it.
    let ci = CiRun::from_env(&|name| env::var(name).ok(), root);
    let mut shared_note = None;
    let shared = match (&args.shared_cache, &ci) {
        (Some(dir), Some(run)) => {
            let dir = Path::new(dir);
            let dir = normalize(&if dir.is_absolute() {
                dir.to_path_buf()
            } else {
                root.join(dir)
            });
            let root_path = normalize(root);
            let firescale = root_path.join(".firescale");
            // The records' own folder only: never the repo, a folder above
            // it, or firescale's other folders.
            let unsafe_dir = root_path.starts_with(&dir)
                || (dir.starts_with(&firescale) && !dir.starts_with(firescale.join("shared")));
            if unsafe_dir {
                shared_note = Some(format!(
                    "--shared-cache {} was not used: it must be a folder of its own, such as .firescale/shared, not the repo, a folder above it, or another firescale folder.",
                    args.shared_cache.as_deref().unwrap_or_default()
                ));
                None
            } else {
                if !run.writes() && run.default_ref().is_none() {
                    shared_note = Some("--shared-cache: the event names no default branch, so no shared step result is used.".to_owned());
                }
                Some(SharedCache::new(dir, run.clone()))
            }
        }
        (Some(_), None) => {
            shared_note = Some("--shared-cache is used only in GitHub Actions (GITHUB_ACTIONS=true); on this machine the shared cache is neither read nor written.".to_owned());
            None
        }
        (None, _) => None,
    };
    let shared_settings = configkey::shared_settings(&verify_config);
    let cache = StepCache::new(
        root,
        graph,
        &CacheSettings {
            version: version(),
            read: !args.no_cache,
            sets_ci: !verify_config.no_ci_variable,
            settings: &shared_settings,
            machine: firescale_core::stepcache::this_machine(),
            read_env: &read_env,
        },
    )
    .with_ci(ci.as_ref().map(CiRun::key_salt), shared)
    .with_inputs(verify_config.project_inputs.clone());
    // What the config's difference from the base commit's reaches, so that
    // a step it does not reach can count as the same on the base (ADR 0058).
    // Not for a `firescale.config.ts`: it is a file of every key, and its
    // base copy is never run, so the changed files answer for it.
    let cache = match &base {
        BaseChoice::Compare(_) if root.join(TS_CONFIG).exists() => cache,
        BaseChoice::Compare(base) => cache.with_base_config(configkey::effect_since(
            root,
            &base.commit,
            &facts,
            &verify_config,
            &plan,
            args.affected,
            args.base.as_deref(),
        )),
        BaseChoice::Nothing(_) => cache,
    };
    // A change to a setting that every key holds: say so once (ADR 0043,
    // ADR 0058).
    let config_note = cache.config_note().filter(|_| !args.no_cache);
    let times = StepTimes::new(root);
    let options = RunOptions {
        stop: &STOP,
        progress: progress.as_ref(),
        jobs,
        config: &verify_config,
        heartbeat: heartbeat_from_env(),
        record: record.as_ref(),
        cache: Some(&cache),
        every_step: matches!(base, BaseChoice::Compare(_)),
        deadline: None,
        base_run: false,
        times: Some(&times),
        budget,
    };
    let mut verify = run_verify_with(root, version(), check, &plan, &options)?;
    match &base {
        BaseChoice::Compare(base) if verify.result != VerifyResult::Stopped => {
            let base_check = |worktree: &Path| -> Result<Vec<String>> {
                let mut host = NodeHost::new(worktree);
                let config = load_config(worktree, &packs, &mut host)?;
                let selection = select_rules(config.rules, None)?;
                let outcome = run_check(worktree, &selection.rules, version(), &mut host, false)?;
                Ok(violation_ids(&outcome.report))
            };
            // A base that cannot be compared never fails the run: every
            // failure then blocks, as without a base.
            match changed_since(root, base) {
                Ok(changed) => {
                    compare(
                        &mut verify,
                        &BaseRun {
                            root,
                            base,
                            changed: changed.as_ref(),
                            cache: &cache,
                            plan: &plan,
                            options: &options,
                            check: &base_check,
                            head_violations: &head_violations,
                            judge_passes: mode == VerifyMode::Full && verify_config.label_fixed,
                            shortcuts: !args.no_cache,
                        },
                    );
                    // The tamper guard: the same base, the same changed
                    // files, and its policy from the base commit (ADR 0042).
                    if let Some(changed) = &changed {
                        let mut found = tamper::guard(root, base, changed);
                        // The base's tests on the head's code, as evidence
                        // for what the guard found; report only (ADR 0089).
                        // Never in a pull_request_target run, which holds
                        // secrets and a token that can write: no code of the
                        // change may run there (ADR 0045).
                        let target_run =
                            env::var("GITHUB_EVENT_NAME").as_deref() == Ok("pull_request_target");
                        if !target_run {
                            firescale_core::oldtests::judge(
                                root,
                                base,
                                changed,
                                &mut found,
                                &plan,
                                &options,
                                hook_deadline,
                                &say,
                            );
                        }
                        add_tamper(&mut verify, found);
                    }
                }
                Err(error) => {
                    verify.base = Some(BaseReport {
                        compared: false,
                        reason: Some(format!(
                            "the files that changed since {} could not be listed ({})",
                            base.short(),
                            error.message()
                        )),
                        ..BaseReport::default()
                    });
                }
            }
        }
        // Stopped (ADR 0059): nothing more is started, not even git. The
        // report names the base and says that nothing was compared.
        BaseChoice::Compare(base) => {
            verify.base = Some(BaseReport {
                compared: false,
                reference: Some(base.reference.clone()),
                commit: Some(base.commit.clone()),
                reason: Some(firescale_core::blame::STOPPED_REASON.to_owned()),
                ..BaseReport::default()
            });
        }
        BaseChoice::Nothing(reason) => {
            verify.base = Some(BaseReport {
                compared: false,
                reason: Some(reason.clone()),
                ..BaseReport::default()
            });
        }
    }
    if verify.result == VerifyResult::Stopped {
        say("stopped: every command verify started has been stopped");
    }
    // Nothing failed: every step that can read an ignored config passed
    // with these bytes, or was left out because they were seen before.
    // Not after a run in which --budget left a step out: that step has not
    // run with these bytes, and the record would say that it had.
    if let Some(digest) = &ignored_config {
        if matches!(verify.result, VerifyResult::Ok | VerifyResult::Partial)
            && verify.not_run.is_empty()
        {
            stepcache::remember_ignored_config(root, digest);
        }
    }
    let mut notes = outcome.warnings;
    verify.notes.extend(shared_note);
    verify.notes.extend(config_note);
    // FSCALE_LOG=cache shows, per step, whether its result came from the cache.
    if env::var("FSCALE_LOG").is_ok_and(|value| value.split(',').any(|part| part == "cache")) {
        notes.extend(cache.log());
    }
    Ok((verify, notes))
}

/// `fscale verify`: the one command that says whether the work is done.
/// With `--pr-base`, `args` with `--base` set to the base branch's commit
/// as GitHub's event gives it (ADR 0045), or the commit a merge queue's
/// group is built on (ADR 0080); the working tree must be the
/// commit the run is for.
fn with_pr_base(root: &Path, args: &Args) -> Result<Args> {
    if !args.pr_base {
        return Ok(args.clone());
    }
    if args.base.is_some() {
        return Err(Error::usage(
            "--pr-base and --base both name the base. Pass one of them.",
        ));
    }
    let read = |name: &str| env::var(name).ok();
    let checkout = || -> Result<String> {
        let head = firescale_core::git::run_git(root, &["rev-parse", "HEAD"])?;
        Ok(head.stdout.trim().to_owned())
    };
    // A merge queue run (ADR 0080): the base is the commit the group is
    // built on, from the merge_group event.
    if tamper::github::is_merge_group(&read) {
        let group = tamper::github::merge_group(root, &read)?;
        let head = checkout()?;
        if head != group.head {
            return Err(Error::user(format!(
                "--pr-base: the checkout is at {head}, not at the merge group's commit ({}).",
                group.head
            )));
        }
        let mut args = args.clone();
        args.base = Some(group.base);
        return Ok(args);
    }
    let pr = tamper::github::pull_request(root, &read)?;
    let head = checkout()?;
    if head != pr.checked_out && head != pr.head {
        return Err(Error::user(format!(
            "--pr-base: the checkout is at {head}, not at the commit GitHub ran for ({}) or the pull request's head ({}).",
            pr.checked_out, pr.head
        )));
    }
    let mut args = args.clone();
    args.base = Some(pr.base_tip);
    Ok(args)
}

/// `fscale ledger [--json]`: the agent fix runs summed (M28, ADR 0048).
/// Damaged lines are skipped and named; the exit code is 0 either way.
fn ledger_command(root: &Path, args: &Args, extra: &[String]) -> Result<Output> {
    if !extra.is_empty() {
        return Err(Error::usage(format!(
            "ledger takes no arguments, got \"{}\". Run `fscale ledger` or `fscale ledger --json`.",
            extra.join(" ")
        )));
    }
    // Never read through a symbolic link (ADR 0060).
    if let Some(link) = firescale_core::ownfolder::linked(root, &["ledger.jsonl"]) {
        return Err(Error::user(firescale_core::ownfolder::refusal(&link)));
    }
    let path = root.join(".firescale").join("ledger.jsonl");
    let text = match std::fs::read(&path) {
        Ok(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(Error::user(format!(
                "could not read {} ({error}). Check its permissions.",
                path.display()
            )))
        }
    };
    let summary = firescale_core::ledger::summarise(".firescale/ledger.jsonl", text.as_deref());
    if args.json {
        let mut json = serde_json::to_string_pretty(&summary)
            .map_err(|error| Error::internal(error.to_string()))?;
        json.push('\n');
        return Ok(Output::ok(json));
    }
    Ok(Output::ok(firescale_core::ledger::format_human(&summary)))
}

/// `fscale tamper`: the tamper guard alone (ADR 0045). It runs nothing from
/// the repo: it reads the base commit, and the working tree or (with
/// `--head` or `--pr-base`) a commit, from git's objects.
fn tamper_command(root: &Path, args: &Args, extra: &[String]) -> Result<Output> {
    if !extra.is_empty() {
        return Err(Error::usage(format!(
            "tamper takes no arguments, got \"{}\".",
            extra.join(" ")
        )));
    }
    if args.pr_base && (args.base.is_some() || args.head.is_some()) {
        return Err(Error::usage(
            "--pr-base takes the base and the head from GitHub's event. Drop --base and --head, or --pr-base.",
        ));
    }
    // Approvals count only where nothing of the change has run: a
    // pull_request_target run, with the reviews GitHub gave it (ADR 0046).
    if args.approvals.is_some()
        && (!args.pr_base || env::var("GITHUB_EVENT_NAME").as_deref() != Ok("pull_request_target"))
    {
        return Err(Error::usage(
            "--approvals is read only with --pr-base in a pull_request_target run, where none of the change's code has run. Anywhere else a file of approvals could be written by the change itself.",
        ));
    }
    let mut approvals = None;
    let (base, head) = if args.pr_base {
        // The base branch as it is now against what merging the pull
        // request into it gives: the policy and the comparison both come
        // from the base branch's tip, not from the older commit the change
        // branched from, which its author chose (ADR 0045).
        let pr = tamper::github::pull_request(root, &|name| env::var(name).ok())?;
        let merged = tamper::github::merged_tree(root, &pr.base_tip, &pr.head)?;
        if let Some(file) = &args.approvals {
            let text = std::fs::read_to_string(file).map_err(|error| {
                Error::user(format!("--approvals: could not read {file} ({error})."))
            })?;
            approvals = Some(tamper::approvals_from_reviews(&text, &pr)?);
        }
        (
            firescale_core::blame::BaseCommit {
                reference: "the pull request's base branch (from GitHub's event)".to_owned(),
                commit: pr.base_tip,
            },
            tamper::Head::Commit(merged),
        )
    } else if let Some(head) = &args.head {
        let resolved = firescale_core::git::run_git(
            root,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{head}^{{commit}}"),
            ],
        )?;
        let head_id = resolved.stdout.trim().to_owned();
        if resolved.code != 0 || head_id.is_empty() {
            return Err(Error::usage(format!(
                "--head \"{head}\" is not a commit in this repo."
            )));
        }
        let reference = args.base.clone().unwrap_or_else(|| format!("{head_id}^"));
        let commit = tamper::github::merge_base(root, &head_id, &reference)?;
        (
            firescale_core::blame::BaseCommit { reference, commit },
            tamper::Head::Commit(head_id),
        )
    } else {
        match choose_base(root, args.base.as_deref())? {
            BaseChoice::Compare(base) => (base, tamper::Head::WorkingTree),
            BaseChoice::Nothing(reason) => {
                return Err(Error::user(format!(
                    "the tamper guard has no base to compare with: {reason}."
                )));
            }
        }
    };
    let changed = match &head {
        tamper::Head::Commit(commit) => tamper::changed_between(root, &base, commit)?,
        tamper::Head::WorkingTree => changed_since(root, &base)?.ok_or_else(|| {
            Error::user("the files changed since the base could not be listed (a shallow clone?). Fetch the whole history.")
        })?,
    };
    let mut report = tamper::guard_at(root, &base, &head, &changed);
    // In a pull_request_target run a change that needs approval blocks
    // until one is read: no `--approvals` means none (found in review).
    if args.pr_base
        && approvals.is_none()
        && env::var("GITHUB_EVENT_NAME").as_deref() == Ok("pull_request_target")
    {
        if let tamper::Head::Commit(_) = &head {
            let pr = tamper::github::pull_request(root, &|name| env::var(name).ok())?;
            approvals = Some(tamper::Approvals {
                head: pr.head,
                by: Vec::new(),
            });
        }
    }
    report.approvals = approvals;
    let code = if report.blocks() {
        EXIT_VIOLATIONS
    } else {
        EXIT_OK
    };
    let mut stdout = if args.json {
        let mut text = serde_json::to_string_pretty(&report)
            .map_err(|error| Error::internal(error.to_string()))?;
        text.push('\n');
        text
    } else {
        firescale_core::verify::format_tamper_human(&report)
    };
    // In GitHub Actions each finding is also an annotation on its line.
    if !args.json && env::var("GITHUB_ACTIONS").as_deref() == Ok("true") {
        stdout.push_str(&firescale_core::verify::tamper_annotations(&report));
    }
    Ok(Output {
        stdout,
        notes: Vec::new(),
        code,
    })
}

fn verify(root: &Path, args: &Args, extra: &[String]) -> Result<Output> {
    if !extra.is_empty() {
        return Err(Error::usage(format!(
            "verify takes no arguments, got \"{}\".",
            extra.join(" ")
        )));
    }
    if args.head.is_some() {
        return Err(Error::usage(
            "verify judges the working tree; --head is for `fscale tamper`.",
        ));
    }
    // Before anything runs, the dry run included.
    refuse_budget_in_ci(args, &|name| env::var(name).ok())?;
    budget(args)?;
    let resolved = with_pr_base(root, args)?;
    let args = &resolved;
    if args.dry_run {
        if args.json && !args.affected {
            return Err(Error::usage(
                "verify --dry-run prints the plan for people and has no --json form. Drop one of them, or add --affected: verify --affected --dry-run --json prints which units a change runs, and why.",
            ));
        }
        return verify_dry_run(root, args);
    }
    // Progress is for a person watching: on in a terminal, off when stderr
    // goes to a file or a pipe, unless --progress or --no-progress says.
    let progress = args
        .progress
        .unwrap_or_else(|| std::io::stderr().is_terminal());
    let started = Instant::now();
    let (report, notes) = match verify_report(root, args, progress, VerifyMode::Full, None) {
        Ok(done) => done,
        // Ctrl+C during the check also reaches the Node host and git, which
        // then fail: that is a stop, not an error. `--json` still prints a
        // report, so a script that reads it never gets an empty document.
        Err(error) if STOP.load(std::sync::atomic::Ordering::SeqCst) => {
            return Ok(Output {
                stdout: if args.json {
                    format_verify_json(&stopped_in_check(
                        version(),
                        &check_command(args),
                        u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                    ))
                } else {
                    format!("verify: STOPPED - {STOPPED_IN_CHECK}\n")
                },
                notes: vec![format!(
                    "fscale: verify was stopped before the check finished ({}). Run fscale verify again to check the repo.",
                    error.message()
                )],
                code: EXIT_STOPPED,
            });
        }
        Err(error) => return Err(error),
    };
    Ok(Output {
        stdout: if args.json {
            format_verify_json(&report)
        } else {
            format_verify_human(&report)
        },
        notes,
        code: if report.result == VerifyResult::Stopped {
            EXIT_STOPPED
        } else if report.ok {
            EXIT_OK
        } else {
            EXIT_VIOLATIONS
        },
    })
}

/// What the check found for one command of a hook file (ADR 0064).
#[derive(Debug, Clone)]
enum HookStart {
    /// It gave firescale's answer, with this version.
    Starts(String),
    /// Why it cannot start on this machine.
    Cannot(String),
    /// Not started: it is not a command fscale writes. Why, and what to do.
    NotStarted(String),
    /// Not started by `fscale doctor`, which runs no script of the repo and
    /// downloads nothing, though fscale writes the command. Why.
    Skipped(String),
}

/// Checks the stop hooks in the files of `agents`: starts each command in
/// them that fscale writes itself (`allowed`, the first being the one it
/// writes here, and any pin a release wrote), once, the way an agent starts
/// it, after saying on stderr what it runs. Any other command came from the
/// repo's files and is never started (review of ADR 0064). For `doctor`, a
/// pin is started with npx's `--no`, which downloads nothing, and a command
/// that runs the repo's `fscale` script is not started (second review).
/// One result for each command of each file.
fn hook_starts(
    root: &Path,
    agents: &[Agent],
    allowed: &[&str],
    doctor: bool,
) -> Vec<(Agent, String, HookStart)> {
    let found: Vec<(Agent, String)> = agents
        .iter()
        .flat_map(|agent| {
            installed_prefixes(root, *agent)
                .into_iter()
                .map(move |prefix| (*agent, prefix))
        })
        .collect();
    let (path, dropped) = check_path(root);
    let path_note = match dropped.as_slice() {
        [] => String::new(),
        [one] => format!(
            " It leaves out of the PATH the folder a package manager put there for its own run ({one}), which an agent does not have."
        ),
        more => format!(
            " It leaves out of the PATH the {} folders a package manager put there for its own run ({}), which an agent does not have.",
            more.len(),
            more.join(", ")
        ),
    };
    let mut done: Vec<(String, HookStart)> = Vec::new();
    let mut results = Vec::new();
    for (agent, prefix) in &found {
        if !may_start(prefix, allowed) {
            let why = not_started(*agent, prefix, allowed.first().copied().unwrap_or_default());
            results.push((*agent, prefix.clone(), HookStart::NotStarted(why)));
            continue;
        }
        if let Some((_, result)) = done.iter().find(|(started, _)| started == prefix) {
            results.push((*agent, prefix.clone(), result.clone()));
            continue;
        }
        let pin = pinned_version(prefix);
        if doctor && pin.is_none() && has_fscale_script(root) {
            let why = format!(
                "it runs this repo's own `fscale` package script, and doctor runs no script of the repo. Check it yourself: `{}`.",
                probe_line(prefix)
            );
            done.push((prefix.clone(), HookStart::Skipped(why.clone())));
            results.push((*agent, prefix.clone(), HookStart::Skipped(why)));
            continue;
        }
        let started = match pin {
            Some(version) if doctor => no_download(version),
            _ => prefix.clone(),
        };
        // Claude Code sets CLAUDE_PROJECT_DIR for the commands it starts.
        let claude = found
            .iter()
            .any(|(other, same)| *other == Agent::Claude && same == prefix);
        let _ = writeln!(
            std::io::stderr(),
            "fscale: checking that the hook can start: running `{}` in the repo root, as an agent would.{}{path_note}",
            probe_line(&started),
            launch_note(&started)
        );
        let mut check = check_start(root, &started, path.as_deref(), claude, !doctor);
        if let Some(version) = pin {
            check = pinned_answer(version, check);
        }
        let result = match (pin, start_verdict(&started, &check)) {
            (_, Ok(answer)) => HookStart::Starts(answer),
            (Some(version), Err(_)) if doctor && not_downloaded(&check) => {
                HookStart::Skipped(format!(
                    "firescale {version} is not downloaded yet, and doctor downloads nothing. The agent's first stop downloads it through `{prefix}`, which needs the network."
                ))
            }
            (_, Err(why)) => HookStart::Cannot(why),
        };
        done.push((prefix.clone(), result.clone()));
        results.push((*agent, prefix.clone(), result));
    }
    results
}

/// After `hooks install`: checks the stop hooks of the files in `files`
/// ([`hook_starts`]) and says on `out` whether each starts. True when each
/// command was started and gave firescale's answer.
fn check_hook_starts(root: &Path, files: &[&str], allowed: &[&str], out: &mut String) -> bool {
    let agents: Vec<Agent> = AGENTS
        .into_iter()
        .filter(|agent| files.contains(&agent.config_file()))
        .collect();
    let mut all_start = true;
    let mut cannot = false;
    let mut said: Vec<String> = Vec::new();
    for (agent, prefix, result) in hook_starts(root, &agents, allowed, false) {
        match result {
            HookStart::NotStarted(why) | HookStart::Skipped(why) => {
                all_start = false;
                out.push_str(&format!(
                    "The hook in {} was not started: {why}\n",
                    agent.config_file()
                ));
            }
            _ if said.contains(&prefix) => {}
            HookStart::Starts(answer) => {
                let other = if answer == version() {
                    String::new()
                } else {
                    format!(
                        " That is not this fscale ({}): the agent will run firescale {answer}.",
                        version()
                    )
                };
                out.push_str(&format!(
                    "The hook starts on this machine: `{}` answered firescale {answer}.{other}\n",
                    probe_line(&prefix)
                ));
                said.push(prefix);
            }
            HookStart::Cannot(why) => {
                all_start = false;
                cannot = true;
                out.push_str(&format!("The hook cannot start on this machine: {why}\n"));
                said.push(prefix);
            }
        }
    }
    if cannot {
        out.push_str(
            "An agent whose stop hook cannot start stops without verify having run: Claude Code, Codex and Cursor take it as a hook error, not as a reason to keep working (docs/hooks.md). The hook files are written, since the command may start on another machine. Make the command work here (npx comes with Node.js; a package script needs the repo's dependencies installed), or use another one: edit it in the files above, or remove the hook and run this again with --bin \"<a command that starts fscale here>\", for example --bin fscale when fscale is on the PATH.\n",
        );
    }
    all_start
}

/// `fscale doctor`'s check of Claude Code's stop hook limit (ADR 0083): a
/// warning when the kept times of the steps the hook would run add up to
/// more than its `timeout`, or Claude Code's default when it sets none.
fn hook_limit_findings(root: &Path) -> Vec<firescale_core::doctor::Finding> {
    use firescale_core::doctor::{Finding, Status as Health};
    let Some(timeout) = claude_hook_timeout(root) else {
        return Vec::new();
    };
    let Ok((times, _)) = kept_plan_times(root) else {
        return Vec::new();
    };
    let file = Agent::Claude.config_file();
    let edit = format!("edit `timeout` in {file}.");
    let (limit, whose, raise) = match timeout {
        // Seconds, a point allowed: whole seconds are what it is read to.
        Some(value) => match timeout_seconds(&value) {
            Some(seconds) => (
                Duration::try_from_secs_f64(seconds.floor()).unwrap_or(Duration::MAX),
                "`timeout`".to_owned(),
                edit,
            ),
            None => (
                Duration::from_secs(CLAUDE_DEFAULT_TIMEOUT),
                format!("Claude Code's default: the `timeout` in the file, {value}, is not a number of seconds"),
                edit,
            ),
        },
        None => (
            Duration::from_secs(CLAUDE_DEFAULT_TIMEOUT),
            "Claude Code's default".to_owned(),
            "run fscale hooks install --agent claude, which adds a `timeout` set from these times."
                .to_owned(),
        ),
    };
    if times.total <= limit {
        return Vec::new();
    }
    vec![Finding {
        section: "hooks",
        project: None,
        subject: file.to_owned(),
        status: Health::Warning,
        detail: format!(
            "the steps the stop hook would run took {} in all when they last ran, more than its limit of {} ({whose}). Claude Code ends a stop hook that runs past its limit, and the agent stops without verify having finished.",
            format_limit(times.total),
            format_limit(limit)
        ),
        fix: Some(format!(
            "work on a branch: the stop hook then runs only the projects a change reaches, as fscale verify --affected does; or give the hook a smaller scope (a faster command for the slow steps under verify in the config); or raise the limit: {raise}"
        )),
    }]
}

/// `fscale doctor`'s checks of the agent hooks: each stop hook in the
/// repo's hook files that fscale would write here is started the same way
/// as after `hooks install`, on this machine (ADR 0064).
fn hook_findings(root: &Path) -> Vec<firescale_core::doctor::Finding> {
    use firescale_core::doctor::{Finding, Status as Health};
    let writes = default_bin(root);
    hook_starts(root, &AGENTS, &[writes.as_str()], true)
        .into_iter()
        .map(|(agent, prefix, result)| {
            // What doctor started: a pin without a download.
            let started = pinned_version(&prefix).map_or_else(|| prefix.clone(), no_download);
            let (status, detail, fix) = match result {
                HookStart::Starts(answer) if answer == version() => (
                    Health::Ok,
                    format!(
                        "the stop hook starts: `{}` answered firescale {answer}.",
                        probe_line(&started)
                    ),
                    None,
                ),
                HookStart::Starts(answer) => (
                    Health::Warning,
                    format!(
                        "the stop hook starts: `{}` answered firescale {answer}, which is not this fscale ({}): the agent runs firescale {answer}.",
                        probe_line(&started),
                        version()
                    ),
                    None,
                ),
                HookStart::Cannot(why) => (
                    Health::Problem,
                    format!("the stop hook cannot start: {why}"),
                    Some(format!(
                        "make `{prefix}` start on this machine (npx comes with Node.js; a package script needs the repo's dependencies installed). Until then the agent stops without verify having run."
                    )),
                ),
                HookStart::NotStarted(why) | HookStart::Skipped(why) => (
                    Health::Warning,
                    format!("the stop hook was not started: {why}"),
                    None,
                ),
            };
            Finding {
                section: "hooks",
                project: None,
                subject: agent.config_file().to_owned(),
                status,
                detail,
                fix,
            }
        })
        .collect()
}

/// The time limit `hooks install` writes on Claude Code's stop hook, and
/// why (ADR 0083).
struct HookLimit {
    /// `None` when nothing is known to set it from: no `timeout` is
    /// written, so a later install can still write one (review of ADR 0083).
    seconds: Option<u64>,
    /// Why this limit: the end of the line that says it.
    why: String,
}

/// A `timeout` value as people read it: a time when it is a whole number
/// of seconds, else the value as the file has it.
fn shown_timeout(value: &serde_json::Value) -> String {
    match value.as_u64().filter(|seconds| *seconds > 0) {
        Some(seconds) => format_limit(Duration::from_secs(seconds)),
        None => value.to_string(),
    }
}

impl HookLimit {
    /// The line `hooks install` prints about the limit.
    fn line(&self, kept: KeptTimeout) -> String {
        let file = Agent::Claude.config_file();
        let default = format_limit(Duration::from_secs(CLAUDE_DEFAULT_TIMEOUT));
        let why = &self.why;
        let Some(seconds) = self.seconds else {
            return match kept {
                KeptTimeout::Kept(have) => format!(
                    "Claude Code's stop hook keeps the `timeout` of {} in {file}{why}.\n",
                    shown_timeout(&have)
                ),
                _ => format!(
                    "Claude Code's stop hook has no `timeout` in {file}, so Claude Code's default of {default} applies{why}.\n"
                ),
            };
        };
        let wanted = format_limit(Duration::from_secs(seconds));
        match kept {
            KeptTimeout::Kept(have) if have.as_u64() == Some(seconds) => {
                format!("Claude Code's stop hook keeps the `timeout` of {wanted} in {file}.\n")
            }
            KeptTimeout::Kept(have) => format!(
                "Claude Code's stop hook keeps the `timeout` of {} in {file}: firescale does not change a value in the file. To give it {wanted}, edit that value.\n",
                shown_timeout(&have)
            ),
            KeptTimeout::CannotAdd => format!(
                "Claude Code's stop hook in {file} has no `timeout`, and it cannot go in as a new line without changing a line already there. Add \"timeout\": {seconds} to that entry by hand ({wanted}{why}); without it Claude Code ends the hook after {default}.\n"
            ),
            KeptTimeout::None => {
                format!("Claude Code's stop hook may run for {wanted} (`timeout` in {file}){why}.\n")
            }
        }
    }
}

/// The value of `--timeout`, or the limit worked out from the kept step
/// times; `None` when no Claude Code hook is being installed.
fn hook_limit(root: &Path, args: &Args, agents: &[Agent]) -> Result<Option<HookLimit>> {
    if let Some(value) = &args.timeout {
        if !agents.contains(&Agent::Claude) {
            return Err(Error::usage(
                "--timeout is written only on the Claude Code hook: Codex and Cursor have no time limit field firescale writes. Pass --agent claude or --all.",
            ));
        }
        let text = value.trim();
        let seconds = if !text.is_empty() && text.chars().all(|c| c.is_ascii_digit()) {
            text.parse::<u64>().ok()
        } else {
            firescale_core::config::time_text_seconds(text)
        };
        return match seconds {
            Some(seconds) if (1..=MAX_HOOK_TIMEOUT).contains(&seconds) => Ok(Some(HookLimit {
                seconds: Some(seconds),
                why: ", as --timeout says".to_owned(),
            })),
            _ => Err(Error::usage(format!(
                "--timeout needs a time longer than zero and at most {MAX_HOOK_TIMEOUT} seconds (about 24 days, the longest a timer waits), such as \"30m\" or \"1h 30m\", or a number of seconds; got \"{value}\"."
            ))),
        };
    }
    if !agents.contains(&Agent::Claude) {
        return Ok(None);
    }
    let unknown = |why: String| HookLimit { seconds: None, why };
    let again = "After a fscale verify, run this again to set it from the steps' times, or pass --timeout <time>";
    let kept = match kept_plan_times(root) {
        Ok(kept) => kept,
        Err(why) => {
            return Ok(Some(unknown(format!(
                ": {why}, so the steps' kept times are not known. {again}"
            ))));
        }
    };
    let Some((times, base)) = Some(kept).filter(|(times, _)| times.known > 0) else {
        return Ok(Some(unknown(format!(
            ": no step has a kept time yet. {again}"
        ))));
    };
    let seconds = hook_timeout_for(times.total, base);
    let unknown = match times.unknown {
        0 => String::new(),
        1 => " (1 planned step has no kept time and is not counted)".to_owned(),
        n => format!(" ({n} planned steps have no kept time and are not counted)"),
    };
    let why = if seconds == CLAUDE_DEFAULT_TIMEOUT
        && 2 * times.total + base <= Duration::from_secs(CLAUDE_DEFAULT_TIMEOUT)
    {
        format!(
            ", its default: the planned steps took {} when they last ran{unknown}",
            format_limit(times.total)
        )
    } else {
        format!(
            ": twice the {} the planned steps took when they last ran, and {} for the base run{unknown}",
            format_limit(times.total),
            format_limit(base)
        )
    };
    Ok(Some(HookLimit {
        seconds: Some(seconds),
        why,
    }))
}

/// The kept times of every step a full verify plans, and the time the base
/// run may take (`verify.baseBudget`). An error says why the plan or the
/// times are not known.
fn kept_plan_times(
    root: &Path,
) -> std::result::Result<(firescale_core::verify::PlannedTimes, Duration), String> {
    let config = load_verify_config(root).map_err(|error| error.message().to_owned())?;
    let facts = detect_facts(root).map_err(|error| error.message().to_owned())?;
    let plan = plan_verify(root, &facts, &config, false, None);
    let times = planned_times(&plan, &StepTimes::new(root)).map_err(|error| error.to_string())?;
    let base = match config.base_budget {
        Some(Limit::After(budget)) => budget,
        // No budget: the base run may take as long as the steps.
        Some(Limit::Off) => times.total,
        None => DEFAULT_BASE_BUDGET,
    };
    Ok((times, base))
}

/// `fscale hooks install` and `fscale hooks run`.
fn hooks(root: &Path, args: &Args, rest: &[String]) -> Result<Output> {
    // The stop hook decides whether an agent may stop: a run that leaves
    // slow steps out must not decide that (ADR 0057).
    if args.budget.is_some() {
        return Err(Error::usage(
            "--budget cannot be used in the stop hook: it leaves slow steps out, so an agent could stop without them having run. Remove --budget from the hook command (fscale hooks run --agent <name>); a person can still run fscale verify --budget <time> by hand.",
        ));
    }
    match rest {
        [action] if action == "install" => {
            let agents: Vec<Agent> = match (&args.agent, args.all) {
                (Some(_), true) => {
                    return Err(Error::usage("pass either --agent <name> or --all, not both."));
                }
                (Some(name), false) => vec![Agent::parse(name)?],
                (None, true) => AGENTS.to_vec(),
                (None, false) => {
                    return Err(Error::usage(
                        "hooks install needs --agent <claude|codex|cursor> or --all.",
                    ));
                }
            };
            // A command given with --bin is written as it is. Otherwise the
            // repo decides, never this machine's PATH (ADR 0064).
            let writes = default_bin(root);
            let bin = args.bin.clone().unwrap_or_else(|| writes.clone());
            let bin = bin.as_str();
            // The check starts only a command fscale writes: this one, or
            // the repo's default (review of ADR 0064).
            let allowed = [bin, writes.as_str()];
            // Claude Code's entry gets a time limit (ADR 0083).
            let limit = hook_limit(root, args, &agents)?;
            let mut out = String::new();
            let mut notes = Vec::new();
            let mut ready = Vec::new();
            let mut refused = Vec::new();
            let mut limit_line = None;
            for agent in agents {
                let timeout = limit.as_ref().and_then(|limit| limit.seconds);
                let state = match install_hook_with(root, agent, bin, timeout) {
                    Ok((state, kept)) => {
                        if let (Agent::Claude, Some(limit)) = (agent, &limit) {
                            limit_line = Some(limit.line(kept));
                        }
                        match state {
                            Installed::Added => "installed",
                            Installed::TimeoutAdded => "timeout added",
                            Installed::AlreadyThere => "already installed",
                        }
                    }
                    // With --all, a file that is refused does not stop the
                    // others: each is tried, and the output says which were
                    // written and which were not (ADR 0060).
                    Err(Error::User(message)) if args.all => {
                        notes.push(format!("fscale: {message}"));
                        refused.push(agent.config_file());
                        "refused"
                    }
                    Err(error) => return Err(error),
                };
                out.push_str(&format!("{state:<17} {}\n", agent.config_file()));
                if state != "refused" {
                    ready.push(agent.config_file());
                }
            }
            // A hook already installed keeps its own command: the agent runs
            // that one, not `bin` (second review of ADR 0064).
            let kept: Vec<String> = AGENTS
                .into_iter()
                .filter(|agent| ready.contains(&agent.config_file()))
                .flat_map(|agent| {
                    installed_prefixes(root, agent)
                        .into_iter()
                        .filter(|prefix| prefix != bin)
                        .map(move |prefix| {
                            format!("`{}` in {}", hook_command(&prefix, agent), agent.config_file())
                        })
                })
                .collect();
            if refused.is_empty() && kept.is_empty() {
                out.push_str(&format!(
                    "\nEach agent now runs `{bin} hooks run --agent <name>` before it stops, and keeps working while `fscale verify` fails.\n"
                ));
            } else if refused.is_empty() {
                out.push_str(&format!(
                    "\nEach agent now runs the stop hook in its file before it stops, and keeps working while `fscale verify` fails. A hook already there keeps its command: {}.\n",
                    kept.join(", ")
                ));
            } else {
                out.push_str(&format!(
                    "\nNot every hook is installed. Refused and left as it was: {}. The reason, and what to add by hand, is printed above this list (on stderr). Do that, then run this again.\n",
                    refused.join(", ")
                ));
                if !ready.is_empty() {
                    out.push_str(&format!(
                        "The hook is in {}: there the agent runs `{bin} hooks run --agent <name>` before it stops, and keeps working while `fscale verify` fails.\n",
                        ready.join(", ")
                    ));
                }
            }
            if let Some(line) = limit_line {
                out.push_str(&line);
            }
            let starts = check_hook_starts(root, &ready, &allowed, &mut out);
            let code = if !refused.is_empty() {
                EXIT_USAGE
            } else if starts {
                EXIT_OK
            } else {
                EXIT_VIOLATIONS
            };
            Ok(Output { stdout: out, notes, code })
        }
        [action] if action == "run" => {
            let Some(name) = &args.agent else {
                return Err(Error::usage("hooks run needs --agent <claude|codex|cursor>."));
            };
            let agent = Agent::parse(name)?;
            // An agent pipes JSON in. A person running this by hand has a
            // terminal on stdin, and must not be left waiting for input.
            let stdin = if std::io::stdin().is_terminal() {
                String::new()
            } else {
                std::io::read_to_string(std::io::stdin()).unwrap_or_default()
            };
            let input = parse_hook_input(&stdin);
            // The hook's own time limit: Claude Code's `timeout` (ADR 0083),
            // less a margin to answer in. Past it, nothing more is started
            // for the base's tests on the head's code (ADR 0089).
            let hook_deadline = (agent == Agent::Claude).then(|| {
                let seconds = firescale_core::hooks::claude_hook_timeout(root)
                    .flatten()
                    .as_ref()
                    .and_then(firescale_core::hooks::timeout_seconds)
                    .unwrap_or(600.0);
                Instant::now()
                    + Duration::from_secs_f64((seconds - HOOK_MARGIN_SECS).max(0.0))
            });
            // A hook that cannot run verify must not trap the agent: report and let it stop.
            // An agent reads the hook's stderr: no progress unless asked for.
            let report = match verify_report(
                root,
                args,
                args.progress.unwrap_or(false),
                VerifyMode::Hook,
                hook_deadline,
            ) {
                Ok((report, _)) if report.result == VerifyResult::Stopped => {
                    return Ok(Output {
                        stdout: String::new(),
                        notes: vec![
                            "fscale: verify was stopped before it finished, so the agent is not held back.".to_owned(),
                        ],
                        code: 1,
                    });
                }
                Ok((report, _)) => report,
                Err(error) => {
                    return Ok(Output {
                        stdout: String::new(),
                        notes: vec![format!(
                            "fscale: the stop hook could not run verify, so the agent is not held back: {}",
                            error.message()
                        )],
                        code: 1,
                    });
                }
            };
            let max_blocks = max_blocks_from_env();
            let decision = decide(root, agent, &input, report.ok, max_blocks);
            let answer = reply_with(
                agent,
                &decision,
                &block_summary(&report),
                max_blocks,
                input.last_message.as_deref(),
            );
            let mut notes = if answer.stderr.is_empty() {
                Vec::new()
            } else {
                vec![answer.stderr.trim_end().to_owned()]
            };
            // The agent may stop (ADR 0020), but not on the strength of a
            // clean pass: say which steps verified nothing.
            // The agent may stop, but a person reading the log should see
            // what passed only the second time (ADR 0039).
            let flaky: Vec<String> = report
                .steps
                .iter()
                .filter(|step| step.label == Some(firescale_core::blame::Label::Flaky))
                .map(|step| match &step.project {
                    Some(project) => format!("{} in {project}", step.name),
                    None => step.name.clone(),
                })
                .collect();
            if report.ok && !flaky.is_empty() {
                notes.push(format!(
                    "fscale: flaky, so the agent is not held back: {} failed, then passed when run again. Nothing about it is kept.",
                    flaky.join(", ")
                ));
            }
            if report.ok && !report.skipped.is_empty() {
                notes.push(format!(
                    "fscale: verify was only partial, so this is not a clean pass. {} of {} steps ran{}; skipped: {}. Run `fscale verify` for the details, or set verify.failOnSkipped to true to make skipped steps block.",
                    report.steps_run - report.cache.as_ref().map_or(0, |cache| cache.hits),
                    report.steps_total,
                    match report.cache.as_ref().map_or(0, |cache| cache.hits) {
                        0 => String::new(),
                        hits => format!(", {hits} passed from the cache"),
                    },
                    report
                        .skipped
                        .iter()
                        .map(|skip| match &skip.project {
                            Some(project) if project == "." => {
                                format!("{} in the repo root", skip.step)
                            }
                            Some(project) => format!("{} in {project}", skip.step),
                            None => skip.step.clone(),
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            Ok(Output {
                stdout: answer.stdout,
                notes,
                code: answer.exit_code,
            })
        }
        _ => Err(Error::usage(
            "hooks needs an action: `fscale hooks install --all` or `fscale hooks run --agent <name>`.",
        )),
    }
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
        // With --json, a name only firescale gives: what the check of a
        // hook's command asks for (ADR 0064).
        if args.json {
            let answer = serde_json::json!({ "name": "firescale", "version": version() });
            return Ok(Output::ok(format!("{answer}\n")));
        }
        return Ok(Output::ok(format!("{}\n", version())));
    }
    let Some((command, rest)) = args.positionals.split_first() else {
        return Ok(Output::ok(HELP));
    };
    if args.help {
        return Ok(Output::ok(HELP));
    }
    // A limit no other command reads must not look as if it set one
    // (review of ADR 0083).
    if args.timeout.is_some()
        && !(command == "hooks" && rest.first().is_some_and(|sub| sub == "install"))
    {
        return Err(Error::usage(
            "--timeout is for `fscale hooks install`: it is the time limit written on Claude Code's stop hook. No other command takes it.",
        ));
    }
    // `init` makes a repo root out of the folder it is run in. Every other
    // command works on the root found from there.
    if command == "init" {
        return scaffold::init(&cwd, &args, rest);
    }
    let root = find_root(&cwd);
    match command.as_str() {
        "adopt" => scaffold::adopt(&root, &args, rest),
        "sync" => scaffold::sync(&root, &args, rest),
        "check" => check(&root, &args, rest),
        "fix" => fix(&root, &args, rest),
        "explain" => explain(&root, rest),
        "facts" => facts(&root, &args, rest),
        "graph" => graph(&root, &args, rest),
        "doctor" => doctor(&root, &args, rest),
        "add" => add(&root, &args, rest),
        "schema" => schema(&root, rest),
        "verify" => verify(&root, &args, rest),
        "tamper" => tamper_command(&root, &args, rest),
        "ledger" => ledger_command(&root, &args, rest),
        "hooks" => hooks(&root, &args, rest),
        _ => Err(Error::usage(format!("unknown command \"{command}\"."))),
    }
}

/// True for `fscale hooks run ...`, wherever its options are written
/// (`fscale --agent claude hooks run`). Claude Code and Codex read exit code
/// 2 from a stop hook as "do not stop", so this command must never use it
/// for its own errors: a broken hook must not trap an agent. The value of an
/// option is never taken for the command, so `fscale --rules hooks run` is
/// not the hook.
fn is_stop_hook(argv: &[String]) -> bool {
    matches!(positionals_of(argv).as_slice(), ["hooks", "run", ..])
}

fn main() -> ExitCode {
    let argv: Vec<String> = env::args().skip(1).collect();
    let error_code = if is_stop_hook(&argv) { 1 } else { EXIT_USAGE };
    let (stdout, stderr, code) = match run() {
        Ok(output) => {
            let notes: String = output.notes.iter().map(|note| format!("{note}\n")).collect();
            (output.stdout, notes, output.code)
        }
        Err(Error::Usage(message)) => (
            String::new(),
            format!("fscale: {message}\nRun `fscale --help` to see the available commands and options.\n"),
            error_code,
        ),
        Err(Error::User(message)) => (String::new(), format!("fscale: {message}\n"), error_code),
        Err(Error::Internal(message)) => (
            String::new(),
            format!(
                "fscale: unexpected error. Please report it at https://github.com/ihsankoolz/firescale/issues\n{message}\n"
            ),
            error_code,
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
    use super::{
        ci_run, is_stop_hook, normalize, parse_args, refuse_budget_in_ci, Args, FLAG_OPTIONS, HELP,
        VALUE_OPTIONS,
    };
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

    fn stop_hook(argv: &[&str]) -> bool {
        let argv: Vec<String> = argv.iter().map(|arg| (*arg).to_owned()).collect();
        is_stop_hook(&argv)
    }

    #[test]
    fn the_stop_hook_is_seen_wherever_its_options_are() {
        for argv in [
            &["hooks", "run"][..],
            &["hooks", "run", "--agent", "claude"],
            &["--agent", "claude", "hooks", "run"],
            &["hooks", "--agent", "claude", "run"],
            &["--cwd", "some dir", "hooks", "run", "--agent", "claude"],
            &["--agent=claude", "--json", "hooks", "--bin", "x", "run"],
            // Command lines parse_args refuses are still the hook's.
            &["--nope", "hooks", "run"],
            &["hooks", "run", "--agent"],
            &["--agent", "--json", "hooks", "run"],
            &["-x", "hooks", "run", "extra"],
        ] {
            assert!(stop_hook(argv), "{argv:?}");
        }
    }

    #[test]
    fn the_value_of_an_option_is_never_the_command() {
        for argv in [
            &[][..],
            &["hooks"],
            &["hooks", "install"],
            &["verify", "hooks", "run"],
            &["--rules", "hooks", "run"],
            &["--agent", "hooks", "run"],
            &["hooks", "--bin", "run"],
            &["check", "--rules", "hooks", "run"],
            &["--bin", "hooks", "--agent", "run", "verify"],
            // After `--` every word is an argument, in the order written.
            &["--", "--agent", "hooks", "run"],
        ] {
            assert!(!stop_hook(argv), "{argv:?}");
        }
    }

    // A value-taking option that `positionals_of` does not know would have
    // its value read as the command. Both read the same list; this holds
    // them to it, option by option.
    #[test]
    fn every_option_that_takes_a_value_has_it_skipped() {
        for &name in VALUE_OPTIONS {
            let args = parse(&[name, "hooks", "run"]).unwrap();
            assert_eq!(args.positionals, ["run"], "{name}");
            assert!(!stop_hook(&[name, "hooks", "run"]), "{name}");
            assert!(stop_hook(&[name, "value", "hooks", "run"]), "{name}");
            let inline = format!("{name}=value");
            assert!(stop_hook(&[inline.as_str(), "hooks", "run"]), "{name}");
        }
        for &name in FLAG_OPTIONS {
            let args = parse(&[name, "hooks", "run"]).unwrap();
            assert_eq!(args.positionals, ["hooks", "run"], "{name}");
            assert!(stop_hook(&[name, "hooks", "run"]), "{name}");
        }
    }

    // The help is the list people read. An option it shows with a `<value>`
    // must be in VALUE_OPTIONS, and one without in FLAG_OPTIONS, so a new
    // option cannot be documented without being in the right list.
    #[test]
    fn the_help_and_the_option_lists_agree() {
        let options = HELP
            .split_once("\nOptions:\n")
            .and_then(|(_, rest)| rest.split_once("\nExit codes:"))
            .map(|(options, _)| options)
            .unwrap();
        let mut flags = vec!["--version", "--help"];
        let mut values = Vec::new();
        for line in options.lines().filter(|line| line.starts_with("  --")) {
            let mut words = line.split_whitespace();
            let name = words.next().unwrap();
            if words.next().is_some_and(|next| next.starts_with('<')) {
                values.push(name);
            } else {
                flags.push(name);
            }
        }
        let sorted = |names: &[&'static str]| {
            let mut names = names.to_vec();
            names.sort_unstable();
            names
        };
        assert_eq!(
            sorted(&values),
            sorted(VALUE_OPTIONS),
            "options with a value"
        );
        assert_eq!(sorted(&flags), sorted(FLAG_OPTIONS), "options without one");
    }

    #[test]
    fn a_ci_run_is_known_by_github_actions_or_a_ci_variable_that_is_not_empty() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).to_owned())
            }
        };
        assert_eq!(ci_run(&env(&[])), None);
        assert_eq!(ci_run(&env(&[("CI", "")])), None);
        assert_eq!(ci_run(&env(&[("GITHUB_ACTIONS", "false")])), None);
        assert_eq!(
            ci_run(&env(&[("CI", "true")])),
            Some("CI is set in the environment")
        );
        assert_eq!(
            ci_run(&env(&[("CI", "1"), ("GITHUB_ACTIONS", "true")])),
            Some("GITHUB_ACTIONS is true in the environment")
        );
        // Only with the flag is a CI run refused.
        let with = parse(&["verify", "--budget", "30m"]).unwrap();
        let without = parse(&["verify"]).unwrap();
        assert!(refuse_budget_in_ci(&with, &env(&[("CI", "true")])).is_err());
        assert!(refuse_budget_in_ci(&without, &env(&[("CI", "true")])).is_ok());
        assert!(refuse_budget_in_ci(&with, &env(&[])).is_ok());
    }

    #[test]
    fn normalize_removes_dot_segments() {
        assert_eq!(normalize(Path::new("a/b/../c/./d")), Path::new("a/c/d"));
        assert_eq!(normalize(Path::new("a/b/..")), Path::new("a"));
        assert_eq!(normalize(Path::new("../a")), Path::new("../a"));
    }
}
