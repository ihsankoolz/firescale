//! `core/ci-exists`: A CI workflow exists and runs on pull requests.

use std::fs;

use std::path::{Path, PathBuf};

use firescale_core::facts::detect_facts;
use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

#[cfg(test)]
mod tests;
pub(crate) mod workflow;

pub const ID: &str = "core/ci-exists";

const WORKFLOWS: &str = ".github/workflows";

/// The only file the fix ever creates.
pub(crate) const STARTER: &str = ".github/workflows/ci.yml";

/// Configs of other CI systems. The rule only understands GitHub Actions
/// triggers, so any of these at the root means there is nothing to report.
const OTHER_CI: [&str; 14] = [
    ".gitlab-ci.yml",
    ".circleci/config.yml",
    "azure-pipelines.yml",
    "bitbucket-pipelines.yml",
    "Jenkinsfile",
    ".travis.yml",
    ".gitlab-ci.yaml",
    ".circleci/config.yaml",
    ".drone.yml",
    ".woodpecker.yml",
    ".buildkite/pipeline.yml",
    ".buildkite/pipeline.yaml",
    "appveyor.yml",
    ".appveyor.yml",
];

pub struct CiExists;

pub(crate) fn is_workflow(file: &str) -> bool {
    file.strip_prefix(".github/workflows/").is_some_and(|rest| {
        !rest.contains('/') && (rest.ends_with(".yml") || rest.ends_with(".yaml"))
    })
}

