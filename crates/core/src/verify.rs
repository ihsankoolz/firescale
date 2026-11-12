//! `fscale verify`: conformance, then lint, typecheck and tests (SPEC §8).
//!
//! The output is one compact summary written for an agent to act on: what
//! failed, the end of its output, the command to run again, and every step
//! that did not run. A run with skipped steps is never reported as a plain
//! pass (ADR 0020).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::activity::Activity;
use crate::services::{listening, services_for_folder, Service, PROBE_TIMEOUT};

use crate::error::{Error, Result};
use crate::facts::{source, Project, RepoFacts};
use crate::process::{run_tree, Ending, Watch};
use crate::progress::{format_duration, format_limit, Progress, DEFAULT_HEARTBEAT};
use crate::runs::{RunRecord, RunningStep};
use crate::setup::{refused_service, service_down, setup_problem, SetupProblem};

/// Version of the `fscale verify --json` format.
pub const VERIFY_SCHEMA_VERSION: u32 = 4;

/// The line the summary ends with when a step needs setup (ADR 0026).
pub const DOCTOR_HINT: &str =
    "Run `fscale doctor` to check this machine's tools, env variables, dependencies and services in one go.";

/// How long a step may run when the config sets no limit (ADR 0024).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// The steps after `check`, in the order they run.
pub const STEPS: [&str; 3] = ["lint", "typecheck", "test"];

/// How many lines of a failed command's output the summary keeps.
pub const TAIL_LINES: usize = 30;

/// A step's setting in the `verify` section of the config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepOverride {
    /// Run this command for the step.
    Command(String),
    /// Do not run the step.
    Skip,
}

/// A project's setting under `verify.projects` in the config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectOverride {
    /// Run none of this project's steps.
    Off,
    /// Per-step settings for this project.
    Steps(BTreeMap<String, StepOverride>),
}

/// A time limit from the config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// Stop the step after this long.
    After(Duration),
    /// No limit (`false` in the config).
    Off,
}

/// The time limits set for one project under `verify.projects`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectTimeouts {
    /// `verify.projects.<folder>.timeout`: every step of the project.
    pub all: Option<Limit>,
    /// `verify.projects.<folder>.<step>.timeout`.
    pub steps: BTreeMap<String, Limit>,
}

/// The `verify` section of the config.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VerifyConfig {
    /// Settings for the whole repo, keyed by `lint`, `typecheck` or `test`.
    /// A command here runs once, from the repo root, in place of every
    /// project's own command for that step.
    pub steps: BTreeMap<String, StepOverride>,
    /// When true, a step that was skipped because no command was found makes
    /// verify fail.
    pub fail_on_skipped: bool,
    /// Settings per project, keyed by the project's folder.
    pub projects: BTreeMap<String, ProjectOverride>,
    /// `verify.timeout`: the limit of every step, unless a more specific
    /// one is set. [`DEFAULT_TIMEOUT`] when absent.
    pub timeout: Option<Limit>,
    /// `verify.<step>.timeout`: the limit of that step, at the root and in
    /// every project.
    pub step_timeouts: BTreeMap<String, Limit>,
    /// Limits per project, keyed by the project's folder.
    pub project_timeouts: BTreeMap<String, ProjectTimeouts>,
    /// `verify.ci: false`: do not set `CI=1` for the commands.
    pub no_ci_variable: bool,
    /// `verify.stopWhenServiceDown: false`: a step whose output shows that
    /// a local service it needs is not running keeps running to its end
    /// (ADR 0025). By default it is stopped as soon as that is clear.
    pub keep_running_when_service_down: bool,
}

impl VerifyConfig {
    /// The time limit of a step, and the config key that sets it most
    /// specifically. The most specific setting wins: the project's step,
    /// the project, the step for the whole repo, `verify.timeout`, then
    /// [`DEFAULT_TIMEOUT`].
    #[must_use]
    pub fn timeout_for(&self, project: Option<&str>, step: &str) -> (Option<Duration>, String) {
        let project_settings = project.and_then(|folder| self.project_timeouts.get(folder));
        let limit = project_settings
            .and_then(|settings| settings.steps.get(step).copied())
            .or_else(|| project_settings.and_then(|settings| settings.all))
            .or_else(|| self.step_timeouts.get(step).copied())
            .or(self.timeout)
            .unwrap_or(Limit::After(DEFAULT_TIMEOUT));
        let key = match project {
            Some(folder) => format!("verify.projects[\"{folder}\"].{step}.timeout"),
            None => format!("verify.{step}.timeout"),
        };
        match limit {
            Limit::After(duration) => (Some(duration), key),
            Limit::Off => (None, key),
        }
    }
}

/// What will run for one step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Planned {
    Command(String),
    /// Not run because no command was found, with the reason. Verify names
    /// these and does not report a clean pass: the repo may be missing a
    /// check it should have.
    Skipped(String),
    /// Not run on purpose, with the reason: turned off in the config, or
    /// covered by another step. Reported as `off`, with no warning.
    NotNeeded(String),
}

/// The steps of one project, run in its folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectPlan {
    /// The project's folder: repo-relative, forward slashes. `.` for a
    /// project at the repo root that the root's own commands do not cover.
    pub root: String,
    /// `node`, `rust`, `python` or `go`. `None` for a folder that is only
    /// named in the config.
    pub kind: Option<String>,
    pub steps: Vec<(String, Planned)>,
    /// The command that installs the project's dependencies, named when a
    /// step fails because the environment is not set up.
    pub install: Option<String>,
    /// True for a Python project for which no virtual environment was
    /// found, so its commands use `python` from the PATH (ADR 0022).
    pub python_from_path: bool,
}

/// The commands `verify` will run, and how they were chosen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VerifyPlan {
    /// `config`, `nx`, `turbo`, `scripts`, `cargo`, `python`, `go`,
    /// `projects` or `none`. Describes the repo root.
    pub runner: String,
    /// Steps run from the repo root.
    pub steps: Vec<(String, Planned)>,
    /// Steps run in the folders of projects found below the root.
    pub projects: Vec<ProjectPlan>,
    /// From `verify.failOnSkipped`.
    pub fail_on_skipped: bool,
    /// The command that installs the root's dependencies, if known.
    pub install: Option<String>,
    /// True when the root is a Python project with no virtual environment.
    pub python_from_path: bool,
}

const OFF_IN_CONFIG: &str = "turned off in your firescale config";

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

fn command_of<'a>(commands: &'a crate::facts::Commands, step: &str) -> &'a Option<String> {
    match step {
        "lint" => &commands.lint,
        "typecheck" => &commands.typecheck,
        _ => &commands.test,
    }
}

/// What one step of a project found by `fscale facts` will do, from the
/// commands detected for it.
fn project_step(project: &Project, step: &str) -> Planned {
    if let Some(command) = command_of(&project.commands, step) {
        return Planned::Command(command.clone());
    }
    match (project.kind.as_str(), step) {
        ("rust", "typecheck") => Planned::NotNeeded("cargo clippy already type-checks".to_owned()),
        ("go", "typecheck") => Planned::NotNeeded("go vet already type-checks".to_owned()),
        ("python", "lint") => Planned::Skipped("no linter was found (looked for ruff)".to_owned()),
        ("python", "typecheck") => {
            Planned::Skipped("no type checker was found (looked for mypy)".to_owned())
        }
        ("python", _) => {
            Planned::Skipped("no test runner was found (looked for pytest)".to_owned())
        }
        ("node", "lint") => Planned::Skipped(
            "package.json has no lint script, and no oxlint, eslint or biome config was found"
                .to_owned(),
        ),
        ("node", "typecheck") => Planned::Skipped(
            "package.json has no typecheck script, and there is no tsconfig.json that tsc --noEmit can check"
                .to_owned(),
        ),
        ("node", _) => Planned::Skipped(
            "package.json has no test script, and no vitest or jest config was found".to_owned(),
        ),
        _ => Planned::Skipped(format!("no {step} command found")),
    }
}

/// True for a Python project whose commands use `python` from the PATH,
/// because no virtual environment was found for it.
fn from_path(project: &Project) -> bool {
    project
        .interpreter
        .as_ref()
        .is_some_and(|interpreter| interpreter.source == "path")
}

fn from_override(setting: &StepOverride) -> Planned {
    match setting {
        StepOverride::Command(command) => Planned::Command(command.clone()),
        StepOverride::Skip => Planned::NotNeeded(OFF_IN_CONFIG.to_owned()),
    }
}

/// True for a project that gets steps of its own: one `fscale facts`
/// discovered below the root, or one at the root of a kind that the root's
/// commands (`covered`) do not run. Workspace members are covered by the root.
fn runs_by_itself(project: &Project, covered: &str) -> bool {
    project.source == source::DISCOVERED
        || (project.source == source::ROOT && project.kind != covered)
}

/// The steps of the projects that run by themselves, and of any other folder
/// named under `verify.projects`. `covered` is the kind of project the
/// root's own commands run.
fn plan_projects(facts: &RepoFacts, config: &VerifyConfig, covered: &str) -> Vec<ProjectPlan> {
    let mut plans: Vec<ProjectPlan> = Vec::new();
    let mut discovered: Vec<&Project> = facts
        .projects
        .iter()
        .filter(|project| runs_by_itself(project, covered))
        .collect();
    discovered.sort_by(|a, b| (&a.root, &a.kind).cmp(&(&b.root, &b.kind)));
    for project in discovered {
        // A folder can hold projects of two kinds. Its config entry is used once.
        let first_at_root = !plans.iter().any(|plan| plan.root == project.root);
        // True when a command detected for the project (not one from the
        // config) will run: only then does its interpreter matter.
        let mut detected_runs = false;
        let steps: Vec<(String, Planned)> = match config.projects.get(&project.root) {
            Some(ProjectOverride::Off) => STEPS
                .iter()
                .filter(|_| first_at_root)
                .map(|step| {
                    (
                        (*step).to_owned(),
                        Planned::NotNeeded(format!("this project is {OFF_IN_CONFIG}")),
                    )
                })
                .collect(),
            settings => STEPS
                .iter()
                .filter_map(|step| {
                    let own = match settings {
                        Some(ProjectOverride::Steps(map)) => map.get(*step),
                        _ => None,
                    };
                    let planned = match own {
                        Some(setting) if first_at_root => from_override(setting),
                        Some(_) => return None,
                        // A setting for the whole repo covers this project too.
                        None if config.steps.contains_key(*step) => return None,
                        None => {
                            let planned = project_step(project, step);
                            detected_runs |= matches!(planned, Planned::Command(_));
                            planned
                        }
                    };
                    Some(((*step).to_owned(), planned))
                })
                .collect(),
        };
        if !steps.is_empty() {
            plans.push(ProjectPlan {
                root: project.root.clone(),
                kind: Some(project.kind.clone()),
                steps,
                install: project.commands.install.clone(),
                python_from_path: detected_runs && from_path(project),
            });
        }
    }
    // A folder that was not discovered runs exactly the commands it is given.
    for (folder, setting) in &config.projects {
        if plans.iter().any(|plan| plan.root == *folder)
            || facts
                .projects
                .iter()
                .any(|project| runs_by_itself(project, covered) && project.root == *folder)
        {
            continue;
        }
        if let ProjectOverride::Steps(map) = setting {
            let steps: Vec<(String, Planned)> = STEPS
                .iter()
                .filter_map(|step| {
                    map.get(*step)
                        .map(|setting| ((*step).to_owned(), from_override(setting)))
                })
                .collect();
            if !steps.is_empty() {
                plans.push(ProjectPlan {
                    root: folder.clone(),
                    kind: None,
                    steps,
                    install: None,
                    python_from_path: false,
                });
            }
        }
    }
    plans.sort_by(|a, b| (&a.root, &a.kind).cmp(&(&b.root, &b.kind)));
    plans
}

/// Checks that every folder named under `verify.projects` exists.
pub fn check_project_folders(root: &Path, config: &VerifyConfig) -> Result<()> {
    for folder in config.projects.keys() {
        if !project_dir(root, folder).is_dir() {
            return Err(Error::user(format!(
                "verify.projects names \"{folder}\", but there is no such folder. Fix the path in your firescale config; `fscale facts` lists the projects that were found."
            )));
        }
    }
    Ok(())
}

