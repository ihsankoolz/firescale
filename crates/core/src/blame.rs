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

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::affected::{changed_files, Changed};
use crate::error::{Error, Result};
use crate::git::{run_git, run_git_env};
use crate::pertest::{follow_renames, TestResults};
use crate::progress::{format_duration, format_limit};
use crate::stepcache::{now_secs, StepCache};
use crate::verify::{
    run_base_step, settle, signature_within, Limit, ProjectPlan, RunOptions, StepReport,
    StepStatus, VerifyPlan, VerifyReport, DEFAULT_BASE_BUDGET,
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
    /// The base failure's signature, compared with the head's.
    #[serde(skip)]
    pub signature: Vec<String>,
    /// The base's per-test results, for pytest, Vitest and Jest.
    #[serde(skip)]
    pub tests: Option<TestResults>,
}

impl BaseOutcome {
    fn new(status: BaseStatus, source: BaseSource, note: Option<String>) -> Self {
        Self {
            status,
            source,
            note,
            signature: Vec::new(),
            tests: None,
        }
    }
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
/// default branch, and when the base is `HEAD` and the tree is clean.
///
/// On the default branch itself the base is its upstream (`origin/main`),
/// never the branch: a commit made there in this session would otherwise
/// become the base and hide its own failures. With no upstream there is no
/// base, unless `--base` names one.
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
        None => {
            let Some(branch) = default_branch(root) else {
                return Ok(BaseChoice::Nothing(
                    "no default branch was found (origin/HEAD, main or master); pass --base <ref> to compare with one".to_owned(),
                ));
            };
            let local = branch.strip_prefix("origin/").unwrap_or(&branch).to_owned();
            let current = git_line(root, &["symbolic-ref", "--quiet", "--short", "HEAD"]);
            if current.as_deref() == Some(local.as_str()) {
                match git_line(
                    root,
                    &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"],
                ) {
                    Some(upstream) => upstream,
                    None => {
                        return Ok(BaseChoice::Nothing(format!(
                            "this is the default branch {local} with no upstream, so commits made here cannot be told from the base; pass --base <ref> to compare with one"
                        )))
                    }
                }
            } else {
                branch
            }
        }
    };
    let Some(commit) = git_line(root, &["merge-base", "HEAD", &reference]) else {
        if crate::git::shallow_without_merge_base(root, &reference) {
            return Ok(BaseChoice::Nothing(crate::git::shallow_note(&reference)));
        }
        if given.is_some() {
            return Err(Error::user(format!(
                "--base \"{reference}\" is not a commit git can compare with HEAD. Pass a branch, tag or commit that exists, for example --base origin/main."
            )));
        }
        return Ok(BaseChoice::Nothing(format!(
            "HEAD and {reference} have no commit in common (a shallow clone?)"
        )));
    };
    // firescale's own folder is not a change.
    let dirty = run_git(
        root,
        &[
            "status",
            "--porcelain",
            "--untracked-files=normal",
            "--",
            ".",
            ":(exclude).firescale",
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

/// The files that differ between the base commit and the working tree,
/// firescale's own folder left out.
///
/// # Errors
///
/// When git cannot compare them.
pub fn changed_since(root: &Path, base: &BaseCommit) -> Result<Option<BTreeSet<String>>> {
    Ok(match changed_files(root, Some(&base.commit))? {
        Changed::Everything | Changed::Shallow(_) => None,
        Changed::Files(files) => Some(
            files
                .into_iter()
                .filter(|path| !path.starts_with(".firescale/"))
                .collect(),
        ),
    })
}

/// The renames git sees between the base commit and the working tree:
/// old path to new path, repo-relative.
#[must_use]
pub fn renames_since(root: &Path, base: &BaseCommit) -> BTreeMap<String, String> {
    let Ok(out) = run_git(
        root,
        &[
            "-c",
            "core.quotepath=off",
            "diff",
            "-M",
            "--name-status",
            "--relative",
            "-z",
            &base.commit,
        ],
    ) else {
        return BTreeMap::new();
    };
    let parts: Vec<&str> = out.stdout.split('\0').collect();
    let mut renames = BTreeMap::new();
    let mut index = 0;
    while index < parts.len() {
        let status = parts[index];
        if status.starts_with('R') && index + 2 < parts.len() {
            renames.insert(
                parts[index + 1].replace('\\', "/"),
                parts[index + 2].replace('\\', "/"),
            );
            index += 3;
        } else if status.starts_with('C') && index + 2 < parts.len() {
            index += 3;
        } else {
            index += 2;
        }
    }
    renames
}

// ------------------------------------------------------------ the store

const STORE_FORMAT: u32 = 4;

/// A base result kept for a later run with the same base.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredBase {
    format: u32,
    key: String,
    status: BaseStatus,
    ran_at: u64,
    duration_ms: u64,
    /// The failure's signature (hashes of its normalised output lines).
    #[serde(default)]
    signature: Vec<String>,
    /// The failing tests, for pytest, Vitest and Jest (ADR 0038).
    #[serde(default)]
    failed_tests: Option<Vec<String>>,
    #[serde(default)]
    passed_tests: u64,
}

fn store_dir(root: &Path) -> PathBuf {
    root.join(".firescale").join("base")
}

fn lookup(root: &Path, key: &str) -> Option<(BaseStatus, Vec<String>, Option<TestResults>)> {
    let text = fs::read_to_string(store_dir(root).join(format!("{key}.json"))).ok()?;
    let stored: StoredBase = serde_json::from_str(&text).ok()?;
    let fresh = now_secs().saturating_sub(stored.ran_at) <= crate::stepcache::MAX_AGE.as_secs();
    (stored.format == STORE_FORMAT && stored.key == key && fresh).then(|| {
        let tests = stored.failed_tests.map(|failed| TestResults {
            failed: failed.into_iter().collect(),
            passed: stored.passed_tests,
        });
        (stored.status, stored.signature, tests)
    })
}

/// Keeps a base result. Only passed and failed are kept: the others say
/// nothing about the code.
fn keep(
    root: &Path,
    key: &str,
    status: BaseStatus,
    duration_ms: u64,
    signature: &[String],
    tests: Option<&TestResults>,
) {
    if !matches!(status, BaseStatus::Passed | BaseStatus::Failed) {
        return;
    }
    let dir = store_dir(root);
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    crate::stepcache::ignore_firescale_folder(root);
    let stored = StoredBase {
        format: STORE_FORMAT,
        key: key.to_owned(),
        status,
        ran_at: now_secs(),
        duration_ms,
        signature: signature.to_vec(),
        failed_tests: tests.map(|tests| tests.failed.iter().cloned().collect()),
        passed_tests: tests.map_or(0, |tests| tests.passed),
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
    /// The head's env files linked in (ADR 0043), removed before the
    /// worktree.
    files: std::sync::Mutex<Vec<PathBuf>>,
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
        sweep(root);
        let path = top.to_string_lossy().into_owned();
        // The repo's own hooks (post-checkout) never run, and Git LFS does
        // not download anything: the base run stays within its budget.
        let no_hooks = std::env::temp_dir().join("firescale-no-hooks");
        let _ = fs::create_dir_all(&no_hooks);
        let hooks = format!("core.hooksPath={}", no_hooks.to_string_lossy());
        let out = run_git_env(
            root,
            &[
                "-c", &hooks, "worktree", "add", "--detach", "--quiet", &path, commit,
            ],
            &[("GIT_LFS_SKIP_SMUDGE", "1")],
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
            files: std::sync::Mutex::new(Vec::new()),
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

    /// Links the head's file `target` in at `link`, and remembers it.
    fn link_file(&self, target: &Path, link: &Path) -> bool {
        let made = link_file(target, link);
        if made {
            self.files
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(link.to_path_buf());
        }
        made
    }
}

impl Drop for Worktree {
    fn drop(&mut self) {
        for file in self
            .files
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
        {
            let _ = fs::remove_file(file);
        }
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

/// Removes base worktrees that an earlier run left behind (killed before it
/// could clean up): those whose folder is gone, and those older than six
/// hours, which no run of today's budget can still be using.
fn sweep(root: &Path) {
    let _ = run_git(root, &["worktree", "prune"]);
    let Some(list) = git_line(root, &["worktree", "list", "--porcelain"]) else {
        return;
    };
    for line in list.lines() {
        let Some(path) = line.strip_prefix("worktree ") else {
            continue;
        };
        let path = PathBuf::from(path);
        let ours = path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("firescale-base-"));
        let old = fs::metadata(&path)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > Duration::from_secs(6 * 3600));
        if ours && old {
            unlink_dependency_links(&path);
            let _ = run_git(
                root,
                &["worktree", "remove", "--force", &path.to_string_lossy()],
            );
        }
    }
}

/// Removes, inside a left-behind worktree, the links it holds to the
/// head's dependency folders, before anything deletes it.
fn unlink_dependency_links(top: &Path) {
    for entry in ignore::WalkBuilder::new(top)
        .hidden(false)
        .git_ignore(false)
        .follow_links(false)
        .max_depth(Some(6))
        .build()
        .filter_map(std::result::Result::ok)
    {
        let name = entry.file_name().to_string_lossy();
        if DEPENDENCY_FOLDERS.contains(&name.as_ref()) && is_link(entry.path()) {
            unlink_dir(entry.path());
        }
    }
}

fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| {
        meta.file_type().is_symlink() || {
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                // FILE_ATTRIBUTE_REPARSE_POINT: a junction.
                meta.file_attributes() & 0x400 != 0
            }
            #[cfg(not(windows))]
            {
                false
            }
        }
    })
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

/// Links a file of the head into a worktree: a symbolic link, else (on
/// Windows without the right to make one) a hard link, else a copy, which
/// is removed with the worktree. Its contents are never read here.
fn link_file(target: &Path, link: &Path) -> bool {
    if link.exists() || fs::symlink_metadata(link).is_ok() || !target.is_file() {
        return false;
    }
    if link.parent().is_none_or(|parent| !parent.is_dir()) {
        return false;
    }
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(target, link).is_ok();
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(target, link).is_ok();
    linked || fs::hard_link(target, link).is_ok() || fs::copy(target, link).is_ok()
}

/// The env files of the head that are linked into a worktree (ADR 0043).
const ENV_FILES: [&str; 2] = [".env", ".env.local"];

/// The head's env files (`.env`, `.env.local`) at the root and in each
/// project's folder, by repo-relative path, with a hash of each one's
/// bytes. Git ignores them, so a worktree has none, and tests that read
/// one fail there for a reason of the worktree. Only the hash goes into a
/// kept result's key; the contents are never logged, kept or shown.
fn env_files(root: &Path, plan: &VerifyPlan) -> Vec<(String, String)> {
    let mut folders: BTreeSet<String> = BTreeSet::new();
    folders.insert(".".to_owned());
    for project in &plan.projects {
        folders.insert(project.root.clone());
    }
    let mut found = Vec::new();
    for folder in folders {
        for name in ENV_FILES {
            let path = folder_of(root, Some(&folder)).join(name);
            // A link to elsewhere is not followed.
            let plain = fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_file());
            if !plain {
                continue;
            }
            let Ok(bytes) = fs::read(&path) else {
                continue;
            };
            let rel = if folder == "." {
                name.to_owned()
            } else {
                format!("{folder}/{name}")
            };
            found.push((rel, blake3::hash(&bytes).to_hex()[..16].to_owned()));
        }
    }
    found
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
    found.retain(|folder| !links_into_repo(root, &folder_of(root, Some(folder))));
    found.sort();
    found
}

