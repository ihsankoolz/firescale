//! `fscale fix --agent <name> [--rule <id>]`: one rule's remaining
//! violations handed to an agent, with every guardrail of SPEC §7 (M27,
//! ADR 0047). The pieces are in `firescale_core::agentrun`; this puts them
//! in order and runs the re-check.

use std::env;
use std::path::{Path, PathBuf};
use std::time::Instant;

use firescale_core::agentrun::{
    append_ledger, diff_guard, now_utc, repo_limits, staged_changes, Adapter, AgentResult,
    LedgerEntry, Policy, ProgramAdapter, RepoState, Request, RunWorktree,
};
use firescale_core::blame::BaseCommit;
use firescale_core::check::run_check;
use firescale_core::config::load_config;
use firescale_core::explain::rule_skill;
use firescale_core::git::run_git;
use firescale_core::model::Fixable;
use firescale_core::node::{NodeHost, NODE_ENV};
use firescale_core::select::select_rules;
use firescale_core::tamper;
use firescale_core::{version, Error, Result};

use crate::{Args, Output, EXIT_OK, EXIT_VIOLATIONS};

/// Overrides the path of the agent adapters' entry point (`agents/cli.js`).
const AGENTS_ENV: &str = "FSCALE_AGENTS";

/// Looks for the adapters next to the binary, as the Node host is found:
/// this repo's dev layout (`target/<profile>/fscale` beside
/// `packages/agents`) and an npm install (`node_modules/firescale/agents`).
fn agents_script_near_exe() -> Option<PathBuf> {
    let exe = env::current_exe().ok()?;
    exe.ancestors().skip(1).take(5).find_map(|dir| {
        [
            dir.join("packages")
                .join("agents")
                .join("dist")
                .join("cli.js"),
            dir.join("firescale").join("agents").join("cli.js"),
            dir.join("node_modules")
                .join("firescale")
                .join("agents")
                .join("cli.js"),
        ]
        .into_iter()
        .find(|candidate| candidate.is_file())
    })
}

/// The agents `--agent` names: `fake`, the stand-in the tests use, and the
/// Claude Code and Codex adapters (M28, ADR 0048), TypeScript programs
/// started with Node that speak the same protocol.
fn adapter(name: &str) -> Result<Box<dyn Adapter>> {
    match name {
        "fake" => {
            let program = env::var_os("FSCALE_FAKE_AGENT")
                .map_or_else(|| PathBuf::from("fscale-fake-agent"), PathBuf::from);
            Ok(Box::new(ProgramAdapter {
                name: "fake".to_owned(),
                program,
                args: Vec::new(),
            }))
        }
        "claude" | "codex" => {
            let script = env::var_os(AGENTS_ENV)
                .map(PathBuf::from)
                .or_else(agents_script_near_exe)
                .ok_or_else(|| {
                    Error::user(format!(
                        "the {name} adapter was not found beside fscale. Install firescale from npm (it ships the adapters in `agents/`), or set {AGENTS_ENV} to the adapters' `cli.js`."
                    ))
                })?;
            let node = env::var_os(NODE_ENV).map_or_else(|| PathBuf::from("node"), PathBuf::from);
            Ok(Box::new(ProgramAdapter {
                name: name.to_owned(),
                program: node,
                args: vec![script.to_string_lossy().into_owned(), name.to_owned()],
            }))
        }
        other => Err(Error::usage(format!(
            "unknown agent \"{other}\". Pass --agent claude, codex, or fake (the stand-in the tests use)."
        ))),
    }
}

/// The limits of a run, from `agent` in `firescale.config.json`:
/// `maxTurns` (30), `maxCostUsd` (2) and `timeoutSeconds` (900).
fn limits(root: &Path) -> (u32, f64, u64) {
    let config = std::fs::read_to_string(root.join("firescale.config.json"))
        .ok()
        .and_then(|text| {
            serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}')).ok()
        });
    let agent = config.as_ref().and_then(|c| c.get("agent"));
    let get = |key: &str| {
        agent
            .and_then(|a| a.get(key))
            .and_then(serde_json::Value::as_f64)
    };
    (
        get("maxTurns").map_or(30, |v| v.clamp(1.0, 1000.0) as u32),
        get("maxCostUsd")
            .filter(|v| *v > 0.0 && v.is_finite())
            .map_or(2.0, |v| v),
        get("timeoutSeconds").map_or(900, |v| v.clamp(1.0, 86_400.0) as u64),
    )
}