/// Chooses a command for each step.
///
/// At the repo root the order of preference is: the config, Nx, Turborepo,
/// package scripts (then tool config files), Cargo, then a Python or Go
/// project at the root. Projects that `fscale facts` discovered below the
/// root each run their own steps in their own folder. Workspace members are
/// covered by the root's commands.
///
/// `base` is the `--base` ref of an affected run; it is passed on to
/// `nx affected`.
#[must_use]
pub fn plan_verify(
    root: &Path,
    facts: &RepoFacts,
    config: &VerifyConfig,
    affected: bool,
    base: Option<&str>,
) -> VerifyPlan {
    let overrides = &config.steps;
    // Passed to Nx only when it is a plain ref name, so it needs no quoting.
    let nx_base = base
        .filter(|base| {
            !base.is_empty()
                && !base.starts_with('-')
                && base
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._/-~^@".contains(c))
        })
        .map(|base| format!(" --base={base}"))
        .unwrap_or_default();
    let has_tool = |name: &str| facts.tools.iter().any(|tool| tool == name);
    let has_package_json = root.join("package.json").is_file();
    let rust_only = !has_package_json
        && facts.languages.iter().any(|language| language == "rust")
        && root.join("Cargo.toml").is_file();
    let root_project = |kind: &str| {
        facts
            .projects
            .iter()
            .find(|project| project.root == "." && project.kind == kind)
    };
    let prefix = exec_prefix(facts.package_manager.as_deref());

    let detected_runner = if has_tool("nx") {
        "nx"
    } else if has_tool("turbo") {
        "turbo"
    } else if has_package_json {
        "scripts"
    } else if rust_only {
        "cargo"
    } else if root_project("python").is_some() {
        "python"
    } else if root_project("go").is_some() {
        "go"
    } else {
        "none"
    };

    let detect = |step: &str| -> Planned {
        match detected_runner {
            "nx" => Planned::Command(if affected {
                format!("{prefix} nx affected -t {step}{nx_base}")
            } else {
                format!("{prefix} nx run-many -t {step}")
            }),
            "turbo" if turbo_has_task(root, step) => Planned::Command(format!(
                "{prefix} turbo run {step}{}",
                if affected { " --affected" } else { "" }
            )),
            "turbo" => Planned::Skipped(format!("turbo.json has no {step} task")),
            "scripts" => command_of(&facts.commands, step)
                .clone()
                // No script: a linter or test runner with a config file still counts.
                .or_else(|| command_of(&facts.root_tool_commands, step).clone())
                .map_or_else(
                    || Planned::Skipped(format!("package.json has no {step} script")),
                    Planned::Command,
                ),
            "cargo" => match step {
                "lint" => Planned::Command(
                    "cargo clippy --workspace --all-targets -- -D warnings".to_owned(),
                ),
                "test" => Planned::Command("cargo test --workspace".to_owned()),
                _ => Planned::NotNeeded("cargo clippy already type-checks".to_owned()),
            },
            kind @ ("python" | "go") => root_project(kind).map_or_else(
                || Planned::Skipped(format!("no {step} command found")),
                |project| project_step(project, step),
            ),
            _ => Planned::Skipped(format!(
                "no {step} command found; set verify.{step} in your firescale config"
            )),
        }
    };

    // The kind of project the root's commands run. A project of another kind
    // at the root gets its own steps, so that it is run or named, not dropped.
    let covered = match detected_runner {
        "nx" | "turbo" | "scripts" => "node",
        "cargo" => "rust",
        other => other,
    };
    let projects = plan_projects(facts, config, covered);
    // With no commands of its own, the root only lists what the config sets
    // for it, as long as a project has a step that runs or is named as
    // skipped. Projects that are all turned off do not make the root silent.
    let projects_have_steps = projects.iter().any(|project| {
        project
            .steps
            .iter()
            .any(|(_, planned)| !matches!(planned, Planned::NotNeeded(_)))
    });
    let root_runs = detected_runner != "none" || !projects_have_steps;
    let steps: Vec<(String, Planned)> = STEPS
        .iter()
        .filter_map(|step| {
            let planned = match overrides.get(*step) {
                Some(setting) => from_override(setting),
                None if root_runs => detect(step),
                None => return None,
            };
            Some(((*step).to_owned(), planned))
        })
        .collect();
    let all_from_config = STEPS.iter().all(|step| overrides.contains_key(*step));
    let covering = facts
        .projects
        .iter()
        .find(|project| project.root == "." && project.kind == covered);
    let install = covering.and_then(|project| project.commands.install.clone());
    let python_from_path = covering.is_some_and(from_path);
    VerifyPlan {
        runner: if all_from_config {
            "config"
        } else if root_runs {
            detected_runner
        } else {
            "projects"
        }
        .to_owned(),
        steps,
        projects,
        fail_on_skipped: config.fail_on_skipped,
        install,
        python_from_path,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StepStatus {
    Passed,
    Failed,
    /// No command was found for the step, so nothing was verified.
    Skipped,
    /// The command could not do its work because the environment is not
    /// set up: a dependency is not installed, or a program is not on the
    /// PATH. Not a problem in the code (ADR 0022).
    SetupNeeded,
    /// It gave no result within its time limit, so it was stopped with its
    /// whole process tree (ADR 0024).
    TimedOut,
    /// It was running when verify was stopped (Ctrl+C or a termination
    /// signal), so it was stopped with its whole process tree.
    Stopped,
    /// An earlier step failed, or verify was stopped, so this one did not run.
    NotRun,
    /// Not run on purpose: turned off in the config, or covered by another step.
    Off,
}

impl StepStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "FAILED",
            Self::SetupNeeded => "SETUP",
            Self::TimedOut => "TIMED OUT",
            Self::Stopped => "STOPPED",
            Self::Skipped => "skipped",
            Self::NotRun => "not run",
            Self::Off => "off",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepReport {
    /// `check`, `lint`, `typecheck` or `test`.
    pub name: String,
    /// The folder of the project the step belongs to. Absent for a step of
    /// the repo root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// The kind of that project: `node`, `rust`, `python` or `go`. Absent
    /// for a root step and for a folder that is only named in the config.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub status: StepStatus,
    /// The command that ran, or would have. Absent for a step with no command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    /// One line about the outcome: a count, or why the step did not run.
    pub detail: String,
    /// The last lines of a failed step's output, without colour codes.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub output_tail: Vec<String>,
    /// For a step whose status is `setup-needed`, or `timed-out` with output
    /// that shows a setup problem: the command that sets the environment up.
    /// Absent when no command is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
    /// For a step whose status is `timed-out`: its time limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// For a step whose status is `timed-out`: the config key that sets its
    /// time limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_key: Option<String>,
}

/// The overall outcome of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum VerifyResult {
    /// Every step ran and passed.
    Ok,
    /// Nothing failed, but at least one step was skipped for lack of a command.
    Partial,
    /// A step failed, or steps were skipped while `verify.failOnSkipped` is on.
    Failed,
    /// No step failed in the code, but at least one could not run because
    /// the environment is not set up.
    SetupNeeded,
    /// Verify was stopped by Ctrl+C or a termination signal before every
    /// step finished. The steps that finished are reported.
    Stopped,
}

/// A step that was skipped because no command was found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkippedStep {
    pub step: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub reason: String,
}

/// The `fscale verify --json` document. Described by
/// `packages/cli/schemas/verify-output.schema.json`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyReport {
    pub schema_version: u32,
    pub version: String,
    /// True when the exit code is 0: the result is `ok`, or `partial`.
    pub ok: bool,
    pub result: VerifyResult,
    pub runner: String,
    /// Steps that should run: every step except the ones that are `off`.
    pub steps_total: usize,
    /// Steps that did run, passing or failing.
    pub steps_run: usize,
    pub fail_on_skipped: bool,
    /// Name of the first step that failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_step: Option<String>,
    /// The folder of the project whose step failed. Absent for the repo root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_project: Option<String>,
    /// The command to run again after fixing the failure, from the failed
    /// project's folder when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rerun: Option<String>,
    pub steps: Vec<StepReport>,
    /// Every step that was skipped because no command was found.
    pub skipped: Vec<SkippedStep>,
    /// Things the user should know that are not failures: steps that were
    /// skipped because no command was found.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// Things worth knowing about how steps ran: Python projects that ran
    /// with `python` from the PATH because no virtual environment was found.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
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

/// The platform shell, set up to run one command line.
#[cfg(windows)]
fn shell(command: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut shell = Command::new("cmd");
    // The command line must reach cmd exactly as written. Rust's normal
    // argument quoting would turn `"` into `\"`, which cmd does not
    // understand. With /S, cmd strips the outer pair of quotes and runs the rest.
    shell
        .args(["/D", "/S", "/C"])
        .raw_arg(format!("\"{command}\""));
    shell
}

/// The platform shell, set up to run one command line.
#[cfg(not(windows))]
fn shell(command: &str) -> Command {
    let mut shell = Command::new("sh");
    shell.args(["-c", command]);
    shell
}

/// The folder of a project, from its repo-relative path.
fn project_dir(root: &Path, folder: &str) -> PathBuf {
    folder
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

/// Where a step runs: the repo root, or a project.
struct Unit<'a> {
    root: &'a Path,
    project: Option<&'a ProjectPlan>,
    /// The command that installs this unit's dependencies.
    install: Option<&'a str>,
}

/// The next step when the environment is not set up: the command that
/// starts the services, the install command, or nothing when the summary
/// can only say what to install by hand.
fn setup_fix(problem: &crate::setup::SetupProblem, install: Option<&str>) -> Option<String> {
    if problem.missing_service.is_some() {
        return problem.start_services.clone();
    }
    match (&problem.missing_program, install) {
        (Some(_), _) | (None, None) => None,
        (None, Some(install)) => Some(install.to_owned()),
    }
}

/// How many tests must have errored, with none passed or failed, before a
/// declared service that is down explains the run (ADR 0025).
const ERRORS_BEFORE_PROBE: u64 = 3;

/// How often a running step may probe a service.
const PROBE_EVERY: Duration = Duration::from_secs(5);

/// What a running step's output showed about the services it needs.
#[derive(Default)]
struct Early {
    /// Set once it is clear the step cannot succeed.
    problem: Option<SetupProblem>,
    last_probe: Option<Instant>,
    declared: Option<Vec<Service>>,
}

/// True for a refused connection whose service is known by its client or
/// its port, or that the repo itself names, so a test that connects to a
/// closed port on purpose is not taken for a missing service.
fn known_service(found: &crate::setup::RefusedService, declared: &[Service]) -> bool {
    !found.described.starts_with("a service on")
        || found
            .port
            .is_some_and(|port| declared.iter().any(|service| service.port == port))
}

/// Looks at a running step's output for a local service that is not
/// running, and probes the service to be sure (ADR 0025). Returns the
/// service and what showed it, once it is clear the step cannot succeed:
///
/// - the output shows a refused connection to a known local service, and
///   nothing listens on its port now; or
/// - a known test runner reports only errors so far (at least
///   [`ERRORS_BEFORE_PROBE`]), and a service that the step's
///   `.env.example` or Compose file names is not listening.
fn service_down_while_running(
    activity: &Activity,
    early: &mut Early,
    root: &Path,
    dir: &Path,
) -> Option<(String, String)> {
    if early
        .last_probe
        .is_some_and(|last| last.elapsed() < PROBE_EVERY)
    {
        return None;
    }
    let declared = early
        .declared
        .get_or_insert_with(|| services_for_folder(root, dir));
    let tally = activity.tally();
    let runner_failing = activity.runner().is_none() || tally.failed + tally.errors > 0;
    if let Some(found) = refused_service(&activity.recent().join("\n")) {
        if let Some(port) = found
            .port
            .filter(|_| runner_failing && known_service(&found, declared))
        {
            early.last_probe = Some(Instant::now());
            if !listening(&found.host, port, PROBE_TIMEOUT) {
                // The repo's own name for it, when it names the port.
                let described = declared
                    .iter()
                    .find(|service| {
                        service.port == port && found.described.starts_with("a service on")
                    })
                    .map_or(found.described, Service::describe);
                return Some((
                    described,
                    "the output shows refused connections and nothing listens on that port"
                        .to_owned(),
                ));
            }
            return None;
        }
    }
    if activity.runner().is_some()
        && tally.errors >= ERRORS_BEFORE_PROBE
        && tally.passed == 0
        && tally.failed == 0
        && !declared.is_empty()
    {
        early.last_probe = Some(Instant::now());
        if let Some(down) = declared
            .iter()
            .find(|service| !listening(&service.host, service.port, PROBE_TIMEOUT))
        {
            return Some((
                down.describe(),
                format!(
                    "every test so far has errored ({}), and {} names it",
                    tally.describe(),
                    down.source
                ),
            ));
        }
    }
    None
}

/// How a run behaves, apart from what it runs (ADR 0024).
pub struct RunOptions<'a> {
    /// Set by Ctrl+C or a termination signal: running commands are stopped
    /// with their process trees, and nothing new starts.
    pub stop: &'a AtomicBool,
    /// Where live progress lines go. `None`: no progress.
    pub progress: Option<&'a Progress>,
    /// How many units (the root and the projects) may run at once.
    pub jobs: usize,
    /// The time limits, and whether `CI=1` is set.
    pub config: &'a VerifyConfig,
    /// How often a running step gets a heartbeat line.
    pub heartbeat: Duration,
    /// This run's record, which names the commands it is running.
    pub record: Option<&'a RunRecord>,
}