/// True when a dependency folder holds a link into the repo: a workspace
/// package in `node_modules`, or an editable install in a virtual
/// environment. Linked into the base worktree it would make the base run
/// the head's code, so it is never linked (ADR 0037).
fn links_into_repo(root: &Path, folder: &Path) -> bool {
    let Ok(real_root) = root.canonicalize() else {
        return true;
    };
    let points_in =
        |path: &Path| fs::canonicalize(path).is_ok_and(|target| target.starts_with(&real_root));
    let name = folder
        .file_name()
        .map(|name| name.to_string_lossy().into_owned());
    if name.as_deref() == Some("node_modules") {
        let Ok(entries) = fs::read_dir(folder) else {
            return false;
        };
        for entry in entries.filter_map(std::result::Result::ok) {
            let path = entry.path();
            let scoped = entry.file_name().to_string_lossy().starts_with('@');
            if scoped && !is_link(&path) {
                if let Ok(inner) = fs::read_dir(&path) {
                    for package in inner.filter_map(std::result::Result::ok) {
                        if is_link(&package.path()) && points_in(&package.path()) {
                            return true;
                        }
                    }
                }
            } else if is_link(&path) && points_in(&path) {
                return true;
            }
        }
        return false;
    }
    // A virtual environment: an editable install leaves a `.pth` or an
    // `.egg-link` naming a folder of the repo.
    let shown = real_root.to_string_lossy().into_owned();
    let plain = root.to_string_lossy().into_owned();
    ignore::WalkBuilder::new(folder)
        .hidden(false)
        .git_ignore(false)
        .follow_links(false)
        .max_depth(Some(5))
        .build()
        .filter_map(std::result::Result::ok)
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy();
            name.ends_with(".pth") || name.ends_with(".egg-link")
        })
        .any(|entry| {
            fs::read_to_string(entry.path()).is_ok_and(|text| {
                let text = text.replace('\\', "/");
                text.contains(&shown.replace('\\', "/")) || text.contains(&plain.replace('\\', "/"))
            })
        })
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
    /// False with `--no-cache`: no step counts as having the same inputs,
    /// and no kept base result is used; base results are still kept.
    pub shortcuts: bool,
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
/// then settles the result again: only `new` failures, and failures the
/// base cannot judge, block.
///
/// A failure is `pre-existing` only with evidence: it fails on the base
/// too, and every line of its output on the head (normalised, see
/// [`crate::verify::failure_signature`]) is in the base's. A base that
/// fails another way (a missing module, a broken environment, one error
/// fewer) never hides a head failure. Nothing here can fail the run: a
/// base step that cannot run is "not judged", and its failure blocks.
pub fn compare(report: &mut VerifyReport, run: &BaseRun<'_>) {
    let started = Instant::now();
    let budget = match run.options.config.base_budget {
        Some(Limit::Off) => None,
        Some(Limit::After(limit)) => Some(limit),
        None => Some(DEFAULT_BASE_BUDGET),
    };
    let deadline = budget.map(|limit| started + limit);
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
    let links = if run.options.config.base_install.is_some() {
        Vec::new()
    } else {
        dependency_folders(run.root)
    };
    let envs = env_files(run.root, run.plan);
    let env_part = envs
        .iter()
        .map(|(path, hash)| format!("{path}:{hash}"))
        .collect::<Vec<_>>()
        .join(",");
    let environment = match &run.options.config.base_install {
        Some(install) => format!("install={install};env={env_part}"),
        None => format!("links={};env={env_part}", links.join(",")),
    };
    let links = WorktreeSetup {
        folders: links,
        env_files: envs.into_iter().map(|(path, _)| path).collect(),
    };

    // 1. What each step's base result is, where it is known without a run.
    let mut to_run: Vec<Pending> = Vec::new();
    let mut confirmed: BTreeSet<usize> = BTreeSet::new();
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
        // The stop hook only judges failures: a passing step gets no base
        // result, so it is never labelled `fixed` there (ADR 0039).
        if head == BaseStatus::Passed && !run.judge_passes {
            continue;
        }
        let folder = step.project.as_deref();
        let kind = step.kind.as_deref();
        let dir = folder_of(run.root, folder);
        let same = run.shortcuts
            && run.changed.is_some_and(|changed| {
                run.cache
                    .same_on_base(folder, kind, &step.name, &command, &dir, changed)
            });
        if same {
            let mut outcome = BaseOutcome::new(head, BaseSource::SameInputs, None);
            outcome.signature.clone_from(&step.signature);
            outcome.tests.clone_from(&step.test_results);
            report.steps[index].base = Some(outcome);
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
        let kept = if run.shortcuts {
            key.as_deref().and_then(|key| lookup(run.root, key))
        } else {
            None
        };
        // A kept failure of a step that now passes on the head is used only
        // with the run in a worktree of the head that was made beside it,
        // in the same run: it may have failed for a reason of that earlier
        // worktree, such as dependencies installed since (ADR 0040).
        // Otherwise the step runs on the base again if a worktree is made.
        let kept = match kept {
            Some((BaseStatus::Failed, ..)) if head == BaseStatus::Passed => {
                let paired = key
                    .as_deref()
                    .and_then(|key| fixed_key(run, step, key, &environment))
                    .and_then(|fixed| lookup(run.root, &fixed))
                    .is_some_and(|(status, ..)| status == BaseStatus::Passed);
                if paired {
                    confirmed.insert(index);
                    kept
                } else {
                    None
                }
            }
            other => other,
        };
        if let Some((status, signature, tests)) = kept {
            let mut outcome = BaseOutcome::new(status, BaseSource::Cache, None);
            outcome.signature = signature;
            outcome.tests = tests;
            report.steps[index].base = Some(outcome);
            summary.from_cache += 1;
            continue;
        }
        // A lockfile or requirements list this step's key covers differs
        // between the base and the head: the head's installed dependencies
        // do not stand for the base's, so without an install in the base
        // worktree this project has no base (ADR 0040).
        if run.options.config.base_install.is_none() {
            let changed = run.changed.map_or_else(Vec::new, |changed| {
                run.cache.changed_dependencies(folder, kind, &dir, changed)
            });
            if let Some(first) = changed.first() {
                report.steps[index].base = Some(BaseOutcome::new(
                    BaseStatus::SetupNeeded,
                    BaseSource::Ran,
                    Some(dependencies_note(first)),
                ));
                continue;
            }
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
    let mut check_failed = report
        .steps
        .first()
        .is_some_and(|step| step.name == "check" && step.status == StepStatus::Failed);
    // Each step whose lockfile or requirements list changed was taken out
    // before (it has no base without an install). The check is the repo
    // root's: any changed list counts for it, since custom rules may load
    // packages installed anywhere.
    let check_dependencies: Option<String> = if run.options.config.base_install.is_none() {
        run.changed.and_then(|changed| {
            changed
                .iter()
                .find(|path| crate::facts::is_dependency_list(path))
                .map(|first| dependencies_note(first))
        })
    } else {
        None
    };
    if check_failed {
        if let Some(problem) = &check_dependencies {
            report.notes.push(format!(
                "check: {problem}, so its enforced violations block as they would without a base."
            ));
            check_failed = false;
        }
    }
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
                    report.steps[pending.index].base = Some(BaseOutcome::new(
                        BaseStatus::SetupNeeded,
                        BaseSource::Ran,
                        Some(format!("the base worktree could not be made ({problem})")),
                    ));
                }
            }
            Ok(worktree) => run_in_worktree(
                report,
                run,
                &worktree,
                &to_run,
                check_failed,
                &links,
                deadline,
                &mut summary,
                &say,
            ),
        }
    }

    // 3. A step that passes on the head and failed on the base is `fixed`
    // only when that failure came from the base's code, not from the
    // worktree it ran in (ADR 0040).
    let fixed_evidence = confirm_fixed(
        report,
        run,
        &environment,
        &links,
        &to_run,
        &confirmed,
        deadline,
        &say,
    );

    // 4. Labels, by evidence.
    let short = run.base.short().to_owned();
    let budget_text = budget.map_or_else(|| "no limit".to_owned(), format_limit);
    let renames = renames_since(run.root, run.base);
    let mut per_step_notes: Vec<String> = Vec::new();
    for (index, step) in report.steps.iter_mut().enumerate() {
        if step.name == "check" {
            continue;
        }
        let failed = matches!(step.status, StepStatus::Failed | StepStatus::TimedOut);
        let signature = step.signature.clone();
        let timed_out = step.status == StepStatus::TimedOut;
        let fail_fast = signature
            .iter()
            .any(|line| line == crate::verify::FAIL_FAST);
        let place_text = place(step);
        let prefix = step
            .project
            .as_deref()
            .filter(|project| *project != ".")
            .map_or_else(String::new, |project| format!("{project}/"));
        let head_tests = step.test_results.clone();
        let is_test = step.name == "test";
        let Some(outcome) = step.base.as_mut() else {
            continue;
        };
        // Per-test comparison, when both sides have per-test results.
        let per_test = match (&head_tests, &outcome.tests) {
            (Some(head), Some(base)) if failed && outcome.status == BaseStatus::Failed => {
                let base_failed = follow_renames(&base.failed, &renames, &prefix);
                let new_tests: BTreeSet<String> =
                    head.failed.difference(&base_failed).cloned().collect();
                Some((new_tests, base.passed == 0 && head.passed > 0, base_failed))
            }
            _ => None,
        };
        if failed
            && outcome.status == BaseStatus::Failed
            && (head_tests.is_none() || outcome.tests.is_none())
            && !timed_out
            && is_test
        {
            per_step_notes.push(format!(
                "{place_text}: its runner gives no per-test results, so it is compared as a whole step"
            ));
        }
        let (label, why) = match (failed, outcome.status) {
            (true, BaseStatus::Passed) => (Some(Label::New), None),
            (true, BaseStatus::Failed) if timed_out => (
                None,
                Some(format!(
                    "it timed out on the head but failed on the base commit {short}, which is not the same failure"
                )),
            ),
            (true, BaseStatus::Failed) if fail_fast => (
                None,
                Some(
                    "it stopped at its first failure, so a new failure after it cannot be ruled out (run it with cargo test --no-fail-fast, or without -x, --maxfail or --bail, to compare it)"
                        .to_owned(),
                ),
            ),
            (true, BaseStatus::Failed) => match &per_test {
                // The base's tests did not run at all: its environment
                // failed, not its code.
                Some((_, true, _)) => (
                    None,
                    Some(format!(
                        "no test passed on the base commit {short}, so its run says nothing about the code"
                    )),
                ),
                Some((new_tests, false, _)) if !new_tests.is_empty() => (Some(Label::New), None),
                _ if signature_within(&signature, &outcome.signature) => {
                    (Some(Label::PreExisting), None)
                }
                Some(_) => (
                    None,
                    Some(format!(
                        "the same tests fail on the base commit {short}, but not in the same way (its output on the head has lines the base's has not)"
                    )),
                ),
                None => (
                    None,
                    Some(format!(
                        "it fails on the base commit {short} too, but not in the same way (its output on the head has lines the base's has not)"
                    )),
                ),
            },
            (false, BaseStatus::Failed) => match fixed_evidence.get(&index) {
                Some(None) => (Some(Label::Fixed), None),
                Some(Some(why)) => {
                    let note = format!(
                        "it fails on the base commit {short}, but {why}, so it is not labelled fixed"
                    );
                    per_step_notes.push(format!("{place_text}: {note}."));
                    outcome.note = Some(note);
                    (None, None)
                }
                None => (None, None),
            },
            (true, BaseStatus::SetupNeeded) => (
                None,
                Some(outcome.note.clone().unwrap_or_else(|| {
                    format!("the base run of it on {short} needed setup")
                })),
            ),
            (true, BaseStatus::TimedOut) => {
                (None, Some(format!("it timed out on the base commit {short} too")))
            }
            (true, BaseStatus::OverBudget) => (
                None,
                Some(format!(
                    "the base run ran out of its time budget ({budget_text}, verify.baseBudget) before it finished"
                )),
            ),
            (true, BaseStatus::Stopped) => {
                (None, Some("verify was stopped during the base run".to_owned()))
            }
            (false, _) => (None, None),
        };
        if let Some(why) = why {
            outcome.note = Some(format!(
                "{why}, so this failure blocks as it would without a base"
            ));
        }
        // Each failing test's label.
        let base_failed: Option<BTreeSet<String>> = match (&per_test, outcome.status) {
            (Some((_, false, base_failed)), _) => Some(base_failed.clone()),
            (_, BaseStatus::Passed) => Some(BTreeSet::new()),
            _ => None,
        };
        step.label = label;
        if let Some(base_failed) = base_failed {
            for test in &mut step.failed_tests {
                test.label = Some(if base_failed.contains(&test.id) {
                    Label::PreExisting
                } else {
                    Label::New
                });
            }
        }
    }
    report.notes.extend(per_step_notes);
    // Failing steps that got no base result at all (the budget ran out
    // before they were reached) block too; say why.
    for step in &mut report.steps {
        if step.name != "check"
            && matches!(step.status, StepStatus::Failed | StepStatus::TimedOut)
            && step.base.is_none()
        {
            step.base = Some(BaseOutcome::new(
                BaseStatus::OverBudget,
                BaseSource::Ran,
                Some(format!(
                    "the base run ran out of its time budget ({budget_text}, verify.baseBudget) before it reached this step, so this failure blocks as it would without a base"
                )),
            ));
            summary.over_budget = true;
        }
    }
    // The base run's own time, before any rerun of the head.
    summary.duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    rerun_new_failures(report, run, deadline, &say);
    let notes: Vec<String> = report
        .steps
        .iter()
        .filter(|step| matches!(step.status, StepStatus::Failed | StepStatus::TimedOut))
        .filter_map(|step| {
            step.base
                .as_ref()
                .and_then(|outcome| outcome.note.as_ref())
                .map(|note| format!("{}: {note}.", place(step)))
        })
        .collect();
    report.notes.extend(notes);
    report.base = Some(summary);
    let stopped = report.result == crate::verify::VerifyResult::Stopped
        || run.options.stop.load(Ordering::SeqCst);
    settle(report, stopped);
}

