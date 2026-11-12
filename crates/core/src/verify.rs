//! `fscale verify`: conformance, then lint, typecheck and tests (SPEC §8).
//!
//! The output is one compact summary written for an agent to act on: what
//! failed, the end of its output, the command to run again, and every step
//! that did not run. A run with skipped steps is never reported as a plain
//! pass (ADR 0020).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use serde::Serialize;

use crate::error::{Error, Result};
use crate::facts::{source, Project, RepoFacts};

/// Version of the `fscale verify --json` format.
pub const VERIFY_SCHEMA_VERSION: u32 = 2;

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
                        None => project_step(project, step),
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
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StepStatus {
    Passed,
    Failed,
    /// No command was found for the step, so nothing was verified.
    Skipped,
    /// An earlier step failed, so this one did not run.
    NotRun,
    /// Not run on purpose: turned off in the config, or covered by another step.
    Off,
}

impl StepStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "FAILED",
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

/// Runs one command through the platform shell, in `dir`, and reports how it went.
fn run_step(
    dir: &Path,
    name: &str,
    project: Option<&ProjectPlan>,
    command: &str,
) -> Result<StepReport> {
    let kind = project.and_then(|project| project.kind.clone());
    let project = project.map(|project| project.root.as_str());
    let started = Instant::now();
    let output = shell(command)
        .current_dir(dir)
        .stdin(Stdio::null())
        .env("NO_COLOR", "1")
        .env("FORCE_COLOR", "0")
        .output()
        .map_err(|error| {
            let key = project.map_or_else(
                || format!("verify.{name}"),
                |project| format!("verify.projects[\"{project}\"].{name}"),
            );
            Error::user(format!(
                "Could not start the {name} command `{command}` ({error}). Check the command, or set {key} in your firescale config."
            ))
        })?;
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let code = output.status.code();
    let passed = output.status.success();
    Ok(StepReport {
        name: name.to_owned(),
        project: project.map(str::to_owned),
        kind,
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

/// Runs the plan after the check step: the root's steps, then each project's
/// steps in its own folder. Stops at the first failure: the steps after it
/// are reported as not run.
pub fn run_verify(
    root: &Path,
    version: &str,
    check: CheckStep,
    plan: &VerifyPlan,
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
    }];
    let mut failed = !check.ok;

    let root_steps = plan.steps.iter().map(|step| (None, step));
    let project_steps = plan
        .projects
        .iter()
        .flat_map(|project| project.steps.iter().map(move |step| (Some(project), step)));
    for (project, (name, planned)) in root_steps.chain(project_steps) {
        let idle = |status: StepStatus, command: Option<&String>, detail: &str| StepReport {
            name: name.clone(),
            project: project.map(|project| project.root.clone()),
            kind: project.and_then(|project| project.kind.clone()),
            status,
            command: command.cloned(),
            exit_code: None,
            duration_ms: 0,
            detail: detail.to_owned(),
            output_tail: Vec::new(),
        };
        let report = match planned {
            Planned::Skipped(reason) => idle(StepStatus::Skipped, None, reason),
            Planned::NotNeeded(reason) => idle(StepStatus::Off, None, reason),
            Planned::Command(command) if failed => {
                idle(StepStatus::NotRun, Some(command), "an earlier step failed")
            }
            Planned::Command(command) => {
                let dir =
                    project.map_or_else(|| root.to_path_buf(), |p| project_dir(root, &p.root));
                run_step(&dir, name, project, command)?
            }
        };
        failed = failed || report.status == StepStatus::Failed;
        steps.push(report);
    }

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
    let steps_run = steps
        .iter()
        .filter(|step| matches!(step.status, StepStatus::Passed | StepStatus::Failed))
        .count();
    let first_failure = steps.iter().find(|step| step.status == StepStatus::Failed);
    let result = if first_failure.is_some() || (plan.fail_on_skipped && !skipped.is_empty()) {
        VerifyResult::Failed
    } else if skipped.is_empty() {
        VerifyResult::Ok
    } else {
        VerifyResult::Partial
    };
    Ok(VerifyReport {
        schema_version: VERIFY_SCHEMA_VERSION,
        version: version.to_owned(),
        ok: result != VerifyResult::Failed,
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
    })
}

/// The first line of the summary. It reads as a clean pass only when every
/// step ran and passed.
fn headline(report: &VerifyReport) -> String {
    if let Some(step) = &report.failed_step {
        return match &report.failed_project {
            Some(project) => format!("verify: FAILED at {step} in {}", place_name(project)),
            None => format!("verify: FAILED at {step}"),
        };
    }
    match report.result {
        VerifyResult::Ok => "verify: OK".to_owned(),
        VerifyResult::Partial => format!(
            "verify: PARTIAL - {} of {} ran, {} skipped",
            report.steps_run,
            plural_steps(report.steps_total),
            report.skipped.len()
        ),
        VerifyResult::Failed => format!(
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
    if let Some(failure) = report
        .steps
        .iter()
        .find(|step| step.status == StepStatus::Failed)
    {
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
        if let Some(command) = &report.rerun {
            out.push_str(&format!(
                "\nFix the failure, then run again{place}: {command}\n"
            ));
        }
    }
    if !report.warnings.is_empty() {
        out.push('\n');
        for warning in &report.warnings {
            out.push_str(&format!("warning: {warning}\n"));
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
                                }],
                                fail_on_skipped,
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
        let facts = crate::facts::detect_facts(fixture.root()).unwrap();
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
                "frontend test: skipped (package.json has no test script, and no vitest or jest config was found)",
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
        let facts = crate::facts::detect_facts(fixture.root()).unwrap();
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
        let facts = crate::facts::detect_facts(fixture.root()).unwrap();
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
        let facts = crate::facts::detect_facts(fixture.root()).unwrap();
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
                },
                ProjectPlan {
                    root: "two/deep".to_owned(),
                    kind: None,
                    steps: vec![
                        ("lint".to_owned(), Planned::Command(fail.to_owned())),
                        ("test".to_owned(), Planned::Command(show.to_owned())),
                    ],
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
        let facts = crate::facts::detect_facts(fixture.root()).unwrap();
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
}