/// Never set: the stop flag of a run that cannot be stopped from outside.
static NEVER_STOP: AtomicBool = AtomicBool::new(false);

impl RunOptions<'_> {
    /// One unit at a time, no progress, the default limits, no record.
    #[must_use]
    pub fn quiet(config: &VerifyConfig) -> RunOptions<'_> {
        RunOptions {
            stop: &NEVER_STOP,
            progress: None,
            jobs: 1,
            config,
            heartbeat: DEFAULT_HEARTBEAT,
            record: None,
        }
    }

    fn say(&self, text: &str) {
        if let Some(progress) = self.progress {
            progress.line(text);
        }
    }
}

/// The variables every command gets. `CI=1` makes tools that would wait for
/// input or watch for changes run once (ADR 0024); a `CI` the environment
/// already sets is kept.
fn environment(command: &mut Command, config: &VerifyConfig) {
    command.env("NO_COLOR", "1").env("FORCE_COLOR", "0");
    let ci_set = std::env::var_os("CI").is_some_and(|value| !value.is_empty());
    if !config.no_ci_variable && !ci_set {
        command.env("CI", "1");
    }
}

/// Runs one command through the platform shell, in the unit's folder, with
/// no input and a time limit, and reports how it went. A failure whose
/// output shows an environment that is not set up is reported as
/// `setup-needed`.
fn run_step(
    unit: &Unit<'_>,
    name: &str,
    command: &str,
    options: &RunOptions<'_>,
) -> Result<StepReport> {
    let kind = unit.project.and_then(|project| project.kind.clone());
    let project = unit.project.map(|project| project.root.as_str());
    let dir = project.map_or_else(|| unit.root.to_path_buf(), |p| project_dir(unit.root, p));
    let place = match project {
        Some(project) => format!("{name} in {}", place_name(project)),
        None => name.to_owned(),
    };
    let (timeout, timeout_key) = options.config.timeout_for(project, name);
    options.say(&format!("{place}: started: {command}"));
    let mut shell = shell(command);
    shell.current_dir(&dir);
    environment(&mut shell, options.config);
    let spawned = Mutex::new(None);
    let heartbeat = |elapsed: Duration, last: Option<&str>| {
        // `last output: ...`, or a running count of tests (ADR 0025).
        let last = last.map_or_else(String::new, |doing| format!("; {doing}"));
        options.say(&format!(
            "{place}: still running after {}{last}",
            format_duration(elapsed)
        ));
    };
    let on_spawn = |pid: u32| {
        *spawned.lock().unwrap_or_else(PoisonError::into_inner) = Some(pid);
        if let Some(record) = options.record {
            record.started(RunningStep {
                place: place.clone(),
                command: command.to_owned(),
                pid,
            });
        }
    };
    let early = Mutex::new(Early::default());
    let stop_early = !options.config.keep_running_when_service_down;
    let on_output = |activity: &Activity| {
        let mut early = early.lock().unwrap_or_else(PoisonError::into_inner);
        if early.problem.is_some() {
            return false;
        }
        let Some((service, evidence)) =
            service_down_while_running(activity, &mut early, unit.root, &dir)
        else {
            return false;
        };
        let problem = service_down(&service, unit.root, &dir);
        let then = if stop_early {
            "stopping the step (set verify.stopWhenServiceDown to false to let it run on)"
        } else {
            "the step runs on because verify.stopWhenServiceDown is false"
        };
        let start = setup_fix(&problem, unit.install)
            .map_or_else(String::new, |fix| format!("; start it with: {fix}"));
        options.say(&format!(
            "{place}: SETUP NEEDED, seen while running: {service} is not running ({evidence}){start}; {then}"
        ));
        early.problem = Some(problem);
        stop_early
    };
    let watch = Watch {
        timeout,
        stop: options.stop,
        heartbeat: options.heartbeat,
        on_heartbeat: &heartbeat,
        command,
        on_output: &on_output,
        on_spawn: &on_spawn,
    };
    let finished = run_tree(shell, &watch).map_err(|error| {
        let key = project.map_or_else(
            || format!("verify.{name}"),
            |project| format!("verify.projects[\"{project}\"].{name}"),
        );
        Error::user(format!(
            "Could not start the {name} command `{command}` ({error}). Check the command, or set {key} in your firescale config."
        ))
    })?;
    if let (Some(record), Some(pid)) = (
        options.record,
        *spawned.lock().unwrap_or_else(PoisonError::into_inner),
    ) {
        record.ended(pid);
    }
    let duration_ms = u64::try_from(finished.duration.as_millis()).unwrap_or(u64::MAX);
    let took = format_duration(finished.duration);
    let mut report = StepReport {
        name: name.to_owned(),
        project: project.map(str::to_owned),
        kind,
        status: StepStatus::Passed,
        command: Some(command.to_owned()),
        exit_code: None,
        duration_ms,
        detail: command.to_owned(),
        output_tail: Vec::new(),
        fix: None,
        timeout_ms: None,
        timeout_key: None,
    };
    let text = || {
        strip_ansi(&format!(
            "{}\n{}",
            String::from_utf8_lossy(&finished.stdout),
            String::from_utf8_lossy(&finished.stderr)
        ))
    };
    let seen_early = early
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner)
        .problem;
    let code = match finished.ending {
        Ending::Exited(code) => code,
        Ending::GaveUp => {
            let problem =
                seen_early.unwrap_or_else(|| service_down("a local service", unit.root, &dir));
            report.status = StepStatus::SetupNeeded;
            report.detail = format!("{command} (stopped early after {took}): {}", problem.reason);
            report.output_tail = tail(&finished.stdout, &finished.stderr);
            report.fix = setup_fix(&problem, unit.install);
            options.say(&format!(
                "{place}: SETUP NEEDED after {took} (stopped early)"
            ));
            return Ok(report);
        }
        Ending::Stopped => {
            report.status = StepStatus::Stopped;
            report.detail = format!("{command} (stopped: verify was interrupted)");
            options.say(&format!("{place}: STOPPED after {took}"));
            return Ok(report);
        }
        Ending::TimedOut => {
            let limit = timeout.map_or_else(String::new, format_limit);
            report.status = StepStatus::TimedOut;
            report.detail = format!("{command} (no result within {limit})");
            report.output_tail = tail(&finished.stdout, &finished.stderr);
            report.timeout_ms =
                timeout.map(|limit| u64::try_from(limit.as_millis()).unwrap_or(u64::MAX));
            report.timeout_key = Some(timeout_key);
            // A step that hangs often hangs on the environment, such as a
            // database that is not running: say so when the output shows it.
            if let Some(problem) =
                setup_problem(&text(), None, unit.root, &dir).or_else(|| seen_early.clone())
            {
                report.detail = format!(
                    "{command} (no result within {limit}; the output shows {})",
                    problem.reason
                );
                report.fix = setup_fix(&problem, unit.install);
            }
            options.say(&format!("{place}: TIMED OUT after {took} (limit {limit})"));
            return Ok(report);
        }
    };
    report.exit_code = code;
    if code == Some(0) {
        options.say(&format!("{place}: passed after {took}"));
        return Ok(report);
    }
    report.output_tail = tail(&finished.stdout, &finished.stderr);
    let exit = match code {
        Some(code) => format!("exit {code}"),
        None => "killed by a signal".to_owned(),
    };
    match setup_problem(&text(), code, unit.root, &dir).or(seen_early) {
        Some(problem) => {
            report.status = StepStatus::SetupNeeded;
            report.detail = format!("{command} ({exit}): {}", problem.reason);
            report.fix = setup_fix(&problem, unit.install);
            if report.fix.is_none() {
                if let Some(program) = &problem.missing_program {
                    report.detail.push_str(&format!(
                        "; install {program} and make sure it is on the PATH"
                    ));
                }
            }
            options.say(&format!("{place}: SETUP NEEDED after {took} ({exit})"));
        }
        None => {
            report.status = StepStatus::Failed;
            report.detail = format!("{command} ({exit})");
            options.say(&format!("{place}: FAILED after {took} ({exit})"));
        }
    }
    Ok(report)
}

/// How a project's folder is written in a sentence.
fn place_name(folder: &str) -> &str {
    if folder == "." {
        "the repo root"
    } else {
        folder
    }
}

fn plural_steps(count: usize) -> String {
    format!("{count} step{}", if count == 1 { "" } else { "s" })
}

/// True for a step that did not pass and stops the rest of its unit:
/// failed, needs setup, timed out or stopped.
fn blocks(status: StepStatus) -> bool {
    matches!(
        status,
        StepStatus::Failed | StepStatus::SetupNeeded | StepStatus::TimedOut | StepStatus::Stopped
    )
}

/// True for a step that ran to a result.
fn ran(status: StepStatus) -> bool {
    matches!(
        status,
        StepStatus::Passed | StepStatus::Failed | StepStatus::SetupNeeded | StepStatus::TimedOut
    )
}

/// Runs the steps of one unit (the root, or a project) in order. A step that
/// blocks stops the rest, which are reported as not run.
fn run_unit(
    unit: &Unit<'_>,
    unit_steps: &[(String, Planned)],
    check_failed: bool,
    options: &RunOptions<'_>,
) -> Result<Vec<StepReport>> {
    let mut steps = Vec::new();
    let mut failed = check_failed;
    for (name, planned) in unit_steps {
        let idle = |status: StepStatus, command: Option<&String>, detail: &str| StepReport {
            name: name.clone(),
            project: unit.project.map(|project| project.root.clone()),
            kind: unit.project.and_then(|project| project.kind.clone()),
            status,
            command: command.cloned(),
            exit_code: None,
            duration_ms: 0,
            detail: detail.to_owned(),
            output_tail: Vec::new(),
            fix: None,
            timeout_ms: None,
            timeout_key: None,
        };
        let report = match planned {
            Planned::Skipped(reason) => idle(StepStatus::Skipped, None, reason),
            Planned::NotNeeded(reason) => idle(StepStatus::Off, None, reason),
            Planned::Command(command) if options.stop.load(Ordering::SeqCst) => {
                idle(StepStatus::NotRun, Some(command), "verify was stopped")
            }
            Planned::Command(command) if failed => {
                idle(StepStatus::NotRun, Some(command), "an earlier step failed")
            }
            Planned::Command(command) => run_step(unit, name, command, options)?,
        };
        failed = failed || blocks(report.status);
        steps.push(report);
    }
    Ok(steps)
}

/// Runs the plan after the check step: the root's steps, then each project's
/// steps in its own folder. The root and each project are independent: a
/// failure stops the rest of its own steps, which are reported as not run,
/// and the others still run. A failing `check` stops everything.
///
/// One unit at a time, with no progress and the default time limits; see
/// [`run_verify_with`].
pub fn run_verify(
    root: &Path,
    version: &str,
    check: CheckStep,
    plan: &VerifyPlan,
) -> Result<VerifyReport> {
    let config = VerifyConfig::default();
    run_verify_with(root, version, check, plan, &RunOptions::quiet(&config))
}