/// What a worktree is given from the head: its dependency folders (none
/// with `verify.baseInstall`) and its env files.
struct WorktreeSetup {
    folders: Vec<String>,
    env_files: Vec<String>,
}

/// Gives a worktree its dependencies: the head's dependency folders linked
/// in, or `verify.baseInstall` run at its root. `which` names it in notes
/// (`base`, or `head` for the control run of ADR 0040). Returns why the
/// worktree cannot judge anything, if it cannot.
fn prepare_worktree(
    run: &BaseRun<'_>,
    worktree: &Worktree,
    links: &WorktreeSetup,
    deadline: Option<Instant>,
    say: &dyn Fn(&str),
    which: &str,
) -> Option<String> {
    // The head's installed dependencies, linked in: no network, no install
    // into a shared interpreter (ADR 0037). A link that cannot be made
    // means the base cannot be judged.
    let mut not_judged: Option<String> = None;
    for folder in &links.folders {
        let made = worktree.link(
            &folder_of(run.root, Some(folder)),
            &folder_of(&worktree.root, Some(folder)),
        );
        if !made && not_judged.is_none() {
            not_judged = Some(format!(
                "the head's {folder} could not be linked into the {which} worktree"
            ));
        }
    }
    // The head's ignored env files, which tests read (ADR 0043). Where the
    // worktree has the file already (tracked there) or lacks the folder,
    // nothing is linked; that is no reason not to judge.
    for file in &links.env_files {
        let _ = worktree.link_file(
            &folder_of(run.root, Some(file)),
            &folder_of(&worktree.root, Some(file)),
        );
    }
    if let Some(install) = run
        .options
        .config
        .base_install
        .as_ref()
        .filter(|_| not_judged.is_none())
    {
        say(&format!("installing: {install}"));
        match run_base_step(
            &worktree.root,
            None,
            "install",
            install,
            run.options,
            deadline,
        ) {
            Ok(installed) if installed.status == StepStatus::Passed => {}
            Ok(installed) => {
                not_judged = Some(format!(
                    "verify.baseInstall (`{install}`) did not succeed in the {which} worktree: {}",
                    installed.detail
                ));
            }
            Err(error) => {
                not_judged = Some(format!(
                    "verify.baseInstall (`{install}`) could not run in the {which} worktree: {}",
                    error.message()
                ));
            }
        }
    }
    not_judged
}

