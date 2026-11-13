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

use crate::activity::{Activity, Runner, Tally};
use crate::services::{listening, services_for_folder, Service, PROBE_TIMEOUT};

use crate::error::{Error, Result};
use crate::facts::{source, Project, RepoFacts};
use crate::graph::{project_id, Affected, Graph};
use crate::process::{run_tree, Ending, Watch};
use crate::progress::{format_duration, format_limit, Progress, DEFAULT_HEARTBEAT};
use crate::runs::{RunRecord, RunningStep};
use crate::setup::{refused_service, service_down, setup_problem, SetupProblem};
use crate::stepcache::{format_iso, format_time, now_secs, Entry, StepCache};

/// Version of the `fscale verify --json` format.
pub const VERIFY_SCHEMA_VERSION: u32 = 6;

/// Errors with no test passed and none failed before a test step is stopped
/// as likely setup, by default (ADR 0035).
pub const DEFAULT_ERRORS_BEFORE_STOP: u64 = 10;

/// The time limit of the extra pytest run that finds the first error after
/// a step was stopped early or timed out (ADR 0035).
pub const FIRST_ERROR_LIMIT: Duration = Duration::from_secs(60);

/// The line the summary ends with when a step needs setup (ADR 0026).
pub const DOCTOR_HINT: &str =
    "Run `fscale doctor` to check this machine's tools, env variables, dependencies and services in one go.";

/// How long a step may run when the config sets no limit (ADR 0024).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// How long the base run of blame-aware verify may take in all, when the
/// config sets no budget (ADR 0037).
pub const DEFAULT_BASE_BUDGET: Duration = Duration::from_secs(5 * 60);

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
    /// `verify.stopAfterErrors`: how many errors a test step may report,
    /// with no test passed and none failed, before it is stopped as likely
    /// setup (ADR 0035). `None`: the default; `Some(0)`: `false`, never.
    pub stop_after_errors: Option<u64>,
    /// `verify.baseBudget`: how long the base run of blame-aware verify may
    /// take in all (ADR 0037). [`DEFAULT_BASE_BUDGET`] when absent.
    pub base_budget: Option<Limit>,
    /// `verify.baseInstall`: a command that installs dependencies in the
    /// base commit's worktree, run at its root before the base steps.
    pub base_install: Option<String>,
}

impl VerifyConfig {
    /// The error count at which a test step with no pass and no failure is
    /// stopped, or `None` when that is turned off.
    #[must_use]
    pub fn errors_before_stop(&self) -> Option<u64> {
        match self.stop_after_errors {
            None => Some(DEFAULT_ERRORS_BEFORE_STOP),
            Some(0) => None,
            Some(count) => Some(count),
        }
    }

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
    /// The kind of project the root's own commands run (`node` for Nx,
    /// Turborepo and package scripts, `rust` for Cargo).
    pub covered: String,
    /// True when the root's commands are not tied to its projects: Nx and
    /// Turborepo pick their own projects, and a command from the config can
    /// run anything.
    pub root_whole_repo: bool,
    /// Set by [`limit_to_affected`] for `--affected`: which units run.
    pub affected: Option<AffectedPlan>,
}

/// Which units of a plan an affected run runs, and why (ADR 0032).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AffectedPlan {
    /// The `--base` ref, if one was given.
    pub base: Option<String>,
    /// Set when every project is affected, with the reason.
    pub everything: Option<String>,
    /// How many files changed.
    pub changed_files: usize,
    /// One entry per unit with steps: the repo root first, then each project.
    pub units: Vec<AffectedUnit>,
    /// Said once, below the summary: why everything ran, for example.
    pub notes: Vec<String>,
    /// What the graph cannot see, such as dynamic imports.
    pub warnings: Vec<String>,
}

/// One unit of an affected run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AffectedUnit {
    /// The project's folder. Absent for the repo root's own steps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub affected: bool,
    /// Why it runs, or why it does not.
    pub reason: String,
}

impl AffectedPlan {
    fn unit(&self, project: Option<&ProjectPlan>) -> Option<&AffectedUnit> {
        self.units.iter().find(|unit| match project {
            None => unit.project.is_none(),
            Some(plan) => {
                unit.project.as_deref() == Some(plan.root.as_str()) && unit.kind == plan.kind
            }
        })
    }

    /// True when the unit runs. A unit not listed runs.
    #[must_use]
    pub fn runs(&self, project: Option<&ProjectPlan>) -> bool {
        self.unit(project).is_none_or(|unit| unit.affected)
    }
}

/// Marks the units of `plan` that a change does not affect, so verify
/// reports them as `not affected` instead of running them.
///
/// - The root's own steps run when every project is affected, when one of
///   the projects they cover is (the root's project and the workspace
///   members of the kind they run), or, when anything at all is affected,
///   if they are not tied to projects (Nx, Turborepo, config commands) or
///   cover no project.
/// - A project runs when its node in the graph is affected.
/// - A folder named only in `verify.projects` runs whenever anything is
///   affected: `--affected` cannot tell what it uses.
pub fn limit_to_affected(
    plan: &mut VerifyPlan,
    facts: &RepoFacts,
    graph: &Graph,
    affected: &Affected,
    base: Option<&str>,
) {
    let anything = !affected.is_empty();
    let mut units = Vec::new();
    if plan
        .steps
        .iter()
        .any(|(_, planned)| !matches!(planned, Planned::NotNeeded(_)))
    {
        let covered: Vec<&Project> = facts
            .projects
            .iter()
            // Workspace members of every kind: a Cargo workspace beside a
            // package.json root has no plan of its own, so the root's
            // commands are what run it.
            .filter(|project| {
                project.source == source::WORKSPACE
                    || (project.root == "." && project.kind == plan.covered)
            })
            .collect();
        let hit = covered.iter().find_map(|project| {
            let id = project_id(&project.kind, &project.root);
            affected.reason(&id).map(|reason| {
                if project.root == "." {
                    reason
                } else {
                    format!("{}: {reason}", graph.display(&id))
                }
            })
        });
        let (runs, reason) = if let Some(reason) = &affected.everything {
            (true, reason.clone())
        } else if let Some(reason) = hit {
            (true, reason)
        } else if anything && plan.root_whole_repo {
            (
                true,
                "the repo root's commands are not tied to one project, so they run whenever anything changed".to_owned(),
            )
        } else if anything && covered.is_empty() {
            (
                true,
                "the repo root's commands cover no project, so they run whenever anything changed"
                    .to_owned(),
            )
        } else {
            (
                false,
                "no change reaches the projects the repo root's commands run".to_owned(),
            )
        };
        units.push(AffectedUnit {
            project: None,
            kind: None,
            affected: runs,
            reason,
        });
    }
    for project in &plan.projects {
        // A project whose steps are all turned off is not counted.
        if project
            .steps
            .iter()
            .all(|(_, planned)| matches!(planned, Planned::NotNeeded(_)))
        {
            continue;
        }
        let (runs, reason) = match &project.kind {
            Some(kind) => {
                let id = project_id(kind, &project.root);
                match affected.reason(&id) {
                    Some(reason) => (true, reason),
                    None => (false, "no change reaches this project".to_owned()),
                }
            }
            None if anything => (
                true,
                "named in verify.projects but not a detected project, so --affected cannot tell what it uses; it runs whenever anything changed".to_owned(),
            ),
            None => (false, "nothing changed".to_owned()),
        };
        units.push(AffectedUnit {
            project: Some(project.root.clone()),
            kind: project.kind.clone(),
            affected: runs,
            reason,
        });
    }
    plan.affected = Some(AffectedPlan {
        base: base.map(str::to_owned),
        everything: affected.everything.clone(),
        changed_files: affected.changed_files,
        units,
        notes: Vec::new(),
        warnings: graph.warnings.clone(),
    });
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
        covered: covered.to_owned(),
        root_whole_repo: matches!(detected_runner, "nx" | "turbo") || !overrides.is_empty(),
        affected: None,
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
    /// Not run because `--affected` found that no change reaches its project
    /// (ADR 0032). Not the same as skipped: the step has a command.
    NotAffected,
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
            Self::NotAffected => "not affected",
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
    /// For a step that ran pytest or unittest: the tests its output counted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tests: Option<TestCounts>,
    /// For a pytest step that was stopped early or timed out: the text of
    /// its first error, from one more run with `--maxfail=1` (ADR 0035).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub first_error: Vec<String>,
    /// Set when the step was not run now: it passed before on the same
    /// inputs, and that pass came from the step cache (ADR 0036).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache: Option<CacheHit>,
    /// With a base to compare with: `new`, `pre-existing`, `fixed` or
    /// `flaky` (ADR 0037). Absent when the step has no label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<crate::blame::Label>,
    /// With a base to compare with: how the step did on the base commit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<crate::blame::BaseOutcome>,
    /// For pytest, Vitest and Jest: each failing test, with its label when
    /// there is a base to compare with (ADR 0038).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failed_tests: Vec<FailedTest>,
    /// The runner's per-test results. Not in JSON.
    #[serde(skip)]
    pub test_results: Option<crate::pertest::TestResults>,
    /// The failure's signature: one short hash per normalised line of its
    /// output (ADR 0037). A failure is `pre-existing` only when its
    /// signature on the head is contained in the base's. Not in JSON.
    #[serde(skip)]
    pub signature: Vec<String>,
}

/// A failing test of a step (ADR 0038).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedTest {
    /// Its file and name: `tests/test_auth.py::test_login`,
    /// `src/a.test.ts > math > adds`.
    pub id: String,
    /// With a base: `new` or `pre-existing`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<crate::blame::Label>,
}

/// Where a step's pass from the cache came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheHit {
    /// How long the step took when it ran.
    pub duration_ms: u64,
    /// When it ran, as `2026-11-13T07:06:00Z`.
    pub ran_at: String,
}

/// What the step cache did in a run (ADR 0036).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheSummary {
    /// False with `--no-cache`: every step ran, and passes were still stored.
    pub read: bool,
    /// Steps whose pass came from the cache instead of running now.
    pub hits: usize,
    /// The time those steps took when they last ran.
    pub saved_ms: u64,
}

/// Tests a known runner reported for a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct TestCounts {
    pub passed: u64,
    pub failed: u64,
    pub errors: u64,
    pub skipped: u64,
}

impl From<Tally> for TestCounts {
    fn from(tally: Tally) -> Self {
        Self {
            passed: tally.passed,
            failed: tally.failed,
            errors: tally.errors,
            skipped: tally.skipped,
        }
    }
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
    /// Nothing failed that the change introduced: every failure also fails
    /// on the base commit (ADR 0037). Exit code 0.
    PreExisting,
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
    /// `all`, or `affected` for `--affected`: only the projects a change
    /// reaches ran, so `result` describes them, not the whole repo.
    pub scope: Scope,
    /// For `--affected`: which projects ran, and why.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub affected: Option<AffectedReport>,
    /// What the step cache did. Absent when it was not used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache: Option<CacheSummary>,
    /// Blame-aware verify: the base commit, or why there is none (ADR 0037).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<crate::blame::BaseReport>,
}