fn git_lines(root: &Path, args: &[&str]) -> Vec<String> {
    run_git(root, args)
        .ok()
        .filter(|out| out.code == 0)
        .map(|out| {
            out.stdout
                .lines()
                .map(str::to_owned)
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Guardrail 1, part two: a clean tree, and no secrets tracked.
fn preflight_repo(root: &Path) -> std::result::Result<(), String> {
    if run_git(root, &["rev-parse", "--verify", "--quiet", "HEAD"])
        .map_or(true, |out| out.code != 0)
    {
        return Err("fix --agent needs a git repo with at least one commit.".to_owned());
    }
    let dirty = git_lines(
        root,
        &[
            "status",
            "--porcelain",
            "--untracked-files=normal",
            "--",
            ".",
            ":(exclude).firescale",
        ],
    );
    if !dirty.is_empty() {
        return Err(format!(
            "the working tree has uncommitted changes ({}). Commit or stash them first: an agent run must start from a clean tree.",
            dirty.len()
        ));
    }
    let tracked = git_lines(root, &["-c", "core.quotepath=off", "ls-files"]);
    if let Some(secret) = tracked
        .iter()
        .find(|path| firescale_core::agentrun::is_secret(path))
    {
        return Err(format!(
            "{secret} is tracked by git and looks like a secret. Remove it from git (`fscale explain core/no-committed-env`) before any agent run."
        ));
    }
    Ok(())
}

fn prompt(rule: &str, skill: &str, diagnostics: &serde_json::Value) -> String {
    format!(
        "Fix the remaining violations of the firescale rule {rule} in this repository, and nothing else.\n\nFollow the rule's SKILL.md below. Do not change tests, CI, lint or type-check settings, or any file the violations do not need. Do not commit, push or tag: firescale checks the change and commits it.\n\n## SKILL.md\n\n{skill}\n\n## The violations (fscale check --json)\n\n{}\n",
        serde_json::to_string_pretty(diagnostics).unwrap_or_default()
    )
}

/// The outcome of one run, before it is written down.
struct Run {
    result: &'static str,
    reason: Option<&'static str>,
    detail: String,
    files: usize,
    lines: usize,
    agent: AgentResult,
}

pub(crate) fn fix_with_agent(root: &Path, args: &Args, agent_name: &str) -> Result<Output> {
    let started = Instant::now();
    let adapter = adapter(agent_name)?;
    // 1. Pre-flight: the agent, the tree, the secrets.
    let version_line = adapter.preflight().map_err(Error::user)?;
    preflight_repo(root).map_err(Error::user)?;
    let limits_of_repo = repo_limits(root);
    if let Some(why) = &limits_of_repo.read_only {
        return Err(Error::user(format!(
            "the repo's own agent settings let no agent edit files ({why}); an agent run never has wider permissions than they give."
        )));
    }
    // Codex's `exec` takes no list of tools, commands or paths to deny
    // (ADR 0048): where the repo's own settings deny something, a Codex run
    // would be wider than they allow.
    if agent_name == "codex" {
        let mut denied: Vec<&str> = limits_of_repo
            .denied
            .iter()
            .chain(&limits_of_repo.denied_paths)
            .map(String::as_str)
            .collect();
        denied.sort_unstable();
        denied.dedup();
        if !denied.is_empty() {
            return Err(Error::user(format!(
                "the repo's own agent settings deny {}, and Codex cannot be told to: `codex exec` takes no list of tools, commands or paths to deny, so a Codex run would have wider permissions than the repo gives. Use --agent claude, or remove those rules if Codex may do this.",
                denied.iter().map(|d| format!("`{d}`")).collect::<Vec<_>>().join(", ")
            )));
        }
    }

    // The rule and its violations, checked now.
    let packs = firescale_rules::builtin_packs();
    let mut host = NodeHost::new(root);
    let config = load_config(root, &packs, &mut host)?;
    let protected = config.protected_paths.clone();
    let selection = select_rules(config.rules, None)?;
    let outcome = run_check(root, &selection.rules, version(), &mut host, false)?;
    let wanted = args.rule.as_deref();
    let rule_id = match wanted {
        Some(id) => id.to_owned(),
        None => match outcome
            .report
            .diagnostics
            .iter()
            .find(|d| d.fixable != Fixable::Deterministic)
        {
            Some(diagnostic) => diagnostic.rule_id.clone(),
            None => {
                return Ok(Output::ok(
                    "Nothing for an agent to fix: no rule has a violation that its deterministic fix cannot repair.\n",
                ))
            }
        },
    };
    let Some(rule) = selection.rules.iter().find(|rule| rule.meta.id == rule_id) else {
        return Err(Error::usage(format!(
            "rule \"{rule_id}\" is not enabled in this repo. `fscale check` lists the rules that are."
        )));
    };
    // Secrets in the code itself: no agent run until they are gone.
    if outcome
        .report
        .diagnostics
        .iter()
        .any(|d| d.rule_id == "core/no-secrets-in-source")
    {
        return Err(Error::user(
            "core/no-secrets-in-source has findings: there may be keys or tokens in tracked files. Remove them before any agent run (`fscale explain core/no-secrets-in-source`).",
        ));
    }
    let diagnostics: Vec<&firescale_core::report::ReportedDiagnostic> = outcome
        .report
        .diagnostics
        .iter()
        .filter(|d| d.rule_id == rule_id)
        .collect();
    if diagnostics.is_empty() {
        return Ok(Output::ok(format!(
            "Nothing for an agent to fix: {rule_id} has no violation.\n"
        )));
    }
    let diagnostics_json =
        serde_json::to_value(&diagnostics).map_err(|e| Error::internal(e.to_string()))?;
    let skill = rule_skill(rule)?;
    // What the fix may change: the rule's inputs and the files its
    // violations name.
    let mut allowed: Vec<String> = rule.meta.inputs.clone();
    allowed.extend(diagnostics.iter().map(|d| d.file.clone()));
    let description = rule.meta.description.clone();

    // 2. Isolation: a worktree on a new branch, rolled back when dropped
    // unless the run passes.
    let base = run_git(root, &["rev-parse", "HEAD"])?
        .stdout
        .trim()
        .to_owned();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let branch = format!("fscale/fix/{}-{stamp}", rule_id.replace('/', "-"));
    let worktree = RunWorktree::add(root, &branch)?;
    // Taken once the run's branch exists: what the agent may not change.
    let before = RepoState::capture(root);

    // 3 to 6. A scoped request under the policy.
    let (max_turns, max_cost, timeout) = limits(root);
    let policy = Policy::strict(&limits_of_repo, &protected, max_turns, max_cost, timeout);
    if !policy
        .allowed_tools
        .iter()
        .any(|tool| tool == "Edit" || tool == "Write")
    {
        return Err(Error::user(
            "the repo's own agent settings deny every tool that edits files; an agent run never has wider permissions than they give.",
        ));
    }
    let request = Request {
        cwd: worktree.path.to_string_lossy().into_owned(),
        rule: rule_id.clone(),
        prompt: prompt(&rule_id, &skill, &diagnostics_json),
        skill,
        diagnostics: diagnostics_json,
        policy: policy.clone(),
    };
    let agent = match adapter.run(&request) {
        Ok(result) => result,
        Err(error) => AgentResult {
            ok: false,
            error: Some(error.message().chars().take(160).collect()),
            ..AgentResult::default()
        },
    };

    let mut protected_all = protected.clone();
    protected_all.extend(limits_of_repo.denied_paths.iter().cloned());
    let run = judge(&Judged {
        root,
        worktree: &worktree.path,
        base: &base,
        agent: &agent,
        policy: &policy,
        allowed: &allowed,
        protected: &protected_all,
        rule_id: &rule_id,
        before: &before,
        branch: &branch,
    })?;

    // 8 and 9. Pass: one commit with trailers. Fail: roll back (the
    // worktree guard does it when dropped).
    let mut out = String::new();
    let passed = run.result == "pass";
    if passed {
        let message = format!(
            "fix({rule_id}): {description}\n\nFirescale-Rule: {rule_id}\nFirescale-Agent: {}\nFirescale-Outcome: pass\n",
            adapter.name()
        );
        let committed = run_git(
            &worktree.path,
            &["commit", "--quiet", "--no-verify", "-m", &message],
        )?;
        if committed.code != 0 {
            return Err(Error::user(format!(
                "the fix could not be committed: {}",
                committed.stderr.trim()
            )));
        }
        worktree.keep_branch();
        out.push_str(&format!(
            "fix --agent {}: PASSED - {rule_id} is fixed on the branch {branch} (one commit, {} file{}, {} line{} changed). Review it and merge it yourself; nothing was pushed.\n",
            adapter.name(),
            run.files,
            if run.files == 1 { "" } else { "s" },
            run.lines,
            if run.lines == 1 { "" } else { "s" },
        ));
    } else {
        drop(worktree);
        out.push_str(&format!(
            "fix --agent {}: FAILED - {rule_id}: {}. The run was rolled back; your branch and files are untouched.\n",
            adapter.name(),
            run.detail
        ));
    }
    out.push_str(&format!("Agent: {}\n", version_line));
    if agent_name == "codex" {
        out.push_str("Warning: Codex runs with a shell and can read any file the sandbox allows, your untracked .env files included; firescale cannot pass it the denied commands and paths. The diff guard, the re-check and the time limit are what held this run (ADR 0048).\n");
    }

    // 10. The ledger: codes and counts only.
    let entry = LedgerEntry {
        schema: 1,
        time: now_utc(),
        rule: rule_id,
        agent: adapter.name().to_owned(),
        result: run.result.to_owned(),
        reason: run.reason.map(str::to_owned),
        files_changed: run.files,
        lines_changed: run.lines,
        duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        cost_usd: run.agent.cost_usd.filter(|c| c.is_finite() && *c >= 0.0),
        turns: run.agent.turns,
    };
    append_ledger(root, &entry)?;
    Ok(Output {
        stdout: out,
        notes: Vec::new(),
        code: if passed { EXIT_OK } else { EXIT_VIOLATIONS },
    })
}

/// What `judge` looks at.
struct Judged<'a> {
    root: &'a Path,
    worktree: &'a Path,
    base: &'a str,
    agent: &'a AgentResult,
    policy: &'a Policy,
    allowed: &'a [String],
    protected: &'a [String],
    rule_id: &'a str,
    before: &'a RepoState,
    branch: &'a str,
}

/// The environment of the agent, set on this process while the re-check
/// runs code the agent wrote (rules, tests), and taken off again.
struct AgentEnvironment(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl AgentEnvironment {
    fn set() -> Self {
        let mut old = Vec::new();
        for (name, value) in firescale_core::agentrun::agent_env() {
            old.push((name, env::var_os(name)));
            env::set_var(name, value);
        }
        Self(old)
    }
}

impl Drop for AgentEnvironment {
    fn drop(&mut self) {
        for (name, value) in self.0.drain(..) {
            match value {
                Some(value) => env::set_var(name, value),
                None => env::remove_var(name),
            }
        }
    }
}

/// Guardrails 5, 7 and 8, and nothing outside the worktree: what the run
/// did, judged against the commit it started from. Stages the change.
fn judge(j: &Judged<'_>) -> Result<Run> {
    let agent = j.agent;
    let fail = |reason: &'static str, detail: String, files: usize, lines: usize| Run {
        result: "fail",
        reason: Some(reason),
        detail,
        files,
        lines,
        agent: agent.clone(),
    };
    // Stage everything, tracked or new, and compare with the base commit,
    // not the worktree's HEAD: a commit the agent made itself counts too.
    let _ = run_git(j.worktree, &["add", "--all"]);
    let changed = staged_changes(j.worktree, j.base);
    let lines: usize = git_lines(
        j.worktree,
        &[
            "diff",
            "--cached",
            "--numstat",
            "--no-renames",
            j.base,
            "--",
        ],
    )
    .iter()
    .map(|line| {
        line.split_whitespace()
            .take(2)
            .filter_map(|n| n.parse::<usize>().ok())
            .sum::<usize>()
    })
    .sum();
    let files = changed.len();

    // Nothing outside the worktree: refs, git config, hooks, your HEAD and
    // files. A change there fails the run and is put back where it can be.
    let after = RepoState::capture(j.root);
    let outside = j.before.changes(&after, j.branch);
    if !outside.is_empty() {
        j.before.restore(j.root, &after, j.branch);
        return Ok(fail(
            "outside-worktree",
            format!("the agent {}", outside.join(", ")),
            files,
            lines,
        ));
    }
    let head = git_lines(j.worktree, &["rev-parse", "HEAD"]).join("");
    if head != j.base {
        return Ok(fail(
            "agent-committed",
            "the agent made commits of its own; firescale makes the run's one commit".to_owned(),
            files,
            lines,
        ));
    }
    // Ignored files are not in the diff, but a re-check would see them.
    let ignored: Vec<String> = git_lines(
        j.worktree,
        &[
            "status",
            "--porcelain",
            "--ignored",
            "--untracked-files=all",
        ],
    )
    .into_iter()
    .filter_map(|line| line.strip_prefix("!! ").map(str::to_owned))
    .collect();
    if let Some(path) = ignored.first() {
        return Ok(fail(
            "diff-guard",
            format!("it created the ignored file {path}"),
            files,
            lines,
        ));
    }
    if !agent.ok {
        let detail = agent
            .error
            .clone()
            .unwrap_or_else(|| "the agent reported an error".to_owned());
        let reason = if agent.timed_out {
            "timeout"
        } else {
            "agent-error"
        };
        return Ok(fail(reason, detail, files, lines));
    }
    if let Some(cost) = agent.cost_usd {
        if !cost.is_finite() || cost < 0.0 {
            return Ok(fail(
                "agent-error",
                "the agent reported a cost that is not a number of 0 or more".to_owned(),
                files,
                lines,
            ));
        }
        if cost > j.policy.max_cost_usd {
            return Ok(fail(
                "over-budget",
                format!(
                    "the agent spent ${cost:.2}, over the limit of ${:.2}",
                    j.policy.max_cost_usd
                ),
                files,
                lines,
            ));
        }
    }
    if let Some(turns) = agent.turns {
        if turns > j.policy.max_turns {
            return Ok(fail(
                "over-budget",
                format!(
                    "the agent took {turns} turns, over the limit of {}",
                    j.policy.max_turns
                ),
                files,
                lines,
            ));
        }
    }
    if changed.is_empty() {
        return Ok(fail(
            "no-change",
            "the agent changed nothing".to_owned(),
            0,
            0,
        ));
    }
    // 7. The diff guard, both sides of every path.
    if let Some(why) = diff_guard(&changed, j.allowed, j.protected) {
        return Ok(fail("diff-guard", why, files, lines));
    }
    // 8. The re-check runs code the agent may have written: under the
    // agent's environment, and the repo is checked again after it.
    let guard = AgentEnvironment::set();
    let packs = firescale_rules::builtin_packs();
    let mut host = NodeHost::new(j.worktree);
    let config = load_config(j.worktree, &packs, &mut host)?;
    let selection = select_rules(config.rules, None)?;
    let outcome = run_check(j.worktree, &selection.rules, version(), &mut host, false)?;
    drop(host);
    let left = outcome
        .report
        .diagnostics
        .iter()
        .filter(|d| d.rule_id == j.rule_id)
        .count();
    if left > 0 {
        return Ok(fail(
            "still-failing",
            format!(
                "{left} violation{} of the rule remain",
                if left == 1 { "" } else { "s" }
            ),
            files,
            lines,
        ));
    }
    // The referee: the tamper guard over the diff, and blame-aware verify.
    let base_commit = BaseCommit {
        reference: "the commit the run started from".to_owned(),
        commit: j.base.to_owned(),
    };
    let changed_set = changed.iter().map(|c| c.path.clone()).collect();
    let report = tamper::guard(j.worktree, &base_commit, &changed_set);
    if let Some(finding) = report.findings.first() {
        return Ok(fail(
            "tamper",
            format!(
                "the tamper guard found {}: {}:{}: {}",
                finding.kind.as_str(),
                finding.file,
                finding.line,
                finding.what
            ),
            files,
            lines,
        ));
    }
    let verify_args = Args {
        base: Some(j.base.to_owned()),
        ..Args::default()
    };
    let (verify, _) =
        crate::verify_report(j.worktree, &verify_args, false, crate::VerifyMode::Full)?;
    drop(guard);
    let after_checks = RepoState::capture(j.root);
    let outside = j.before.changes(&after_checks, j.branch);
    if !outside.is_empty() {
        j.before.restore(j.root, &after_checks, j.branch);
        return Ok(fail(
            "outside-worktree",
            format!(
                "code from the change, run by the re-check, {}",
                outside.join(", ")
            ),
            files,
            lines,
        ));
    }
    if !verify.ok {
        return Ok(fail(
            "verify",
            format!(
                "verify fails on the change{}",
                verify
                    .failed_step
                    .as_ref()
                    .map(|step| format!(" at {step}"))
                    .unwrap_or_default()
            ),
            files,
            lines,
        ));
    }
    Ok(Run {
        result: "pass",
        reason: None,
        detail: String::new(),
        files,
        lines,
        agent: agent.clone(),
    })
}
