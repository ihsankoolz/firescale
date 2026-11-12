//! `core/ci-exists`: A CI workflow exists and runs on pull requests.

use std::fs;

use std::path::{Path, PathBuf};

use firescale_core::facts::detect_facts;
use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::install::ci_install_commands;

#[cfg(test)]
mod tests;

pub const ID: &str = "core/ci-exists";

const WORKFLOWS: &str = ".github/workflows";

/// The only file the fix ever creates.
const STARTER: &str = ".github/workflows/ci.yml";

/// Pinned to commit SHAs. The comment is the tag the SHA belongs to.
const CHECKOUT: &str = "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1";
const SETUP_NODE: &str = "actions/setup-node@949feb2413d6458794dcd2491c4babbbce0c15c1 # v7.1.0";

const INSTALL_PLACEHOLDER: &str = "# Add the step that installs your dependencies here.";

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

fn is_workflow(file: &str) -> bool {
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
fn starter_blocker(ctx: &RuleContext) -> Result<Option<(String, Fixable)>> {
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
fn candidate_files(ctx: &RuleContext) -> Result<Vec<String>> {
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

/// The starter workflow. LF line endings.
fn starter_workflow(install: Option<&[&str]>, has_package_manager: bool) -> String {
    let mut lines: Vec<String> = [
        "# Created by firescale (core/ci-exists). This file belongs to your team:",
        "# edit it freely. firescale will not rewrite it.",
        "name: CI",
        "",
        "on:",
        "  pull_request:",
        "  push:",
        "    branches: [main, master]",
        "",
        "permissions:",
        "  contents: read",
        "",
        "jobs:",
        "  verify:",
        "    runs-on: ubuntu-latest",
        "    steps:",
    ]
    .iter()
    .map(|line| (*line).to_owned())
    .collect();
    lines.push(format!("      - uses: {CHECKOUT}"));
    lines.push(format!("      - uses: {SETUP_NODE}"));
    lines.push("        with:".to_owned());
    lines.push("          node-version: 24".to_owned());
    match install {
        Some(commands) => {
            lines.extend(
                commands
                    .iter()
                    .map(|command| format!("      - run: {command}")),
            );
        }
        None if has_package_manager => lines.push(format!("      {INSTALL_PLACEHOLDER}")),
        None => {}
    }
    lines.push("      # Runs check, lint, typecheck and test.".to_owned());
    lines.push("      - run: npx firescale verify".to_owned());
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// Removes a `#` comment: a `#` at the start of the text or after whitespace.
fn strip_comment(line: &str) -> &str {
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
fn mentions_pull_request(text: &str) -> bool {
    triggers(text)
        .iter()
        .any(|t| t == "pull_request" || t == "pull_request_target")
}

impl BuiltinRule for CiExists {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Reliability,
            description: "A CI workflow exists and runs on pull requests.".to_owned(),
            inputs: [".github/workflows/*.yml", ".github/workflows/*.yaml"]
                .iter()
                .chain(OTHER_CI.iter())
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
        if !candidate_files(ctx)?.is_empty() || starter_blocker(ctx)?.is_some() {
            return Ok(FixResult::default());
        }
        let facts = detect_facts(ctx.root())?;
        let install = facts
            .package_manager
            .as_deref()
            .and_then(|manager| ci_install_commands(manager, &facts.lockfiles));
        let has_package_manager = facts.package_manager.is_some();
        ctx.create_tracked(
            STARTER,
            &starter_workflow(install.as_deref(), has_package_manager),
        )?;
        let mut notes = vec![format!(
            "Review {STARTER}: it runs `npx firescale verify`. Add anything your project needs before that step (services, secrets, other toolchains)."
        )];
        if install.is_none() && has_package_manager {
            notes.push(format!(
                "{STARTER} has no install step: firescale does not know the install command for CI with your package manager. Add it where the comment says."
            ));
        }
        Ok(FixResult {
            changed_files: vec![STARTER.to_owned()],
            notes,
        })
    }
}
