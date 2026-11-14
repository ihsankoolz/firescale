//! `fscale fix --agent <name> [--rule <id>]`: one rule's remaining
//! violations handed to an agent, with every guardrail of SPEC §7 (M27,
//! ADR 0047). The pieces are in `firescale_core::agentrun`; this puts them
//! in order and runs the re-check.

use std::env;
use std::path::{Path, PathBuf};
use std::time::Instant;

use firescale_core::agentrun::{
    append_ledger, diff_guard, now_utc, repo_limits, Adapter, AgentResult, LedgerEntry, Policy,
    ProgramAdapter, RepoState, Request, RunWorktree,
};
use firescale_core::blame::BaseCommit;
use firescale_core::check::run_check;
use firescale_core::config::load_config;
use firescale_core::explain::rule_skill;
use firescale_core::git::run_git;
use firescale_core::model::Fixable;
use firescale_core::node::NodeHost;
use firescale_core::select::select_rules;
use firescale_core::tamper;
use firescale_core::{version, Error, Result};

use crate::{Args, Output, EXIT_OK, EXIT_VIOLATIONS};

/// The agents `--agent` names. Only `fake` is wired in M27; the Claude
/// Code and Codex adapters arrive in M28.
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
        "claude" | "codex" | "gemini" => Err(Error::usage(format!(
            "the {name} adapter is not built yet (M28). `--agent fake` runs the stand-in agent the tests use."
        ))),
        other => Err(Error::usage(format!(
            "unknown agent \"{other}\". Pass --agent fake (the stand-in), or claude or codex once their adapters exist."
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
        get("maxCostUsd").map_or(2.0, |v| v.max(0.0)),
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

    // 2. Isolation: a worktree on a new branch.
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

    let run = judge(
        root,
        &worktree.path,
        &base,
        &agent,
        &policy,
        &allowed,
        &protected,
        &rule_id,
        &before,
        &branch,
    );
    let run = match run {
        Ok(run) => run,
        Err(error) => {
            worktree.roll_back();
            return Err(error);
        }
    };

    // 8 and 9. Pass: one commit with trailers. Fail: roll back.
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
            worktree.roll_back();
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
        worktree.roll_back();
        out.push_str(&format!(
            "fix --agent {}: FAILED - {rule_id}: {}. The run was rolled back; your branch and files are untouched.\n",
            adapter.name(),
            run.detail
        ));
    }
    out.push_str(&format!("Agent: {}\n", version_line));

    // 10. The ledger.
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
        cost_usd: run.agent.cost_usd,
        turns: run.agent.turns,
    };
    append_ledger(root, &entry)?;
    Ok(Output {
        stdout: out,
        notes: Vec::new(),
        code: if passed { EXIT_OK } else { EXIT_VIOLATIONS },
    })
}

/// Guardrails 5, 7 and 8, and the rule against pushing or tagging: what
/// the run did, judged in the worktree. Stages the change when it passes.
#[allow(clippy::too_many_arguments)]
fn judge(
    root: &Path,
    worktree: &Path,
    base: &str,
    agent: &AgentResult,
    policy: &Policy,
    allowed: &[String],
    protected: &[String],
    rule_id: &str,
    before: &RepoState,
    branch: &str,
) -> Result<Run> {
    let fail = |reason: &'static str, detail: String, files: usize, lines: usize| Run {
        result: "fail",
        reason: Some(reason),
        detail,
        files,
        lines,
        agent: agent.clone(),
    };
    // Stage everything to measure it, tracked or new.
    let _ = run_git(worktree, &["add", "--all"]);
    let changed = git_lines(
        worktree,
        &[
            "-c",
            "core.quotepath=off",
            "diff",
            "--cached",
            "--name-only",
        ],
    );
    let lines: usize = git_lines(worktree, &["diff", "--cached", "--numstat"])
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
    let after = RepoState::capture(root);
    let outside = before.changes(&after, branch);
    if !outside.is_empty() {
        before.restore(root, &after, branch);
        return Ok(fail(
            "outside-worktree",
            format!("the agent {}", outside.join(", ")),
            files,
            lines,
        ));
    }
    if !agent.ok {
        let detail = agent
            .error
            .clone()
            .unwrap_or_else(|| "the agent reported an error".to_owned());
        let reason = if detail.contains("time limit") {
            "timeout"
        } else {
            "agent-error"
        };
        return Ok(fail(reason, detail, files, lines));
    }
    if let Some(cost) = agent.cost_usd {
        if cost > policy.max_cost_usd {
            return Ok(fail(
                "over-budget",
                format!(
                    "the agent spent ${cost:.2}, over the limit of ${:.2}",
                    policy.max_cost_usd
                ),
                files,
                lines,
            ));
        }
    }
    if let Some(turns) = agent.turns {
        if turns > policy.max_turns {
            return Ok(fail(
                "over-budget",
                format!(
                    "the agent took {turns} turns, over the limit of {}",
                    policy.max_turns
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
    // 7. The diff guard.
    if let Some(why) = diff_guard(&changed, allowed, protected) {
        return Ok(fail("diff-guard", why, files, lines));
    }
    // 8. The re-check: the rule again, in the worktree.
    let packs = firescale_rules::builtin_packs();
    let mut host = NodeHost::new(worktree);
    let config = load_config(worktree, &packs, &mut host)?;
    let selection = select_rules(config.rules, None)?;
    let outcome = run_check(worktree, &selection.rules, version(), &mut host, false)?;
    let left = outcome
        .report
        .diagnostics
        .iter()
        .filter(|d| d.rule_id == rule_id)
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
        commit: base.to_owned(),
    };
    let changed_set = changed.iter().cloned().collect();
    let report = tamper::guard(worktree, &base_commit, &changed_set);
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
    let mut verify_args = Args {
        base: Some(base.to_owned()),
        ..Args::default()
    };
    verify_args.no_cache = false;
    let (verify, _) = crate::verify_report(worktree, &verify_args, false, crate::VerifyMode::Full)?;
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