#[allow(clippy::too_many_arguments)]
fn run_in_worktree(
    report: &mut VerifyReport,
    run: &BaseRun<'_>,
    worktree: &Worktree,
    to_run: &[Pending],
    check_failed: bool,
    links: &WorktreeSetup,
    deadline: Option<Instant>,
    summary: &mut BaseReport,
    say: &dyn Fn(&str),
) {
    let not_judged = prepare_worktree(run, worktree, links, deadline, say, "base");

    // The check, when it failed on the head and can be judged.
    if check_failed && not_judged.is_none() && !past(deadline) {
        match (run.check)(&worktree.root) {
            Ok(base_ids) => label_check(report, run, &base_ids),
            Err(error) => {
                report.steps[0].base = Some(BaseOutcome::new(
                    BaseStatus::SetupNeeded,
                    BaseSource::Ran,
                    Some(format!(
                        "the check could not run on the base commit ({})",
                        error.message()
                    )),
                ));
            }
        }
    }

    for pending in to_run {
        let step = &report.steps[pending.index];
        let outcome = if let Some(problem) = &not_judged {
            BaseOutcome::new(
                BaseStatus::SetupNeeded,
                BaseSource::Ran,
                Some(problem.clone()),
            )
        } else if run.options.stop.load(Ordering::SeqCst) {
            BaseOutcome::new(BaseStatus::Stopped, BaseSource::Ran, None)
        } else if past(deadline) {
            summary.over_budget = true;
            BaseOutcome::new(BaseStatus::OverBudget, BaseSource::Ran, None)
        } else {
            let command = step.command.clone().unwrap_or_default();
            let project = plan_of(run.plan, step);
            let dir = folder_of(&worktree.root, step.project.as_deref());
            say(&format!("{}: started: {command}", place(step)));
            let ran = if dir.is_dir() {
                run_base_step(
                    &worktree.root,
                    project,
                    &step.name,
                    &command,
                    run.options,
                    deadline,
                )
                .map_err(|error| error.message().to_owned())
            } else {
                Err("its folder does not exist on the base commit".to_owned())
            };
            match ran {
                Err(problem) => BaseOutcome::new(
                    BaseStatus::SetupNeeded,
                    BaseSource::Ran,
                    Some(format!("it could not run on the base commit ({problem})")),
                ),
                Ok(ran) => {
                    summary.ran += 1;
                    let refused_link = ran
                        .output_tail
                        .iter()
                        .any(|line| line.contains("ERR_PNPM_UNSAFE_TASK_RUN_STATE_PATH"));
                    let status = match ran.status {
                        StepStatus::Passed => BaseStatus::Passed,
                        // pnpm refuses a linked node_modules for recursive
                        // runs: that says nothing about the code.
                        StepStatus::Failed if refused_link => BaseStatus::SetupNeeded,
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
                        keep(
                            run.root,
                            key,
                            status,
                            ran.duration_ms,
                            &ran.signature,
                            ran.test_results.as_ref(),
                        );
                    }
                    let note = (status == BaseStatus::SetupNeeded).then(|| {
                        if refused_link {
                            "pnpm refused the head's node_modules linked into the base worktree; set verify.baseInstall to an install such as `pnpm install --frozen-lockfile --offline`".to_owned()
                        } else {
                            format!("the base run of it needed setup ({})", ran.detail)
                        }
                    });
                    let mut outcome = BaseOutcome::new(status, BaseSource::Ran, note);
                    outcome.signature = ran.signature;
                    outcome.tests = ran.test_results;
                    outcome
                }
            }
        };
        report.steps[pending.index].base = Some(outcome);
    }
}

