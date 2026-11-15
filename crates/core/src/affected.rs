//! `--affected`: narrowing a report to what a change touches.
//!
//! A file is affected when it changed, or when it belongs to a project that
//! changed or that depends, directly or not, on a project that changed.
//! A finding on a file that belongs to no project below the repo root is also
//! kept when a changed file is an input of its rule.

use std::collections::BTreeSet;
use std::path::Path;

use crate::cache::input_matcher;
use crate::error::{Error, Result};
use crate::facts::{Project, RepoFacts};
use crate::git::run_git;
use crate::model::{RuleMeta, Status};
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
            if crate::git::shallow_without_merge_base(root, base) {
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

/// Keeps only the diagnostics a change can have caused and recomputes the
/// counts. A diagnostic is kept when its file changed, when its file belongs
/// to an affected project, or when its file belongs to no project below the
/// repo root and a changed file is an input of its rule. The last case is
/// for findings on repo-level files: a new script in `web/package.json`
/// makes `AGENTS.md` stale, and neither `AGENTS.md` nor its owner changed.
pub fn narrow_report(
    report: &mut CheckReport,
    facts: &RepoFacts,
    changed: &Changed,
    rules: &[&RuleMeta],
) {
    let Changed::Files(changed) = changed else {
        return;
    };
    let projects = affected_projects(facts, changed);
    // The rules that read a changed file. Inputs that cannot be read as
    // globs count as matching: keeping too much is the safe side.
    let reads_a_change: BTreeSet<&str> = rules
        .iter()
        .filter(|meta| {
            input_matcher(&meta.id, &meta.inputs).map_or(true, |matcher| {
                changed.iter().any(|file| matcher.is_match(file.as_str()))
            })
        })
        .map(|meta| meta.id.as_str())
        .collect();
    report.diagnostics.retain(|diagnostic| {
        if changed.contains(&diagnostic.file) {
            return true;
        }
        let owner = owner(&facts.projects, &diagnostic.file);
        owner.is_some_and(|project| projects.contains(&project.name))
            || (owner.is_none_or(|project| project.root == ".")
                && reads_a_change.contains(diagnostic.rule_id.as_str()))
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
    use crate::model::{Category, Fixable, Severity};
    use crate::report::{ReportedDiagnostic, Summary};

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

    fn rule(id: &str, inputs: &[&str]) -> RuleMeta {
        RuleMeta {
            id: id.to_owned(),
            severity: Severity::Medium,
            category: Category::Consistency,
            description: String::new(),
            inputs: inputs.iter().map(|input| (*input).to_owned()).collect(),
            has_fix: false,
            after: Vec::new(),
        }
    }

    fn report(findings: &[(&str, &str)]) -> CheckReport {
        CheckReport {
            schema_version: 1,
            version: "0.0.0".to_owned(),
            ok: false,
            summary: Summary {
                rules: 0,
                violations: findings.len(),
                enforced_violations: findings.len(),
            },
            rules: Vec::new(),
            diagnostics: findings
                .iter()
                .map(|(rule_id, file)| ReportedDiagnostic {
                    rule_id: (*rule_id).to_owned(),
                    message: String::new(),
                    file: (*file).to_owned(),
                    fixable: Fixable::Manual,
                    range: None,
                    project: None,
                    status: Status::Enforced,
                    severity: Severity::Medium,
                })
                .collect(),
        }
    }

    fn kept(
        projects: Vec<Project>,
        changed: &[&str],
        rules: &[RuleMeta],
        findings: &[(&str, &str)],
    ) -> Vec<String> {
        let facts = RepoFacts {
            projects,
            ..RepoFacts::default()
        };
        let mut report = report(findings);
        let rules: Vec<&RuleMeta> = rules.iter().collect();
        narrow_report(&mut report, &facts, &Changed::Files(set(changed)), &rules);
        assert_eq!(report.summary.violations, report.diagnostics.len());
        assert_eq!(report.ok, report.diagnostics.is_empty());
        report
            .diagnostics
            .iter()
            .map(|d| format!("{} {}", d.rule_id, d.file))
            .collect()
    }

    /// The rules of the tests below: one reads every package.json and reports
    /// a repo-level file, one reads only env files.
    fn rules() -> Vec<RuleMeta> {
        vec![
            rule("agent-ready/agents-md", &["AGENTS.md", "**/package.json"]),
            rule("core/no-committed-env", &["**/.env"]),
        ]
    }

    // The stop hook let an agent stop with a stale AGENTS.md: a new `lint`
    // script in web/package.json makes agents-md report AGENTS.md, which did
    // not change and which no affected project owns.
    #[test]
    fn a_repo_level_finding_is_kept_when_a_changed_file_is_an_input_of_its_rule() {
        let findings = [("agent-ready/agents-md", "AGENTS.md")];
        // No project at the repo root.
        let services = vec![project("web", "web", &[]), project("api", "api", &[])];
        assert_eq!(
            kept(services, &["web/package.json"], &rules(), &findings),
            ["agent-ready/agents-md AGENTS.md"]
        );
        // A root project that did not change owns AGENTS.md.
        assert_eq!(
            kept(graph(), &["packages/lib/package.json"], &rules(), &findings),
            ["agent-ready/agents-md AGENTS.md"]
        );
    }

    #[test]
    fn a_repo_level_finding_is_dropped_when_no_changed_file_is_an_input_of_its_rule() {
        let findings = [
            ("agent-ready/agents-md", "AGENTS.md"),
            ("core/no-committed-env", ".env"),
        ];
        assert_eq!(
            kept(graph(), &["packages/lib/src/a.ts"], &rules(), &findings),
            [] as [&str; 0]
        );
        // Only the rule that reads the changed file keeps its finding.
        assert_eq!(
            kept(graph(), &["packages/lib/package.json"], &rules(), &findings),
            ["agent-ready/agents-md AGENTS.md"]
        );
    }

    #[test]
    fn a_finding_inside_a_project_that_is_not_affected_is_still_dropped() {
        let findings = [
            ("core/no-committed-env", "packages/lib/.env"),
            ("core/no-committed-env", "packages/other/.env"),
        ];
        // The new file is an input of the rule, and other is still left out.
        assert_eq!(
            kept(graph(), &["packages/lib/sub/.env"], &rules(), &findings),
            ["core/no-committed-env packages/lib/.env"]
        );
    }

    #[test]
    fn a_rule_whose_inputs_cannot_be_read_keeps_its_repo_level_findings() {
        let broken = [rule("custom/broken", &["["])];
        assert_eq!(
            kept(
                graph(),
                &["packages/lib/src/a.ts"],
                &broken,
                &[("custom/broken", "AGENTS.md")]
            ),
            ["custom/broken AGENTS.md"]
        );
    }
}
