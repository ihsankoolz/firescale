//! The `fscale` command.

mod scaffold;

use std::collections::BTreeSet;
use std::env;
use std::io::{IsTerminal, Write};
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use firescale_core::affected::{changed_files, narrow_report, Changed};
use firescale_core::blame::{
    changed_since, choose_base, compare, violation_id, BaseChoice, BaseReport, BaseRun,
};
use firescale_core::check::run_check;
use firescale_core::config::{
    find_root, load_config, load_verify_config, ConfiguredRule, RuleSource,
};
use firescale_core::diff::unified_diff;
use firescale_core::doctor::{format_doctor_human, format_doctor_json, run_doctor, ThisMachine};
use firescale_core::explain::{explain_rule, find_rule};
use firescale_core::facts::{detect_facts, format_facts_human, format_facts_json};
use firescale_core::fixloop::{
    plan_order, run_fix_loop, LoopFailure, LoopReport, LoopRule, MAX_PASSES,
};
use firescale_core::graph::{affected, build_graph, format_graph_human, format_graph_json, Graph};
use firescale_core::hooks::{
    block_summary, decide, install_hook, max_blocks_from_env, parse_hook_input, reply, Agent,
    AGENTS,
};
use firescale_core::model::Status;
use firescale_core::node::NodeHost;
use firescale_core::process::{install_stop_handler, EXIT_STOPPED, STOP};
use firescale_core::progress::{format_duration, heartbeat_from_env, Progress};
use firescale_core::report::{format_human, format_json, CheckReport};
use firescale_core::rule::{fix_rule_with, BuiltinPack, Change, FixOutcome};
use firescale_core::runs::RunRecord;
use firescale_core::sarif::format_sarif;
use firescale_core::scratch::Scratch;
use firescale_core::select::{select_rules, Selection};
use firescale_core::sharedcache::{CiRun, SharedCache};
use firescale_core::stepcache::{CacheSettings, StepCache};
use firescale_core::tamper;
use firescale_core::verify::add_tamper;
use firescale_core::verify::{
    check_project_folders, format_verify_human, format_verify_json, format_verify_plan,
    limit_to_affected, plan_verify, run_verify_with, CheckStep, RunOptions, VerifyPlan,
    VerifyReport, VerifyResult, TAIL_LINES,
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
  fscale init [--packs <list>] Start a repo in an empty folder: config, then every fix
  fscale adopt [--dry-run]     Bring an existing repo in: add what is missing, overwrite nothing
  fscale sync [--check]        Repair drift: run the fixes again
  fscale verify [options]      Check, then lint, typecheck and test: the definition of done
  fscale explain <rule>        Print what a rule checks and how to fix it
  fscale facts [--json]        Print detected repo facts
  fscale graph [--explain]     Print the project graph; --explain says where each edge comes from
  fscale doctor [--json]       Check this machine: tools, env variables, dependencies, services
  fscale add <generator> --json <input> [--dry-run]
                               Scaffold new files from JSON input (package, rule)
  fscale schema [<generator>]  Print a generator's input JSON Schema, or list the generators
  fscale hooks install         Make an agent run verify before it may stop (--agent, --all)
  fscale --version             Print the firescale version
  fscale --help                Print this help

Options:
  --rules <glob>   check, fix: only matching rules, e.g. \"core/*\" (comma-separated)
  --json           check, verify, facts, graph, doctor: print JSON (see docs/)
  --sarif          check: print SARIF 2.1.0 (see docs/sarif.md)
  --affected       check, verify: only what changed, and projects that depend on it
  --base <ref>     With --affected: also count commits since the merge base with <ref>
  --explain        graph: show where each edge comes from
  --no-cache       check: run every rule again, ignoring .firescale/cache;
                   verify: run every step again (passes are still stored)
  --dry-run        fix, adopt, add: print what would change as a diff, and change nothing;
                   verify: print what would run (with --affected, and why), run nothing
  --packs <list>   init: rule packs to turn on, comma-separated (default: core,agent-ready)
  --force          init: run in a folder that is not empty (nothing is overwritten)
  --check          sync: change nothing, and exit 1 when sync would change something
  --agent <name>   hooks: claude, codex or cursor
  --all            hooks install: every supported agent
  --bin <command>  hooks install: how the hook starts fscale (default: fscale)
  --jobs <n>       verify: run up to <n> projects at once (default: 1)
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
    shared_cache: Option<String>,
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
            | "--affected" | "--all" | "--force" | "--check" | "--progress" | "--no-progress"
            | "--explain" => {
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
                    "--explain" => args.explain = true,
                    "--all" => args.all = true,
                    "--force" => args.force = true,
                    "--check" => args.check = true,
                    "--progress" => args.progress = Some(true),
                    "--no-progress" => args.progress = Some(false),
                    _ => args.json = true,
                }
            }
            "--rules" | "--cwd" | "--base" | "--agent" | "--bin" | "--packs" | "--jobs"
            | "--shared-cache" => {
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
                    "--agent" => args.agent = Some(value),
                    "--bin" => args.bin = Some(value),
                    "--packs" => args.packs = Some(value),
                    "--jobs" => args.jobs = Some(value),
                    "--shared-cache" => args.shared_cache = Some(value),
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
    let report = run_doctor(root, &facts, &ThisMachine);
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
    if !args.affected {
        return Ok((plan, None));
    }
    let changed = match changed {
        Some(changed) => changed,
        None => changed_files(root, base)?,
    };
    let graph = build_graph(root, facts);
    let mut set = affected(&graph, &changed);
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
    let check_command = if args.affected {
        "fscale check --affected"
    } else {
        "fscale check"
    };
    Ok(Output::ok(format_verify_plan(&plan, check_command)))
}

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