/// The key under which a confirmed `fixed` is kept: the step's base key
/// and its own key on the head, so it holds only while both are the same.
fn fixed_key(
    run: &BaseRun<'_>,
    step: &StepReport,
    base_key: &str,
    environment: &str,
) -> Option<String> {
    let folder = step.project.as_deref();
    let head = run
        .cache
        .key(
            folder,
            step.kind.as_deref(),
            &step.name,
            step.command.as_deref()?,
            &folder_of(run.root, folder),
        )
        .ok()?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(format!("fixed\0{base_key}\0{}\0{environment}", head.key).as_bytes());
    Some(format!("fixed-{}", hasher.finalize().to_hex()))
}

/// For each step that passes on the head and failed on the base: `None`
/// when the base's failure is a result of its code, or why it may not be.
///
/// The base runs in a worktree, which lacks what git ignores (build
/// outputs, generated files, local settings), so a step can fail there for
/// want of them. To tell, the step runs in a worktree of the head too (the
/// `HEAD` commit with the working tree's changes copied in), set up as the
/// base worktree was, in the same run as the base: if it passes there, the
/// worktree can run it, and the base's failure came from the code. Only
/// that pairing is kept, under [`fixed_key`] (ADR 0040).
#[allow(clippy::too_many_arguments)]
fn confirm_fixed(
    report: &VerifyReport,
    run: &BaseRun<'_>,
    environment: &str,
    links: &WorktreeSetup,
    to_run: &[Pending],
    confirmed: &BTreeSet<usize>,
    deadline: Option<Instant>,
    say: &dyn Fn(&str),
) -> BTreeMap<usize, Option<String>> {
    let verdict = |status: BaseStatus| {
        match status {
        BaseStatus::Passed => None,
        BaseStatus::Failed => Some(
            "it fails in a worktree of the head too, so that failure comes from the worktree, not from the code"
                .to_owned(),
        ),
        BaseStatus::SetupNeeded => Some(
            "it needs setup in a worktree of the head, so that failure may come from the worktree, not from the code"
                .to_owned(),
        ),
        BaseStatus::TimedOut => Some(
            "it timed out in a worktree of the head, so that failure may come from the worktree, not from the code"
                .to_owned(),
        ),
        BaseStatus::OverBudget => Some(
            "the base run's time budget (verify.baseBudget) ran out before a run in a worktree of the head could tell whether that failure comes from the code"
                .to_owned(),
        ),
        BaseStatus::Stopped => Some(
            "verify was stopped before it could tell whether that failure comes from the code"
                .to_owned(),
        ),
    }
    };
    let mut verdicts: BTreeMap<usize, Option<String>> = BTreeMap::new();
    let mut needed: Vec<(usize, Option<String>)> = Vec::new();
    for (index, step) in report.steps.iter().enumerate() {
        if step.name == "check" || step.status != StepStatus::Passed {
            continue;
        }
        let Some(outcome) = &step.base else {
            continue;
        };
        if outcome.status != BaseStatus::Failed {
            continue;
        }
        if confirmed.contains(&index) {
            verdicts.insert(index, None);
            continue;
        }
        if outcome.source != BaseSource::Ran {
            verdicts.insert(
                index,
                Some("that result is from an earlier run, which cannot tell now whether it came from the code".to_owned()),
            );
            continue;
        }
        if outcome
            .tests
            .as_ref()
            .is_some_and(|tests| tests.passed == 0)
        {
            verdicts.insert(
                index,
                Some("no test passed there, so that run says nothing about the code".to_owned()),
            );
            continue;
        }
        let key = to_run
            .iter()
            .find(|pending| pending.index == index)
            .and_then(|pending| pending.key.as_deref())
            .and_then(|base_key| fixed_key(run, step, base_key, environment));
        needed.push((index, key));
    }
    if needed.is_empty() {
        return verdicts;
    }
    let made = if run.options.stop.load(Ordering::SeqCst) {
        Err(Some("verify was stopped".to_owned()))
    } else if past(deadline) {
        Err(None)
    } else {
        git_line(run.root, &["rev-parse", "--verify", "HEAD"])
            .ok_or_else(|| Some("git could not name the head commit".to_owned()))
            .and_then(|head| {
                say("checking in a worktree of the head that what failed on the base came from the code");
                Worktree::add(run.root, &head).map_err(Some)
            })
            .and_then(|worktree| {
                overlay_head(run.root, &worktree.root, deadline, run.options.stop)?;
                Ok(worktree)
            })
    };
    let worktree = match made {
        Ok(worktree) => worktree,
        Err(problem) => {
            for (index, _) in needed {
                verdicts.insert(
                    index,
                    problem.as_ref().map_or_else(
                        || verdict(BaseStatus::OverBudget),
                        |problem| {
                            Some(format!(
                                "a worktree of the head, to tell whether that failure comes from the code, could not be made ({problem})"
                            ))
                        },
                    ),
                );
            }
            return verdicts;
        }
    };
    let not_judged = prepare_worktree(run, &worktree, links, deadline, say, "head");
    for (index, key) in needed {
        let step = &report.steps[index];
        let status = if let Some(problem) = &not_judged {
            verdicts.insert(
                index,
                Some(format!(
                    "{problem}, so it cannot tell whether that failure comes from the code"
                )),
            );
            continue;
        } else if run.options.stop.load(Ordering::SeqCst) {
            BaseStatus::Stopped
        } else if past(deadline) {
            BaseStatus::OverBudget
        } else {
            let command = step.command.clone().unwrap_or_default();
            match run_base_step(
                &worktree.root,
                plan_of(run.plan, step),
                &step.name,
                &command,
                run.options,
                deadline,
            ) {
                Err(_) => BaseStatus::SetupNeeded,
                Ok(ran) => {
                    let status = match ran.status {
                        StepStatus::Passed => BaseStatus::Passed,
                        StepStatus::Failed => BaseStatus::Failed,
                        StepStatus::TimedOut if past(deadline) => BaseStatus::OverBudget,
                        StepStatus::TimedOut => BaseStatus::TimedOut,
                        StepStatus::Stopped => BaseStatus::Stopped,
                        _ => BaseStatus::SetupNeeded,
                    };
                    if let Some(key) = key.as_ref().filter(|_| status == BaseStatus::Passed) {
                        keep(run.root, key, status, ran.duration_ms, &[], None);
                    }
                    say(&format!(
                        "{}: {} in a worktree of the head",
                        place(step),
                        if status == BaseStatus::Passed {
                            "passed"
                        } else {
                            "did not pass"
                        }
                    ));
                    status
                }
            }
        };
        verdicts.insert(index, verdict(status));
    }
    verdicts
}

