//! `fscale verify`: conformance, then lint, typecheck and tests (SPEC §8).
//!
//! The output is one compact summary written for an agent to act on: what
//! failed, the end of its output, and the command to run again.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

use serde::Serialize;

use crate::error::{Error, Result};
use crate::facts::RepoFacts;

/// Version of the `fscale verify --json` format.
pub const VERIFY_SCHEMA_VERSION: u32 = 1;

/// The steps after `check`, in the order they run.
pub const STEPS: [&str; 3] = ["lint", "typecheck", "test"];

/// How many lines of a failed command's output the summary keeps.
const TAIL_LINES: usize = 30;

/// A step's setting in the `verify` section of the config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepOverride {
    /// Run this command for the step.
    Command(String),
    /// Do not run the step.
    Skip,
}

/// What will run for one step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Planned {
    Command(String),
    /// Not run, with the reason.
    Skipped(String),
}

/// The commands `verify` will run, and how they were chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyPlan {
    /// `config`, `nx`, `turbo`, `scripts`, `cargo` or `none`.
    pub runner: String,
    pub steps: Vec<(String, Planned)>,
}

fn exec_prefix(package_manager: Option<&str>) -> &'static str {
    match package_manager {
        Some("pnpm") => "pnpm exec",
        Some("yarn") => "yarn",
        Some("bun") => "bunx",
        _ => "npx",
    }
}

/// True when `turbo.json` defines the task, under `tasks` (v2) or `pipeline` (v1).
fn turbo_has_task(root: &Path, task: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(root.join("turbo.json")) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}'))
    else {
        return false;
    };
    ["tasks", "pipeline"].iter().any(|key| {
        value
            .get(key)
            .and_then(|tasks| tasks.as_object())
            .is_some_and(|tasks| {
                tasks
                    .keys()
                    .any(|name| name == task || name.ends_with(&format!("#{task}")))
            })
    })
}

/// Chooses a command for each step. Order of preference: the config, Nx,
/// Turborepo, package scripts, then Cargo for a Rust-only repo.
#[must_use]
pub fn plan_verify(
    root: &Path,
    facts: &RepoFacts,
    overrides: &BTreeMap<String, StepOverride>,
    affected: bool,
) -> VerifyPlan {
    let has_tool = |name: &str| facts.tools.iter().any(|tool| tool == name);
    let has_package_json = root.join("package.json").is_file();
    let rust_only = !has_package_json && facts.languages.iter().any(|language| language == "rust");
    let prefix = exec_prefix(facts.package_manager.as_deref());

    let detected_runner = if has_tool("nx") {
        "nx"
    } else if has_tool("turbo") {
        "turbo"
    } else if has_package_json {
        "scripts"
    } else if rust_only {
        "cargo"
    } else {
        "none"
    };

    let detect = |step: &str| -> Planned {
        match detected_runner {
            "nx" => Planned::Command(if affected {
                format!("{prefix} nx affected -t {step}")
            } else {
                format!("{prefix} nx run-many -t {step}")
            }),
            "turbo" if turbo_has_task(root, step) => Planned::Command(format!(
                "{prefix} turbo run {step}{}",
                if affected { " --affected" } else { "" }
            )),
            "turbo" => Planned::Skipped(format!("turbo.json has no {step} task")),
            "scripts" => {
                let command = match step {
                    "lint" => &facts.commands.lint,
                    "typecheck" => &facts.commands.typecheck,
                    _ => &facts.commands.test,
                };
                command.clone().map_or_else(
                    || Planned::Skipped(format!("package.json has no {step} script")),
                    Planned::Command,
                )
            }
            "cargo" => match step {
                "lint" => Planned::Command(
                    "cargo clippy --workspace --all-targets -- -D warnings".to_owned(),
                ),
                "test" => Planned::Command("cargo test --workspace".to_owned()),
                _ => Planned::Skipped("cargo clippy already type-checks".to_owned()),
            },
            _ => Planned::Skipped(format!(
                "no {step} command found; set verify.{step} in your firescale config"
            )),
        }
    };

    let steps: Vec<(String, Planned)> = STEPS
        .iter()
        .map(|step| {
            let planned = match overrides.get(*step) {
                Some(StepOverride::Command(command)) => Planned::Command(command.clone()),
                Some(StepOverride::Skip) => {
                    Planned::Skipped("turned off in your firescale config".to_owned())
                }
                None => detect(step),
            };
            ((*step).to_owned(), planned)
        })
        .collect();
    let all_from_config = STEPS.iter().all(|step| overrides.contains_key(*step));
    VerifyPlan {
        runner: if all_from_config {
            "config"
        } else {
            detected_runner
        }
        .to_owned(),
        steps,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StepStatus {
    Passed,
    Failed,
    Skipped,
    /// An earlier step failed, so this one did not run.
    NotRun,
}

impl StepStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "FAILED",
            Self::Skipped => "skipped",
            Self::NotRun => "not run",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepReport {
    /// `check`, `lint`, `typecheck` or `test`.
    pub name: String,
    pub status: StepStatus,
    /// The command that ran, or would have. Absent for a skipped step.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    /// One line about the outcome: a count, or why the step was skipped.
    pub detail: String,
    /// The last lines of a failed step's output, without colour codes.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub output_tail: Vec<String>,
}

/// The `fscale verify --json` document. Described by
/// `packages/cli/schemas/verify-output.schema.json`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyReport {
    pub schema_version: u32,
    pub version: String,
    pub ok: bool,
    pub runner: String,
    /// Name of the first step that failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_step: Option<String>,
    /// The command to run again after fixing the failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rerun: Option<String>,
    pub steps: Vec<StepReport>,
}

/// The result of the `check` step, worked out by the caller.
pub struct CheckStep {
    pub ok: bool,
    pub detail: String,
    /// Lines to show when the check failed: the violations.
    pub output: Vec<String>,
    pub duration_ms: u64,
    /// The command that repeats the step.
    pub command: String,
}

/// Removes ANSI escape sequences, which only add noise for an agent.
#[must_use]
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            // Parameters and intermediates, then one final byte in @..~.
            for next in chars.by_ref() {
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
        }
    }
    out
}