fn on_disk(root: &Path, file: &str) -> PathBuf {
    file.split('/')
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

/// True when something is at this path, whatever it is. A broken symlink counts.
fn occupied(path: &Path) -> bool {
    path.symlink_metadata().is_ok()
}

/// Names of the entries of `.github/workflows/` on disk, sorted. Empty when
/// the folder does not exist; `None` when it exists and cannot be listed.
fn workflow_folder_names(root: &Path) -> Option<Vec<String>> {
    let entries = match fs::read_dir(on_disk(root, WORKFLOWS)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(Vec::new()),
        Err(_) => return None,
    };
    let mut names = Vec::new();
    for entry in entries {
        names.push(entry.ok()?.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    Some(names)
}

/// CI configs that are on disk, tracked or not: workflow files first, then
/// the configs of other CI systems. Sorted within each group.
fn ci_files_on_disk(root: &Path) -> Vec<String> {
    let mut files: Vec<String> = workflow_folder_names(root)
        .unwrap_or_default()
        .into_iter()
        .map(|name| format!("{WORKFLOWS}/{name}"))
        .filter(|file| is_workflow(file) && on_disk(root, file).is_file())
        .collect();
    let mut others: Vec<String> = OTHER_CI
        .iter()
        .filter(|other| on_disk(root, other).is_file())
        .map(|other| (*other).to_owned())
        .collect();
    others.sort();
    files.extend(others);
    files
}

/// The fix's own precondition, also used by the check so that a finding is
/// only marked deterministic when the fix will act: nothing on disk is, or
/// could be mistaken for, a workflow, and the starter's path is free.
fn starter_can_be_created(root: &Path) -> bool {
    // A folder that cannot be listed may hold anything.
    let Some(names) = workflow_folder_names(root) else {
        return false;
    };
    let no_workflow = names.iter().all(|name| {
        let lower = name.to_lowercase();
        !(lower.ends_with(".yml") || lower.ends_with(".yaml"))
    });
    // Each folder on the way is a real folder or absent: not a file, and not
    // a link, which would lead the write out of the repo.
    let folders_free = [".github", WORKFLOWS].iter().all(|folder| {
        on_disk(root, folder)
            .symlink_metadata()
            .map_or(true, |meta| meta.is_dir())
    });
    no_workflow
        && folders_free
        && !occupied(&on_disk(root, STARTER))
        && OTHER_CI
            .iter()
            .all(|other| !occupied(&on_disk(root, other)))
}

/// A CI config that git tracks but the disk does not have. It may hold
/// staged work, so nothing is created while there is one.
fn tracked_but_missing(ctx: &RuleContext) -> Result<Option<String>> {
    Ok(ctx
        .git()
        .tracked_files()?
        .into_iter()
        .filter(|file| is_workflow(file) || OTHER_CI.contains(&file.as_str()))
        .find(|file| !occupied(&on_disk(ctx.root(), file))))
}

/// Why the starter cannot be created, as `(message, fixable)`, or `None`
/// when it can. Asked by the check and again by the fix.
pub(crate) fn starter_blocker(ctx: &RuleContext) -> Result<Option<(String, Fixable)>> {
    let in_repo = ctx.git().is_repo()?;
    if in_repo {
        if let Some(missing) = tracked_but_missing(ctx)? {
            return Ok(Some((
                format!(
                    "\"{missing}\" is tracked by git but missing from disk. Restore it with `git restore {missing}`, or commit its deletion."
                ),
                Fixable::Manual,
            )));
        }
        if let Some(untracked) = ci_files_on_disk(ctx.root()).first() {
            return Ok(Some((
                format!(
                    "\"{untracked}\" exists but is not tracked by git, so it does not count as CI. Run `git add {untracked}` and commit it."
                ),
                Fixable::Manual,
            )));
        }
    }
    if !starter_can_be_created(ctx.root()) {
        return Ok(Some((
            "No CI workflow found in .github/workflows. Add a workflow that runs your build and tests on pull requests (see SKILL.md).".to_owned(),
            Fixable::Agent,
        )));
    }
    if in_repo && ctx.git().is_ignored(STARTER)? {
        return Ok(Some((
            format!(
                "No CI workflow found in .github/workflows, and a .gitignore rule ignores {STARTER}, so one could not be committed. Remove that rule, then add a workflow."
            ),
            Fixable::Manual,
        )));
    }
    Ok(None)
}

/// CI configs that count. In a git repo: tracked files that are also on disk
/// (a tracked file deleted from the working tree is absent). Otherwise a
/// plain listing of the places that matter.
pub(crate) fn candidate_files(ctx: &RuleContext) -> Result<Vec<String>> {
    if ctx.git().is_repo()? {
        return Ok(ctx
            .git()
            .tracked_files()?
            .into_iter()
            .filter(|file| is_workflow(file) || OTHER_CI.contains(&file.as_str()))
            .filter(|file| on_disk(ctx.root(), file).is_file())
            .collect());
    }
    let mut files = ci_files_on_disk(ctx.root());
    files.sort();
    Ok(files)
}

/// Removes a `#` comment: a `#` at the start of the text or after whitespace.
pub(crate) fn strip_comment(line: &str) -> &str {
    let mut prev_space = true;
    for (i, c) in line.char_indices() {
        if c == '#' && prev_space {
            return &line[..i];
        }
        prev_space = c.is_whitespace();
    }
    line
}

fn unquote(s: &str) -> &str {
    let s = s.trim();
    for q in ['"', '\''] {
        if let Some(inner) = s.strip_prefix(q).and_then(|r| r.strip_suffix(q)) {
            return inner.trim();
        }
    }
    s
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Splits `key: value` at the first colon that ends the key.
fn key_of(entry: &str) -> &str {
    let key = entry.split(':').next().unwrap_or(entry);
    unquote(key)
}

/// The trigger names listed under the top-level `on` key.
///
/// Reads these shapes: scalar, flow list, flow mapping, block list and block
/// mapping (first nesting level only).
fn triggers(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().map(strip_comment).collect();
    let mut out = Vec::new();
    let Some(start) = lines.iter().position(|l| {
        let t = l.trim_end();
        indent_of(t) == 0 && ["on:", "\"on\":", "'on':"].iter().any(|p| t.starts_with(p))
    }) else {
        return out;
    };
    let head = lines[start].trim_end();
    let rest = head[head.find(':').map_or(0, |i| i + 1)..].trim();
    if !rest.is_empty() {
        if let Some(inner) = rest.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            out.extend(inner.split(',').map(|i| unquote(i).to_owned()));
        } else if let Some(inner) = rest.strip_prefix('{').and_then(|r| r.strip_suffix('}')) {
            out.extend(inner.split(',').map(|i| key_of(i).to_owned()));
        } else {
            out.push(unquote(rest).to_owned());
        }
        return out;
    }
    let mut level: Option<usize> = None;
    for line in &lines[start + 1..] {
        if line.trim().is_empty() {
            continue;
        }
        let indent = indent_of(line);
        let t = line.trim();
        if indent == 0 && !t.starts_with('-') {
            break;
        }
        let lvl = *level.get_or_insert(indent);
        if indent != lvl {
            continue;
        }
        match t.strip_prefix('-') {
            Some(item) => out.push(unquote(item).to_owned()),
            None => out.push(key_of(t).to_owned()),
        }
    }
    out
}

/// True when the workflow's `on` key lists a pull request trigger.
pub(crate) fn mentions_pull_request(text: &str) -> bool {
    triggers(text)
        .iter()
        .any(|t| t == "pull_request" || t == "pull_request_target")
}

/// Creates the starter workflow, which runs `npx firescale verify` on pull
/// requests. The caller has checked that nothing is in the way. Shared with
/// `group-project/ci-verify`, so either rule's fix writes the same file.
pub(crate) fn create_starter(ctx: &RuleContext) -> Result<FixResult> {
    let setup = current_setup(ctx)?;
    ctx.create_tracked(STARTER, &workflow::workflow(&setup))?;
    let mut changed_files = vec![STARTER.to_owned()];
    let mut notes = vec![format!(
        "Review {STARTER}: it runs `npx firescale verify` (only what a pull request changes, against its base) and keeps a shared step cache written from the default branch only. Add anything your project needs before that step (services, secrets, other toolchains)."
    )];
    if !occupied(&on_disk(ctx.root(), workflow::TAMPER_PATH)) {
        ctx.create_tracked(workflow::TAMPER_PATH, &workflow::tamper_workflow())?;
        changed_files.push(workflow::TAMPER_PATH.to_owned());
        notes.push(TAMPER_NOTE.to_owned());
    }
    notes.extend(setup.notes());
    Ok(FixResult {
        changed_files,
        notes,
    })
}

const TAMPER_NOTE: &str = "Review .github/workflows/tamper-guard.yml: on pull_request_target it runs `fscale tamper --pr-base` as the default branch defines it, reading the pull request's commits without running them (ADR 0045). Require its tamper-guard check in branch protection, and set verify.tamper to \"enforced\", to make it block.";

/// The tamper guard's workflow to write now: when the CI workflow is one
/// firescale wrote and nobody has edited, and the tamper guard's file is
/// missing or is firescale's, unedited and older. `(content, create)`.
fn newer_tamper(ctx: &RuleContext) -> Result<Option<(String, bool)>> {
    let tracked = ctx.git().is_repo()? && ctx.git().tracked_files()?.iter().any(|f| f == STARTER);
    if !tracked
        || !ctx
            .read(STARTER)?
            .is_some_and(|text| workflow::firescale_wrote(&text))
    {
        return Ok(None);
    }
    let path = on_disk(ctx.root(), workflow::TAMPER_PATH);
    let newer = workflow::tamper_workflow();
    if !occupied(&path) {
        let ignored = ctx.git().is_ignored(workflow::TAMPER_PATH)?;
        return Ok((!ignored).then_some((newer, true)));
    }
    if path.symlink_metadata().is_ok_and(|meta| !meta.is_file()) {
        return Ok(None);
    }
    let Some(text) = ctx.read(workflow::TAMPER_PATH)? else {
        return Ok(None);
    };
    Ok(
        (workflow::firescale_wrote(&text) && text.replace("\r\n", "\n") != newer)
            .then_some((newer, false)),
    )
}

/// What the workflow sets up for this repo now.
fn current_setup(ctx: &RuleContext) -> Result<workflow::Setup> {
    let facts = detect_facts(ctx.root())?;
    let has_python_version = on_disk(ctx.root(), ".python-version").is_file();
    let mut setup = workflow::Setup::from_facts(&facts, has_python_version);
    setup.default_branch = default_branch(ctx)?;
    Ok(setup)
}

/// The repo's default branch, when it can be told: `origin/HEAD`, else the
/// branch of a repo that has no other branch yet (a new repo). `None`
/// otherwise, and for a name that would need quoting in YAML.
fn default_branch(ctx: &RuleContext) -> Result<Option<String>> {
    if !ctx.git().is_repo()? {
        return Ok(None);
    }
    let line = |args: &[&str]| -> Option<String> {
        firescale_core::git::run_git(ctx.root(), args)
            .ok()
            .filter(|out| out.code == 0)
            .map(|out| out.stdout.trim().to_owned())
    };
    let remote = line(&[
        "symbolic-ref",
        "--quiet",
        "--short",
        "refs/remotes/origin/HEAD",
    ])
    .and_then(|head| head.strip_prefix("origin/").map(str::to_owned));
    let branch = match remote {
        Some(branch) => Some(branch),
        None => {
            let branches = line(&["for-each-ref", "--format=%(refname:short)", "refs/heads/"])
                .unwrap_or_default();
            if branches.lines().count() <= 1 {
                ctx.git().current_branch()?
            } else {
                None
            }
        }
    };
    Ok(branch.filter(|name| {
        !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'/'))
    }))
}

/// The workflow firescale would write now, when the tracked starter is one
/// firescale wrote and nobody has edited, and it differs: `fscale sync`
/// offers it. A workflow a person wrote, or edited, is never rewritten.
fn newer_starter(ctx: &RuleContext) -> Result<Option<String>> {
    let path = on_disk(ctx.root(), STARTER);
    let linked = path.symlink_metadata().is_ok_and(|meta| !meta.is_file());
    if linked || !ctx.git().is_repo()? || !ctx.git().tracked_files()?.iter().any(|f| f == STARTER) {
        return Ok(None);
    }
    let Some(text) = ctx.read(STARTER)? else {
        return Ok(None);
    };
    if !workflow::firescale_wrote(&text) {
        return Ok(None);
    }
    let newer = workflow::workflow(&current_setup(ctx)?);
    Ok((text.replace("\r\n", "\n") != newer).then_some(newer))
}

impl BuiltinRule for CiExists {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Reliability,
            description: "A CI workflow exists and runs on pull requests.".to_owned(),
            // `.gitignore` and a plain file named `.github` decide whether the
            // starter can be created, so they are inputs too.
            inputs: [".github/workflows/*.yml", ".github/workflows/*.yaml"]
                .iter()
                .chain(OTHER_CI.iter())
                .chain([".gitignore", "**/.gitignore", ".github"].iter())
                .map(|s| (*s).to_owned())
                .collect(),
            has_fix: true,
            after: Vec::new(),
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        let files = candidate_files(ctx)?;
        let workflows: Vec<&String> = files.iter().filter(|f| is_workflow(f)).collect();
        if workflows.is_empty() {
            if files.iter().any(|f| OTHER_CI.contains(&f.as_str())) {
                return Ok(Vec::new());
            }
            let finding = |message: String, fixable: Fixable| Finding {
                message,
                file: WORKFLOWS.to_owned(),
                range: None,
                project: None,
                fixable,
                severity: None,
            };
            let (message, fixable) = starter_blocker(ctx)?.unwrap_or_else(|| {
                (
                    "No CI workflow found in .github/workflows. Add a workflow that runs your build and tests on pull requests; `fscale fix` can add a starter one.".to_owned(),
                    Fixable::Deterministic,
                )
            });
            return Ok(vec![finding(message, fixable)]);
        }
        if newer_starter(ctx)?.is_some() || newer_tamper(ctx)?.is_some() {
            return Ok(vec![Finding {
                message: format!(
                    "{STARTER} is a workflow firescale wrote and nobody has edited, and a newer one exists: it runs only the projects a pull request changes, judged against its base, and keeps a shared step cache written from the default branch only. `fscale sync` replaces it; a workflow you edited is never rewritten."
                ),
                file: STARTER.to_owned(),
                range: None,
                project: None,
                fixable: Fixable::Deterministic,
                severity: Some(Severity::Low),
            }]);
        }
        for file in &workflows {
            if let Some(text) = ctx.read(file)? {
                if mentions_pull_request(&text) {
                    return Ok(Vec::new());
                }
            }
        }
        let first = workflows[0].clone();
        Ok(vec![Finding {
            message: format!(
                "No workflow in .github/workflows runs on pull requests. Add a `pull_request` trigger under `on:` in \"{first}\" so changes are checked before they merge."
            ),
            file: first,
            range: None,
            project: None,
            fixable: Fixable::Agent,
            severity: None,
        }])
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        // Never trust the diagnostics alone: look at the disk again, and only
        // create a file where there is none.
        let newer_ci = newer_starter(ctx)?;
        let newer_guard = newer_tamper(ctx)?;
        if newer_ci.is_some() || newer_guard.is_some() {
            let mut result = FixResult::default();
            if let Some(newer) = newer_ci {
                ctx.write(STARTER, &newer)?;
                result.changed_files.push(STARTER.to_owned());
                result.notes.push(format!(
                    "{STARTER} was a workflow firescale wrote, unedited; it is now the newer one (only what a pull request changes, and a shared step cache). Review it."
                ));
            }
            match newer_guard {
                Some((text, true)) => {
                    ctx.create_tracked(workflow::TAMPER_PATH, &text)?;
                    result.changed_files.push(workflow::TAMPER_PATH.to_owned());
                    result.notes.push(TAMPER_NOTE.to_owned());
                }
                Some((text, false)) => {
                    ctx.write(workflow::TAMPER_PATH, &text)?;
                    result.changed_files.push(workflow::TAMPER_PATH.to_owned());
                }
                None => {}
            }
            return Ok(result);
        }
        if !candidate_files(ctx)?.is_empty() || starter_blocker(ctx)?.is_some() {
            return Ok(FixResult::default());
        }
        create_starter(ctx)
    }
}