fn verify_report(
    root: &Path,
    args: &Args,
    progress: bool,
    mode: VerifyMode,
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
    let check_command = match (&args.affected, &args.base) {
        (true, Some(base)) => format!("fscale check --affected --base {base}"),
        (true, None) => "fscale check --affected".to_owned(),
        (false, _) => "fscale check".to_owned(),
    };
    let check_command = check_command.as_str();
    say(&format!("check: started: {check_command}"));
    let started = Instant::now();
    let outcome = run_check(root, &selection.rules, version(), &mut host, !args.no_cache)?;
    let mut report = outcome.report;
    let facts = detect_facts(root)?;
    let changed = if args.affected {
        let changed = changed_files(root, args.base.as_deref())?;
        narrow_report(&mut report, &facts, &changed);
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
    let cache = StepCache::new(
        root,
        graph,
        &CacheSettings {
            version: version(),
            read: !args.no_cache,
            sets_ci: !verify_config.no_ci_variable,
            machine: firescale_core::stepcache::this_machine(),
            read_env: &read_env,
        },
    )
    .with_ci(ci.as_ref().map(CiRun::key_salt), shared);
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
                            judge_passes: mode == VerifyMode::Full,
                            shortcuts: !args.no_cache,
                        },
                    );
                    // The tamper guard: the same base, the same changed
                    // files, and its policy from the base commit (ADR 0042).
                    if let Some(changed) = &changed {
                        add_tamper(&mut verify, tamper::guard(root, base, changed));
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
        BaseChoice::Compare(_) => {}
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
    let mut notes = outcome.warnings;
    verify.notes.extend(shared_note);
    // FSCALE_LOG=cache shows, per step, whether its result came from the cache.
    if env::var("FSCALE_LOG").is_ok_and(|value| value.split(',').any(|part| part == "cache")) {
        notes.extend(cache.log());
    }
    Ok((verify, notes))
}

/// `fscale verify`: the one command that says whether the work is done.
fn verify(root: &Path, args: &Args, extra: &[String]) -> Result<Output> {
    if !extra.is_empty() {
        return Err(Error::usage(format!(
            "verify takes no arguments, got \"{}\".",
            extra.join(" ")
        )));
    }
    if args.dry_run {
        if args.json {
            return Err(Error::usage(
                "verify --dry-run prints the plan for people and has no --json form. Drop one of them.",
            ));
        }
        return verify_dry_run(root, args);
    }
    // Progress is for a person watching: on in a terminal, off when stderr
    // goes to a file or a pipe, unless --progress or --no-progress says.
    let progress = args
        .progress
        .unwrap_or_else(|| std::io::stderr().is_terminal());
    let (report, notes) = match verify_report(root, args, progress, VerifyMode::Full) {
        Ok(done) => done,
        // Ctrl+C during the check also reaches the Node host and git, which
        // then fail: that is a stop, not an error.
        Err(error) if STOP.load(std::sync::atomic::Ordering::SeqCst) => {
            return Ok(Output {
                stdout: if args.json {
                    String::new()
                } else {
                    "verify: STOPPED - interrupted before the check finished\n".to_owned()
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

/// `fscale hooks install` and `fscale hooks run`.
fn hooks(root: &Path, args: &Args, rest: &[String]) -> Result<Output> {
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
            let bin = args.bin.as_deref().unwrap_or("fscale");
            let mut out = String::new();
            let mut notes = Vec::new();
            for agent in agents {
                let changed = install_hook(root, agent, bin)?;
                out.push_str(&format!(
                    "{:<17} {}\n",
                    if changed { "installed" } else { "already installed" },
                    agent.config_file()
                ));
                if agent == Agent::Cursor && changed {
                    notes.push(
                        "note: the Cursor hook format could not be checked against Cursor's docs when this was written. Confirm that .cursor/hooks.json matches your Cursor version."
                            .to_owned(),
                    );
                }
            }
            out.push_str(&format!(
                "\nEach agent now runs `{bin} hooks run --agent <name>` before it stops, and keeps working while `fscale verify` fails.\n"
            ));
            Ok(Output { stdout: out, notes, code: EXIT_OK })
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
            // A hook that cannot run verify must not trap the agent: report and let it stop.
            // An agent reads the hook's stderr: no progress unless asked for.
            let report = match verify_report(
                root,
                args,
                args.progress.unwrap_or(false),
                VerifyMode::Hook,
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
            let answer = reply(agent, &decision, &block_summary(&report), max_blocks);
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
        return Ok(Output::ok(format!("{}\n", version())));
    }
    let Some((command, rest)) = args.positionals.split_first() else {
        return Ok(Output::ok(HELP));
    };
    if args.help {
        return Ok(Output::ok(HELP));
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
        "hooks" => hooks(&root, &args, rest),
        _ => Err(Error::usage(format!("unknown command \"{command}\"."))),
    }
}

/// True for `fscale hooks run ...`. Claude Code and Codex read exit code 2
/// from a stop hook as "do not stop", so this command must never use it for
/// its own errors: a broken hook must not trap an agent.
fn is_stop_hook() -> bool {
    let words: Vec<String> = env::args()
        .skip(1)
        .filter(|arg| !arg.starts_with('-'))
        .take(2)
        .collect();
    words == ["hooks", "run"]
}

fn main() -> ExitCode {
    let error_code = if is_stop_hook() { 1 } else { EXIT_USAGE };
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