fn tail(stdout: &[u8], stderr: &[u8]) -> Vec<String> {
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    );
    let lines: Vec<String> = strip_ansi(&text)
        .lines()
        .map(|line| line.trim_end().to_owned())
        .filter(|line| !line.is_empty())
        .collect();
    lines[lines.len().saturating_sub(TAIL_LINES)..].to_vec()
}

/// Runs one command through the platform shell and reports how it went.
fn run_step(root: &Path, name: &str, command: &str) -> Result<StepReport> {
    let started = Instant::now();
    let mut shell = if cfg!(windows) {
        let mut shell = Command::new("cmd");
        shell.args(["/D", "/S", "/C", command]);
        shell
    } else {
        let mut shell = Command::new("sh");
        shell.args(["-c", command]);
        shell
    };
    let output = shell
        .current_dir(root)
        .stdin(Stdio::null())
        .env("NO_COLOR", "1")
        .env("FORCE_COLOR", "0")
        .output()
        .map_err(|error| {
            Error::user(format!(
                "Could not start the {name} command `{command}` ({error}). Check the command, or set verify.{name} in your firescale config."
            ))
        })?;
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let code = output.status.code();
    let passed = output.status.success();
    Ok(StepReport {
        name: name.to_owned(),
        status: if passed {
            StepStatus::Passed
        } else {
            StepStatus::Failed
        },
        command: Some(command.to_owned()),
        exit_code: code,
        duration_ms,
        detail: if passed {
            command.to_owned()
        } else {
            match code {
                Some(code) => format!("{command} (exit {code})"),
                None => format!("{command} (killed by a signal)"),
            }
        },
        output_tail: if passed {
            Vec::new()
        } else {
            tail(&output.stdout, &output.stderr)
        },
    })
}

/// Runs the plan after the check step. Stops at the first failure: the steps
/// after it are reported as not run.
pub fn run_verify(
    root: &Path,
    version: &str,
    check: CheckStep,
    plan: &VerifyPlan,
) -> Result<VerifyReport> {
    let mut steps = vec![StepReport {
        name: "check".to_owned(),
        status: if check.ok {
            StepStatus::Passed
        } else {
            StepStatus::Failed
        },
        command: Some(check.command),
        exit_code: Some(i32::from(!check.ok)),
        duration_ms: check.duration_ms,
        detail: check.detail,
        output_tail: if check.ok { Vec::new() } else { check.output },
    }];
    let mut failed = !check.ok;

    for (name, planned) in &plan.steps {
        let report = match planned {
            Planned::Skipped(reason) => StepReport {
                name: name.clone(),
                status: StepStatus::Skipped,
                command: None,
                exit_code: None,
                duration_ms: 0,
                detail: reason.clone(),
                output_tail: Vec::new(),
            },
            Planned::Command(command) if failed => StepReport {
                name: name.clone(),
                status: StepStatus::NotRun,
                command: Some(command.clone()),
                exit_code: None,
                duration_ms: 0,
                detail: "an earlier step failed".to_owned(),
                output_tail: Vec::new(),
            },
            Planned::Command(command) => run_step(root, name, command)?,
        };
        failed = failed || report.status == StepStatus::Failed;
        steps.push(report);
    }

    let first_failure = steps.iter().find(|step| step.status == StepStatus::Failed);
    Ok(VerifyReport {
        schema_version: VERIFY_SCHEMA_VERSION,
        version: version.to_owned(),
        ok: first_failure.is_none(),
        runner: plan.runner.clone(),
        failed_step: first_failure.map(|step| step.name.clone()),
        rerun: first_failure.and_then(|step| step.command.clone()),
        steps,
    })
}