/// What a verify run covered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    /// Every project.
    All,
    /// Only the projects a change reaches (`--affected`).
    Affected,
}

/// The `affected` object of `fscale verify --affected --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AffectedReport {
    /// The `--base` ref, or null for uncommitted changes only.
    pub base: Option<String>,
    /// True when every project is affected.
    pub everything: bool,
    /// Why every project is affected. Absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub changed_files: usize,
    /// Units with steps: the repo root's own steps count as one.
    pub projects_total: usize,
    /// Units that ran.
    pub projects_run: usize,
    /// Each unit, in plan order.
    pub projects: Vec<AffectedUnit>,
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
    /// One identity per enforced violation (rule, file, message), so the
    /// violations can be compared with the base commit's (ADR 0037).
    pub violations: Vec<String>,
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
        .map(|line| hide_secrets(line.trim_end()))
        .filter(|line| !line.is_empty())
        .collect();
    lines[lines.len().saturating_sub(TAIL_LINES)..].to_vec()
}

/// The counts a known test runner reported in a finished step's output.
fn final_tally(command: &str, stdout: &[u8], stderr: &[u8]) -> Option<Tally> {
    let mut activity = Activity::new(command);
    activity.feed(0, stdout);
    activity.feed(1, stderr);
    activity.runner().map(|_| activity.tally())
}

/// True when `--maxfail=1` can be added to the end of `command` and reach
/// pytest: the command names pytest, and is one program with its
/// arguments, with no pipe, `&&`, `;`, redirection or substitution outside
/// quotes (`pytest | tee log` would hand the flag to `tee`).
fn takes_maxfail(command: &str) -> bool {
    if Runner::from_command(command) != Some(Runner::Pytest) {
        return false;
    }
    let mut quote: Option<char> = None;
    for c in command.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '|' | '&' | ';' | '<' | '>' | '`' | '$' | '\n') => return false,
            _ => {}
        }
    }
    quote.is_none()
}

/// The lines of pytest output short summary: `ERROR tests/a.py::t - ...`
/// and `FAILED ...`, in order.
fn summary_lines(output: &str) -> Vec<&str> {
    let mut lines = output.lines().map(str::trim);
    if !lines.any(|line| line.contains("short test summary info")) {
        return Vec::new();
    }
    lines
        .take_while(|line| !line.starts_with('='))
        .filter(|line| line.starts_with("ERROR ") || line.starts_with("FAILED "))
        .collect()
}

/// What pytest says about its first error or failure: the first block of
/// `E   ...` lines (at most 12) and the first line of the short summary, with
/// connection strings' secrets replaced by `***`. The last 10 lines when
/// neither is there.
#[must_use]
pub fn first_error_lines(output: &str) -> Vec<String> {
    let mut found: Vec<String> = output
        .lines()
        .map(str::trim_end)
        .skip_while(|line| !line.trim_start().starts_with("E "))
        .take_while(|line| line.trim_start().starts_with("E "))
        .take(12)
        .map(|line| line.trim().to_owned())
        .collect();
    if let Some(summary) = summary_lines(output).first() {
        found.push((*summary).to_owned());
    }
    if found.is_empty() {
        let lines: Vec<&str> = output
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect();
        found = lines[lines.len().saturating_sub(10)..]
            .iter()
            .map(|line| line.trim_end().to_owned())
            .collect();
    }
    found
        .iter()
        .map(|line| {
            let line = hide_secrets(line);
            if line.chars().count() > 300 {
                format!("{}...", line.chars().take(300).collect::<String>())
            } else {
                line
            }
        })
        .collect()
}

/// A line with what could be a secret replaced by `***`: a URL's user and
/// password (up to the last `@`, so a password with `/` in it is hidden
/// too), `password=` and `pwd=` values, and the names after `user` and
/// `role`, quoted with `"`, `'` or backquotes (ADR 0035).
#[must_use]
pub fn hide_secrets(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(index) = rest.find("://") {
        let (before, after) = rest.split_at(index + 3);
        out.push_str(before);
        let end = after
            .find(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '`')
            .unwrap_or(after.len());
        match after[..end].rfind('@') {
            Some(at) => {
                out.push_str("***");
                rest = &after[at..];
            }
            None => rest = after,
        }
    }
    out.push_str(rest);
    let mut line = out;
    for key in ["password=", "pwd=", "passwd="] {
        let mut out = String::with_capacity(line.len());
        let mut rest = line.as_str();
        while let Some(index) = rest.to_ascii_lowercase().find(key) {
            let (before, after) = rest.split_at(index + key.len());
            out.push_str(before);
            let end = after
                .find(|c: char| {
                    c.is_whitespace() || matches!(c, '"' | '\'' | ';' | '&' | ',' | ')')
                })
                .unwrap_or(after.len());
            if end > 0 {
                out.push_str("***");
            }
            rest = &after[end..];
        }
        out.push_str(rest);
        line = out;
    }
    hide_user(&line)
}

/// `... for user "app"` and MySQL's `user 'app'@` with the user name
/// replaced by `***`, so a failed login's text never names the user.
fn hide_user(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some((index, word)) = ["user ", "role ", "credentials for "]
        .iter()
        .filter_map(|word| rest.find(word).map(|index| (index, *word)))
        .min()
    {
        let (before, after) = rest.split_at(index + word.len());
        out.push_str(before);
        let Some(quote) = after
            .chars()
            .next()
            .filter(|c| matches!(c, '"' | '\'' | '`'))
        else {
            rest = after;
            continue;
        };
        match after[1..].find(quote) {
            Some(end) => {
                out.push(quote);
                out.push_str("***");
                out.push(quote);
                rest = &after[end + 2..];
            }
            None => rest = after,
        }
    }
    out.push_str(rest);
    out
}

/// The usual port of a kind of service named in an error, for the kinds
/// of [`SERVICE_CLIENTS`].
fn usual_port(kind: &str) -> u16 {
    match kind {
        "postgresql" => 5432,
        "redis" => 6379,
        "mysql" => 3306,
        "mongodb" => 27017,
        _ => 5672,
    }
}

/// The first `localhost:<port>` or `127.0.0.1:<port>` in a line.
fn local_port_in(line: &str) -> Option<u16> {
    for host in ["localhost:", "127.0.0.1:", "[::1]:"] {
        if let Some(index) = line.find(host) {
            let digits: String = line[index + host.len()..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            if let Ok(port) = digits.parse() {
                return Some(port);
            }
        }
    }
    None
}

/// For a finished pytest step with errors and no failures (`failed` is the
/// runner's own count): when the message of every error in its short
/// summary says a connection failed and names a local service that is not
/// running (a refused connection; or the service's address, name or client
/// as a whole word, while nothing listens on its port), the step needs
/// setup, not a code change (ADR 0035).
fn errors_name_unreachable_service(
    output: &str,
    failed: u64,
    root: &Path,
    dir: &Path,
) -> Option<SetupProblem> {
    let summary = summary_lines(output);
    if failed > 0 || summary.is_empty() || summary.iter().any(|line| line.starts_with("FAILED ")) {
        return None;
    }
    let declared = services_for_folder(root, dir);
    let mut probed: BTreeMap<(String, u16), bool> = BTreeMap::new();
    let mut down = |host: &str, port: u16| {
        *probed
            .entry((host.to_owned(), port))
            .or_insert_with(|| !listening(host, port, PROBE_TIMEOUT))
    };
    let mut named: Option<String> = None;
    for line in &summary {
        // `ERROR tests/a.py::test_b - <message>`: the test's path and name
        // are not evidence, only the message is.
        let message = line.split_once(" - ").map(|(_, message)| message)?;
        if let Some(found) = refused_service(message) {
            named.get_or_insert(found.described);
            continue;
        }
        let lower = message.to_ascii_lowercase();
        if !says_unreachable(&lower) {
            return None;
        }
        let by_declared = declared
            .iter()
            .find(|service| output_names(&lower, service) && down(&service.host, service.port))
            .map(Service::describe);
        let described = match by_declared {
            Some(described) => described,
            None => {
                let (kind, _) = SERVICE_CLIENTS.iter().find(|(_, clients)| {
                    clients.iter().any(|client| names_client(&lower, client))
                })?;
                let name = match *kind {
                    "postgresql" => "PostgreSQL",
                    "redis" => "Redis",
                    "mysql" => "MySQL",
                    "mongodb" => "MongoDB",
                    _ => "RabbitMQ",
                };
                // The port in the message, else the folder's own service of
                // that kind (when it is up, this is not the cause), else the
                // usual one.
                let own = declared
                    .iter()
                    .find(|service| service.name.to_ascii_lowercase().contains(kind));
                let (host, port) = match (local_port_in(&lower), own) {
                    (Some(port), _) => ("localhost".to_owned(), port),
                    (None, Some(service)) => (service.host.clone(), service.port),
                    (None, None) => ("localhost".to_owned(), usual_port(kind)),
                };
                if !down(&host, port) {
                    return None;
                }
                format!("{name} on localhost:{port}")
            }
        };
        named.get_or_insert(described);
    }
    let described = named?;
    let mut problem = crate::setup::service_down(&described, root, dir);
    problem.reason = format!(
        "every error ({}) says {described} cannot be reached, and it is not running; the tests that need it cannot pass until it is started",
        summary.len()
    );
    Some(problem)
}

/// Services the step's folder names that are not listening, described.
fn services_not_running(root: &Path, dir: &Path) -> Vec<String> {
    let mut seen = Vec::new();
    for service in services_for_folder(root, dir) {
        let described = service.describe();
        if !seen.contains(&described) && !listening(&service.host, service.port, PROBE_TIMEOUT) {
            seen.push(described);
        }
    }
    seen
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
    /// False when `--affected` found that no change reaches this unit.
    runs: bool,
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

/// How long a service must stay down, from the first probe that found it
/// down to a second one, before a running step is judged (ADR 0025). A
/// step that starts its own database, or waits for one, gets this long.
pub const CONFIRM_AFTER: Duration = Duration::from_secs(10);

/// Why a running step looks like it cannot succeed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EarlyKind {
    /// The output shows refused connections to the service: setup needed.
    Refused,
    /// Every test so far has errored, a service the repo names is down, and
    /// the output names that service: a likely cause, not a certain one, so
    /// the step counts as failed.
    AllErrored,
    /// Every test so far has errored, with none passed and none failed, and
    /// no service is to blame on the evidence: likely setup (ADR 0035).
    NoPass,
}

/// Client libraries that name a kind of service in an error, lower case.
const SERVICE_CLIENTS: [(&str, &[&str]); 5] = [
    (
        "postgresql",
        &["asyncpg", "psycopg", "libpq", "pg8000", "postgres"],
    ),
    ("redis", &["redis", "aioredis", "ioredis"]),
    (
        "mysql",
        &["pymysql", "mysqlclient", "aiomysql", "mysql", "mariadb"],
    ),
    ("mongodb", &["pymongo", "motor", "mongoose", "mongo"]),
    ("rabbitmq", &["pika", "aio_pika", "amqp", "rabbitmq"]),
];

/// Words that say a connection could not be made. Evidence that an error
/// is about reaching a service, not about what the service returned
/// (`relation "orders" does not exist` is a code or migration problem).
const UNREACHABLE: [&str; 22] = [
    "connection refused",
    "connect call failed",
    "could not connect",
    "cannot connect",
    "can't connect",
    "unable to connect",
    "failed to connect",
    "cannot reach",
    "can't reach",
    "could not reach",
    "unreachable",
    "is it running",
    "not running",
    "connectionerror",
    "connectionrefusederror",
    "connection error",
    "timed out",
    "econnrefused",
    "errno 111",
    "error 111",
    "no route to host",
    "name or service not known",
];

/// True when `text` (lower case) says a connection could not be made.
fn says_unreachable(text: &str) -> bool {
    UNREACHABLE.iter().any(|phrase| text.contains(phrase))
}

/// True when `needle` occurs in `text` with no digit right after it and no
/// letter or digit right before it: `localhost:5432` is not in
/// `localhost:54321`, `port 6379` is not in `report 6379`.
fn contains_bounded(text: &str, needle: &str) -> bool {
    let mut from = 0;
    while let Some(found) = text[from..].find(needle) {
        let start = from + found;
        let end = start + needle.len();
        let after_ok = !text[end..].starts_with(|c: char| c.is_ascii_digit());
        let before_ok = !needle.starts_with(|c: char| c.is_ascii_alphanumeric())
            || !text[..start].ends_with(|c: char| c.is_ascii_alphanumeric());
        if after_ok && before_ok {
            return true;
        }
        from = start + 1;
        while !text.is_char_boundary(from) {
            from += 1;
        }
    }
    false
}

/// The words of `text`: runs of letters, digits and `_`.
fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|word| !word.is_empty())
}

