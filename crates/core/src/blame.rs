//! Blame-aware verify (M19, ADR 0037): each failure is compared with the
//! base commit, and only a failure the change introduced blocks.
//!
//! - The **head** is the working tree, uncommitted changes included. The
//!   **base** is the merge base of `HEAD` and `--base <ref>`, or of the
//!   default branch.
//! - With a base, every step runs (an old failure cannot hide a new one).
//! - A step's result on the base comes, cheapest first, from: the step's
//!   own key being the same on both (no file it reads changed, so its
//!   result on the base is its result on the head), the base-result store
//!   in `.firescale/base/`, or a run in a temporary git worktree of the base
//!   commit, within a time budget. The user's working tree and index are
//!   never touched.
//! - Labels: `new` (fails on the head, passed on the base), `pre-existing`
//!   (fails on both), `fixed` (failed on the base, passes on the head).
//!   Only `new` failures block. A base result that needed setup, timed out,
//!   ran over the budget or did not run gives no label: the failure blocks,
//!   as it would without a base.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::affected::{changed_files, Changed};
use crate::error::{Error, Result};
use crate::git::run_git;
use crate::progress::{format_duration, format_limit};
use crate::stepcache::{now_secs, StepCache};
use crate::verify::{
    run_base_step, settle, Limit, ProjectPlan, RunOptions, StepReport, StepStatus, VerifyPlan,
    VerifyReport, DEFAULT_BASE_BUDGET,
};

/// How a failure compares with the base commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Label {
    /// Fails on the head, passed on the base: the change broke it. Blocks.
    New,
    /// Fails on the head and on the base. Does not block.
    PreExisting,
    /// Failed on the base, passes on the head.
    Fixed,
    /// Failed on the head, and passed when run again (M21). Does not block.
    Flaky,
}

impl Label {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::PreExisting => "pre-existing",
            Self::Fixed => "fixed",
            Self::Flaky => "flaky",
        }
    }
}

/// A step's result on the base commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BaseStatus {
    Passed,
    Failed,
    SetupNeeded,
    TimedOut,
    /// The base run's time budget ran out first.
    OverBudget,
    /// Verify was stopped first.
    Stopped,
}

/// Where a base result came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BaseSource {
    /// No file the step reads differs between the base and the head, so
    /// its result on the head is its result on the base.
    SameInputs,
    /// The base-result store, from an earlier run with the same base.
    Cache,
    /// Run now, in a worktree of the base commit.
    Ran,
}

/// How a step did on the base commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BaseOutcome {
    pub status: BaseStatus,
    pub source: BaseSource,
    /// For a step the base run could not judge: why its failure blocks as
    /// it would without a base.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The `base` object of `fscale verify --json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BaseReport {
    /// True when failures were compared with a base commit.
    pub compared: bool,
    /// The ref the base came from (`main`, `origin/main`, or `--base`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// The base commit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// Without a base: why there is none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Steps whose base result was their head result (same inputs).
    pub same_inputs: usize,
    /// Steps whose base result came from the base-result store.
    pub from_cache: usize,
    /// Steps run in the base worktree.
    pub ran: usize,
    /// The base run's time budget, in milliseconds; absent with no limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_ms: Option<u64>,
    /// How long the base run took (worktree, install and steps).
    pub duration_ms: u64,
    /// True when the budget ran out before every base step had run.
    pub over_budget: bool,
}

/// The base commit to compare with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseCommit {
    /// The ref it came from.
    pub reference: String,
    /// The merge base, a full commit id.
    pub commit: String,
}

impl BaseCommit {
    #[must_use]
    pub fn short(&self) -> &str {
        &self.commit[..self.commit.len().min(7)]
    }
}

/// The base of a run, or why there is none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaseChoice {
    Compare(BaseCommit),
    Nothing(String),
}

fn git_line(root: &Path, args: &[&str]) -> Option<String> {
    let out = run_git(root, args).ok()?;
    (out.code == 0)
        .then(|| out.stdout.trim().to_owned())
        .filter(|line| !line.is_empty())
}