/// [`run_verify`] with the options of the run: progress, parallel units,
/// time limits and the stop flag. With more than one job, units run side by
/// side; the report lists them in the plan's order all the same.
pub fn run_verify_with(
    root: &Path,
    version: &str,
    check: CheckStep,
    plan: &VerifyPlan,
    options: &RunOptions<'_>,
) -> Result<VerifyReport> {
    let mut steps = vec![StepReport {
        name: "check".to_owned(),
        project: None,
        kind: None,
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
        fix: None,
        timeout_ms: None,
        timeout_key: None,
    }];
    let check_failed = !check.ok;

    let units: Vec<(&[(String, Planned)], Unit<'_>)> = std::iter::once((
        plan.steps.as_slice(),
        Unit {
            root,
            project: None,
            install: plan.install.as_deref(),
        },
    ))
    .chain(plan.projects.iter().map(|project| {
        (
            project.steps.as_slice(),
            Unit {
                root,
                project: Some(project),
                install: project.install.as_deref(),
            },
        )
    }))
    .collect();
    let jobs = options.jobs.clamp(1, units.len().max(1));
    let results: Vec<Result<Vec<StepReport>>> = if jobs == 1 {
        units
            .iter()
            .map(|(unit_steps, unit)| run_unit(unit, unit_steps, check_failed, options))
            .collect()
    } else {
        let next = AtomicUsize::new(0);
        let slots: Vec<Mutex<Option<Result<Vec<StepReport>>>>> =
            units.iter().map(|_| Mutex::new(None)).collect();
        std::thread::scope(|scope| {
            for _ in 0..jobs {
                scope.spawn(|| loop {
                    let index = next.fetch_add(1, Ordering::SeqCst);
                    let Some((unit_steps, unit)) = units.get(index) else {
                        break;
                    };
                    let result = run_unit(unit, unit_steps, check_failed, options);
                    *slots[index].lock().unwrap_or_else(PoisonError::into_inner) = Some(result);
                });
            }
        });
        slots
            .into_iter()
            .map(|slot| {
                slot.into_inner()
                    .unwrap_or_else(PoisonError::into_inner)
                    .unwrap_or_else(|| {
                        Err(Error::internal(
                            "A verify unit did not finish. Please report this.",
                        ))
                    })
            })
            .collect()
    };
    for result in results {
        steps.extend(result?);
    }
    let stopped = options.stop.load(Ordering::SeqCst)
        || steps.iter().any(|step| step.status == StepStatus::Stopped);

    let skipped: Vec<SkippedStep> = steps
        .iter()
        .filter(|step| step.status == StepStatus::Skipped)
        .map(|step| SkippedStep {
            step: step.name.clone(),
            project: step.project.clone(),
            reason: step.detail.clone(),
        })
        .collect();
    let warnings = match skipped.first() {
        None => Vec::new(),
        Some(first) => {
            let names: Vec<String> = skipped
                .iter()
                .map(|skip| match &skip.project {
                    Some(project) => format!("{} in {}", skip.step, place_name(project)),
                    None => skip.step.clone(),
                })
                .collect();
            let (key, whose) = match &first.project {
                Some(project) => (
                    format!("verify.projects[\"{project}\"].{}", first.step),
                    "that project",
                ),
                None => (format!("verify.{}", first.step), "this repo"),
            };
            vec![format!(
                "{} skipped because no command was found: {}. Set {key} in your firescale config to a command, or to false if {whose} has none.",
                if skipped.len() == 1 { "1 step was" } else { "steps were" },
                names.join(", ")
            )]
        }
    };
    let steps_total = steps
        .iter()
        .filter(|step| step.status != StepStatus::Off)
        .count();
    let steps_run = steps.iter().filter(|step| ran(step.status)).count();
    let mut on_path: Vec<&str> = Vec::new();
    if plan.python_from_path
        && plan
            .steps
            .iter()
            .any(|(_, p)| matches!(p, Planned::Command(_)))
    {
        on_path.push(".");
    }
    for project in &plan.projects {
        if project.python_from_path
            && project
                .steps
                .iter()
                .any(|(_, planned)| matches!(planned, Planned::Command(_)))
        {
            on_path.push(&project.root);
        }
    }
    let notes = if on_path.is_empty() {
        Vec::new()
    } else {
        let places: Vec<&str> = on_path.iter().map(|folder| place_name(folder)).collect();
        vec![format!(
            "no virtual environment was found for {}, so {} Python commands use python from the PATH. To use one, create it in the project's folder (python -m venv .venv) or activate it.",
            places.join(", "),
            if places.len() == 1 { "its" } else { "their" }
        )]
    };
    let first_failure = steps
        .iter()
        .find(|step| blocks(step.status) && step.status != StepStatus::Stopped);
    let any_failed = steps
        .iter()
        .any(|step| matches!(step.status, StepStatus::Failed | StepStatus::TimedOut));
    let result = if stopped {
        VerifyResult::Stopped
    } else if any_failed || (plan.fail_on_skipped && !skipped.is_empty()) {
        VerifyResult::Failed
    } else if first_failure.is_some() {
        VerifyResult::SetupNeeded
    } else if skipped.is_empty() {
        VerifyResult::Ok
    } else {
        VerifyResult::Partial
    };
    Ok(VerifyReport {
        schema_version: VERIFY_SCHEMA_VERSION,
        version: version.to_owned(),
        ok: matches!(result, VerifyResult::Ok | VerifyResult::Partial),
        result,
        runner: plan.runner.clone(),
        steps_total,
        steps_run,
        fail_on_skipped: plan.fail_on_skipped,
        failed_step: first_failure.map(|step| step.name.clone()),
        failed_project: first_failure.and_then(|step| step.project.clone()),
        rerun: first_failure.and_then(|step| step.command.clone()),
        steps,
        skipped,
        warnings,
        notes,
    })
}

/// `test in backend/auth`, or `test` for a step of the repo root.
fn step_place(step: &StepReport) -> String {
    match &step.project {
        Some(project) => format!("{} in {}", step.name, place_name(project)),
        None => step.name.clone(),
    }
}

/// The first line of the summary. It reads as a clean pass only when every
/// step ran and passed, and it names every step that failed, timed out or
/// needs setup.
fn headline(report: &VerifyReport) -> String {
    let named = |status: StepStatus| -> Vec<String> {
        report
            .steps
            .iter()
            .filter(|step| step.status == status)
            .map(step_place)
            .collect()
    };
    let failed = named(StepStatus::Failed);
    let timed_out = named(StepStatus::TimedOut);
    let setup = named(StepStatus::SetupNeeded);
    let mut parts = Vec::new();
    if report.result == VerifyResult::Stopped {
        parts.push(format!(
            "STOPPED - interrupted after {} of {} ran",
            report.steps_run,
            plural_steps(report.steps_total)
        ));
    }
    if !failed.is_empty() {
        parts.push(format!("FAILED at {}", failed.join(", ")));
    }
    if !timed_out.is_empty() {
        parts.push(format!("TIMED OUT at {}", timed_out.join(", ")));
    }
    if !setup.is_empty() {
        parts.push(format!("SETUP NEEDED for {}", setup.join(", ")));
    }
    if !parts.is_empty() {
        return format!("verify: {}", parts.join("; "));
    }
    match report.result {
        VerifyResult::Ok => "verify: OK".to_owned(),
        VerifyResult::Partial => format!(
            "verify: PARTIAL - {} of {} ran, {} skipped",
            report.steps_run,
            plural_steps(report.steps_total),
            report.skipped.len()
        ),
        VerifyResult::Failed | VerifyResult::SetupNeeded | VerifyResult::Stopped => format!(
            "verify: FAILED - {} of {} skipped, and verify.failOnSkipped is on",
            report.skipped.len(),
            plural_steps(report.steps_total)
        ),
    }
}