/// Renders the compact summary. No durations, so the text is stable.
#[must_use]
pub fn format_verify_human(report: &VerifyReport) -> String {
    let mut out = match &report.failed_step {
        Some(step) => format!("verify: FAILED at {step}\n"),
        None => "verify: OK\n".to_owned(),
    };
    for step in &report.steps {
        out.push_str(&format!(
            "  {:<10} {:<8} {}\n",
            step.name,
            step.status.label(),
            step.detail
        ));
    }
    if let Some(failure) = report
        .steps
        .iter()
        .find(|step| step.status == StepStatus::Failed)
    {
        if !failure.output_tail.is_empty() {
            out.push_str(&format!("\nLast lines of {}:\n", failure.name));
            for line in &failure.output_tail {
                out.push_str(&format!("  {line}\n"));
            }
        }
        if let Some(command) = &report.rerun {
            out.push_str(&format!("\nFix the failure, then run again: {command}\n"));
        }
    }
    out
}

/// Renders the report as the documented JSON format.
#[must_use]
pub fn format_verify_json(report: &VerifyReport) -> String {
    let json = serde_json::to_string_pretty(report).unwrap_or_else(|_| "{}".to_owned());
    format!("{json}\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::Commands;
    use crate::testing::{Fixture, Track};

    fn facts(
        tools: &[&str],
        pm: Option<&str>,
        commands: Commands,
        languages: &[&str],
    ) -> RepoFacts {
        RepoFacts {
            package_manager: pm.map(str::to_owned),
            tools: tools.iter().map(|tool| (*tool).to_owned()).collect(),
            languages: languages
                .iter()
                .map(|language| (*language).to_owned())
                .collect(),
            commands,
            ..RepoFacts::default()
        }
    }

    fn commands_of(plan: &VerifyPlan) -> Vec<String> {
        plan.steps
            .iter()
            .map(|(name, planned)| match planned {
                Planned::Command(command) => format!("{name}: {command}"),
                Planned::Skipped(reason) => format!("{name}: skipped ({reason})"),
            })
            .collect()
    }

    #[test]
    fn package_scripts_are_used_when_there_is_no_task_runner() {
        let fixture = Fixture::new(&[("package.json", "{}")], Track::NoGit).unwrap();
        let commands = Commands {
            lint: Some("pnpm run lint".to_owned()),
            test: Some("pnpm run test".to_owned()),
            ..Commands::default()
        };
        let plan = plan_verify(
            fixture.root(),
            &facts(&[], Some("pnpm"), commands, &["javascript"]),
            &BTreeMap::new(),
            false,
        );
        assert_eq!(plan.runner, "scripts");
        assert_eq!(
            commands_of(&plan),
            [
                "lint: pnpm run lint",
                "typecheck: skipped (package.json has no typecheck script)",
                "test: pnpm run test",
            ]
        );
    }

    #[test]
    fn nx_and_turbo_take_precedence_and_honour_affected() {
        let fixture = Fixture::new(
            &[
                ("package.json", "{}"),
                ("turbo.json", r#"{"tasks":{"lint":{},"test":{}}}"#),
            ],
            Track::NoGit,
        )
        .unwrap();
        let nx = plan_verify(
            fixture.root(),
            &facts(&["nx", "turbo"], Some("pnpm"), Commands::default(), &[]),
            &BTreeMap::new(),
            true,
        );
        assert_eq!(nx.runner, "nx");
        assert_eq!(commands_of(&nx)[0], "lint: pnpm exec nx affected -t lint");

        let turbo = plan_verify(
            fixture.root(),
            &facts(&["turbo"], None, Commands::default(), &[]),
            &BTreeMap::new(),
            false,
        );
        assert_eq!(turbo.runner, "turbo");
        assert_eq!(
            commands_of(&turbo),
            [
                "lint: npx turbo run lint",
                "typecheck: skipped (turbo.json has no typecheck task)",
                "test: npx turbo run test",
            ]
        );
    }

    #[test]
    fn a_rust_only_repo_uses_cargo_and_config_overrides_win() {
        let fixture =
            Fixture::new(&[("Cargo.toml", "[package]\nname = \"x\"\n")], Track::NoGit).unwrap();
        let rust = facts(&[], None, Commands::default(), &["rust"]);
        let plan = plan_verify(fixture.root(), &rust, &BTreeMap::new(), false);
        assert_eq!(plan.runner, "cargo");
        assert_eq!(commands_of(&plan)[2], "test: cargo test --workspace");

        let overrides = BTreeMap::from([
            ("lint".to_owned(), StepOverride::Skip),
            (
                "test".to_owned(),
                StepOverride::Command("make test".to_owned()),
            ),
        ]);
        let plan = plan_verify(fixture.root(), &rust, &overrides, false);
        assert_eq!(
            commands_of(&plan)[0],
            "lint: skipped (turned off in your firescale config)"
        );
        assert_eq!(commands_of(&plan)[2], "test: make test");
    }

    #[test]
    fn strip_ansi_removes_colour_codes() {
        assert_eq!(
            strip_ansi("\u{1b}[31mred\u{1b}[0m plain \u{1b}[1;32mok\u{1b}[m"),
            "red plain ok"
        );
    }

    fn passing_check() -> CheckStep {
        CheckStep {
            ok: true,
            detail: "1 rule, no enforced violations".to_owned(),
            output: Vec::new(),
            duration_ms: 1,
            command: "fscale check".to_owned(),
        }
    }

    #[test]
    fn stops_at_the_first_failing_step_and_keeps_its_output() {
        let fixture = Fixture::new(&[], Track::NoGit).unwrap();
        let fail = if cfg!(windows) {
            "echo boom 1>&2 & exit /b 3"
        } else {
            "echo boom >&2; exit 3"
        };
        let plan = VerifyPlan {
            runner: "config".to_owned(),
            steps: vec![
                ("lint".to_owned(), Planned::Command("echo fine".to_owned())),
                ("typecheck".to_owned(), Planned::Command(fail.to_owned())),
                ("test".to_owned(), Planned::Command("echo never".to_owned())),
            ],
        };
        let report = run_verify(fixture.root(), "0.0.0", passing_check(), &plan).unwrap();
        assert!(!report.ok);
        assert_eq!(report.failed_step.as_deref(), Some("typecheck"));
        assert_eq!(report.rerun.as_deref(), Some(fail));
        let statuses: Vec<StepStatus> = report.steps.iter().map(|step| step.status).collect();
        assert_eq!(
            statuses,
            [
                StepStatus::Passed,
                StepStatus::Passed,
                StepStatus::Failed,
                StepStatus::NotRun
            ]
        );
        assert_eq!(report.steps[2].exit_code, Some(3));
        assert_eq!(report.steps[2].output_tail, ["boom"]);
        let text = format_verify_human(&report);
        assert!(text.starts_with("verify: FAILED at typecheck\n"), "{text}");
        assert!(
            text.contains("Last lines of typecheck:\n  boom\n"),
            "{text}"
        );
        assert!(text.contains("Fix the failure, then run again: "), "{text}");
    }

    #[test]
    fn a_failing_check_stops_everything_else() {
        let fixture = Fixture::new(&[], Track::NoGit).unwrap();
        let check = CheckStep {
            ok: false,
            detail: "1 enforced violation".to_owned(),
            output: vec![".env is tracked".to_owned()],
            duration_ms: 1,
            command: "fscale check".to_owned(),
        };
        let plan = VerifyPlan {
            runner: "config".to_owned(),
            steps: vec![("lint".to_owned(), Planned::Command("echo never".to_owned()))],
        };
        let report = run_verify(fixture.root(), "0.0.0", check, &plan).unwrap();
        assert_eq!(report.failed_step.as_deref(), Some("check"));
        assert_eq!(report.rerun.as_deref(), Some("fscale check"));
        assert_eq!(report.steps[1].status, StepStatus::NotRun);
    }

    #[test]
    fn everything_passing_or_skipped_is_ok() {
        let fixture = Fixture::new(&[], Track::NoGit).unwrap();
        let plan = VerifyPlan {
            runner: "none".to_owned(),
            steps: vec![
                ("lint".to_owned(), Planned::Command("echo fine".to_owned())),
                (
                    "test".to_owned(),
                    Planned::Skipped("no test command found".to_owned()),
                ),
            ],
        };
        let report = run_verify(fixture.root(), "0.0.0", passing_check(), &plan).unwrap();
        assert!(report.ok);
        assert!(format_verify_human(&report).starts_with("verify: OK\n"));
        assert!(report.rerun.is_none());
    }
}