/// The default branch: `origin/HEAD`, then `origin/main`, `origin/master`,
/// `main`, `master`, whichever exists first.
fn default_branch(root: &Path) -> Option<String> {
    if let Some(head) = git_line(
        root,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    ) {
        return Some(head);
    }
    ["origin/main", "origin/master", "main", "master"]
        .into_iter()
        .find(|name| {
            git_line(
                root,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("{name}^{{commit}}"),
                ],
            )
            .is_some()
        })
        .map(str::to_owned)
}

/// Finds the base: the merge base of `HEAD` and `given`, or of the default
/// branch. There is none outside git, before the first commit, with no
/// default branch, or when the base is `HEAD` and the tree is clean.
///
/// # Errors
///
/// When `--base` names something git cannot compare with `HEAD`.
pub fn choose_base(root: &Path, given: Option<&str>) -> Result<BaseChoice> {
    let Some(head) = git_line(root, &["rev-parse", "--verify", "--quiet", "HEAD"]) else {
        return Ok(BaseChoice::Nothing(
            "this is not a git repo, or it has no commit yet".to_owned(),
        ));
    };
    let reference = match given {
        Some(given) => given.to_owned(),
        None => match default_branch(root) {
            Some(branch) => branch,
            None => {
                return Ok(BaseChoice::Nothing(
                    "no default branch was found (origin/HEAD, main or master); pass --base <ref> to compare with one".to_owned(),
                ))
            }
        },
    };
    let Some(commit) = git_line(root, &["merge-base", "HEAD", &reference]) else {
        if given.is_some() {
            return Err(Error::user(format!(
                "--base \"{reference}\" is not a commit git can compare with HEAD. Pass a branch, tag or commit that exists, for example --base origin/main."
            )));
        }
        return Ok(BaseChoice::Nothing(format!(
            "HEAD and {reference} have no commit in common"
        )));
    };
    let dirty = run_git(
        root,
        &[
            "status",
            "--porcelain",
            "--untracked-files=normal",
            "--",
            ".",
        ],
    )
    .map(|out| out.code != 0 || !out.stdout.trim().is_empty())
    .unwrap_or(true);
    if commit == head && !dirty {
        return Ok(BaseChoice::Nothing(format!(
            "the working tree is the base commit {} ({reference}) with no change, so there is nothing to compare",
            &commit[..commit.len().min(7)]
        )));
    }
    Ok(BaseChoice::Compare(BaseCommit { reference, commit }))
}

/// The files that differ between the base commit and the working tree.
///
/// # Errors
///
/// When git cannot compare them.
pub fn changed_since(root: &Path, base: &BaseCommit) -> Result<Option<BTreeSet<String>>> {
    Ok(match changed_files(root, Some(&base.commit))? {
        Changed::Everything => None,
        Changed::Files(files) => Some(files),
    })
}

// ------------------------------------------------------------ the store

const STORE_FORMAT: u32 = 2;

/// A base result kept for a later run with the same base.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredBase {
    format: u32,
    key: String,
    status: BaseStatus,
    ran_at: u64,
    duration_ms: u64,
}

fn store_dir(root: &Path) -> PathBuf {
    root.join(".firescale").join("base")
}

fn lookup(root: &Path, key: &str) -> Option<BaseStatus> {
    let text = fs::read_to_string(store_dir(root).join(format!("{key}.json"))).ok()?;
    let stored: StoredBase = serde_json::from_str(&text).ok()?;
    let fresh = now_secs().saturating_sub(stored.ran_at) <= crate::stepcache::MAX_AGE.as_secs();
    (stored.format == STORE_FORMAT && stored.key == key && fresh).then_some(stored.status)
}

/// Keeps a base result. Only passed and failed are kept: the others say
/// nothing about the code.
fn keep(root: &Path, key: &str, status: BaseStatus, duration_ms: u64) {
    if !matches!(status, BaseStatus::Passed | BaseStatus::Failed) {
        return;
    }
    let dir = store_dir(root);
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let stored = StoredBase {
        format: STORE_FORMAT,
        key: key.to_owned(),
        status,
        ran_at: now_secs(),
        duration_ms,
    };
    let Ok(text) = serde_json::to_string_pretty(&stored) else {
        return;
    };
    let temp = dir.join(format!(".{key}.{}.tmp", std::process::id()));
    if fs::write(&temp, text).is_ok() && fs::rename(&temp, dir.join(format!("{key}.json"))).is_err()
    {
        let _ = fs::remove_file(&temp);
    }
}