/// Renders the compact summary. No durations, so the text is stable.
#[must_use]
pub fn format_verify_human(report: &VerifyReport) -> String {
    let mut out = format!("{}\n", headline(report));
    let mut current: Option<(&str, Option<&str>)> = None;
    for step in &report.steps {
        let project = step
            .project
            .as_deref()
            .map(|project| (project, step.kind.as_deref()));
        if project != current {
            if let Some((folder, kind)) = project {
                // The kind is named when the folder alone does not say which
                // project this is: two kinds in one folder, or the repo root.
                let shared = report.steps.iter().any(|other| {
                    other.project.as_deref() == Some(folder) && other.kind.as_deref() != kind
                });
                let place = place_name(folder);
                match kind {
                    Some(kind) if shared || folder == "." => {
                        out.push_str(&format!("  in {place} ({kind}):\n"));
                    }
                    _ => out.push_str(&format!("  in {place}:\n")),
                }
            }
            current = project;
        }
        out.push_str(&format!(
            "{}  {:<10} {:<8} {}\n",
            if project.is_some() { "  " } else { "" },
            step.name,
            step.status.label(),
            step.detail
        ));
    }
    for failure in report.steps.iter().filter(|step| blocks(step.status)) {
        let place = failure
            .project
            .as_ref()
            .map_or_else(String::new, |project| {
                format!(" in {}", place_name(project))
            });
        if !failure.output_tail.is_empty() {
            out.push_str(&format!("\nLast lines of {}{place}:\n", failure.name));
            for line in &failure.output_tail {
                out.push_str(&format!("  {line}\n"));
            }
        }
        let Some(command) = &failure.command else {
            continue;
        };
        if failure.status == StepStatus::Stopped {
            continue;
        }
        if failure.status == StepStatus::TimedOut {
            let limit = failure
                .timeout_ms
                .map_or_else(String::new, |ms| format_limit(Duration::from_millis(ms)));
            out.push_str(&format!(
                "\nTimed out: {}{place} gave no result within {limit}, so it was stopped with every process it started.\n",
                failure.name
            ));
            if let Some(fix) = &failure.fix {
                out.push_str(&format!(
                    "Its output shows the environment is not ready. Set it up{place}: {fix}\n"
                ));
            }
            if let Some(key) = &failure.timeout_key {
                out.push_str(&format!(
                    "If it is only slow, raise {key} in your firescale config.\n"
                ));
            }
            out.push_str(&format!("Then run again{place}: {command}\n"));
            continue;
        }
        if failure.status == StepStatus::SetupNeeded {
            out.push_str(&format!(
                "\nSetup needed for {}{place}: the environment is not ready, so this is not a problem in the code. Do not change code for it.\n",
                failure.name
            ));
            match &failure.fix {
                Some(fix) => out.push_str(&format!("Set it up{place}: {fix}\n")),
                None => out.push_str("Set up what the line above names, or ask a person to.\n"),
            }
            out.push_str(&format!("Then run again{place}: {command}\n"));
        } else {
            out.push_str(&format!(
                "\nFix the failure, then run again{place}: {command}\n"
            ));
        }
    }
    if report
        .steps
        .iter()
        .any(|step| step.status == StepStatus::SetupNeeded)
    {
        out.push_str(&format!("\n{DOCTOR_HINT}\n"));
    }
    if report.result == VerifyResult::Stopped {
        out.push_str("\nStopped: verify was interrupted (Ctrl+C or a termination signal), and every command it had started was stopped. Steps marked STOPPED or not run did not finish; run fscale verify again to check them.\n");
    }
    if !report.warnings.is_empty() {
        out.push('\n');
        for warning in &report.warnings {
            out.push_str(&format!("warning: {warning}\n"));
        }
    }
    if !report.notes.is_empty() {
        out.push('\n');
        for note in &report.notes {
            out.push_str(&format!("note: {note}\n"));
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
                Planned::Skipped(reason) | Planned::NotNeeded(reason) => {
                    format!("{name}: skipped ({reason})")
                }
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
            &VerifyConfig::default(),
            false,
            None,
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
            &VerifyConfig::default(),
            true,
            Some("origin/main"),
        );
        assert_eq!(nx.runner, "nx");
        assert_eq!(
            commands_of(&nx)[0],
            "lint: pnpm exec nx affected -t lint --base=origin/main"
        );
        // A base that is not a plain ref name is left out instead of being quoted.
        let odd = plan_verify(
            fixture.root(),
            &facts(&["nx"], Some("pnpm"), Commands::default(), &[]),
            &VerifyConfig::default(),
            true,
            Some("main; echo hi"),
        );
        assert_eq!(commands_of(&odd)[0], "lint: pnpm exec nx affected -t lint");

        let turbo = plan_verify(
            fixture.root(),
            &facts(&["turbo"], None, Commands::default(), &[]),
            &VerifyConfig::default(),
            false,
            None,
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
        let plan = plan_verify(fixture.root(), &rust, &VerifyConfig::default(), false, None);
        assert_eq!(plan.runner, "cargo");
        assert_eq!(commands_of(&plan)[2], "test: cargo test --workspace");

        let overrides = VerifyConfig {
            steps: BTreeMap::from([
                ("lint".to_owned(), StepOverride::Skip),
                (
                    "test".to_owned(),
                    StepOverride::Command("make test".to_owned()),
                ),
            ]),
            ..VerifyConfig::default()
        };
        let plan = plan_verify(fixture.root(), &rust, &overrides, false, None);
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
            ..VerifyPlan::default()
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
            ..VerifyPlan::default()
        };
        let report = run_verify(fixture.root(), "0.0.0", check, &plan).unwrap();
        assert_eq!(report.failed_step.as_deref(), Some("check"));
        assert_eq!(report.rerun.as_deref(), Some("fscale check"));
        assert_eq!(report.steps[1].status, StepStatus::NotRun);
    }

    /// This test used to assert "verify: OK" for a run with a skipped step.
    /// That was the false green reported from a real repo (ADR 0020): the
    /// run still exits 0, but the headline now says it was only partial.
    #[test]
    fn a_skipped_step_makes_the_run_partial_not_ok() {
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
            ..VerifyPlan::default()
        };
        let report = run_verify(fixture.root(), "0.0.0", passing_check(), &plan).unwrap();
        assert!(report.ok);
        assert_eq!(report.result, VerifyResult::Partial);
        assert_eq!((report.steps_run, report.steps_total), (2, 3));
        let text = format_verify_human(&report);
        assert!(
            text.starts_with("verify: PARTIAL - 2 of 3 steps ran, 1 skipped\n"),
            "{text}"
        );
        assert!(report.rerun.is_none());
        assert_eq!(
            report.skipped,
            [SkippedStep {
                step: "test".to_owned(),
                project: None,
                reason: "no test command found".to_owned(),
            }]
        );
    }

    #[test]
    fn everything_passing_is_ok() {
        let fixture = Fixture::new(&[], Track::NoGit).unwrap();
        let plan = VerifyPlan {
            runner: "config".to_owned(),
            steps: vec![
                ("lint".to_owned(), Planned::Command("echo fine".to_owned())),
                (
                    "typecheck".to_owned(),
                    Planned::NotNeeded("turned off in your firescale config".to_owned()),
                ),
                ("test".to_owned(), Planned::Command("echo fine".to_owned())),
            ],
            ..VerifyPlan::default()
        };
        let report = run_verify(fixture.root(), "0.0.0", passing_check(), &plan).unwrap();
        assert!(report.ok);
        assert_eq!(report.result, VerifyResult::Ok);
        // A step that is off on purpose is not counted, and is not "skipped".
        assert_eq!((report.steps_run, report.steps_total), (3, 3));
        assert!(report.skipped.is_empty());
        assert_eq!(report.steps[2].status, StepStatus::Off);
        let text = format_verify_human(&report);
        assert!(text.starts_with("verify: OK\n"), "{text}");
        assert!(text.contains("  typecheck  off      turned off"), "{text}");
        assert!(!text.contains("skipped"), "{text}");
    }

    /// The headline is a plain "OK" only when no step was skipped, whatever
    /// mix of steps the run has, at the root and in projects.
    #[test]
    fn no_headline_says_ok_when_any_step_was_skipped() {
        let fixture = Fixture::new(&[("svc/keep.txt", "")], Track::NoGit).unwrap();
        let kinds = [
            Planned::Command("echo fine".to_owned()),
            Planned::Skipped("no command found".to_owned()),
            Planned::NotNeeded("off".to_owned()),
        ];
        let mut cases = 0;
        for lint in &kinds {
            for typecheck in &kinds {
                for test in &kinds {
                    for project_test in &kinds {
                        for fail_on_skipped in [false, true] {
                            let plan = VerifyPlan {
                                runner: "scripts".to_owned(),
                                steps: vec![
                                    ("lint".to_owned(), lint.clone()),
                                    ("typecheck".to_owned(), typecheck.clone()),
                                    ("test".to_owned(), test.clone()),
                                ],
                                projects: vec![ProjectPlan {
                                    root: "svc".to_owned(),
                                    kind: Some("python".to_owned()),
                                    steps: vec![("test".to_owned(), project_test.clone())],
                                    install: None,
                                    python_from_path: false,
                                }],
                                fail_on_skipped,
                                install: None,
                                python_from_path: false,
                            };
                            let report =
                                run_verify(fixture.root(), "0.0.0", passing_check(), &plan)
                                    .unwrap();
                            let any_skipped = [lint, typecheck, test, project_test]
                                .iter()
                                .any(|planned| matches!(planned, Planned::Skipped(_)));
                            let text = format_verify_human(&report);
                            let first = text.lines().next().unwrap_or_default();
                            assert_eq!(first == "verify: OK", !any_skipped, "{text}");
                            assert_eq!(report.result == VerifyResult::Ok, !any_skipped, "{text}");
                            if any_skipped {
                                assert!(!first.contains("OK"), "{text}");
                                assert!(text.contains("\nwarning: "), "{text}");
                                assert_eq!(report.ok, !fail_on_skipped, "{text}");
                                assert!(!report.skipped.is_empty());
                                // Every skipped step is named in the warning.
                                for skip in &report.skipped {
                                    let name = match &skip.project {
                                        Some(project) => format!("{} in {project}", skip.step),
                                        None => skip.step.clone(),
                                    };
                                    assert!(report.warnings[0].contains(&name), "{text}");
                                }
                            } else {
                                assert!(report.ok && report.warnings.is_empty(), "{text}");
                            }
                            cases += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(cases, 162);
    }

    #[test]
    fn fail_on_skipped_turns_a_partial_run_into_a_failure() {
        let fixture = Fixture::new(&[], Track::NoGit).unwrap();
        let plan = VerifyPlan {
            runner: "scripts".to_owned(),
            steps: vec![
                ("lint".to_owned(), Planned::Command("echo fine".to_owned())),
                (
                    "typecheck".to_owned(),
                    Planned::Skipped("package.json has no typecheck script".to_owned()),
                ),
                (
                    "test".to_owned(),
                    Planned::Skipped("package.json has no test script".to_owned()),
                ),
            ],
            fail_on_skipped: true,
            ..VerifyPlan::default()
        };
        let report = run_verify(fixture.root(), "0.0.0", passing_check(), &plan).unwrap();
        assert!(!report.ok);
        assert_eq!(report.result, VerifyResult::Failed);
        // No step failed, so there is nothing to run again.
        assert_eq!(report.failed_step, None);
        assert_eq!(report.rerun, None);
        let text = format_verify_human(&report);
        assert!(
            text.starts_with(
                "verify: FAILED - 2 of 4 steps skipped, and verify.failOnSkipped is on\n"
            ),
            "{text}"
        );
        let json = format_verify_json(&report);
        assert!(json.contains("\"result\": \"failed\""), "{json}");
        assert!(json.contains("\"failOnSkipped\": true"), "{json}");
    }

    fn course_facts() -> (Fixture, RepoFacts) {
        let fixture = crate::testing::course_project().unwrap();
        let facts = crate::facts::detect_facts_with(fixture.root(), None).unwrap();
        (fixture, facts)
    }

    fn project_commands(plan: &VerifyPlan) -> Vec<String> {
        plan.projects
            .iter()
            .flat_map(|project| {
                project.steps.iter().map(|(name, planned)| {
                    let what = match planned {
                        Planned::Command(command) => command.clone(),
                        Planned::Skipped(reason) => format!("skipped ({reason})"),
                        Planned::NotNeeded(reason) => format!("off ({reason})"),
                    };
                    format!("{} {name}: {what}", project.root)
                })
            })
            .collect()
    }

    #[test]
    fn each_discovered_project_gets_its_own_steps_in_its_own_folder() {
        let (fixture, facts) = course_facts();
        let plan = plan_verify(
            fixture.root(),
            &facts,
            &VerifyConfig::default(),
            false,
            None,
        );
        // The root has no commands of its own, so it lists no steps.
        assert_eq!(plan.runner, "projects");
        assert!(plan.steps.is_empty());
        let roots: Vec<_> = plan.projects.iter().map(|p| p.root.as_str()).collect();
        assert_eq!(
            roots,
            [
                "backend/audit_service",
                "backend/auth_service",
                "backend/ledger_service",
                "backend/market_service",
                "backend/realtime_service",
                "frontend",
            ]
        );
        let commands = project_commands(&plan);
        assert_eq!(commands.len(), 18);
        assert_eq!(
            &commands[..6],
            [
                "backend/audit_service lint: skipped (no linter was found (looked for ruff))",
                "backend/audit_service typecheck: skipped (no type checker was found (looked for mypy))",
                "backend/audit_service test: python -m pytest",
                "backend/auth_service lint: python -m ruff check .",
                "backend/auth_service typecheck: skipped (no type checker was found (looked for mypy))",
                "backend/auth_service test: python -m pytest",
            ]
        );
        assert_eq!(
            &commands[15..],
            [
                "frontend lint: npx --no oxlint",
                "frontend typecheck: npx --no tsc --noEmit",
                "frontend test: npm run test:run",
            ]
        );
    }

    #[test]
    fn the_config_can_set_turn_off_and_add_project_steps() {
        let (fixture, facts) = course_facts();
        let steps = |pairs: &[(&str, StepOverride)]| {
            ProjectOverride::Steps(
                pairs
                    .iter()
                    .map(|(step, setting)| ((*step).to_owned(), setting.clone()))
                    .collect(),
            )
        };
        let config = VerifyConfig {
            // For the whole repo: one command at the root, none per project.
            steps: BTreeMap::from([("typecheck".to_owned(), StepOverride::Skip)]),
            projects: BTreeMap::from([
                ("backend/audit_service".to_owned(), ProjectOverride::Off),
                (
                    "frontend".to_owned(),
                    steps(&[
                        (
                            "test".to_owned().as_str(),
                            StepOverride::Command("npm run test:e2e".to_owned()),
                        ),
                        // A project's own setting wins over the repo-wide one.
                        ("typecheck", StepOverride::Command("npx tsc -b".to_owned())),
                    ]),
                ),
                // Not a discovered project: it runs exactly what it is given.
                (
                    "backend/shared".to_owned(),
                    steps(&[("lint", StepOverride::Command("ruff check .".to_owned()))]),
                ),
            ]),
            fail_on_skipped: true,
            ..VerifyConfig::default()
        };
        let plan = plan_verify(fixture.root(), &facts, &config, false, None);
        assert!(plan.fail_on_skipped);
        assert_eq!(
            plan.steps,
            [(
                "typecheck".to_owned(),
                Planned::NotNeeded("turned off in your firescale config".to_owned())
            )]
        );
        let commands = project_commands(&plan);
        assert_eq!(
            commands
                .iter()
                .filter(|line| line.starts_with("backend/audit_service")
                    || line.starts_with("backend/shared")
                    || line.starts_with("frontend")
                    || line.starts_with("backend/auth_service"))
                .collect::<Vec<_>>(),
            [
                "backend/audit_service lint: off (this project is turned off in your firescale config)",
                "backend/audit_service typecheck: off (this project is turned off in your firescale config)",
                "backend/audit_service test: off (this project is turned off in your firescale config)",
                "backend/auth_service lint: python -m ruff check .",
                "backend/auth_service test: python -m pytest",
                "backend/shared lint: ruff check .",
                "frontend lint: npx --no oxlint",
                "frontend typecheck: npx tsc -b",
                "frontend test: npm run test:e2e",
            ]
        );
        assert!(check_project_folders(fixture.root(), &config).is_ok());
        let missing = VerifyConfig {
            projects: BTreeMap::from([("backend/nope".to_owned(), ProjectOverride::Off)]),
            ..VerifyConfig::default()
        };
        let error = check_project_folders(fixture.root(), &missing).unwrap_err();
        assert!(error.message().contains("backend/nope"));
        assert!(error.message().contains("fscale facts"));
    }

    #[test]
    fn a_root_with_its_own_commands_keeps_them_and_adds_discovered_projects() {
        let fixture = Fixture::new(
            &[
                ("package.json", r#"{"scripts":{"lint":"x","test":"y"}}"#),
                ("pnpm-workspace.yaml", "packages:\n  - 'packages/*'\n"),
                (
                    "packages/a/package.json",
                    r#"{"name":"a","scripts":{"test":"z"}}"#,
                ),
                (
                    "services/api/pyproject.toml",
                    "[project]\nname = \"api\"\n[tool.pytest.ini_options]\n",
                ),
            ],
            Track::NoGit,
        )
        .unwrap();
        let facts = crate::facts::detect_facts_with(fixture.root(), None).unwrap();
        let plan = plan_verify(
            fixture.root(),
            &facts,
            &VerifyConfig::default(),
            false,
            None,
        );
        assert_eq!(plan.runner, "scripts");
        assert_eq!(
            commands_of(&plan),
            [
                "lint: npm run lint",
                "typecheck: skipped (package.json has no typecheck script)",
                "test: npm run test",
            ]
        );
        // The workspace member is covered by the root. The Python service is not.
        let roots: Vec<_> = plan.projects.iter().map(|p| p.root.as_str()).collect();
        assert_eq!(roots, ["services/api"]);
    }

    #[test]
    fn a_python_or_go_project_at_the_root_supplies_the_root_steps() {
        let fixture = Fixture::new(
            &[("requirements.txt", "pytest\nruff\n"), ("app.py", "")],
            Track::NoGit,
        )
        .unwrap();
        let facts = crate::facts::detect_facts_with(fixture.root(), None).unwrap();
        let plan = plan_verify(
            fixture.root(),
            &facts,
            &VerifyConfig::default(),
            false,
            None,
        );
        assert_eq!(plan.runner, "python");
        assert_eq!(
            commands_of(&plan),
            [
                "lint: python -m ruff check .",
                "typecheck: skipped (no type checker was found (looked for mypy))",
                "test: python -m pytest",
            ]
        );
        assert!(plan.projects.is_empty());
    }

    #[test]
    fn a_root_script_missing_falls_back_to_the_tools_config_file() {
        let fixture = Fixture::new(
            &[
                ("package.json", r#"{"scripts":{"test":"y"}}"#),
                ("pnpm-lock.yaml", ""),
                ("eslint.config.js", ""),
            ],
            Track::NoGit,
        )
        .unwrap();
        let facts = crate::facts::detect_facts_with(fixture.root(), None).unwrap();
        // `commands` still lists scripts only.
        assert_eq!(facts.commands.lint, None);
        let plan = plan_verify(
            fixture.root(),
            &facts,
            &VerifyConfig::default(),
            false,
            None,
        );
        assert_eq!(
            commands_of(&plan),
            [
                "lint: pnpm exec eslint .",
                "typecheck: skipped (package.json has no typecheck script)",
                "test: pnpm run test",
            ]
        );
    }

    #[test]
    fn project_steps_run_in_the_projects_folder_and_a_failure_names_it() {
        let fixture = Fixture::new(
            &[("one/here.txt", "one\n"), ("two/deep/here.txt", "two\n")],
            Track::NoGit,
        )
        .unwrap();
        // Passes only when run in a folder that holds here.txt.
        let show = if cfg!(windows) {
            "type here.txt"
        } else {
            "cat here.txt"
        };
        let fail = if cfg!(windows) {
            "type here.txt 1>&2 & exit /b 4"
        } else {
            "cat here.txt >&2; exit 4"
        };
        let plan = VerifyPlan {
            runner: "projects".to_owned(),
            projects: vec![
                ProjectPlan {
                    root: "one".to_owned(),
                    kind: None,
                    steps: vec![
                        ("lint".to_owned(), Planned::Command(show.to_owned())),
                        (
                            "typecheck".to_owned(),
                            Planned::Skipped("no type checker was found".to_owned()),
                        ),
                    ],
                    install: None,
                    python_from_path: false,
                },
                ProjectPlan {
                    root: "two/deep".to_owned(),
                    kind: None,
                    steps: vec![
                        ("lint".to_owned(), Planned::Command(fail.to_owned())),
                        ("test".to_owned(), Planned::Command(show.to_owned())),
                    ],
                    install: None,
                    python_from_path: false,
                },
            ],
            ..VerifyPlan::default()
        };
        let report = run_verify(fixture.root(), "0.0.0", passing_check(), &plan).unwrap();
        let got: Vec<_> = report
            .steps
            .iter()
            .map(|step| (step.project.as_deref(), step.name.as_str(), step.status))
            .collect();
        assert_eq!(
            got,
            [
                (None, "check", StepStatus::Passed),
                (Some("one"), "lint", StepStatus::Passed),
                (Some("one"), "typecheck", StepStatus::Skipped),
                (Some("two/deep"), "lint", StepStatus::Failed),
                (Some("two/deep"), "test", StepStatus::NotRun),
            ]
        );
        assert_eq!(report.failed_project.as_deref(), Some("two/deep"));
        assert_eq!(report.steps[3].output_tail, ["two"]);
        let text = format_verify_human(&report);
        assert!(
            text.starts_with("verify: FAILED at lint in two/deep\n"),
            "{text}"
        );
        assert!(
            text.contains("  in one:\n    lint       passed   "),
            "{text}"
        );
        assert!(
            text.contains("\nLast lines of lint in two/deep:\n  two\n"),
            "{text}"
        );
        assert!(
            text.contains("\nFix the failure, then run again in two/deep: "),
            "{text}"
        );
        // A failed run still names the step that was skipped.
        assert!(
            text.contains("warning: 1 step was skipped because no command was found: typecheck in one. Set verify.projects[\"one\"].typecheck in your firescale config"),
            "{text}"
        );
    }

    #[test]
    fn a_step_with_no_command_is_skipped_with_a_visible_warning() {
        let fixture = Fixture::new(&[], Track::NoGit).unwrap();
        let plan = VerifyPlan {
            runner: "scripts".to_owned(),
            steps: vec![
                (
                    "lint".to_owned(),
                    Planned::Skipped("package.json has no lint script".to_owned()),
                ),
                (
                    "typecheck".to_owned(),
                    Planned::NotNeeded("turned off in your firescale config".to_owned()),
                ),
                (
                    "test".to_owned(),
                    Planned::Skipped("package.json has no test script".to_owned()),
                ),
            ],
            ..VerifyPlan::default()
        };
        let report = run_verify(fixture.root(), "0.0.0", passing_check(), &plan).unwrap();
        // Still not a failure.
        assert!(report.ok);
        assert_eq!(
            report.warnings,
            ["steps were skipped because no command was found: lint, test. Set verify.lint in your firescale config to a command, or to false if this repo has none."]
        );
        let text = format_verify_human(&report);
        assert!(
            text.ends_with(&format!("\nwarning: {}\n", report.warnings[0])),
            "{text}"
        );
        assert!(format_verify_json(&report).contains("\"warnings\": ["));

        // A step that is off on purpose gives no warning.
        let quiet = VerifyPlan {
            runner: "config".to_owned(),
            steps: vec![(
                "test".to_owned(),
                Planned::NotNeeded("turned off in your firescale config".to_owned()),
            )],
            ..VerifyPlan::default()
        };
        let report = run_verify(fixture.root(), "0.0.0", passing_check(), &quiet).unwrap();
        assert!(report.warnings.is_empty());
        assert!(!format_verify_human(&report).contains("warning"));
        assert!(!format_verify_json(&report).contains("warnings"));
    }

    // ---------------------------------------------- found by the review

    fn plan_of(files: &[(&str, &str)], config: &VerifyConfig) -> (Fixture, VerifyPlan) {
        let fixture = Fixture::new(files, Track::NoGit).unwrap();
        let facts = crate::facts::detect_facts_with(fixture.root(), None).unwrap();
        let plan = plan_verify(fixture.root(), &facts, config, false, None);
        (fixture, plan)
    }

    const PY_PACKAGE: &str = "[project]\nname = \"x\"\n[tool.pytest.ini_options]\n";

    #[test]
    fn a_second_kind_of_project_at_the_root_is_not_dropped() {
        // Cargo supplies the root's steps. The Python project beside it used
        // to vanish from the plan, and the run said OK.
        let (_rust, plan) = plan_of(
            &[
                ("Cargo.toml", "[package]\nname = \"x\"\n"),
                ("pyproject.toml", PY_PACKAGE),
            ],
            &VerifyConfig::default(),
        );
        assert_eq!(plan.runner, "cargo");
        assert_eq!(
            project_commands(&plan),
            [
                ". lint: skipped (no linter was found (looked for ruff))",
                ". typecheck: skipped (no type checker was found (looked for mypy))",
                ". test: python -m pytest",
            ]
        );
        assert_eq!(plan.projects[0].kind.as_deref(), Some("python"));

        // The same beside package scripts, and Go beside Python.
        let (_node, plan) = plan_of(
            &[
                (
                    "package.json",
                    r#"{"scripts":{"lint":"a","typecheck":"b","test":"c"}}"#,
                ),
                ("pyproject.toml", PY_PACKAGE),
            ],
            &VerifyConfig::default(),
        );
        assert_eq!(plan.runner, "scripts");
        assert_eq!(project_commands(&plan).len(), 3);
        let (_go, plan) = plan_of(
            &[
                ("pyproject.toml", PY_PACKAGE),
                ("go.mod", "module example.com/x\n"),
            ],
            &VerifyConfig::default(),
        );
        assert_eq!(plan.runner, "python");
        assert_eq!(
            project_commands(&plan),
            [
                ". lint: go vet ./...",
                ". typecheck: off (go vet already type-checks)",
                ". test: go test ./...",
            ]
        );

        // "." under verify.projects is the handle for them.
        let off = VerifyConfig {
            projects: BTreeMap::from([(".".to_owned(), ProjectOverride::Off)]),
            ..VerifyConfig::default()
        };
        let (_off, plan) = plan_of(
            &[
                ("Cargo.toml", "[package]\nname = \"x\"\n"),
                ("pyproject.toml", PY_PACKAGE),
            ],
            &off,
        );
        assert!(project_commands(&plan)
            .iter()
            .all(|line| line.contains("off (this project is turned off")));
        // The root's own steps are untouched.
        assert_eq!(commands_of(&plan)[2], "test: cargo test --workspace");
    }

    #[test]
    fn turning_projects_off_does_not_hide_that_nothing_runs() {
        // One project, turned off: the root must still say that it has no
        // commands, or the run would read as OK with only `check`.
        let off = VerifyConfig {
            projects: BTreeMap::from([("svc".to_owned(), ProjectOverride::Off)]),
            ..VerifyConfig::default()
        };
        let (fixture, plan) = plan_of(&[("svc/go.mod", "module example.com/svc\n")], &off);
        assert_eq!(plan.runner, "none");
        assert_eq!(plan.steps.len(), 3);
        assert!(plan
            .steps
            .iter()
            .all(|(_, planned)| matches!(planned, Planned::Skipped(_))));
        let report = run_verify(fixture.root(), "0.0.0", passing_check(), &plan).unwrap();
        assert_eq!(report.result, VerifyResult::Partial);
        assert!(format_verify_human(&report)
            .starts_with("verify: PARTIAL - 1 of 4 steps ran, 3 skipped\n"));

        // The same for a folder that only the config names.
        let only_false = VerifyConfig {
            projects: BTreeMap::from([(
                "misc".to_owned(),
                ProjectOverride::Steps(BTreeMap::from([("lint".to_owned(), StepOverride::Skip)])),
            )]),
            ..VerifyConfig::default()
        };
        let (fixture, plan) = plan_of(&[("misc/keep.txt", "")], &only_false);
        let report = run_verify(fixture.root(), "0.0.0", passing_check(), &plan).unwrap();
        assert_eq!(report.result, VerifyResult::Partial);
    }

    #[test]
    fn a_project_with_an_unreadable_manifest_has_its_steps_skipped_not_dropped() {
        let (fixture, plan) = plan_of(
            &[
                (
                    "a/package.json",
                    r#"{"scripts":{"lint":"echo ok","typecheck":"echo ok","test":"echo ok"}}"#,
                ),
                ("b/package.json", r#"{"scripts":{"test":"exit 1"},}"#),
            ],
            &VerifyConfig::default(),
        );
        let roots: Vec<_> = plan.projects.iter().map(|p| p.root.as_str()).collect();
        assert_eq!(roots, ["a", "b"]);
        assert!(plan.projects[1]
            .steps
            .iter()
            .all(|(_, planned)| matches!(planned, Planned::Skipped(_))));
        drop(fixture);
    }

    #[test]
    fn two_kinds_in_one_folder_are_told_apart() {
        let (fixture, plan) = plan_of(
            &[
                ("app/go.mod", "module example.com/app\n"),
                ("app/package.json", r#"{"scripts":{"test":"echo node"}}"#),
            ],
            &VerifyConfig {
                steps: BTreeMap::from([
                    ("lint".to_owned(), StepOverride::Skip),
                    ("typecheck".to_owned(), StepOverride::Skip),
                ]),
                projects: BTreeMap::from([(
                    "app".to_owned(),
                    ProjectOverride::Steps(BTreeMap::from([(
                        "test".to_owned(),
                        StepOverride::Command("echo custom".to_owned()),
                    )])),
                )]),
                ..VerifyConfig::default()
            },
        );
        // A command set for the folder runs once, in place of both kinds' own.
        assert_eq!(project_commands(&plan), ["app test: echo custom"]);

        let (fixture2, plan) = plan_of(
            &[
                ("app/go.mod", "module example.com/app\n"),
                ("app/package.json", r#"{"scripts":{"test":"echo node"}}"#),
            ],
            &VerifyConfig {
                steps: BTreeMap::from([
                    ("lint".to_owned(), StepOverride::Skip),
                    ("typecheck".to_owned(), StepOverride::Skip),
                    ("test".to_owned(), StepOverride::Skip),
                ]),
                ..VerifyConfig::default()
            },
        );
        assert!(plan.projects.is_empty());
        let plan = VerifyPlan {
            runner: "projects".to_owned(),
            projects: ["go", "node"]
                .iter()
                .map(|kind| ProjectPlan {
                    root: "app".to_owned(),
                    kind: Some((*kind).to_owned()),
                    steps: vec![("test".to_owned(), Planned::Command("echo fine".to_owned()))],
                    install: None,
                    python_from_path: false,
                })
                .collect(),
            ..VerifyPlan::default()
        };
        let report = run_verify(fixture2.root(), "0.0.0", passing_check(), &plan).unwrap();
        let text = format_verify_human(&report);
        assert!(text.contains("  in app (go):\n"), "{text}");
        assert!(text.contains("  in app (node):\n"), "{text}");
        assert!(format_verify_json(&report).contains("\"kind\": \"go\""));
        drop(fixture);
    }

    // ------------------------------- independent projects and setup (ADR 0022)

    /// A command that prints `text` on stderr and exits with `code`.
    fn say_and_exit(text: &str, code: i32) -> String {
        if cfg!(windows) {
            format!("echo {text} 1>&2 & exit /b {code}")
        } else {
            format!("echo \"{text}\" >&2; exit {code}")
        }
    }

    fn project(root: &str, steps: &[(&str, &str)], install: Option<&str>) -> ProjectPlan {
        ProjectPlan {
            root: root.to_owned(),
            kind: Some("python".to_owned()),
            steps: steps
                .iter()
                .map(|(name, command)| {
                    ((*name).to_owned(), Planned::Command((*command).to_owned()))
                })
                .collect(),
            install: install.map(str::to_owned),
            python_from_path: false,
        }
    }

    fn statuses(report: &VerifyReport) -> Vec<(String, StepStatus)> {
        report
            .steps
            .iter()
            .map(|step| (step_place(step), step.status))
            .collect()
    }

    #[test]
    fn one_projects_failure_does_not_stop_the_others() {
        let fixture = Fixture::new(
            &[("a/x.txt", ""), ("b/x.txt", ""), ("c/x.txt", "")],
            Track::NoGit,
        )
        .unwrap();
        let fail = say_and_exit("boom", 1);
        let plan = VerifyPlan {
            runner: "scripts".to_owned(),
            steps: vec![("lint".to_owned(), Planned::Command(fail.clone()))],
            projects: vec![
                project("a", &[("lint", &fail), ("test", "echo fine")], None),
                project("b", &[("lint", "echo fine"), ("test", "echo fine")], None),
                project("c", &[("lint", "echo fine"), ("test", &fail)], None),
            ],
            ..VerifyPlan::default()
        };
        let report = run_verify(fixture.root(), "0.0.0", passing_check(), &plan).unwrap();
        assert_eq!(
            statuses(&report),
            [
                ("check".to_owned(), StepStatus::Passed),
                ("lint".to_owned(), StepStatus::Failed),
                ("lint in a".to_owned(), StepStatus::Failed),
                // Within a project, the first failure still stops the rest.
                ("test in a".to_owned(), StepStatus::NotRun),
                ("lint in b".to_owned(), StepStatus::Passed),
                ("test in b".to_owned(), StepStatus::Passed),
                ("lint in c".to_owned(), StepStatus::Passed),
                ("test in c".to_owned(), StepStatus::Failed),
            ]
        );
        assert_eq!(report.result, VerifyResult::Failed);
        assert_eq!(report.failed_step.as_deref(), Some("lint"));
        assert_eq!(report.failed_project, None);
        let text = format_verify_human(&report);
        assert!(
            text.starts_with("verify: FAILED at lint, lint in a, test in c\n"),
            "{text}"
        );
        // Each failure gets its own rerun line.
        for line in [
            "\nFix the failure, then run again: ",
            "\nFix the failure, then run again in a: ",
            "\nFix the failure, then run again in c: ",
        ] {
            assert!(text.contains(line), "{line}\n{text}");
        }
        assert!(!text.contains("again in b"), "{text}");
    }

    #[test]
    fn a_failing_check_still_stops_every_project() {
        let fixture = Fixture::new(&[("a/x.txt", "")], Track::NoGit).unwrap();
        let plan = VerifyPlan {
            runner: "projects".to_owned(),
            projects: vec![project("a", &[("test", "echo fine")], None)],
            ..VerifyPlan::default()
        };
        let check = CheckStep {
            ok: false,
            ..passing_check()
        };
        let report = run_verify(fixture.root(), "0.0.0", check, &plan).unwrap();
        assert_eq!(report.steps[1].status, StepStatus::NotRun);
    }

    #[test]
    fn a_missing_module_is_setup_needed_with_the_install_command() {
        let fixture =
            Fixture::new(&[("svc/main.py", ""), ("web/x.txt", "")], Track::NoGit).unwrap();
        let missing = say_and_exit("E   ModuleNotFoundError: No module named 'jwt'", 2);
        let plan = VerifyPlan {
            runner: "projects".to_owned(),
            projects: vec![
                project(
                    "svc",
                    &[("lint", "echo fine"), ("test", &missing)],
                    Some(".venv/bin/python -m pip install -r requirements-dev.txt"),
                ),
                project("web", &[("test", "echo fine")], None),
            ],
            ..VerifyPlan::default()
        };
        let report = run_verify(fixture.root(), "0.0.0", passing_check(), &plan).unwrap();
        let test = &report.steps[2];
        assert_eq!(test.status, StepStatus::SetupNeeded);
        assert_eq!(
            test.fix.as_deref(),
            Some(".venv/bin/python -m pip install -r requirements-dev.txt")
        );
        assert!(
            test.detail
                .ends_with(": the Python module 'jwt' is not installed"),
            "{}",
            test.detail
        );
        assert_eq!(report.steps[3].status, StepStatus::Passed);
        assert_eq!(report.result, VerifyResult::SetupNeeded);
        assert!(!report.ok);
        assert_eq!(report.failed_project.as_deref(), Some("svc"));
        let text = format_verify_human(&report);
        assert!(
            text.starts_with("verify: SETUP NEEDED for test in svc\n"),
            "{text}"
        );
        assert!(text.contains("    test       SETUP    "), "{text}");
        assert!(text.contains("Do not change code for it."), "{text}");
        assert!(
            text.contains("Set it up in svc: .venv/bin/python -m pip install -r requirements-dev.txt\nThen run again in svc: "),
            "{text}"
        );
        assert!(!text.contains("Fix the failure"), "{text}");
        let json = format_verify_json(&report);
        assert!(json.contains("\"result\": \"setup-needed\""), "{json}");
        assert!(json.contains("\"status\": \"setup-needed\""), "{json}");
        assert!(json.contains("\"schemaVersion\": 4"), "{json}");
    }

    #[test]
    fn a_missing_module_of_the_project_itself_is_a_normal_failure() {
        let fixture = Fixture::new(&[("svc/app/__init__.py", "")], Track::NoGit).unwrap();
        let missing = say_and_exit("E   ModuleNotFoundError: No module named 'app.services'", 2);
        let plan = VerifyPlan {
            runner: "projects".to_owned(),
            projects: vec![project("svc", &[("test", &missing)], Some("pip install"))],
            ..VerifyPlan::default()
        };
        let report = run_verify(fixture.root(), "0.0.0", passing_check(), &plan).unwrap();
        assert_eq!(report.steps[1].status, StepStatus::Failed);
        assert_eq!(report.steps[1].fix, None);
        assert_eq!(report.result, VerifyResult::Failed);
    }

    #[test]
    fn setup_problems_and_failures_are_named_apart() {
        let fixture = Fixture::new(&[("a/x.txt", ""), ("b/x.txt", "")], Track::NoGit).unwrap();
        let missing = say_and_exit("/usr/bin/python: No module named pytest", 1);
        let plan = VerifyPlan {
            runner: "projects".to_owned(),
            projects: vec![
                project("a", &[("test", &missing)], None),
                project(
                    "b",
                    &[("lint", &say_and_exit("E501 line too long", 1))],
                    None,
                ),
            ],
            ..VerifyPlan::default()
        };
        let report = run_verify(fixture.root(), "0.0.0", passing_check(), &plan).unwrap();
        assert_eq!(report.result, VerifyResult::Failed);
        let text = format_verify_human(&report);
        assert!(
            text.starts_with("verify: FAILED at lint in b; SETUP NEEDED for test in a\n"),
            "{text}"
        );
        // With no install command known, the summary says what to set up.
        assert!(
            text.contains("Set up what the line above names, or ask a person to."),
            "{text}"
        );
        assert!(
            text.contains("\nFix the failure, then run again in b: "),
            "{text}"
        );
    }

    #[test]
    fn plans_carry_each_projects_install_command() {
        let (fixture, facts) = course_facts();
        let plan = plan_verify(
            fixture.root(),
            &facts,
            &VerifyConfig::default(),
            false,
            None,
        );
        let audit = &plan.projects[0];
        assert_eq!(audit.root, "backend/audit_service");
        assert_eq!(
            audit.install.as_deref(),
            Some("python -m pip install -r requirements-dev.txt")
        );
        let frontend = plan.projects.last().unwrap();
        assert_eq!(frontend.install.as_deref(), Some("npm install"));
    }

    #[test]
    fn a_python_project_without_a_venv_is_named_in_a_note() {
        let fixture = Fixture::new(
            &[
                ("a/requirements.txt", "pytest\n"),
                ("a/main.py", ""),
                ("b/requirements.txt", "pytest\n"),
                ("b/main.py", ""),
                (
                    &format!(
                        "b/{}",
                        crate::facts::venv_python(".venv").replace('\\', "/")
                    ),
                    "",
                ),
            ],
            Track::NoGit,
        )
        .unwrap();
        let facts = crate::facts::detect_facts_with(fixture.root(), None).unwrap();
        let mut plan = plan_verify(
            fixture.root(),
            &facts,
            &VerifyConfig::default(),
            false,
            None,
        );
        assert!(plan.projects[0].python_from_path);
        assert!(!plan.projects[1].python_from_path);
        // Run harmless commands in place of pytest.
        for project in &mut plan.projects {
            for (_, planned) in &mut project.steps {
                if let Planned::Command(command) = planned {
                    *command = "echo fine".to_owned();
                }
            }
        }
        let report = run_verify(fixture.root(), "0.0.0", passing_check(), &plan).unwrap();
        assert_eq!(
            report.notes,
            ["no virtual environment was found for a, so its Python commands use python from the PATH. To use one, create it in the project's folder (python -m venv .venv) or activate it."]
        );
        assert!(format_verify_human(&report)
            .contains("\nnote: no virtual environment was found for a,"));
        // Every project has its venv: no note.
        plan.projects[0].python_from_path = false;
        let report = run_verify(fixture.root(), "0.0.0", passing_check(), &plan).unwrap();
        assert!(report.notes.is_empty());
        assert!(!format_verify_json(&report).contains("\"notes\""));
    }

    /// A command that runs for about `seconds` and prints a line first.
    fn slow(seconds: u32) -> String {
        if cfg!(windows) {
            format!("echo started & ping -n {} 127.0.0.1 > NUL", seconds + 1)
        } else {
            format!("echo started; sleep {seconds}")
        }
    }

    fn one_step(root: &str, command: &str) -> ProjectPlan {
        ProjectPlan {
            root: root.to_owned(),
            kind: None,
            steps: vec![("test".to_owned(), Planned::Command(command.to_owned()))],
            install: None,
            python_from_path: false,
        }
    }

    #[test]
    fn a_step_past_its_limit_is_timed_out_and_the_other_projects_still_run() {
        let fixture = Fixture::new(&[("slow/a", ""), ("fast/a", "")], Track::NoGit).unwrap();
        let plan = VerifyPlan {
            runner: "projects".to_owned(),
            projects: vec![one_step("fast", "echo quick"), one_step("slow", &slow(60))],
            ..VerifyPlan::default()
        };
        let config = VerifyConfig {
            project_timeouts: BTreeMap::from([(
                "slow".to_owned(),
                ProjectTimeouts {
                    all: Some(Limit::After(Duration::from_secs(1))),
                    steps: BTreeMap::new(),
                },
            )]),
            ..VerifyConfig::default()
        };
        let started = std::time::Instant::now();
        let report = run_verify_with(
            fixture.root(),
            "0.0.0",
            passing_check(),
            &plan,
            &RunOptions::quiet(&config),
        )
        .unwrap();
        assert!(started.elapsed() < Duration::from_secs(30));
        assert_eq!(report.result, VerifyResult::Failed);
        assert!(!report.ok);
        let slow = report
            .steps
            .iter()
            .find(|s| s.project.as_deref() == Some("slow"))
            .unwrap();
        assert_eq!(slow.status, StepStatus::TimedOut);
        assert_eq!(slow.timeout_ms, Some(1000));
        assert_eq!(slow.output_tail, ["started"]);
        let fast = report
            .steps
            .iter()
            .find(|s| s.project.as_deref() == Some("fast"))
            .unwrap();
        assert_eq!(fast.status, StepStatus::Passed);
        let text = format_verify_human(&report);
        assert!(
            text.starts_with("verify: TIMED OUT at test in slow\n"),
            "{text}"
        );
        assert!(text.contains("(no result within 1s)"), "{text}");
        assert!(
            text.contains("If it is only slow, raise verify.projects[\"slow\"].test.timeout in your firescale config.\nThen run again in slow: "),
            "{text}"
        );
        assert_eq!(report.failed_project.as_deref(), Some("slow"));
    }

    #[test]
    fn a_timed_out_step_whose_output_shows_a_service_down_names_the_fix() {
        let fixture =
            Fixture::new(&[("svc/compose.yaml", "services: {}\n")], Track::NoGit).unwrap();
        let hang = if cfg!(windows) {
            "echo connect ECONNREFUSED 127.0.0.1:5432 & ping -n 30 127.0.0.1 > NUL"
        } else {
            "echo 'connect ECONNREFUSED 127.0.0.1:5432'; sleep 30"
        };
        let plan = VerifyPlan {
            runner: "projects".to_owned(),
            projects: vec![one_step("svc", hang)],
            ..VerifyPlan::default()
        };
        // With the default, the step would be stopped early (ADR 0025); this
        // is the time limit's own path.
        let config = VerifyConfig {
            timeout: Some(Limit::After(Duration::from_secs(1))),
            keep_running_when_service_down: true,
            ..VerifyConfig::default()
        };
        let report = run_verify_with(
            fixture.root(),
            "0.0.0",
            passing_check(),
            &plan,
            &RunOptions::quiet(&config),
        )
        .unwrap();
        let step = &report.steps[1];
        assert_eq!(step.status, StepStatus::TimedOut);
        assert_eq!(step.fix.as_deref(), Some("docker compose up -d"));
        assert!(
            step.detail.contains("PostgreSQL on localhost:5432"),
            "{}",
            step.detail
        );
        let text = format_verify_human(&report);
        assert!(
            text.contains("Its output shows the environment is not ready. Set it up in svc: docker compose up -d\n"),
            "{text}"
        );
    }

    /// A port with nothing listening on it.
    fn closed_port() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    }

    fn early_in(fixture: &Fixture, activity: &Activity) -> Option<(String, String)> {
        let mut early = Early::default();
        service_down_while_running(activity, &mut early, fixture.root(), fixture.root())
    }

    fn fed(command: &str, output: &str) -> Activity {
        let mut activity = Activity::new(command);
        activity.feed(0, output.as_bytes());
        activity
    }

    #[test]
    fn a_refused_connection_to_a_declared_service_that_is_down_is_seen_while_running() {
        let port = closed_port();
        let fixture = Fixture::new(
            &[(
                ".env.example",
                &format!("DATABASE_URL=postgresql://app:pw@localhost:{port}/app\n"),
            )],
            Track::NoGit,
        )
        .unwrap();
        let activity = fed(
            "node test.js",
            &format!("Error: connect ECONNREFUSED 127.0.0.1:{port}\n"),
        );
        let (service, evidence) = early_in(&fixture, &activity).expect("seen");
        assert_eq!(service, format!("PostgreSQL on localhost:{port}"));
        assert!(evidence.contains("refused connections"), "{evidence}");
    }

    #[test]
    fn a_refused_connection_to_an_unknown_port_is_not_taken_for_a_service() {
        // A test may connect to a closed port on purpose.
        let port = closed_port();
        let fixture = Fixture::new(&[], Track::NoGit).unwrap();
        let activity = fed(
            "node test.js",
            &format!("Error: connect ECONNREFUSED 127.0.0.1:{port}\n"),
        );
        assert_eq!(early_in(&fixture, &activity), None);
    }

    #[test]
    fn a_service_that_answers_now_is_not_down() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let fixture = Fixture::new(
            &[(
                ".env.example",
                &format!("REDIS_URL=redis://127.0.0.1:{port}\n"),
            )],
            Track::NoGit,
        )
        .unwrap();
        let activity = fed(
            "node test.js",
            &format!("Error: connect ECONNREFUSED 127.0.0.1:{port}\n"),
        );
        assert_eq!(early_in(&fixture, &activity), None);
        let errors = fed("python -m pytest", "tests/test_a.py EEEE");
        assert_eq!(early_in(&fixture, &errors), None);
    }

    #[test]
    fn tests_that_all_error_with_a_declared_service_down_are_seen_while_running() {
        let port = closed_port();
        let fixture = Fixture::new(
            &[(
                ".env.example",
                &format!(
                    "POSTGRES_SERVER=localhost\nPOSTGRES_PORT={port}\nPOSTGRES_PASSWORD=hunter2\n"
                ),
            )],
            Track::NoGit,
        )
        .unwrap();
        let (service, evidence) =
            early_in(&fixture, &fed("python -m pytest", "tests/test_a.py EEEE")).expect("seen");
        assert_eq!(service, format!("PostgreSQL on localhost:{port}"));
        assert_eq!(
            evidence,
            "every test so far has errored (0 passed, 0 failed, 4 errors), and .env.example (POSTGRES_SERVER) names it"
        );
        assert!(!evidence.contains("hunter2"));
        // Too few errors, a passed test, or an unknown runner: not clear yet.
        assert_eq!(
            early_in(&fixture, &fed("python -m pytest", "tests/test_a.py EE")),
            None
        );
        assert_eq!(
            early_in(&fixture, &fed("python -m pytest", "tests/test_a.py .EEEE")),
            None
        );
        assert_eq!(early_in(&fixture, &fed("node test.js", "EEEE")), None);
    }

    #[test]
    fn a_step_that_cannot_reach_its_service_is_stopped_early_or_runs_on_when_told() {
        let port = closed_port();
        let fixture = Fixture::new(
            &[
                (
                    "svc/.env.example",
                    &format!("DATABASE_URL=postgresql://u:p@localhost:{port}/db\n"),
                ),
                ("svc/compose.yaml", "services: {}\n"),
            ],
            Track::NoGit,
        )
        .unwrap();
        let hang = if cfg!(windows) {
            format!(
                "echo connect ECONNREFUSED 127.0.0.1:{port} & ping -n 4 127.0.0.1 > NUL & exit 1"
            )
        } else {
            format!("echo 'connect ECONNREFUSED 127.0.0.1:{port}'; sleep 3; exit 1")
        };
        let plan = VerifyPlan {
            runner: "projects".to_owned(),
            projects: vec![one_step("svc", &hang)],
            ..VerifyPlan::default()
        };
        let shared = crate::progress::Shared::default();
        let progress = Progress::new(Box::new(shared.clone()));
        let config = VerifyConfig::default();
        let options = RunOptions {
            progress: Some(&progress),
            ..RunOptions::quiet(&config)
        };
        let started = Instant::now();
        let report =
            run_verify_with(fixture.root(), "0.0.0", passing_check(), &plan, &options).unwrap();
        let step = &report.steps[1];
        assert_eq!(step.status, StepStatus::SetupNeeded);
        assert!(
            started.elapsed() < Duration::from_millis(2900),
            "{:?}",
            started.elapsed()
        );
        assert!(step.detail.contains("stopped early"), "{}", step.detail);
        assert_eq!(step.fix.as_deref(), Some("docker compose up -d"));
        let lines = shared.text();
        assert!(
            lines.contains(&format!("[verify] test in svc: SETUP NEEDED, seen while running: PostgreSQL on localhost:{port} is not running (the output shows refused connections and nothing listens on that port); start it with: docker compose up -d; stopping the step (set verify.stopWhenServiceDown to false to let it run on)\n")),
            "{lines}"
        );

        // With the option off, the step runs to its end and says why.
        let shared = crate::progress::Shared::default();
        let progress = Progress::new(Box::new(shared.clone()));
        let config = VerifyConfig {
            keep_running_when_service_down: true,
            ..VerifyConfig::default()
        };
        let options = RunOptions {
            progress: Some(&progress),
            ..RunOptions::quiet(&config)
        };
        let report =
            run_verify_with(fixture.root(), "0.0.0", passing_check(), &plan, &options).unwrap();
        let step = &report.steps[1];
        assert_eq!(step.status, StepStatus::SetupNeeded);
        assert_eq!(step.exit_code, Some(1));
        assert!(!step.detail.contains("stopped early"), "{}", step.detail);
        assert!(
            shared
                .text()
                .contains("the step runs on because verify.stopWhenServiceDown is false"),
            "{}",
            shared.text()
        );
    }

    #[test]
    fn a_service_that_is_down_is_setup_with_the_compose_command() {
        let fixture = Fixture::new(
            &[
                ("docker-compose.yml", "services: {}\n"),
                ("backend/api/a", ""),
            ],
            Track::NoGit,
        )
        .unwrap();
        let fail = if cfg!(windows) {
            "echo redis.exceptions.ConnectionError: Error 10061 connecting to localhost:6379. No connection could be made because the target machine actively refused it. & exit /b 1"
        } else {
            "echo 'redis.exceptions.ConnectionError: Error 111 connecting to localhost:6379. Connection refused.'; exit 1"
        };
        let plan = VerifyPlan {
            runner: "projects".to_owned(),
            projects: vec![ProjectPlan {
                install: Some("pip install -r requirements.txt".to_owned()),
                ..one_step("backend/api", fail)
            }],
            ..VerifyPlan::default()
        };
        let report = run_verify(fixture.root(), "0.0.0", passing_check(), &plan).unwrap();
        assert_eq!(report.result, VerifyResult::SetupNeeded);
        let step = &report.steps[1];
        assert_eq!(step.status, StepStatus::SetupNeeded);
        // Starting the services, not installing dependencies.
        assert_eq!(
            step.fix.as_deref(),
            Some("docker compose -f ../../docker-compose.yml up -d")
        );
    }

    #[test]
    fn a_stop_request_stops_the_running_step_and_starts_nothing_new() {
        let fixture = Fixture::new(&[("a/x", ""), ("b/x", "")], Track::NoGit).unwrap();
        let plan = VerifyPlan {
            runner: "projects".to_owned(),
            projects: vec![
                ProjectPlan {
                    steps: vec![
                        ("lint".to_owned(), Planned::Command(slow(60))),
                        ("test".to_owned(), Planned::Command("echo never".to_owned())),
                    ],
                    ..one_step("a", "")
                },
                one_step("b", "echo never"),
            ],
            ..VerifyPlan::default()
        };
        let config = VerifyConfig::default();
        let stop = AtomicBool::new(false);
        let options = RunOptions {
            stop: &stop,
            ..RunOptions::quiet(&config)
        };
        let started = std::time::Instant::now();
        let report = std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(700));
                stop.store(true, Ordering::SeqCst);
            });
            run_verify_with(fixture.root(), "0.0.0", passing_check(), &plan, &options).unwrap()
        });
        assert!(started.elapsed() < Duration::from_secs(20));
        assert_eq!(report.result, VerifyResult::Stopped);
        let statuses: Vec<(Option<&str>, &str, StepStatus)> = report
            .steps
            .iter()
            .map(|s| (s.project.as_deref(), s.name.as_str(), s.status))
            .collect();
        assert_eq!(
            statuses,
            [
                (None, "check", StepStatus::Passed),
                (Some("a"), "lint", StepStatus::Stopped),
                (Some("a"), "test", StepStatus::NotRun),
                (Some("b"), "test", StepStatus::NotRun),
            ]
        );
        assert_eq!(report.steps[3].detail, "verify was stopped");
        let text = format_verify_human(&report);
        assert!(
            text.starts_with("verify: STOPPED - interrupted after 1 of 4 steps ran\n"),
            "{text}"
        );
        assert!(text.contains("\nStopped: verify was interrupted"), "{text}");
        assert!(!text.contains("Fix the failure"), "{text}");
    }

    #[test]
    fn parallel_projects_are_reported_in_plan_order_with_progress_lines_whole() {
        let fixture = Fixture::new(
            &[("a/x", ""), ("b/x", ""), ("c/x", ""), ("d/x", "")],
            Track::NoGit,
        )
        .unwrap();
        let fail = if cfg!(windows) { "exit /b 1" } else { "exit 1" };
        let plan = VerifyPlan {
            runner: "projects".to_owned(),
            projects: vec![
                one_step("a", &slow(1)),
                one_step("b", fail),
                one_step("c", "echo c"),
                one_step("d", &slow(1)),
            ],
            ..VerifyPlan::default()
        };
        let config = VerifyConfig::default();
        let shared = crate::progress::Shared::default();
        let progress = Progress::new(Box::new(shared.clone()));
        let options = RunOptions {
            jobs: 4,
            progress: Some(&progress),
            ..RunOptions::quiet(&config)
        };
        let parallel =
            run_verify_with(fixture.root(), "0.0.0", passing_check(), &plan, &options).unwrap();
        let serial = run_verify(fixture.root(), "0.0.0", passing_check(), &plan).unwrap();
        let places = |report: &VerifyReport| -> Vec<Option<String>> {
            report.steps.iter().map(|s| s.project.clone()).collect()
        };
        assert_eq!(places(&parallel), places(&serial));
        assert_eq!(format_verify_human(&parallel), format_verify_human(&serial));
        assert_eq!(parallel.result, VerifyResult::Failed);
        let lines = shared.text();
        for folder in ["a", "b", "c", "d"] {
            assert!(
                lines.contains(&format!("[verify] test in {folder}: started: ")),
                "{lines}"
            );
        }
        assert!(
            lines.contains("[verify] test in b: FAILED after "),
            "{lines}"
        );
        assert!(
            lines
                .lines()
                .all(|line| line.starts_with("[verify] test in ")),
            "{lines}"
        );
    }
}