/// Copies the working tree's changes since `HEAD` into `into`, a worktree
/// of `HEAD`: changed and untracked files that git does not ignore, and
/// deletions. Ignored files are not copied: the worktree stays what the
/// base worktree is, apart from the code.
///
/// Errors: `Some(why)` when it cannot be done, `None` when the budget ran
/// out first.
fn overlay_head(
    root: &Path,
    into: &Path,
    deadline: Option<Instant>,
    stop: &std::sync::atomic::AtomicBool,
) -> std::result::Result<(), Option<String>> {
    let listed = |args: &[&str]| -> std::result::Result<Vec<String>, String> {
        let out = run_git(root, args).map_err(|error| error.message().to_owned())?;
        if out.code != 0 {
            return Err(format!(
                "git {} failed: {}",
                args.get(2).unwrap_or(&""),
                out.stderr.lines().next().unwrap_or("").trim()
            ));
        }
        Ok(out
            .stdout
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(|path| path.replace('\\', "/"))
            .collect())
    };
    let mut paths = listed(&[
        "-c",
        "core.quotepath=off",
        "diff",
        "--name-only",
        "-z",
        "--no-renames",
        "--relative",
        "HEAD",
        "--",
    ])
    .map_err(Some)?;
    paths.extend(
        listed(&[
            "-c",
            "core.quotepath=off",
            "ls-files",
            "-z",
            "--others",
            "--exclude-standard",
        ])
        .map_err(Some)?,
    );
    let real_into = into
        .canonicalize()
        .map_err(|error| Some(format!("the worktree cannot be read: {error}")))?;
    for path in paths {
        if path == ".firescale" || path.starts_with(".firescale/") {
            continue;
        }
        if stop.load(Ordering::SeqCst) {
            return Err(Some("verify was stopped".to_owned()));
        }
        if past(deadline) {
            return Err(None);
        }
        let from = folder_of(root, Some(&path));
        let to = folder_of(into, Some(&path));
        // A folder on the way that the commit holds as a link (the head may
        // have a real folder there) is replaced by a real folder, so nothing
        // is ever written through it out of the worktree.
        let mut at = into.to_path_buf();
        let parts: Vec<&str> = path.split('/').collect();
        for part in &parts[..parts.len().saturating_sub(1)] {
            at.push(part);
            if is_link(&at) && fs::remove_file(&at).is_err() {
                let _ = fs::remove_dir(&at);
            }
        }
        match fs::symlink_metadata(&from) {
            // A submodule or a nested repo: left as the commit has it.
            Ok(meta) if meta.is_dir() => {}
            Ok(meta) => {
                if let Some(parent) = to.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|error| Some(format!("{path} could not be copied: {error}")))?;
                    let inside = parent
                        .canonicalize()
                        .is_ok_and(|real| real.starts_with(&real_into));
                    if !inside {
                        return Err(Some(format!("{path} would be copied out of the worktree")));
                    }
                }
                // Never write through a link the commit holds at that path.
                if fs::symlink_metadata(&to).is_ok_and(|old| !old.is_dir()) {
                    let _ = fs::remove_file(&to);
                }
                copy_entry(&from, &to, meta.file_type().is_symlink())
                    .map_err(|error| Some(format!("{path} could not be copied: {error}")))?;
            }
            Err(_) => {
                let _ = fs::remove_file(&to);
            }
        }
    }
    Ok(())
}