/// True when a word of `text` is `client`, or `client` followed by digits
/// (`psycopg2`): `redis` is not in `redistribute`.
fn names_client(text: &str, client: &str) -> bool {
    words(text).any(|word| {
        word.strip_prefix(client)
            .is_some_and(|rest| rest.chars().all(|c| c.is_ascii_digit()))
    })
}

/// True when `text` (lower case) names `service`: its host and port, its
/// name, or the client library of its kind, as whole words (ADR 0035). Only
/// then may a service that is down be named as the cause.
fn output_names(text: &str, service: &Service) -> bool {
    let port = service.port.to_string();
    let host_port = [
        format!("{}:{port}", service.host.to_ascii_lowercase()),
        format!("localhost:{port}"),
        format!("127.0.0.1:{port}"),
        format!("port {port}"),
        format!("', {port})"),
    ];
    if host_port
        .iter()
        .any(|needle| contains_bounded(text, needle))
    {
        return true;
    }
    let name = service.name.to_ascii_lowercase();
    if name.len() >= 3 && words(&name).count() == 1 && words(text).any(|word| word == name) {
        return true;
    }
    SERVICE_CLIENTS
        .iter()
        .filter(|(kind, _)| {
            name.contains(kind) || (*kind == "postgresql" && name.contains("postgres"))
        })
        .any(|(_, clients)| clients.iter().any(|client| names_client(text, client)))
}

/// A service found down while a step runs, before it is confirmed.
#[derive(Debug, Clone)]
struct Suspect {
    described: String,
    evidence: String,
    host: String,
    port: u16,
    kind: EarlyKind,
    since: Instant,
}

/// What a running step's output showed about the services it needs.
struct Early {
    /// Set once it is clear the step cannot succeed.
    problem: Option<(SetupProblem, EarlyKind)>,
    suspect: Option<Suspect>,
    last_probe: Option<Instant>,
    declared: Option<Vec<Service>>,
    confirm_after: Duration,
}