// ------------------------------------------------------------ the worktree

/// A temporary git worktree of the base commit, removed when dropped.
pub struct Worktree {
    repo: PathBuf,
    top: PathBuf,
    /// The links to the head's dependency folders, removed before the
    /// worktree so nothing can delete through them.
    links: std::sync::Mutex<Vec<PathBuf>>,
    /// The firescale root inside it.
    pub root: PathBuf,
}

impl Worktree {
    /// Adds a detached worktree of `commit`. Only `.git`'s worktree list
    /// changes in the user's repo; the working tree and index do not.
    ///
    /// # Errors
    ///
    /// When git cannot add it.
    pub fn add(root: &Path, commit: &str) -> std::result::Result<Self, String> {
        let prefix = git_line(root, &["rev-parse", "--show-prefix"]).unwrap_or_default();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.subsec_nanos());
        let top =
            std::env::temp_dir().join(format!("firescale-base-{}-{nanos}", std::process::id()));
        let path = top.to_string_lossy().into_owned();
        let out = run_git(
            root,
            &["worktree", "add", "--detach", "--quiet", &path, commit],
        )
        .map_err(|error| error.message().to_owned())?;
        if out.code != 0 {
            return Err(format!(
                "git worktree add failed: {}",
                out.stderr.lines().next().unwrap_or("").trim()
            ));
        }
        let inner = prefix
            .trim_end_matches('/')
            .split('/')
            .filter(|part| !part.is_empty())
            .fold(top.clone(), |path, part| path.join(part));
        Ok(Self {
            repo: root.to_path_buf(),
            top,
            root: inner,
            links: std::sync::Mutex::new(Vec::new()),
        })
    }

    /// Links the head's folder `target` in at `link`, and remembers it.
    fn link(&self, target: &Path, link: &Path) -> bool {
        let made = link_dir(target, link);
        if made {
            self.links
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(link.to_path_buf());
        }
        made
    }
}

impl Drop for Worktree {
    fn drop(&mut self) {
        for link in self
            .links
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
        {
            unlink_dir(link);
        }
        let path = self.top.to_string_lossy().into_owned();
        let removed = run_git(&self.repo, &["worktree", "remove", "--force", &path])
            .is_ok_and(|out| out.code == 0);
        if !removed {
            remove_tree(&self.top);
            let _ = run_git(&self.repo, &["worktree", "prune"]);
        }
    }
}

/// Removes a folder without following the links to the head's
/// dependency folders inside it.
fn remove_tree(path: &Path) {
    let _ = fs::remove_dir_all(path);
}