/// Copies a file, or recreates a symbolic link as a link (Unix).
fn copy_entry(from: &Path, to: &Path, link: bool) -> std::io::Result<()> {
    #[cfg(unix)]
    if link {
        return std::os::unix::fs::symlink(fs::read_link(from)?, to);
    }
    #[cfg(not(unix))]
    let _ = link;
    fs::copy(from, to).map(|_| ())
}

/// Why a step has no base after a lockfile or requirements list changed.
fn dependencies_note(path: &str) -> String {
    format!(
        "{path} changed, so the head's installed dependencies do not stand for the base's; set verify.baseInstall to a command that installs them in the base worktree"
    )
}

/// Runs each `new` failure once more on the head before it blocks (ADR
/// 0039). A step that passes the second time is `flaky` and does not block.
/// With per-test results, a step whose new tests all pass the second time
/// is `flaky` too when what still fails is the base's own failure (the
/// output evidence of ADR 0037); its new tests are labelled `flaky`.
/// Nothing about a flaky result is written anywhere: no history is kept,
/// and the second run's pass is not stored in the step cache.
fn rerun_new_failures(
    report: &mut VerifyReport,
    run: &BaseRun<'_>,
    deadline: Option<Instant>,
    say: &dyn Fn(&str),
) {
    for index in 0..report.steps.len() {
        // The check is deterministic: its result on the head is final.
        if report.steps[index].name == "check"
            || report.steps[index].label != Some(Label::New)
            || report.steps[index].status != StepStatus::Failed
            || run.options.stop.load(Ordering::SeqCst)
        {
            continue;
        }
        // The rerun shares the base run's budget; with none left, a new
        // failure blocks without one.
        if past(deadline) {
            let note = format!(
                "{}: not run again to check for flakiness, because the time budget (verify.baseBudget) ran out",
                place(&report.steps[index])
            );
            report.notes.push(note);
            continue;
        }
        let step = &report.steps[index];
        let Some(command) = step.command.clone() else {
            continue;
        };
        let project = plan_of(run.plan, step);
        let name = step.name.clone();
        // A first run that changed the step's own files (a fixture it
        // wrote, a generated file) would make a second run pass for that
        // reason: then it stays new.
        let folder = step.project.clone();
        let dir = folder_of(run.root, folder.as_deref());
        if run
            .cache
            .changed_during_run(folder.as_deref(), step.kind.as_deref(), &dir)
        {
            let note = format!(
                "{}: not run again to check for flakiness, because files it reads changed while verify ran",
                place(step)
            );
            report.notes.push(note);
            continue;
        }
        say(&format!(
            "{}: a new failure; running it once more before it blocks",
            place(step)
        ));
        let Ok(again) =
            crate::verify::run_again(run.root, project, &name, &command, run.options, deadline)
        else {
            continue;
        };
        let step = &mut report.steps[index];
        if again.status == StepStatus::Passed {
            step.label = Some(Label::Flaky);
            for test in &mut step.failed_tests {
                if test.label == Some(Label::New) {
                    test.label = Some(Label::Flaky);
                }
            }
            say(&format!("{}: passed the second time: flaky", place(step)));
            continue;
        }
        // With per-test results: only on positive evidence that each new
        // test passed. The rerun must have finished as a plain failure with
        // the runner's counts, passed as many more tests as there are new
        // ones, and failed only tests that already failed on the base.
        let (Some(results), Some(head), Some(base)) =
            (&again.test_results, &step.test_results, step.base.as_ref())
        else {
            continue;
        };
        if again.status != StepStatus::Failed
            || again
                .signature
                .iter()
                .any(|line| line == crate::verify::FAIL_FAST)
        {
            continue;
        }
        let new_tests: Vec<String> = step
            .failed_tests
            .iter()
            .filter(|test| test.label == Some(Label::New))
            .map(|test| test.id.clone())
            .collect();
        let old_tests: BTreeSet<&str> = step
            .failed_tests
            .iter()
            .filter(|test| test.label == Some(Label::PreExisting))
            .map(|test| test.id.as_str())
            .collect();
        let only_old = results
            .failed
            .iter()
            .all(|id| old_tests.contains(id.as_str()));
        let more_passed = results.passed >= head.passed + new_tests.len() as u64;
        if new_tests.is_empty() || !only_old || !more_passed {
            continue;
        }
        // What still fails must be the base's own failure, by the output
        // evidence, to let it pass.
        if base.status == BaseStatus::Failed && signature_within(&again.signature, &base.signature)
        {
            step.label = Some(Label::Flaky);
            for test in &mut step.failed_tests {
                if test.label == Some(Label::New) {
                    test.label = Some(Label::Flaky);
                }
            }
            say(&format!(
                "{}: its new tests passed the second time: flaky",
                place(step)
            ));
        }
    }
}

fn past(deadline: Option<Instant>) -> bool {
    deadline.is_some_and(|deadline| Instant::now() >= deadline)
}