impl Early {
    fn new(confirm_after: Duration) -> Self {
        Self {
            problem: None,
            suspect: None,
            last_probe: None,
            declared: None,
            confirm_after,
        }
    }
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
/// running, and probes the service to be sure (ADR 0025). It is clear the
/// step cannot succeed when a service found down by one of these stays
/// down on a second probe [`CONFIRM_AFTER`] later:
///
/// - the output shows a refused connection to a known local service, and
///   nothing listens on its port (setup needed); or
/// - a known test runner reports only errors so far (at least
///   [`ERRORS_BEFORE_PROBE`]), and a service that the step's
///   `.env.example` or Compose file names is not listening (a likely cause:
///   the step counts as failed, with the service named).
fn service_down_while_running(
    activity: &Activity,
    early: &mut Early,
    root: &Path,
    dir: &Path,
) -> Option<(String, String, EarlyKind)> {
    if early
        .last_probe
        .is_some_and(|last| last.elapsed() < PROBE_EVERY)
    {
        return None;
    }
    let tally = activity.tally();
    let all_errored = activity.runner().is_some()
        && tally.errors >= ERRORS_BEFORE_PROBE
        && tally.passed == 0
        && tally.failed == 0;
    // A suspect from before: probe it again once enough time has passed.
    if let Some(suspect) = early.suspect.clone() {
        if suspect.since.elapsed() < early.confirm_after {
            return None;
        }
        early.last_probe = Some(Instant::now());
        let still = suspect.kind == EarlyKind::Refused || all_errored;
        if still && !listening(&suspect.host, suspect.port, PROBE_TIMEOUT) {
            return Some((suspect.described, suspect.evidence, suspect.kind));
        }
        early.suspect = None;
        return None;
    }
    let declared = early
        .declared
        .get_or_insert_with(|| services_for_folder(root, dir))
        .clone();
    let runner_failing = activity.runner().is_none() || tally.failed + tally.errors > 0;
    if let Some(found) = refused_service(&activity.recent().join("\n")) {
        if let Some(port) = found
            .port
            .filter(|_| runner_failing && known_service(&found, &declared))
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
                early.suspect = Some(Suspect {
                    described,
                    evidence:
                        "the output shows refused connections and nothing listens on that port"
                            .to_owned(),
                    host: found.host,
                    port,
                    kind: EarlyKind::Refused,
                    since: Instant::now(),
                });
            }
            return None;
        }
    }
    if all_errored && !declared.is_empty() {
        // A service that is down is the cause only when the output names it
        // (ADR 0035); otherwise the step may still be stopped as likely
        // setup, and the service is listed as also not running.
        // Progress lines name test files (`tests/test_redis_cache.py E`):
        // only lines that say a connection failed count.
        let text = activity
            .recent()
            .iter()
            .map(|line| line.to_ascii_lowercase())
            .filter(|line| says_unreachable(line))
            .collect::<Vec<_>>()
            .join("\n");
        let named: Vec<&Service> = declared
            .iter()
            .filter(|service| output_names(&text, service))
            .collect();
        if named.is_empty() {
            return None;
        }
        early.last_probe = Some(Instant::now());
        if let Some(down) = named
            .into_iter()
            .find(|service| !listening(&service.host, service.port, PROBE_TIMEOUT))
        {
            early.suspect = Some(Suspect {
                described: down.describe(),
                evidence: format!(
                    "every test so far has errored ({}), the output names it, and {} lists it",
                    tally.describe(),
                    down.source
                ),
                host: down.host.clone(),
                port: down.port,
                kind: EarlyKind::AllErrored,
                since: Instant::now(),
            });
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
    /// The step cache (ADR 0036). `None`: every step runs and nothing is stored.
    pub cache: Option<&'a StepCache<'a>>,
    /// With a base to compare with (ADR 0037): a failure does not stop the
    /// rest of its project, and a failed check stops nothing, so an old
    /// failure cannot hide a new one.
    pub every_step: bool,
    /// The end of the base run's time budget: a step gets no more than
    /// what is left of it.
    pub deadline: Option<Instant>,
    /// True for the steps of the base run, which get `FSCALE_BASE_RUN=1`.
    pub base_run: bool,
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
            cache: None,
            every_step: false,
            deadline: None,
            base_run: false,
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
fn environment(command: &mut Command, config: &VerifyConfig, base_run: bool) {
    if base_run {
        command.env("FSCALE_BASE_RUN", "1");
    }
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
    // A base step gets no more than what is left of the base run's budget.
    let timeout = match options.deadline {
        Some(deadline) => {
            let left = deadline.saturating_duration_since(Instant::now());
            Some(timeout.map_or(left, |own| own.min(left)))
        }
        None => timeout,
    };
    options.say(&format!("{place}: started: {command}"));
    let mut shell = shell(command);
    shell.current_dir(&dir);
    environment(&mut shell, options.config, options.base_run);
    // Per-test results (ADR 0038): pytest writes a JUnit report through
    // PYTEST_ADDOPTS, so the command stays as the user wrote it.
    let runner = crate::pertest::TestRunner::of(
        command,
        crate::pertest::script_text(&dir, command).as_deref(),
    );
    // Not when the user already asks for a JUnit report: theirs must stay
    // where they put it, and that step is compared as a whole.
    let junit = (runner == Some(crate::pertest::TestRunner::Pytest)
        && !crate::pertest::user_asks_for_junit(unit.root, &dir, command))
    .then(crate::pertest::junit_path);
    if let Some(junit) = &junit {
        let own = std::env::var("PYTEST_ADDOPTS").unwrap_or_default();
        shell.env("PYTEST_ADDOPTS", crate::pertest::addopts(&own, junit));
    }
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
    let early = Mutex::new(Early::new(CONFIRM_AFTER));
    let stop_early = !options.config.keep_running_when_service_down;
    let errors_before_stop = options.config.errors_before_stop();
    let on_output = |activity: &Activity| {
        let mut early = early.lock().unwrap_or_else(PoisonError::into_inner);
        if early.problem.is_some() {
            return false;
        }
        let Some((service, evidence, kind)) =
            service_down_while_running(activity, &mut early, unit.root, &dir)
        else {
            // No service to blame: every test erroring with none passed or
            // failed is still likely setup (ADR 0035).
            let tally = activity.tally();
            let no_pass = activity.runner().is_some()
                && tally.passed == 0
                && tally.failed == 0
                && errors_before_stop.is_some_and(|limit| tally.errors >= limit);
            if !no_pass {
                return false;
            }
            let seen = tally.describe();
            options.say(&format!(
                "{place}: LIKELY SETUP, seen while running: every test so far has errored ({seen}) and none passed; stopping the step to find the first error (set verify.stopAfterErrors to false to let it run on)"
            ));
            early.problem = Some((
                SetupProblem {
                    reason: format!("every test errored ({seen}) and none passed"),
                    missing_program: None,
                    missing_service: None,
                    start_services: None,
                },
                EarlyKind::NoPass,
            ));
            return true;
        };
        let problem = service_down(&service, unit.root, &dir);
        let label = match kind {
            EarlyKind::Refused => "SETUP NEEDED",
            EarlyKind::AllErrored | EarlyKind::NoPass => "LIKELY SETUP",
        };
        let then = if stop_early {
            "stopping the step (set verify.stopWhenServiceDown to false to let it run on)"
        } else {
            "the step runs on because verify.stopWhenServiceDown is false"
        };
        let start = setup_fix(&problem, unit.install)
            .map_or_else(String::new, |fix| format!("; start it with: {fix}"));
        options.say(&format!(
            "{place}: {label}, seen while running: {service} is not running ({evidence}){start}; {then}"
        ));
        early.problem = Some((problem, kind));
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
        tests: None,
        first_error: Vec::new(),
        cache: None,
        label: None,
        base: None,
        signature: Vec::new(),
        failed_tests: Vec::new(),
        test_results: None,
    };
    let text = || {
        strip_ansi(&format!(
            "{}\n{}",
            String::from_utf8_lossy(&finished.stdout),
            String::from_utf8_lossy(&finished.stderr)
        ))
    };
    if !matches!(finished.ending, Ending::Exited(Some(0))) {
        report.signature = failure_signature(&text(), unit.root);
    }
    if let Some(runner) = runner {
        let xml = junit
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok());
        // pytest refused its options (exit 4) and wrote no report: whatever
        // it printed is not evidence of anything, so it can never match the
        // base's output and make a failure pre-existing.
        if junit.is_some() && xml.is_none() && finished.ending == Ending::Exited(Some(4)) {
            report.signature.push(format!(
                "pytest-usage-error-{}",
                crate::pertest::junit_path().to_string_lossy()
            ));
        }
        report.test_results = runner.read(&text(), xml.as_deref());
        report.failed_tests = report
            .test_results
            .as_ref()
            .map(|results| {
                results
                    .failed
                    .iter()
                    .map(|id| FailedTest {
                        id: id.clone(),
                        label: None,
                    })
                    .collect()
            })
            .unwrap_or_default();
    }
    if let Some(junit) = &junit {
        let _ = std::fs::remove_file(junit);
    }
    let early_seen = early
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner)
        .problem;
    // Only refused connections are certain enough to call the step setup.
    let seen_early = early_seen
        .clone()
        .filter(|(_, kind)| *kind == EarlyKind::Refused)
        .map(|(problem, _)| problem);
    let tally = final_tally(command, &finished.stdout, &finished.stderr);
    // Left out when the runner counted nothing (no markers in the output).
    report.tests = tally
        .filter(|tally| !tally.is_empty())
        .map(TestCounts::from);
    // After an early stop or a timeout, pytest's own summary never came:
    // one more short run with --maxfail=1 shows the first error (ADR 0035).
    // The user's command runs as written; this is a separate run.
    let first_error = |report: &mut StepReport| -> Option<SetupProblem> {
        // Only when something errored or failed, and never after a stop.
        let erred = tally.is_some_and(|tally| tally.errors + tally.failed > 0);
        if !erred || !takes_maxfail(command) || options.stop.load(Ordering::SeqCst) {
            return None;
        }
        let probe = format!("{command} --maxfail=1");
        options.say(&format!(
            "{place}: running `{probe}` (at most {}) to find the first error",
            format_limit(FIRST_ERROR_LIMIT)
        ));
        let mut again = self::shell(&probe);
        again.current_dir(&dir);
        environment(&mut again, options.config, options.base_run);
        let quiet = |_: Duration, _: Option<&str>| {};
        let watch = Watch {
            timeout: Some(FIRST_ERROR_LIMIT),
            stop: options.stop,
            heartbeat: options.heartbeat,
            on_heartbeat: &quiet,
            command: &probe,
            on_output: &|_: &Activity| false,
            on_spawn: &on_spawn,
        };
        let rerun = run_tree(again, &watch).ok()?;
        if let (Some(record), Some(pid)) = (
            options.record,
            *spawned.lock().unwrap_or_else(PoisonError::into_inner),
        ) {
            record.ended(pid);
        }
        if rerun.ending == Ending::Stopped {
            return None;
        }
        let output = strip_ansi(&format!(
            "{}\n{}",
            String::from_utf8_lossy(&rerun.stdout),
            String::from_utf8_lossy(&rerun.stderr)
        ));
        report.first_error = first_error_lines(&output);
        let first = report.first_error.join("\n");
        setup_problem(&first, None, unit.root, &dir)
    };
    let also_down = || {
        let down = services_not_running(unit.root, &dir);
        if down.is_empty() {
            String::new()
        } else {
            format!(
                "; also not running, though the output does not name it: {}",
                down.join(", ")
            )
        }
    };
    let code = match finished.ending {
        Ending::Exited(code) => code,
        Ending::GaveUp => {
            report.output_tail = tail(&finished.stdout, &finished.stderr);
            match early_seen {
                Some((problem, EarlyKind::AllErrored)) => {
                    let start = setup_fix(&problem, unit.install)
                        .map_or_else(String::new, |fix| format!(" ({fix})"));
                    let service = problem.missing_service.clone().unwrap_or_default();
                    if let Some(found) = first_error(&mut report) {
                        // The first error's own text is stronger evidence.
                        report.status = StepStatus::SetupNeeded;
                        report.detail = format!(
                            "{command} (stopped early after {took}; its first error shows {})",
                            found.reason
                        );
                        report.fix = setup_fix(&found, unit.install);
                        options.say(&format!(
                            "{place}: SETUP NEEDED after {took} (stopped early)"
                        ));
                    } else {
                        report.status = StepStatus::Failed;
                        report.detail = format!(
                            "{command} (stopped early after {took}: every test errored, the output says {service} cannot be reached, and it was not running; start it{start} and run again. If the tests do not use it, the errors are in the code or the tests.)"
                        );
                        options.say(&format!("{place}: FAILED after {took} (stopped early)"));
                    }
                }
                Some((problem, EarlyKind::NoPass)) => match first_error(&mut report) {
                    Some(found) => {
                        report.status = StepStatus::SetupNeeded;
                        report.detail = format!(
                            "{command} (stopped early after {took}; its first error shows {})",
                            found.reason
                        );
                        report.fix = setup_fix(&found, unit.install);
                        options.say(&format!(
                            "{place}: SETUP NEEDED after {took} (stopped early)"
                        ));
                    }
                    None => {
                        report.status = StepStatus::Failed;
                        let first = if report.first_error.is_empty() {
                            ""
                        } else {
                            "; its first error is shown below"
                        };
                        report.detail = format!(
                            "{command} (stopped early after {took}: {}, which usually means the environment is not ready (a service, a setting, test data){first}{})",
                            problem.reason,
                            also_down()
                        );
                        options.say(&format!("{place}: FAILED after {took} (stopped early)"));
                    }
                },
                other => {
                    let problem = other.map_or_else(
                        || service_down("a local service", unit.root, &dir),
                        |(problem, _)| problem,
                    );
                    report.status = StepStatus::SetupNeeded;
                    report.detail =
                        format!("{command} (stopped early after {took}): {}", problem.reason);
                    report.fix = setup_fix(&problem, unit.install);
                    options.say(&format!(
                        "{place}: SETUP NEEDED after {took} (stopped early)"
                    ));
                }
            }
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
            } else if let Some(found) = first_error(&mut report) {
                // The first error names the environment: setup needed.
                report.status = StepStatus::SetupNeeded;
                report.detail = format!(
                    "{command} (no result within {limit}; its first error shows {})",
                    found.reason
                );
                report.fix = setup_fix(&found, unit.install);
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
    let found = setup_problem(&text(), code, unit.root, &dir)
        .or(seen_early)
        .or_else(|| {
            let failed = tally.map_or(0, |tally| tally.failed);
            errors_name_unreachable_service(&text(), failed, unit.root, &dir)
        });
    match found {
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
    if !unit.runs {
        return Ok(unit_steps
            .iter()
            .map(|(name, planned)| StepReport {
                name: name.clone(),
                project: unit.project.map(|project| project.root.clone()),
                kind: unit.project.and_then(|project| project.kind.clone()),
                status: StepStatus::NotAffected,
                command: match planned {
                    Planned::Command(command) => Some(command.clone()),
                    _ => None,
                },
                exit_code: None,
                duration_ms: 0,
                detail: "no change reaches this project".to_owned(),
                output_tail: Vec::new(),
                fix: None,
                timeout_ms: None,
                timeout_key: None,
                tests: None,
                first_error: Vec::new(),
                cache: None,
                label: None,
                base: None,
                signature: Vec::new(),
                failed_tests: Vec::new(),
                test_results: None,
            })
            .collect());
    }
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
            tests: None,
            first_error: Vec::new(),
            cache: None,
            label: None,
            base: None,
            signature: Vec::new(),
            failed_tests: Vec::new(),
            test_results: None,
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
            Planned::Command(command) => cached_or_run(unit, name, command, options)?,
        };
        failed = !options.every_step && (failed || blocks(report.status));
        steps.push(report);
    }
    Ok(steps)
}

/// The most lines of output a signature keeps.
const SIGNATURE_LINES: usize = 5000;

/// A failure's signature (ADR 0037): one short hash per non-empty line of
/// its output, with the run's root folder written as `<root>` (the head and
/// the base worktree are different folders), digits folded to `#` (counts,
/// times and line numbers change from run to run) and spaces squeezed.
/// Only hashes are kept, never the text.
#[must_use]
pub fn failure_signature(output: &str, root: &Path) -> Vec<String> {
    let mut roots: Vec<String> = Vec::new();
    let shown = root.to_string_lossy().into_owned();
    roots.push(shown.clone());
    roots.push(shown.replace('\\', "/"));
    if let Ok(real) = root.canonicalize() {
        roots.push(real.to_string_lossy().into_owned());
    }
    roots.sort_by_key(|text| std::cmp::Reverse(text.len()));
    roots.dedup();
    let mut lines = Vec::new();
    for line in output.lines().take(SIGNATURE_LINES) {
        let mut line = line.trim().to_owned();
        // pytest's `generated xml file: <the report verify asked for>`.
        if line.is_empty() || line.contains("firescale-junit-") {
            continue;
        }
        for root in &roots {
            if !root.is_empty() {
                line = line.replace(root.as_str(), "<root>");
            }
        }
        let folded: String = line
            .chars()
            .map(|c| if c.is_ascii_digit() { '#' } else { c })
            .collect();
        let squeezed = folded.split_whitespace().collect::<Vec<_>>().join(" ");
        lines.push(blake3::hash(squeezed.as_bytes()).to_hex()[..16].to_owned());
    }
    lines
}

/// True when every line of `head` is in `base`, as often (a multiset).
#[must_use]
pub fn signature_within(head: &[String], base: &[String]) -> bool {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for line in base {
        *counts.entry(line.as_str()).or_default() += 1;
    }
    head.iter().all(|line| match counts.get_mut(line.as_str()) {
        Some(count) if *count > 0 => {
            *count -= 1;
            true
        }
        _ => false,
    })
}

/// Runs one step of the base run in the base commit's worktree at `root`:
/// no cache, no progress lines of its own, `FSCALE_BASE_RUN=1`, and no
/// more time than is left before `deadline` (ADR 0037).
pub(crate) fn run_base_step(
    root: &Path,
    project: Option<&ProjectPlan>,
    name: &str,
    command: &str,
    options: &RunOptions<'_>,
    deadline: Option<Instant>,
) -> Result<StepReport> {
    let base = RunOptions {
        stop: options.stop,
        progress: None,
        jobs: 1,
        config: options.config,
        heartbeat: options.heartbeat,
        record: None,
        cache: None,
        every_step: true,
        deadline,
        base_run: true,
    };
    let unit = Unit {
        root,
        project,
        install: project.and_then(|project| project.install.as_deref()),
        runs: true,
    };
    run_step(&unit, name, command, &base)
}

/// A step's pass from the cache when nothing it depends on changed since it
/// last passed; otherwise the step runs, and a pass is kept for the cache.
fn cached_or_run(
    unit: &Unit<'_>,
    name: &str,
    command: &str,
    options: &RunOptions<'_>,
) -> Result<StepReport> {
    let Some(cache) = options.cache else {
        return run_step(unit, name, command, options);
    };
    let folder = unit.project.map(|project| project.root.as_str());
    let kind = unit.project.and_then(|project| project.kind.as_deref());
    let place = match folder {
        Some(folder) => format!("{name} in {}", place_name(folder)),
        None => name.to_owned(),
    };
    let dir = folder.map_or_else(|| unit.root.to_path_buf(), |f| project_dir(unit.root, f));
    let key = match cache.key(folder, kind, name, command, &dir) {
        Ok(key) => key,
        Err(reason) => {
            cache.note(format!("cache: not cached {place}: {reason}"));
            return run_step(unit, name, command, options);
        }
    };
    if let Some(entry) = cache.hit(&key) {
        cache.note(format!("cache: hit {place}"));
        let took = format_duration(Duration::from_millis(entry.duration_ms));
        let when = format_time(entry.ran_at);
        options.say(&format!(
            "{place}: passed from the cache (it took {took} when it last ran, {when}); not run again"
        ));
        return Ok(StepReport {
            name: name.to_owned(),
            project: folder.map(str::to_owned),
            kind: kind.map(str::to_owned),
            status: StepStatus::Passed,
            command: Some(command.to_owned()),
            exit_code: None,
            duration_ms: 0,
            detail: format!(
                "from the cache: it passed in {took} when it last ran ({when}), and nothing it depends on has changed since"
            ),
            output_tail: Vec::new(),
            fix: None,
            timeout_ms: None,
            timeout_key: None,
            tests: entry.tests,
            first_error: Vec::new(),
            cache: Some(CacheHit {
                duration_ms: entry.duration_ms,
                ran_at: format_iso(entry.ran_at),
            }),
            label: None,
            base: None,
            signature: Vec::new(),
            failed_tests: Vec::new(),
            test_results: None,
        });
    }
    let ran_at = now_secs();
    let report = run_step(unit, name, command, options)?;
    if report.status == StepStatus::Passed {
        cache.note(format!(
            "cache: miss {place}{}",
            if cache.reads() { "" } else { " (--no-cache)" }
        ));
        cache.remember(
            folder,
            kind,
            &key,
            Entry {
                format: 0,
                key: String::new(),
                step: name.to_owned(),
                project: folder.map(str::to_owned),
                command: command.to_owned(),
                duration_ms: report.duration_ms,
                ran_at,
                detail: report.detail.clone(),
                tests: report.tests,
            },
        );
    } else {
        cache.note(format!(
            "cache: miss {place}; not stored, because it did not pass"
        ));
    }
    Ok(report)
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
        tests: None,
        first_error: Vec::new(),
        cache: None,
        label: None,
        base: None,
        signature: Vec::new(),
        failed_tests: Vec::new(),
        test_results: None,
    }];
    let check_failed = !check.ok && !options.every_step;

    let units: Vec<(&[(String, Planned)], Unit<'_>)> = std::iter::once((
        plan.steps.as_slice(),
        Unit {
            root,
            project: None,
            install: plan.install.as_deref(),
            runs: plan
                .affected
                .as_ref()
                .is_none_or(|affected| affected.runs(None)),
        },
    ))
    .chain(plan.projects.iter().map(|project| {
        (
            project.steps.as_slice(),
            Unit {
                root,
                project: Some(project),
                install: project.install.as_deref(),
                runs: plan
                    .affected
                    .as_ref()
                    .is_none_or(|affected| affected.runs(Some(project))),
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
    if let Some(cache) = options.cache {
        cache.finish();
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
    let mut warnings = warnings;
    if let Some(affected) = &plan.affected {
        warnings.extend(affected.warnings.iter().cloned());
    }
    let steps_total = steps
        .iter()
        .filter(|step| !matches!(step.status, StepStatus::Off | StepStatus::NotAffected))
        .count();
    let steps_run = steps.iter().filter(|step| ran(step.status)).count();
    let mut on_path: Vec<&str> = Vec::new();
    let unit_runs =
        |project: Option<&ProjectPlan>| plan.affected.as_ref().is_none_or(|a| a.runs(project));
    if plan.python_from_path
        && unit_runs(None)
        && plan
            .steps
            .iter()
            .any(|(_, p)| matches!(p, Planned::Command(_)))
    {
        on_path.push(".");
    }
    for project in &plan.projects {
        if project.python_from_path
            && unit_runs(Some(project))
            && project
                .steps
                .iter()
                .any(|(_, planned)| matches!(planned, Planned::Command(_)))
        {
            on_path.push(&project.root);
        }
    }
    let mut notes = if on_path.is_empty() {
        Vec::new()
    } else {
        let places: Vec<&str> = on_path.iter().map(|folder| place_name(folder)).collect();
        vec![format!(
            "no virtual environment was found for {}, so {} Python commands use python from the PATH. To use one, create it in the project's folder (python -m venv .venv) or activate it.",
            places.join(", "),
            if places.len() == 1 { "its" } else { "their" }
        )]
    };
    if let Some(affected) = &plan.affected {
        notes.extend(affected.notes.iter().cloned());
    }
    let cache = options.cache.map(|cache| CacheSummary {
        read: cache.reads(),
        hits: steps.iter().filter(|step| step.cache.is_some()).count(),
        saved_ms: steps
            .iter()
            .filter_map(|step| step.cache.as_ref())
            .map(|hit| hit.duration_ms)
            .sum(),
    });
    let mut report = VerifyReport {
        schema_version: VERIFY_SCHEMA_VERSION,
        version: version.to_owned(),
        ok: false,
        result: VerifyResult::Failed,
        runner: plan.runner.clone(),
        steps_total,
        steps_run,
        fail_on_skipped: plan.fail_on_skipped,
        failed_step: None,
        failed_project: None,
        rerun: None,
        steps,
        skipped,
        warnings,
        notes,
        scope: if plan.affected.is_some() {
            Scope::Affected
        } else {
            Scope::All
        },
        affected: affected_report(plan),
        cache,
        base: None,
    };
    settle(&mut report, stopped);
    Ok(report)
}

/// True for a step whose failure blocks: failed, needs setup or timed out,
/// and not labelled `pre-existing` or `flaky` (ADR 0037).
fn blocking(step: &StepReport) -> bool {
    blocks(step.status)
        && step.status != StepStatus::Stopped
        && !matches!(
            step.label,
            Some(crate::blame::Label::PreExisting | crate::blame::Label::Flaky)
        )
}

/// Works out the result, the exit code and the first failure from the
/// steps and their labels. `stopped`: verify was interrupted.
pub(crate) fn settle(report: &mut VerifyReport, stopped: bool) {
    let first_failure = report.steps.iter().find(|step| blocking(step));
    let any_failed = report.steps.iter().any(|step| {
        blocking(step) && matches!(step.status, StepStatus::Failed | StepStatus::TimedOut)
    });
    let pre_existing = report
        .steps
        .iter()
        .any(|step| blocks(step.status) && step.label == Some(crate::blame::Label::PreExisting));
    let result = if stopped {
        VerifyResult::Stopped
    } else if any_failed || (report.fail_on_skipped && !report.skipped.is_empty()) {
        VerifyResult::Failed
    } else if first_failure.is_some() {
        VerifyResult::SetupNeeded
    } else if pre_existing {
        VerifyResult::PreExisting
    } else if report.skipped.is_empty() {
        VerifyResult::Ok
    } else {
        VerifyResult::Partial
    };
    report.failed_step = first_failure.map(|step| step.name.clone());
    report.failed_project = first_failure.and_then(|step| step.project.clone());
    report.rerun = first_failure.and_then(|step| step.command.clone());
    report.ok = matches!(
        result,
        VerifyResult::Ok | VerifyResult::Partial | VerifyResult::PreExisting
    );
    report.result = result;
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
    let base = scoped_headline(report);
    let hits = report.cache.as_ref().map_or(0, |cache| cache.hits);
    if hits == 0 || report.result == VerifyResult::Partial {
        // A partial headline counts the steps from the cache itself.
        return base;
    }
    // Steps from the cache did not run now: the headline says how many.
    let from_cache = format!(
        "{hits} of {} passed from the cache, not run now",
        plural_steps(report.steps_total)
    );
    if base.ends_with(')') && report.affected.is_some() {
        format!("{}; {from_cache})", &base[..base.len() - 1])
    } else {
        format!("{base} ({from_cache})")
    }
}

fn scoped_headline(report: &VerifyReport) -> String {
    let Some(affected) = &report.affected else {
        return whole_headline(report);
    };
    let left_out = affected.projects_total - affected.projects_run;
    let counts = format!(
        "{} of {} ran{}",
        affected.projects_run,
        plural_projects(affected.projects_total),
        if left_out == 0 {
            String::new()
        } else {
            format!(", {left_out} not affected")
        }
    );
    let whole = whole_headline(report);
    match (report.result, whole.as_str()) {
        (VerifyResult::Ok, _) => format!("verify: OK (affected only) - {counts}"),
        (VerifyResult::Partial, _) => {
            format!(
                "verify: PARTIAL (affected only) - {counts}; {}",
                ran_part(report)
            )
        }
        _ => format!("{whole} (affected only: {counts})"),
    }
}

/// `3 of 4 steps ran, 1 skipped`, saying how many passed from the cache
/// instead of running now.
fn ran_part(report: &VerifyReport) -> String {
    let hits = report.cache.as_ref().map_or(0, |cache| cache.hits);
    if hits == 0 {
        return format!(
            "{} of {} ran, {} skipped",
            report.steps_run,
            plural_steps(report.steps_total),
            report.skipped.len()
        );
    }
    format!(
        "{} of {} ran, {hits} passed from the cache, {} skipped",
        report.steps_run.saturating_sub(hits),
        plural_steps(report.steps_total),
        report.skipped.len()
    )
}

fn plural_projects(count: usize) -> String {
    format!("{count} project{}", if count == 1 { "" } else { "s" })
}

/// The headline of a run over every project.
fn whole_headline(report: &VerifyReport) -> String {
    use crate::blame::Label;
    // A failure labelled `new` says so; `pre-existing` and `flaky` ones do
    // not block, and are named apart (ADR 0037).
    let named = |status: StepStatus| -> Vec<String> {
        report
            .steps
            .iter()
            .filter(|step| step.status == status && blocking(step))
            .map(|step| match step.label {
                Some(Label::New) => format!("{} (new)", step_place(step)),
                _ => step_place(step),
            })
            .collect()
    };
    let failed = named(StepStatus::Failed);
    let timed_out = named(StepStatus::TimedOut);
    let setup = named(StepStatus::SetupNeeded);
    let pre_existing: Vec<String> = report
        .steps
        .iter()
        .filter(|step| blocks(step.status) && step.label == Some(Label::PreExisting))
        .map(step_place)
        .collect();
    let mut parts = Vec::new();
    if report.result == VerifyResult::Stopped {
        parts.push(format!(
            "STOPPED - interrupted after {} of {} ran",
            report.steps_run - report.cache.as_ref().map_or(0, |cache| cache.hits),
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
    let base = report
        .base
        .as_ref()
        .and_then(|base| base.commit.as_deref())
        .map_or_else(String::new, |commit| {
            format!(" {}", &commit[..commit.len().min(7)])
        });
    if !parts.is_empty() {
        if !pre_existing.is_empty() {
            parts.push(format!(
                "pre-existing (fails on the base commit{base} too, does not block): {}",
                pre_existing.join(", ")
            ));
        }
        return format!("verify: {}", parts.join("; "));
    }
    match report.result {
        VerifyResult::Ok => "verify: OK".to_owned(),
        VerifyResult::Partial => format!("verify: PARTIAL - {}", ran_part(report)),
        VerifyResult::PreExisting => format!(
            "verify: OK - no new failures; pre-existing (fails on the base commit{base} too): {}",
            pre_existing.join(", ")
        ),
        VerifyResult::Failed | VerifyResult::SetupNeeded | VerifyResult::Stopped => format!(
            "verify: FAILED - {} of {} skipped, and verify.failOnSkipped is on",
            report.skipped.len(),
            plural_steps(report.steps_total)
        ),
    }
}

fn affected_report(plan: &VerifyPlan) -> Option<AffectedReport> {
    plan.affected.as_ref().map(|affected| AffectedReport {
        base: affected.base.clone(),
        everything: affected.everything.is_some(),
        reason: affected.everything.clone(),
        changed_files: affected.changed_files,
        projects_total: affected.units.len(),
        projects_run: affected.units.iter().filter(|unit| unit.affected).count(),
        projects: affected.units.clone(),
    })
}

/// `fscale verify --dry-run`: what would run, and for `--affected` why,
/// without running anything (not even the check).
#[must_use]
pub fn format_verify_plan(plan: &VerifyPlan, check_command: &str) -> String {
    let report = affected_report(plan);
    let steps_of = |steps: &[(String, Planned)], indent: &str, out: &mut String| {
        for (name, planned) in steps {
            let (status, detail) = match planned {
                Planned::Command(command) => ("would run", command.as_str()),
                Planned::Skipped(reason) => ("skipped", reason.as_str()),
                Planned::NotNeeded(reason) => ("off", reason.as_str()),
            };
            out.push_str(&format!("{indent}{name:<10} {status:<10} {detail}\n"));
        }
    };
    let mut out = match &report {
        Some(affected) => {
            let left_out = affected.projects_total - affected.projects_run;
            format!(
                "verify --affected (dry run): {} of {} would run, {left_out} not affected. Nothing was run.\n",
                affected.projects_run,
                plural_projects(affected.projects_total)
            )
        }
        None => "verify (dry run): nothing was run.\n".to_owned(),
    };
    out.push_str(&format!(
        "  {:<10} {:<10} {check_command}\n",
        "check", "would run"
    ));
    let runs =
        |project: Option<&ProjectPlan>| plan.affected.as_ref().is_none_or(|a| a.runs(project));
    if !plan.steps.is_empty() {
        if runs(None) {
            steps_of(&plan.steps, "  ", &mut out);
        } else {
            out.push_str("  the repo root's own steps: not affected\n");
        }
    }
    for project in &plan.projects {
        let place = match &project.kind {
            Some(kind) if project.root == "." => format!("{} ({kind})", place_name(&project.root)),
            _ => place_name(&project.root).to_owned(),
        };
        if runs(Some(project)) {
            out.push_str(&format!("  in {place}:\n"));
            steps_of(&project.steps, "    ", &mut out);
        } else {
            out.push_str(&format!("  in {place}: not affected\n"));
        }
    }
    if let Some(affected) = &report {
        out.push_str(&format!("\n{}\n", affected_intro(affected)));
        for unit in affected.projects.iter().filter(|unit| unit.affected) {
            let place = match (&unit.project, &unit.kind) {
                (None, _) => "the repo root's own steps".to_owned(),
                (Some(folder), Some(kind)) if folder == "." => {
                    format!("{} ({kind})", place_name(folder))
                }
                (Some(folder), _) => folder.clone(),
            };
            out.push_str(&format!("  {place}: {}\n", unit.reason));
        }
    }
    if let Some(affected) = &plan.affected {
        if !affected.warnings.is_empty() {
            out.push('\n');
            for warning in &affected.warnings {
                out.push_str(&format!("warning: {warning}\n"));
            }
        }
        if !affected.notes.is_empty() {
            out.push('\n');
            for note in &affected.notes {
                out.push_str(&format!("note: {note}\n"));
            }
        }
    }
    out
}

/// The line that opens the list of what an affected run ran, and why.
fn affected_intro(affected: &AffectedReport) -> String {
    let compared = match &affected.base {
        Some(base) => format!("changes since the merge base with {base}, and uncommitted ones"),
        None => "uncommitted and untracked changes".to_owned(),
    };
    let files = format!(
        "{} changed file{}",
        affected.changed_files,
        if affected.changed_files == 1 { "" } else { "s" }
    );
    if affected.projects_run == 0 {
        format!(
            "Affected only ({compared}; {files}): no project is affected, so only the check ran."
        )
    } else {
        format!("Affected only ({compared}; {files}). Why each ran:")
    }
}

/// Renders the compact summary. No durations, so the text is stable.
#[must_use]
pub fn format_verify_human(report: &VerifyReport) -> String {
    let mut out = format!("{}\n", headline(report));
    let mut current: Option<(&str, Option<&str>)> = None;
    let mut collapsed: Option<Option<(&str, Option<&str>)>> = None;
    for step in &report.steps {
        let project = step
            .project
            .as_deref()
            .map(|project| (project, step.kind.as_deref()));
        // A unit that --affected left out is one line, not one per step.
        if step.status == StepStatus::NotAffected {
            let whole_unit = report.steps.iter().all(|other| {
                other.name == "check"
                    || other.project.as_deref().map(|p| (p, other.kind.as_deref())) != project
                    || other.status == StepStatus::NotAffected
            });
            if whole_unit {
                if collapsed != Some(project) {
                    let place = match project {
                        None => "the repo root's own steps".to_owned(),
                        Some((folder, Some(kind))) if folder == "." => {
                            format!("in {} ({kind})", place_name(folder))
                        }
                        Some((folder, _)) => format!("in {}", place_name(folder)),
                    };
                    out.push_str(&format!("  {place}: not affected\n"));
                    collapsed = Some(project);
                }
                current = project;
                continue;
            }
        }
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
            if step.cache.is_some() {
                "cached"
            } else {
                step.status.label()
            },
            match step.label {
                Some(label) => format!("[{}] {}", label.as_str(), step.detail),
                None => step.detail.clone(),
            }
        ));
    }
    let base_short = report
        .base
        .as_ref()
        .and_then(|base| base.commit.as_deref())
        .map(|commit| commit[..commit.len().min(7)].to_owned())
        .unwrap_or_default();
    for failure in report.steps.iter().filter(|step| blocks(step.status)) {
        let place = failure
            .project
            .as_ref()
            .map_or_else(String::new, |project| {
                format!(" in {}", place_name(project))
            });
        match failure.label {
            Some(crate::blame::Label::PreExisting) => {
                let how = match failure.base.as_ref().map(|base| base.source) {
                    Some(crate::blame::BaseSource::SameInputs) => {
                        " (no file it reads differs from the base, so it was not run there again)"
                    }
                    _ => ", with the same output",
                };
                out.push_str(&format!(
                    "\nPre-existing: {}{place} fails on the base commit {base_short} too{how}, so it does not block this change.\n",
                    failure.name
                ));
                continue;
            }
            Some(crate::blame::Label::New) => {
                let new_tests: Vec<&str> = failure
                    .failed_tests
                    .iter()
                    .filter(|test| test.label == Some(crate::blame::Label::New))
                    .map(|test| test.id.as_str())
                    .collect();
                let old = failure.failed_tests.len() - new_tests.len();
                if new_tests.is_empty()
                    || failure
                        .base
                        .as_ref()
                        .is_some_and(|base| base.status == crate::blame::BaseStatus::Passed)
                {
                    out.push_str(&format!(
                        "\nNew: {}{place} passes on the base commit {base_short}, so this change broke it.\n",
                        failure.name
                    ));
                } else {
                    out.push_str(&format!(
                        "\nNew: {}{place} also fails on the base commit {base_short}, but {} of its failing tests {} not fail there, so this change broke {}{}.\n",
                        failure.name,
                        new_tests.len(),
                        if new_tests.len() == 1 { "does" } else { "do" },
                        if new_tests.len() == 1 { "it" } else { "them" },
                        if old == 0 { String::new() } else { format!(" ({old} other{} already failed there)", if old == 1 { "" } else { "s" }) }
                    ));
                }
                if !new_tests.is_empty() {
                    out.push_str("New failing tests:\n");
                    for test in &new_tests {
                        out.push_str(&format!("  {test}\n"));
                    }
                }
            }
            _ => {}
        }
        if !failure.output_tail.is_empty() {
            out.push_str(&format!("\nLast lines of {}{place}:\n", failure.name));
            for line in &failure.output_tail {
                out.push_str(&format!("  {line}\n"));
            }
        }
        if !failure.first_error.is_empty() {
            let how = failure
                .command
                .as_ref()
                .map_or_else(String::new, |command| {
                    format!(" (from one more run, `{command} --maxfail=1`)")
                });
            out.push_str(&format!("\nFirst error of {}{place}{how}:\n", failure.name));
            for line in &failure.first_error {
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
            let erroring = failure
                .tests
                .filter(|tests| tests.errors > 0 && tests.passed < tests.errors);
            if let Some(tests) = erroring {
                // Mostly errors is not slowness: a longer limit would not help.
                out.push_str(&format!(
                    "Its counts ({} passed, {} failed, {} errors) show tests erroring, not running slowly, so a longer time limit would not help. Look at the first error{}.\n",
                    tests.passed,
                    tests.failed,
                    tests.errors,
                    if failure.first_error.is_empty() { " in the output" } else { " above" }
                ));
            } else if let Some(key) = &failure.timeout_key {
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
    let fixed: Vec<String> = report
        .steps
        .iter()
        .filter(|step| step.label == Some(crate::blame::Label::Fixed))
        .map(step_place)
        .collect();
    if !fixed.is_empty() {
        out.push_str(&format!(
            "\nFixed by this change (it fails on the base commit {base_short}): {}\n",
            fixed.join(", ")
        ));
    }
    if let Some(base) = &report.base {
        if base.compared {
            let mut how = Vec::new();
            if base.same_inputs > 0 {
                how.push(format!(
                    "{} with the same inputs as on the head (not run there)",
                    plural_steps(base.same_inputs)
                ));
            }
            if base.from_cache > 0 {
                how.push(format!(
                    "{} from an earlier base run",
                    plural_steps(base.from_cache)
                ));
            }
            if base.ran > 0 {
                how.push(format!(
                    "{} run in a worktree of it",
                    plural_steps(base.ran)
                ));
            }
            if !how.is_empty() || base.over_budget {
                out.push_str(&format!(
                    "\nCompared with the base commit {base_short} ({}): {}.{}\n",
                    base.reference.as_deref().unwrap_or("?"),
                    if how.is_empty() { "nothing to compare".to_owned() } else { how.join(", ") },
                    if base.over_budget {
                        " The base run ran out of its time budget (verify.baseBudget); failures it did not reach block as they would without a base."
                    } else {
                        ""
                    }
                ));
            }
        } else if report.steps.iter().any(|step| blocks(step.status)) {
            if let Some(reason) = &base.reason {
                out.push_str(&format!(
                    "\nNo base commit to compare with ({reason}), so every failure blocks.\n"
                ));
            }
        }
    }
    if let Some(cache) = report.cache.as_ref().filter(|cache| cache.hits > 0) {
        out.push_str(&format!(
            "\nFrom the cache: {} passed before on the same inputs, so {} not run again; {} took {} when {} last ran. Run fscale verify --no-cache to run every step.\n",
            plural_steps(cache.hits),
            if cache.hits == 1 { "it was" } else { "they were" },
            if cache.hits == 1 { "it" } else { "they" },
            format_duration(Duration::from_millis(cache.saved_ms)),
            if cache.hits == 1 { "it" } else { "they" },
        ));
    }
    if report.result == VerifyResult::Stopped {
        out.push_str("\nStopped: verify was interrupted (Ctrl+C or a termination signal), and every command it had started was stopped. Steps marked STOPPED or not run did not finish; run fscale verify again to check them.\n");
    }
    if let Some(affected) = &report.affected {
        out.push_str(&format!("\n{}\n", affected_intro(affected)));
        for unit in affected.projects.iter().filter(|unit| unit.affected) {
            let place = match (&unit.project, &unit.kind) {
                (None, _) => "the repo root's own steps".to_owned(),
                (Some(folder), Some(kind)) if folder == "." => {
                    format!("{} ({kind})", place_name(folder))
                }
                (Some(folder), _) => folder.clone(),
            };
            out.push_str(&format!("  {place}: {}\n", unit.reason));
        }
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
            violations: Vec::new(),
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
            violations: Vec::new(),
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
                                ..VerifyPlan::default()
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
        assert!(json.contains("\"schemaVersion\": 6"), "{json}");
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

    /// Runs the watch twice, as if the confirming probe came later.
    fn early_in(fixture: &Fixture, activity: &Activity) -> Option<(String, String)> {
        let mut early = Early::new(Duration::ZERO);
        let first =
            service_down_while_running(activity, &mut early, fixture.root(), fixture.root());
        assert_eq!(
            first.map(|(service, _, _)| service),
            None,
            "never judged on one probe"
        );
        early.last_probe = None;
        service_down_while_running(activity, &mut early, fixture.root(), fixture.root())
            .map(|(service, evidence, _)| (service, evidence))
    }

    #[test]
    fn a_service_that_comes_up_before_the_second_probe_is_not_down() {
        // A step that starts or waits for its own database (ADR 0025).
        let port = closed_port();
        let fixture = Fixture::new(
            &[(
                ".env.example",
                &format!("DATABASE_URL=postgresql://u:p@localhost:{port}/db\n"),
            )],
            Track::NoGit,
        )
        .unwrap();
        let activity = fed(
            "node test.js",
            &format!("Error: connect ECONNREFUSED 127.0.0.1:{port}\n"),
        );
        let mut early = Early::new(Duration::ZERO);
        assert!(
            service_down_while_running(&activity, &mut early, fixture.root(), fixture.root())
                .is_none()
        );
        assert!(early.suspect.is_some());
        let listener = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
        early.last_probe = None;
        assert!(
            service_down_while_running(&activity, &mut early, fixture.root(), fixture.root())
                .is_none()
        );
        assert!(early.suspect.is_none());
        drop(listener);
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
    fn a_declared_service_that_is_down_is_named_only_when_the_output_names_it() {
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
        // Down, but the output never mentions it: not the cause (ADR 0035).
        assert_eq!(
            early_in(&fixture, &fed("python -m pytest", "tests/test_a.py EEEE")),
            None
        );
        // A test file's name in a progress line is not evidence (review of
        // 0.2.1): only a line that says a connection failed.
        assert_eq!(
            early_in(
                &fixture,
                &fed("python -m pytest", "tests/test_postgres_repo.py EEEE")
            ),
            None
        );
        let named = format!("asyncpg: could not connect to localhost:{port}\ntests/test_a.py EEEE");
        let (service, evidence) =
            early_in(&fixture, &fed("python -m pytest", &named)).expect("seen");
        assert_eq!(service, format!("PostgreSQL on localhost:{port}"));
        assert_eq!(
            evidence,
            "every test so far has errored (0 passed, 0 failed, 4 errors), the output names it, and .env.example (POSTGRES_SERVER) lists it"
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
                "echo connect ECONNREFUSED 127.0.0.1:{port} & ping -n 21 127.0.0.1 > NUL & exit 1"
            )
        } else {
            format!("echo 'connect ECONNREFUSED 127.0.0.1:{port}'; sleep 20; exit 1")
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
            // Judged after the confirming probe, before the step's own end.
            started.elapsed() >= CONFIRM_AFTER && started.elapsed() < Duration::from_secs(19),
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

    // ------------------------------------------------- --affected (M17)

    mod affected_runs {
        use super::*;
        use crate::affected::Changed;
        use crate::facts::detect_facts_with;
        use crate::graph::{affected, build_graph};
        use crate::testing::{course_project, COURSE_SERVICES, COURSE_SHARED_USERS};

        /// The units of the course project's plan that a change runs.
        fn running(files: &[&str]) -> Vec<String> {
            let fixture = course_project().unwrap();
            let facts = detect_facts_with(fixture.root(), None).unwrap();
            let graph = build_graph(fixture.root(), &facts);
            let mut plan =
                plan_verify(fixture.root(), &facts, &VerifyConfig::default(), true, None);
            let changed = Changed::Files(files.iter().map(|file| (*file).to_owned()).collect());
            limit_to_affected(&mut plan, &facts, &graph, &affected(&graph, &changed), None);
            let units = &plan.affected.as_ref().unwrap().units;
            assert_eq!(units.len(), 6, "five services and the frontend: {units:?}");
            units
                .iter()
                .filter(|unit| unit.affected)
                .map(|unit| unit.project.clone().unwrap_or_default())
                .collect()
        }

        fn services(names: &[&str]) -> Vec<String> {
            names.iter().map(|name| format!("backend/{name}")).collect()
        }

        #[test]
        fn a_change_in_the_shared_folder_runs_every_service_that_imports_it() {
            assert_eq!(
                running(&["backend/shared/db.py"]),
                services(&COURSE_SHARED_USERS)
            );
        }

        #[test]
        fn a_change_in_one_service_runs_only_that_service() {
            for service in COURSE_SERVICES {
                let file = format!("backend/{service}/unit_test/conftest.py");
                assert_eq!(running(&[&file]), services(&[service]));
            }
        }

        #[test]
        fn a_frontend_change_runs_only_the_frontend() {
            assert_eq!(running(&["frontend/src/main.ts"]), ["frontend"]);
        }

        #[test]
        fn a_change_to_the_root_compose_file_runs_everything() {
            assert_eq!(running(&["docker-compose.yml"]).len(), 6);
        }

        #[test]
        fn a_cargo_member_under_a_package_json_root_runs_the_root_s_steps() {
            let fixture = Fixture::new(
                &[
                    (
                        "package.json",
                        r#"{"name":"mono","scripts":{"test":"cargo test"}}"#,
                    ),
                    ("pnpm-workspace.yaml", "packages:\n  - 'packages/*'\n"),
                    ("packages/ui/package.json", r#"{"name":"ui"}"#),
                    ("Cargo.toml", "[workspace]\nmembers = [\"crates/a\"]\n"),
                    ("crates/a/Cargo.toml", "[package]\nname = \"a\"\n"),
                    ("crates/a/src/lib.rs", ""),
                ],
                Track::NoGit,
            )
            .unwrap();
            let facts = detect_facts_with(fixture.root(), None).unwrap();
            let graph = build_graph(fixture.root(), &facts);
            let mut plan =
                plan_verify(fixture.root(), &facts, &VerifyConfig::default(), true, None);
            let changed = Changed::Files(["crates/a/src/lib.rs".to_owned()].into_iter().collect());
            limit_to_affected(&mut plan, &facts, &graph, &affected(&graph, &changed), None);
            let root = &plan.affected.as_ref().unwrap().units[0];
            assert!(root.project.is_none() && root.affected, "{root:?}");
        }

        fn two_projects(first_runs: bool) -> VerifyPlan {
            let project = |root: &str| ProjectPlan {
                root: root.to_owned(),
                kind: Some("python".to_owned()),
                steps: vec![
                    ("lint".to_owned(), Planned::Skipped("no linter".to_owned())),
                    ("test".to_owned(), Planned::Command("echo fine".to_owned())),
                ],
                install: None,
                python_from_path: false,
            };
            let unit = |root: &str, affected: bool| AffectedUnit {
                project: Some(root.to_owned()),
                kind: Some("python".to_owned()),
                affected,
                reason: if affected {
                    "changed: a/x.py".to_owned()
                } else {
                    "no change reaches this project".to_owned()
                },
            };
            VerifyPlan {
                runner: "projects".to_owned(),
                projects: vec![project("a"), project("b")],
                affected: Some(AffectedPlan {
                    changed_files: 1,
                    units: vec![unit("a", first_runs), unit("b", false)],
                    ..AffectedPlan::default()
                }),
                ..VerifyPlan::default()
            }
        }

        #[test]
        fn left_out_projects_are_not_affected_and_the_headline_counts_projects() {
            let fixture = Fixture::new(&[("a/x.py", ""), ("b/y.py", "")], Track::NoGit).unwrap();
            let report = run_verify(
                fixture.root(),
                "0.0.0",
                passing_check(),
                &two_projects(true),
            )
            .unwrap();
            let text = format_verify_human(&report);
            assert!(
                text.starts_with("verify: PARTIAL (affected only) - 1 of 2 projects ran, 1 not affected; 2 of 3 steps ran, 1 skipped\n"),
                "{text}"
            );
            assert!(text.contains("  in b: not affected\n"), "{text}");
            assert!(text.contains("  a: changed: a/x.py\n"), "{text}");
            let b: Vec<StepStatus> = report
                .steps
                .iter()
                .filter(|step| step.project.as_deref() == Some("b"))
                .map(|step| step.status)
                .collect();
            assert_eq!(b, [StepStatus::NotAffected, StepStatus::NotAffected]);
            // Not affected is not skipped: no warning names b.
            assert_eq!(report.skipped.len(), 1);
            assert!(
                !report.warnings[0].contains(" in b"),
                "{:?}",
                report.warnings
            );
            assert_eq!(report.scope, Scope::Affected);
            let json = format_verify_json(&report);
            assert!(json.contains("\"status\": \"not-affected\""), "{json}");
            assert!(json.contains("\"scope\": \"affected\""), "{json}");
            assert!(json.contains("\"projectsRun\": 1"), "{json}");
        }

        #[test]
        fn an_affected_run_never_reads_as_a_plain_ok() {
            let fixture = Fixture::new(&[("a/x.py", ""), ("b/y.py", "")], Track::NoGit).unwrap();
            for first_runs in [true, false] {
                let mut plan = two_projects(first_runs);
                for project in &mut plan.projects {
                    project.steps.remove(0);
                }
                let report = run_verify(fixture.root(), "0.0.0", passing_check(), &plan).unwrap();
                let text = format_verify_human(&report);
                let first = text.lines().next().unwrap_or_default();
                assert_eq!(report.result, VerifyResult::Ok);
                assert_ne!(first, "verify: OK", "{text}");
                let ran = usize::from(first_runs);
                assert_eq!(
                    first,
                    format!(
                        "verify: OK (affected only) - {ran} of 2 projects ran, {} not affected",
                        2 - ran
                    )
                );
            }
        }

        #[test]
        fn a_failure_in_an_affected_run_says_what_ran() {
            let fixture = Fixture::new(&[("a/x.py", ""), ("b/y.py", "")], Track::NoGit).unwrap();
            let mut plan = two_projects(true);
            let fail = if cfg!(windows) { "exit /b 3" } else { "exit 3" };
            plan.projects[0].steps = vec![("test".to_owned(), Planned::Command(fail.to_owned()))];
            let report = run_verify(fixture.root(), "0.0.0", passing_check(), &plan).unwrap();
            let text = format_verify_human(&report);
            assert!(
                text.starts_with("verify: FAILED at test in a (affected only: 1 of 2 projects ran, 1 not affected)\n"),
                "{text}"
            );
        }

        #[test]
        fn the_dry_run_lists_what_would_run_and_why() {
            let text = format_verify_plan(&two_projects(true), "fscale check --affected");
            assert!(
                text.starts_with("verify --affected (dry run): 1 of 2 projects would run, 1 not affected. Nothing was run.\n"),
                "{text}"
            );
            assert!(
                text.contains("    test       would run  echo fine\n"),
                "{text}"
            );
            assert!(text.contains("  in b: not affected\n"), "{text}");
        }
    }

    // ------------------------------------------- setup in tests (ADR 0035)

    mod first_errors {
        use super::*;

        const PYTEST: &str = "==== test session starts ====\ntests/test_a.py EE\n\n==== ERRORS ====\n____ ERROR at setup of test_x ____\n    def fixture():\n>       connect()\nE   ConnectionRefusedError: [Errno 111] Connect call failed ('127.0.0.1', 5433)\nE   with postgresql://app:Sup3rSecret@localhost:5433/db\n\ntests/conftest.py:9: ConnectionRefusedError\n==== short test summary info ====\nERROR tests/test_a.py::test_x - ConnectionRefusedError: [Errno 111]\nERROR tests/test_a.py::test_y - ConnectionRefusedError: [Errno 111]\n==== 2 errors in 0.1s ====\n";

        #[test]
        fn the_first_error_is_its_e_lines_and_first_summary_line_without_secrets() {
            let lines = first_error_lines(PYTEST);
            assert_eq!(
                lines,
                [
                    "E   ConnectionRefusedError: [Errno 111] Connect call failed ('127.0.0.1', 5433)",
                    "E   with postgresql://***@localhost:5433/db",
                    "ERROR tests/test_a.py::test_x - ConnectionRefusedError: [Errno 111]",
                ]
            );
            assert!(!lines.join("\n").contains("Sup3rSecret"));
        }

        #[test]
        fn a_failed_login_s_user_name_is_never_shown() {
            let text = "E   asyncpg.exceptions.InvalidPasswordError: password authentication failed for user \"course_app\"\nE   Access denied for user 'root'@'localhost' (using password: YES)\n";
            let lines = first_error_lines(text);
            assert_eq!(
                lines,
                [
                    "E   asyncpg.exceptions.InvalidPasswordError: password authentication failed for user \"***\"",
                    "E   Access denied for user '***'@'localhost' (using password: YES)",
                ]
            );
        }

        #[test]
        fn without_e_lines_the_last_lines_stand_in() {
            let text = (1..=15)
                .map(|n| format!("line {n}"))
                .collect::<Vec<_>>()
                .join("\n");
            let lines = first_error_lines(&text);
            assert_eq!(lines.len(), 10);
            assert_eq!(lines[0], "line 6");
        }

        #[test]
        fn summary_lines_are_read_only_from_the_short_summary() {
            let text = "ERROR not a summary\n==== short test summary info ====\nFAILED a::b - assert 1\nERROR a::c - KeyError\n==== 1 failed ====\nERROR after\n";
            assert_eq!(
                summary_lines(text),
                ["FAILED a::b - assert 1", "ERROR a::c - KeyError"]
            );
            assert!(summary_lines("ERROR a::c - KeyError\n").is_empty());
        }

        fn service(name: &str, port: u16) -> Service {
            Service {
                name: name.to_owned(),
                host: "localhost".to_owned(),
                port,
                source: ".env.example (X)".to_owned(),
                start: None,
            }
        }

        #[test]
        fn a_service_is_named_by_its_address_its_name_or_its_client() {
            let redis = service("Redis", 6379);
            let postgres = service("PostgreSQL", 5433);
            assert!(output_names("cannot reach the test redis", &redis));
            assert!(output_names(
                "error 111 connecting to localhost:6379.",
                &redis
            ));
            assert!(output_names(
                "asyncpg.exceptions.toomanyconnections",
                &postgres
            ));
            assert!(output_names(
                "connect call failed ('127.0.0.1', 5433)",
                &postgres
            ));
            assert!(!output_names("keyerror: 'total'", &redis));
            assert!(!output_names("assert 6379 == 1", &postgres));
        }

        #[test]
        fn errors_that_name_no_service_or_a_failure_are_not_setup() {
            let fixture = Fixture::new(&[], Track::NoGit).unwrap();
            let root = fixture.root();
            let check =
                |text: &str, failed: u64| errors_name_unreachable_service(text, failed, root, root);
            let failed = "==== short test summary info ====\nFAILED a::b - assert 1\nERROR a::c - Cannot reach the test Redis at localhost:1\n";
            assert!(check(failed, 0).is_none());
            let plain = "==== short test summary info ====\nERROR a::c - KeyError: 'x'\n";
            assert!(check(plain, 0).is_none());
            let mixed = "==== short test summary info ====\nERROR a::c - Cannot reach the test Redis at localhost:1\nERROR a::d - KeyError: 'x'\n";
            assert!(check(mixed, 0).is_none());
            let redis = "==== short test summary info ====\nERROR a::c - Cannot reach the test Redis at localhost:1\nERROR a::d - redis.exceptions.ConnectionError: Error 111 connecting to localhost:1\n";
            let problem = check(redis, 0).unwrap();
            assert!(
                problem
                    .reason
                    .contains("every error (2) says Redis on localhost:1 cannot be reached"),
                "{}",
                problem.reason
            );
            // From the review of 0.2.1: failures the runner counted (with
            // `-rE`, pytest prints no FAILED line), a client name inside a
            // test's name, and an error about what the database returned.
            assert!(check(redis, 1).is_none());
            let names = "==== short test summary info ====\nERROR tests/test_redis_cache.py::test_redistribute_credits - KeyError: 'plan'\n";
            assert!(check(names, 0).is_none());
            let table = "==== short test summary info ====\nERROR tests/a.py::t - (psycopg.errors.UndefinedTable) relation \"orders\" does not exist\n";
            assert!(check(table, 0).is_none());
        }

        #[test]
        fn a_service_is_named_only_by_whole_words_and_exact_ports() {
            let redis = service("Redis", 6379);
            let postgres = service("PostgreSQL", 5432);
            assert!(!output_names("test_redistribute_credits failed", &redis));
            assert!(!output_names(
                "connect to localhost:54321 refused",
                &postgres
            ));
            assert!(!output_names("see report 6379 for details", &redis));
            assert!(output_names(
                "psycopg2.operationalerror: could not connect",
                &postgres
            ));
        }

        #[test]
        fn only_a_plain_pytest_command_gets_maxfail() {
            assert!(takes_maxfail("python -m pytest"));
            assert!(takes_maxfail(".venv/bin/python -m pytest -q \"tests/a b\""));
            assert!(takes_maxfail("uv run pytest"));
            for command in [
                "python -m pytest 2>&1 | tee run.log",
                "pytest && echo ok",
                "pytest > out.log",
                "make test",
                "tox",
                "pytest $(cat args)",
            ] {
                assert!(!takes_maxfail(command), "{command}");
            }
        }

        #[test]
        fn secrets_are_hidden_in_every_form_the_review_found() {
            for (line, secret) in [
                ("postgresql://app:pa/ss@localhost:5433/db", "pa/ss"),
                (
                    "host=localhost user=app password=Hunter2 dbname=x",
                    "Hunter2",
                ),
                ("role \"course_app\" does not exist", "course_app"),
                (
                    "credentials for `course_app` are not valid for user `course_app`",
                    "course_app",
                ),
                (
                    "FATAL: password authentication failed for user \"course_app\"",
                    "course_app",
                ),
            ] {
                let hidden = hide_secrets(line);
                assert!(!hidden.contains(secret), "{line} -> {hidden}");
            }
            assert_eq!(hide_secrets("plain text"), "plain text");
        }

        #[test]
        fn stop_after_errors_has_a_default_and_can_be_turned_off() {
            let mut config = VerifyConfig::default();
            assert_eq!(
                config.errors_before_stop(),
                Some(DEFAULT_ERRORS_BEFORE_STOP)
            );
            config.stop_after_errors = Some(25);
            assert_eq!(config.errors_before_stop(), Some(25));
            config.stop_after_errors = Some(0);
            assert_eq!(config.errors_before_stop(), None);
        }
    }
}
