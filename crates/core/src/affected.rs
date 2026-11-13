//! `--affected`: narrowing a report to what a change touches.
//!
//! A file is affected when it changed, or when it belongs to a project that
//! changed or that depends, directly or not, on a project that changed.

use std::collections::BTreeSet;
use std::path::Path;

use crate::error::{Error, Result};
use crate::facts::{Project, RepoFacts};
use crate::git::run_git;
use crate::model::Status;
use crate::report::CheckReport;

/// The files that differ from the comparison point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Changed {
    /// There is nothing to compare against (not a git repo, or no commit yet),
    /// so every file counts as changed.
    Everything,
    /// A shallow clone that does not hold the merge base with `--base`:
    /// every file counts as changed (ADR 0041). Holds the note to show.
    Shallow(String),
    Files(BTreeSet<String>),
}

fn names(root: &Path, args: &[&str]) -> Result<Option<Vec<String>>> {
    let output = run_git(root, args)?;
    if output.code != 0 {
        return Ok(None);
    }
    Ok(Some(
        output
            .stdout
            .split('\0')
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect(),
    ))
}

/// Paths are relative to `root`, even when `root` is a folder inside a larger
/// git repo (`--relative`), so they compare with diagnostic paths.
///
/// Lists changed files: uncommitted changes (staged or not) and untracked
/// files, plus, when `base` is given, everything committed since the merge
/// base with that ref.
pub fn changed_files(root: &Path, base: Option<&str>) -> Result<Changed> {
    let has_head = run_git(root, &["rev-parse", "--verify", "--quiet", "HEAD"])?.code == 0;
    if !has_head {
        return Ok(Changed::Everything);
    }
    let mut files = BTreeSet::new();
    if let Some(base) = base {
        let range = format!("{base}...HEAD");
        let Some(committed) = names(
            root,
            &[
                "diff",
                "--name-only",
                "--relative",
                "--no-renames",
                "-z",
                &range,
            ],
        )?
        else {
            if crate::git::is_shallow(root) {
                return Ok(Changed::Shallow(crate::git::shallow_note(base)));
            }
            return Err(Error::user(format!(
                "--base \"{base}\" is not a commit git can compare with HEAD. Pass a branch, tag or commit that exists, for example --base origin/main."
            )));
        };
        files.extend(committed);
    }
    for args in [
        &[
            "diff",
            "--name-only",
            "--relative",
            "--no-renames",
            "-z",
            "HEAD",
        ][..],
        &["ls-files", "-z", "--others", "--exclude-standard"][..],
    ] {
        files.extend(names(root, args)?.unwrap_or_default());
    }
    Ok(Changed::Files(files))
}

/// The project that owns a file: the one with the deepest root containing it.
fn owner<'a>(projects: &'a [Project], file: &str) -> Option<&'a Project> {
    projects
        .iter()
        .filter(|project| {
            project.root == "."
                || file == project.root
                || file.starts_with(&format!("{}/", project.root))
        })
        .max_by_key(|project| {
            if project.root == "." {
                0
            } else {
                project.root.len()
            }
        })
}

/// Names of the projects that changed or depend on one that changed.
fn affected_projects(facts: &RepoFacts, changed: &BTreeSet<String>) -> BTreeSet<String> {
    let mut affected: BTreeSet<String> = changed
        .iter()
        .filter_map(|file| owner(&facts.projects, file))
        .map(|project| project.name.clone())
        .collect();
    loop {
        let more: Vec<String> = facts
            .projects
            .iter()
            .filter(|project| !affected.contains(&project.name))
            .filter(|project| {
                project
                    .depends_on
                    .iter()
                    .any(|name| affected.contains(name))
            })
            .map(|project| project.name.clone())
            .collect();
        if more.is_empty() {
            return affected;
        }
        affected.extend(more);
    }
}

/// Keeps only the diagnostics on affected files and recomputes the counts.
pub fn narrow_report(report: &mut CheckReport, facts: &RepoFacts, changed: &Changed) {
    let Changed::Files(changed) = changed else {
        return;
    };
    let projects = affected_projects(facts, changed);
    report.diagnostics.retain(|diagnostic| {
        changed.contains(&diagnostic.file)
            || owner(&facts.projects, &diagnostic.file)
                .is_some_and(|project| projects.contains(&project.name))
    });
    for rule in &mut report.rules {
        rule.violations = report
            .diagnostics
            .iter()
            .filter(|d| d.rule_id == rule.id)
            .count();
    }
    let enforced = report
        .diagnostics
        .iter()
        .filter(|d| d.status == Status::Enforced)
        .count();
    report.summary.violations = report.diagnostics.len();
    report.summary.enforced_violations = enforced;
    report.ok = enforced == 0;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str, root: &str, depends_on: &[&str]) -> Project {
        Project {
            name: name.to_owned(),
            root: root.to_owned(),
            kind: "node".to_owned(),
            depends_on: depends_on.iter().map(|name| (*name).to_owned()).collect(),
            ..Project::default()
        }
    }

    fn set(files: &[&str]) -> BTreeSet<String> {
        files.iter().map(|file| (*file).to_owned()).collect()
    }

    fn graph() -> Vec<Project> {
        vec![
            project("root", ".", &[]),
            project("lib", "packages/lib", &[]),
            project("app", "packages/app", &["lib"]),
            project("site", "packages/site", &["app"]),
            project("other", "packages/other", &[]),
        ]
    }

    #[test]
    fn a_file_belongs_to_the_deepest_project_that_contains_it() {
        let projects = graph();
        assert_eq!(
            owner(&projects, "packages/lib/src/a.ts").map(|p| p.name.as_str()),
            Some("lib")
        );
        assert_eq!(
            owner(&projects, "README.md").map(|p| p.name.as_str()),
            Some("root")
        );
        assert_eq!(
            owner(&projects, "packages/library/a.ts").map(|p| p.name.as_str()),
            Some("root")
        );
        assert!(owner(&projects[1..], "README.md").is_none());
    }

    #[test]
    fn dependents_of_a_changed_project_are_affected_transitively() {
        let facts = RepoFacts {
            projects: graph(),
            ..RepoFacts::default()
        };
        let affected = affected_projects(&facts, &set(&["packages/lib/src/a.ts"]));
        assert_eq!(affected, set(&["app", "lib", "site"]));
        let affected = affected_projects(&facts, &set(&["packages/site/x.ts"]));
        assert_eq!(affected, set(&["site"]));
    }
}