/// Links a dependency folder of the head into the worktree: a symbolic
/// link on Unix, a junction on Windows (no special rights needed).
fn link_dir(target: &Path, link: &Path) -> bool {
    if link.exists() || !target.is_dir() {
        return false;
    }
    if let Some(parent) = link.parent() {
        let _ = fs::create_dir_all(parent);
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
    #[cfg(windows)]
    {
        std::process::Command::new("cmd")
            .args(["/D", "/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
}

/// Removes a link made by [`link_dir`], never what it points to.
fn unlink_dir(link: &Path) {
    #[cfg(unix)]
    {
        let _ = fs::remove_file(link);
    }
    #[cfg(windows)]
    {
        // A junction is removed as an empty folder; its target is untouched.
        let _ = fs::remove_dir(link);
    }
}

/// The ignored dependency folders that are linked from the head.
const DEPENDENCY_FOLDERS: [&str; 3] = ["node_modules", ".venv", "venv"];

/// Every ignored dependency folder of the head (`node_modules`, `.venv`,
/// `venv`), wherever it is: the repo root, a project, a workspace package.
/// Not those inside another one.
fn dependency_folders(root: &Path) -> Vec<String> {
    let Ok(out) = run_git(
        root,
        &[
            "-c",
            "core.quotepath=off",
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
        ],
    ) else {
        return Vec::new();
    };
    let mut found: Vec<String> = out
        .stdout
        .split('\0')
        .filter_map(|path| path.strip_suffix('/'))
        .map(|path| path.replace('\\', "/"))
        .filter(|path| {
            let parts: Vec<&str> = path.split('/').collect();
            parts
                .last()
                .is_some_and(|last| DEPENDENCY_FOLDERS.contains(last))
                && !parts[..parts.len() - 1]
                    .iter()
                    .any(|part| DEPENDENCY_FOLDERS.contains(part))
        })
        .collect();
    found.sort();
    found
}

fn folder_of(root: &Path, folder: Option<&str>) -> PathBuf {
    match folder {
        None | Some(".") => root.to_path_buf(),
        Some(folder) => folder
            .split('/')
            .fold(root.to_path_buf(), |path, part| path.join(part)),
    }
}

// ------------------------------------------------------------ the run

/// What the base run needs from the caller.
pub struct BaseRun<'a> {
    pub root: &'a Path,
    pub base: &'a BaseCommit,
    /// Files that differ between the base and the head; `None` when git
    /// could not say, so no step counts as having the same inputs.
    pub changed: Option<&'a BTreeSet<String>>,
    pub cache: &'a StepCache<'a>,
    pub plan: &'a VerifyPlan,
    pub options: &'a RunOptions<'a>,
    /// Runs `check` in the base worktree and gives the identities of its
    /// enforced violations. Called only when the head's check failed.
    pub check: &'a dyn Fn(&Path) -> Result<Vec<String>>,
    /// The identities of the head's enforced violations.
    pub head_violations: &'a [String],
    /// When false, passing steps get no base run, so they cannot be
    /// labelled `fixed` unless their base result is already known (the
    /// stop hook, which only needs to judge failures).
    pub judge_passes: bool,
}

struct Pending {
    index: usize,
    key: Option<String>,
}

fn head_outcome(status: StepStatus) -> Option<BaseStatus> {
    match status {
        StepStatus::Passed => Some(BaseStatus::Passed),
        StepStatus::Failed => Some(BaseStatus::Failed),
        StepStatus::TimedOut => Some(BaseStatus::TimedOut),
        _ => None,
    }
}

fn place(step: &StepReport) -> String {
    match &step.project {
        Some(project) if project == "." => format!("{} in the repo root", step.name),
        Some(project) => format!("{} in {project}", step.name),
        None => step.name.clone(),
    }
}

fn plan_of<'a>(plan: &'a VerifyPlan, step: &StepReport) -> Option<&'a ProjectPlan> {
    let project = step.project.as_deref()?;
    plan.projects
        .iter()
        .find(|candidate| candidate.root == project && candidate.kind == step.kind)
}

/// Compares the head's report with the base commit and labels its steps,
/// then settles the result again: only `new` failures block.
///
/// # Errors
///
/// Only when a base step cannot be started at all.
pub fn compare(report: &mut VerifyReport, run: &BaseRun<'_>) -> Result<()> {
    let started = Instant::now();
    let budget = match run.options.config.base_budget {
        Some(Limit::Off) => None,
        Some(Limit::After(limit)) => Some(limit),
        None => Some(DEFAULT_BASE_BUDGET),
    };
    let mut summary = BaseReport {
        compared: true,
        reference: Some(run.base.reference.clone()),
        commit: Some(run.base.commit.clone()),
        budget_ms: budget.map(|limit| u64::try_from(limit.as_millis()).unwrap_or(u64::MAX)),
        ..BaseReport::default()
    };
    let say = |text: &str| {
        if let Some(progress) = run.options.progress {
            progress.line(&format!("base: {text}"));
        }
    };

    // What the base worktree gets: part of the key of a kept base result,
    // so a result from a worktree set up otherwise is never reused.
    let environment = match &run.options.config.base_install {
        Some(install) => format!("install={install}"),
        None => format!("links={}", dependency_folders(run.root).join(",")),
    };

    // 1. What each step's base result is, where it is known without a run.
    let mut to_run: Vec<Pending> = Vec::new();
    for index in 0..report.steps.len() {
        let step = &report.steps[index];
        if step.name == "check" {
            continue;
        }
        let Some(head) = head_outcome(step.status) else {
            continue;
        };
        let Some(command) = step.command.clone() else {
            continue;
        };
        let folder = step.project.as_deref();
        let kind = step.kind.as_deref();
        let dir = folder_of(run.root, folder);
        let same = run
            .changed
            .and_then(|changed| run.cache.same_on_base(folder, kind, &dir, changed));
        if same == Some(true) {
            report.steps[index].base = Some(BaseOutcome {
                status: head,
                source: BaseSource::SameInputs,
                note: None,
            });
            summary.same_inputs += 1;
            continue;
        }
        let key = run
            .cache
            .base_key(
                folder,
                kind,
                &step.name,
                &command,
                &dir,
                &run.base.commit,
                &environment,
            )
            .ok();
        if let Some(status) = key.as_deref().and_then(|key| lookup(run.root, key)) {
            report.steps[index].base = Some(BaseOutcome {
                status,
                source: BaseSource::Cache,
                note: None,
            });
            summary.from_cache += 1;
            continue;
        }
        if head == BaseStatus::Passed && !run.judge_passes {
            continue;
        }
        to_run.push(Pending { index, key });
    }
    // Failures first: they are what the labels are for. Passing steps run on
    // the base only when a worktree is needed anyway, so a run with no
    // failure costs no base run (ADR 0037).
    to_run.sort_by_key(|pending| report.steps[pending.index].status == StepStatus::Passed);
    let failing_left = to_run
        .iter()
        .any(|pending| report.steps[pending.index].status != StepStatus::Passed);
    let check_failed = report
        .steps
        .first()
        .is_some_and(|step| step.name == "check" && step.status == StepStatus::Failed);

    if !failing_left && !check_failed {
        to_run.clear();
    }

    // 2. The base run, in a worktree, when anything is left.
    if !to_run.is_empty() || check_failed {
        say(&format!(
            "comparing with {} ({}): {} to run in a worktree of the base commit",
            run.base.short(),
            run.base.reference,
            if to_run.len() == 1 {
                "1 step".to_owned()
            } else {
                format!("{} steps", to_run.len())
            }
        ));
        match Worktree::add(run.root, &run.base.commit) {
            Err(problem) => {
                report.notes.push(format!(
                    "the base commit {} could not be checked out in a worktree ({problem}), so failures block as they would without a base",
                    run.base.short()
                ));
                for pending in &to_run {
                    report.steps[pending.index].base = Some(BaseOutcome {
                        status: BaseStatus::SetupNeeded,
                        source: BaseSource::Ran,
                        note: Some(format!("the base worktree could not be made ({problem})")),
                    });
                }
            }
            Ok(worktree) => {
                run_in_worktree(
                    report,
                    run,
                    &worktree,
                    &to_run,
                    budget,
                    started,
                    &mut summary,
                    &say,
                )?;
            }
        }
    }

    // 3. Labels.
    let short = run.base.short().to_owned();
    let budget_text = budget.map_or_else(|| "no limit".to_owned(), format_limit);
    for step in &mut report.steps {
        let Some(outcome) = step.base.as_mut() else {
            continue;
        };
        let failed = matches!(step.status, StepStatus::Failed | StepStatus::TimedOut);
        step.label = match (failed, outcome.status) {
            (true, BaseStatus::Passed) => Some(Label::New),
            (true, BaseStatus::Failed) => Some(Label::PreExisting),
            (false, BaseStatus::Failed) => Some(Label::Fixed),
            _ => None,
        };
        if failed && step.label.is_none() {
            let why = match outcome.status {
                BaseStatus::SetupNeeded => format!("the base run of it on {short} needed setup"),
                BaseStatus::TimedOut => format!("it timed out on the base commit {short} too"),
                BaseStatus::OverBudget => format!(
                    "the base run ran out of its time budget ({budget_text}, verify.baseBudget) before it finished"
                ),
                BaseStatus::Stopped => "verify was stopped during the base run".to_owned(),
                BaseStatus::Passed | BaseStatus::Failed => String::new(),
            };
            if outcome.note.is_none() {
                outcome.note = Some(format!(
                    "{why}, so this failure blocks as it would without a base"
                ));
            } else if let Some(note) = outcome.note.as_mut() {
                note.push_str(", so this failure blocks as it would without a base");
            }
        }
    }
    // Failing steps that got no base result at all (the budget ran out
    // before they were reached) block too; say why.
    for step in &mut report.steps {
        if step.name != "check"
            && matches!(step.status, StepStatus::Failed | StepStatus::TimedOut)
            && step.base.is_none()
        {
            step.base = Some(BaseOutcome {
                status: BaseStatus::OverBudget,
                source: BaseSource::Ran,
                note: Some(format!(
                    "the base run ran out of its time budget ({budget_text}, verify.baseBudget) before it reached this step, so this failure blocks as it would without a base"
                )),
            });
            summary.over_budget = true;
        }
    }
    let notes: Vec<String> = report
        .steps
        .iter()
        .filter_map(|step| {
            step.base
                .as_ref()
                .and_then(|outcome| outcome.note.as_ref())
                .filter(|_| matches!(step.status, StepStatus::Failed | StepStatus::TimedOut))
                .map(|note| format!("{}: {note}.", place(step)))
        })
        .collect();
    report.notes.extend(notes);
    summary.duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    report.base = Some(summary);
    let stopped = report.result == crate::verify::VerifyResult::Stopped;
    settle(report, stopped);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_in_worktree(
    report: &mut VerifyReport,
    run: &BaseRun<'_>,
    worktree: &Worktree,
    to_run: &[Pending],
    budget: Option<Duration>,
    started: Instant,
    summary: &mut BaseReport,
    say: &dyn Fn(&str),
) -> Result<()> {
    let deadline = budget.map(|limit| started + limit);
    // The head's installed dependencies, linked in: no network, no install
    // into a shared interpreter (ADR 0037).
    // With verify.baseInstall the install provides them, and nothing is
    // linked: an install must never write through a link into the head.
    let links = if run.options.config.base_install.is_some() {
        Vec::new()
    } else {
        dependency_folders(run.root)
    };
    for folder in links {
        worktree.link(
            &folder_of(run.root, Some(&folder)),
            &folder_of(&worktree.root, Some(&folder)),
        );
    }
    let mut install_failed: Option<String> = None;
    // The head's dependencies stand in for the base's only when they are
    // the same: a changed lockfile or requirements list means the base
    // cannot be judged without an install.
    let changed_dependencies: Vec<&String> = run
        .changed
        .map(|changed| {
            changed
                .iter()
                .filter(|path| {
                    let name = path.rsplit('/').next().unwrap_or(path);
                    crate::facts::LOCKFILE_NAMES.contains(&name)
                        || (name.starts_with("requirements") && name.ends_with(".txt"))
                })
                .collect()
        })
        .unwrap_or_default();
    if run.options.config.base_install.is_none() {
        if let Some(first) = changed_dependencies.first() {
            install_failed = Some(format!(
                "{first} changed, so the head's installed dependencies do not stand for the base's; set verify.baseInstall to a command that installs them in the base worktree"
            ));
        }
    }
    if let Some(install) = run
        .options
        .config
        .base_install
        .as_ref()
        .filter(|_| install_failed.is_none())
    {
        say(&format!("installing: {install}"));
        let installed = run_base_step(
            &worktree.root,
            None,
            "install",
            install,
            run.options,
            deadline,
        )?;
        if installed.status != StepStatus::Passed {
            install_failed = Some(format!(
                "verify.baseInstall (`{install}`) did not succeed in the base worktree: {}",
                installed.detail
            ));
        }
    }

    // The check, when it failed on the head.
    let check_failed = report
        .steps
        .first()
        .is_some_and(|step| step.name == "check" && step.status == StepStatus::Failed);
    if check_failed {
        match (run.check)(&worktree.root) {
            Ok(base_ids) => {
                let base_ids: BTreeSet<&String> = base_ids.iter().collect();
                let new: Vec<&String> = run
                    .head_violations
                    .iter()
                    .filter(|id| !base_ids.contains(id))
                    .collect();
                let step = &mut report.steps[0];
                step.base = Some(BaseOutcome {
                    status: BaseStatus::Failed,
                    source: BaseSource::Ran,
                    note: None,
                });
                step.label = Some(if new.is_empty() && !run.head_violations.is_empty() {
                    Label::PreExisting
                } else {
                    Label::New
                });
                step.output_tail = step
                    .output_tail
                    .iter()
                    .map(|line| {
                        let found = run.head_violations.iter().find(|id| {
                            let mut parts = id.split(ID_SEPARATOR);
                            let (Some(rule), Some(file), Some(message)) =
                                (parts.next(), parts.next(), parts.next())
                            else {
                                return false;
                            };
                            line.starts_with(rule) && line.contains(file) && line.ends_with(message)
                        });
                        match found {
                            Some(id) if base_ids.contains(id) => format!("[pre-existing] {line}"),
                            Some(_) => format!("[new] {line}"),
                            None => line.clone(),
                        }
                    })
                    .collect();
            }
            Err(error) => {
                report.steps[0].base = Some(BaseOutcome {
                    status: BaseStatus::SetupNeeded,
                    source: BaseSource::Ran,
                    note: Some(format!(
                        "the check could not run on the base commit ({})",
                        error.message()
                    )),
                });
            }
        }
    }

    for pending in to_run {
        let step = &report.steps[pending.index];
        let outcome = if let Some(problem) = &install_failed {
            BaseOutcome {
                status: BaseStatus::SetupNeeded,
                source: BaseSource::Ran,
                note: Some(problem.clone()),
            }
        } else if run.options.stop.load(Ordering::SeqCst) {
            BaseOutcome {
                status: BaseStatus::Stopped,
                source: BaseSource::Ran,
                note: None,
            }
        } else if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            summary.over_budget = true;
            BaseOutcome {
                status: BaseStatus::OverBudget,
                source: BaseSource::Ran,
                note: None,
            }
        } else {
            let command = step.command.clone().unwrap_or_default();
            let project = plan_of(run.plan, step);
            say(&format!("{}: started: {command}", place(step)));
            let ran = run_base_step(
                &worktree.root,
                project,
                &step.name,
                &command,
                run.options,
                deadline,
            )?;
            summary.ran += 1;
            let refused_link = ran
                .output_tail
                .iter()
                .any(|line| line.contains("ERR_PNPM_UNSAFE_TASK_RUN_STATE_PATH"));
            let status = match ran.status {
                // pnpm refuses a linked node_modules for recursive runs: that
                // says nothing about the code.
                StepStatus::Failed if refused_link => BaseStatus::SetupNeeded,
                StepStatus::Passed => BaseStatus::Passed,
                StepStatus::Failed => BaseStatus::Failed,
                StepStatus::TimedOut
                    if deadline.is_some_and(|deadline| {
                        Instant::now() + Duration::from_millis(50) >= deadline
                    }) =>
                {
                    summary.over_budget = true;
                    BaseStatus::OverBudget
                }
                StepStatus::TimedOut => BaseStatus::TimedOut,
                StepStatus::Stopped => BaseStatus::Stopped,
                _ => BaseStatus::SetupNeeded,
            };
            say(&format!(
                "{}: {} on the base after {}",
                place(step),
                match status {
                    BaseStatus::Passed => "passed",
                    BaseStatus::Failed => "FAILED",
                    BaseStatus::SetupNeeded => "SETUP NEEDED",
                    BaseStatus::TimedOut => "TIMED OUT",
                    BaseStatus::OverBudget => "OVER BUDGET",
                    BaseStatus::Stopped => "STOPPED",
                },
                format_duration(Duration::from_millis(ran.duration_ms))
            ));
            if let Some(key) = &pending.key {
                keep(run.root, key, status, ran.duration_ms);
            }
            BaseOutcome {
                status,
                source: BaseSource::Ran,
                note: (status == BaseStatus::SetupNeeded).then(|| {
                    if refused_link {
                        "pnpm refused the head's node_modules linked into the base worktree; set verify.baseInstall to an install such as `pnpm install --frozen-lockfile --offline`".to_owned()
                    } else {
                        format!("the base run of it needed setup ({})", ran.detail)
                    }
                }),
            }
        };
        report.steps[pending.index].base = Some(outcome);
    }
    Ok(())
}

/// The separator between the parts of a violation's identity.
pub const ID_SEPARATOR: char = '\u{1f}';

/// The identity of a violation: rule, file and message. The line is left
/// out, so a violation that moved is the same violation.
#[must_use]
pub fn violation_id(rule: &str, file: &str, message: &str) -> String {
    format!("{rule}{ID_SEPARATOR}{file}{ID_SEPARATOR}{message}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Fixture, Track};

    fn git(root: &Path, args: &[&str]) {
        let mut full = vec![
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ];
        full.extend_from_slice(args);
        let out = run_git(root, &full).unwrap();
        assert_eq!(out.code, 0, "git {args:?}: {}", out.stderr);
    }

    fn committed() -> Fixture {
        let fixture = Fixture::new(&[("a.txt", "one\n")], Track::NoGit).unwrap();
        git(
            fixture.root(),
            &["init", "--quiet", "--initial-branch=main"],
        );
        git(fixture.root(), &["add", "--all"]);
        git(
            fixture.root(),
            &["commit", "--quiet", "--no-verify", "-m", "base"],
        );
        fixture
    }

    #[test]
    fn there_is_no_base_outside_git_or_before_the_first_commit() {
        let plain = Fixture::new(&[("a.txt", "one\n")], Track::NoGit).unwrap();
        assert!(matches!(
            choose_base(plain.root(), None).unwrap(),
            BaseChoice::Nothing(_)
        ));
        let empty = Fixture::new(&[("a.txt", "one\n")], Track::All).unwrap();
        assert!(matches!(
            choose_base(empty.root(), None).unwrap(),
            BaseChoice::Nothing(_)
        ));
    }

    #[test]
    fn a_clean_default_branch_has_nothing_to_compare_and_a_change_does() {
        let fixture = committed();
        let BaseChoice::Nothing(reason) = choose_base(fixture.root(), None).unwrap() else {
            panic!("a clean main has no base");
        };
        assert!(reason.contains("nothing to compare"), "{reason}");
        fs::write(fixture.root().join("a.txt"), "two\n").unwrap();
        let BaseChoice::Compare(base) = choose_base(fixture.root(), None).unwrap() else {
            panic!("an uncommitted change has a base");
        };
        assert_eq!(base.reference, "main");
        // A branch with a commit compares with the merge base.
        git(fixture.root(), &["checkout", "--quiet", "-b", "feature"]);
        git(
            fixture.root(),
            &["commit", "--quiet", "--no-verify", "-am", "change"],
        );
        let BaseChoice::Compare(on_branch) = choose_base(fixture.root(), None).unwrap() else {
            panic!("a branch has a base");
        };
        assert_eq!(on_branch.commit, base.commit);
        assert!(choose_base(fixture.root(), Some("no-such-ref")).is_err());
    }

    #[test]
    fn a_worktree_is_removed_with_everything_in_it() {
        let fixture = committed();
        let head = git_line(fixture.root(), &["rev-parse", "HEAD"]).unwrap();
        let path = {
            let worktree = Worktree::add(fixture.root(), &head).unwrap();
            assert!(worktree.root.join("a.txt").is_file());
            fs::create_dir_all(fixture.root().join("node_modules")).unwrap();
            fs::write(fixture.root().join("node_modules").join("kept.txt"), "x").unwrap();
            assert!(worktree.link(
                &fixture.root().join("node_modules"),
                &worktree.root.join("node_modules")
            ));
            worktree.top.clone()
        };
        assert!(!path.exists());
        // The head's folder the link pointed to is still there.
        assert!(fixture
            .root()
            .join("node_modules")
            .join("kept.txt")
            .is_file());
        let list = git_line(fixture.root(), &["worktree", "list", "--porcelain"]).unwrap();
        assert_eq!(list.matches("worktree ").count(), 1, "{list}");
    }

    #[test]
    fn violations_are_the_same_wherever_they_moved() {
        assert_eq!(
            violation_id("core/x", "a.txt", "bad"),
            violation_id("core/x", "a.txt", "bad")
        );
        assert_ne!(
            violation_id("core/x", "a.txt", "bad"),
            violation_id("core/x", "b.txt", "bad")
        );
    }
}