/// Labels the check from the base's violations: `pre-existing` only when
/// every violation of the head is on the base too, as often.
fn label_check(report: &mut VerifyReport, run: &BaseRun<'_>, base_ids: &[String]) {
    let mut left: BTreeMap<&str, usize> = BTreeMap::new();
    for id in base_ids {
        *left.entry(id.as_str()).or_default() += 1;
    }
    let mut new_ids: Vec<&String> = Vec::new();
    for id in run.head_violations {
        match left.get_mut(id.as_str()) {
            Some(count) if *count > 0 => *count -= 1,
            _ => new_ids.push(id),
        }
    }
    let base_set: BTreeSet<&String> = base_ids.iter().collect();
    let step = &mut report.steps[0];
    step.base = Some(BaseOutcome::new(
        if base_ids.is_empty() {
            BaseStatus::Passed
        } else {
            BaseStatus::Failed
        },
        BaseSource::Ran,
        None,
    ));
    step.label = Some(if new_ids.is_empty() && !run.head_violations.is_empty() {
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
                Some(id) if base_set.contains(id) && !new_ids.contains(&id) => {
                    format!("[pre-existing] {line}")
                }
                Some(_) => format!("[new] {line}"),
                None => line.clone(),
            }
        })
        .collect();
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
    fn the_base_of_a_branch_is_its_merge_base_and_main_alone_has_none() {
        let fixture = committed();
        // On main with no upstream: no base, clean or not, since a commit
        // made here could not be told from the base.
        let BaseChoice::Nothing(reason) = choose_base(fixture.root(), None).unwrap() else {
            panic!("main with no upstream has no base");
        };
        assert!(reason.contains("no upstream"), "{reason}");
        let base = git_line(fixture.root(), &["rev-parse", "HEAD"]).unwrap();
        // A branch: with no change there is nothing to compare.
        git(fixture.root(), &["checkout", "--quiet", "-b", "feature"]);
        let BaseChoice::Nothing(reason) = choose_base(fixture.root(), None).unwrap() else {
            panic!("a clean branch at main has nothing to compare");
        };
        assert!(reason.contains("nothing to compare"), "{reason}");
        // An uncommitted change, then a commit: the base is the merge base.
        fs::write(fixture.root().join("a.txt"), "two\n").unwrap();
        let BaseChoice::Compare(dirty) = choose_base(fixture.root(), None).unwrap() else {
            panic!("an uncommitted change has a base");
        };
        assert_eq!(
            (dirty.reference.as_str(), dirty.commit.as_str()),
            ("main", base.as_str())
        );
        git(
            fixture.root(),
            &["commit", "--quiet", "--no-verify", "-am", "change"],
        );
        let BaseChoice::Compare(on_branch) = choose_base(fixture.root(), None).unwrap() else {
            panic!("a branch has a base");
        };
        assert_eq!(on_branch.commit, base);
        // firescale's own folder is not a change.
        git(fixture.root(), &["checkout", "--quiet", "main"]);
        git(fixture.root(), &["checkout", "--quiet", "-b", "other"]);
        fs::create_dir_all(fixture.root().join(".firescale/steps")).unwrap();
        fs::write(fixture.root().join(".firescale/steps/x.json"), "{}").unwrap();
        assert!(matches!(
            choose_base(fixture.root(), None).unwrap(),
            BaseChoice::Nothing(_)
        ));
        // An explicit --base is used on main too, and a bad one is an error.
        git(fixture.root(), &["checkout", "--quiet", "main"]);
        assert!(matches!(
            choose_base(fixture.root(), Some(&base)).unwrap(),
            BaseChoice::Nothing(_)
        ));
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
    fn the_head_overlay_copies_changes_and_deletions_and_skips_firescale() {
        let fixture = committed();
        let root = fixture.root();
        fs::write(root.join("gone.txt"), "x\n").unwrap();
        git(root, &["add", "--all"]);
        git(root, &["commit", "--quiet", "--no-verify", "-m", "gone"]);
        let head = git_line(root, &["rev-parse", "HEAD"]).unwrap();
        fs::write(root.join("a.txt"), "two\n").unwrap();
        fs::remove_file(root.join("gone.txt")).unwrap();
        fs::create_dir_all(root.join("new dir")).unwrap();
        fs::write(root.join("new dir").join("b.txt"), "new\n").unwrap();
        fs::create_dir_all(root.join(".firescale")).unwrap();
        fs::write(root.join(".firescale").join("x.json"), "{}").unwrap();
        let worktree = Worktree::add(root, &head).unwrap();
        let stop = std::sync::atomic::AtomicBool::new(false);
        overlay_head(root, &worktree.root, None, &stop).unwrap();
        assert_eq!(
            fs::read_to_string(worktree.root.join("a.txt")).unwrap(),
            "two\n"
        );
        assert!(!worktree.root.join("gone.txt").exists());
        assert_eq!(
            fs::read_to_string(worktree.root.join("new dir").join("b.txt")).unwrap(),
            "new\n"
        );
        assert!(!worktree.root.join(".firescale").exists());
        // The user's tree is as it was.
        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "two\n");
    }

    #[cfg(unix)]
    #[test]
    fn the_head_overlay_never_writes_through_a_link_the_commit_holds() {
        let outside = Fixture::new(&[("keep.txt", "mine\n")], Track::NoGit).unwrap();
        let fixture = committed();
        let root = fixture.root();
        // HEAD tracks `lib` as a link to a folder outside the repo...
        std::os::unix::fs::symlink(outside.root(), root.join("lib")).unwrap();
        git(root, &["add", "--all"]);
        git(root, &["commit", "--quiet", "--no-verify", "-m", "link"]);
        let head = git_line(root, &["rev-parse", "HEAD"]).unwrap();
        // ...and the working tree has a real folder there, with a new file.
        fs::remove_file(root.join("lib")).unwrap();
        fs::create_dir_all(root.join("lib")).unwrap();
        fs::write(root.join("lib").join("a.txt"), "head\n").unwrap();
        fs::write(root.join("lib").join("keep.txt"), "head\n").unwrap();
        let worktree = Worktree::add(root, &head).unwrap();
        let stop = std::sync::atomic::AtomicBool::new(false);
        overlay_head(root, &worktree.root, None, &stop).unwrap();
        assert!(!outside.root().join("a.txt").exists());
        assert_eq!(
            fs::read_to_string(outside.root().join("keep.txt")).unwrap(),
            "mine\n"
        );
        assert_eq!(
            fs::read_to_string(worktree.root.join("lib").join("a.txt")).unwrap(),
            "head\n"
        );
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

    #[cfg(unix)]
    #[test]
    fn a_node_modules_with_a_workspace_link_is_never_linked() {
        let fixture = committed();
        let root = fixture.root();
        fs::create_dir_all(root.join("packages/lib")).unwrap();
        fs::create_dir_all(root.join("node_modules/@scope")).unwrap();
        fs::create_dir_all(root.join("node_modules/left-pad")).unwrap();
        fs::write(root.join(".gitignore"), "node_modules/\n").unwrap();
        assert!(!links_into_repo(root, &root.join("node_modules")));
        std::os::unix::fs::symlink("../../packages/lib", root.join("node_modules/@scope/lib"))
            .unwrap();
        assert!(links_into_repo(root, &root.join("node_modules")));
        assert!(dependency_folders(root).is_empty());
        // An editable install in a virtual environment.
        let site = root.join(".venv/lib/python3.12/site-packages");
        fs::create_dir_all(&site).unwrap();
        fs::write(root.join(".gitignore"), "node_modules/\n.venv/\n").unwrap();
        assert!(!links_into_repo(root, &root.join(".venv")));
        fs::write(
            site.join("__editable__.app-0.1.pth"),
            format!("{}\n", root.join("src").to_string_lossy()),
        )
        .unwrap();
        assert!(links_into_repo(root, &root.join(".venv")));
    }
}
