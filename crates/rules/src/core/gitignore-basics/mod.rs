//! `core/gitignore-basics`: .gitignore exists and covers the detected stack.

use firescale_core::facts::detect_facts;
use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

#[cfg(test)]
mod tests;

pub const ID: &str = "core/gitignore-basics";

const GITIGNORE: &str = ".gitignore";

pub struct GitignoreBasics;

/// Log files, in every git repo: a log a tool writes into the repo (a saved
/// `verify.log`) is not committed, and `verify --affected` does not count it
/// as a change. The owner's decision after the 0.5.1 run on the course
/// project; a log is not on ADR 0034's list of files that run nothing.
const LOG_FILES: &str = "*.log";

/// Entries every repo of a detected language should ignore: names for the
/// languages, and for each marker-file ecosystem's project its build output
/// (ADR 0051) — a bare name for a project at the root, a path for one in a
/// subfolder (`native/build`), which git reads as that folder only. Then
/// `*.log`, whatever the languages.
fn required_entries(languages: &[String], kinds: &[(&str, &str)]) -> Vec<String> {
    let has = |name: &str| languages.iter().any(|language| language == name);
    let mut entries: Vec<String> = Vec::new();
    if has("javascript") || has("typescript") {
        entries.push("node_modules".to_owned());
    }
    if has("rust") {
        entries.push("target".to_owned());
    }
    if has("python") {
        entries.push("__pycache__".to_owned());
        entries.push(".venv".to_owned());
    }
    for (root, kind) in kinds {
        if let Some(eco) = firescale_core::ecosystems::ecosystem(kind) {
            for entry in eco.ignored {
                let path = if *root == "." {
                    (*entry).to_owned()
                } else {
                    format!("{root}/{entry}")
                };
                if !entries.contains(&path) {
                    entries.push(path);
                }
            }
        }
    }
    entries.push(LOG_FILES.to_owned());
    entries
}

/// The .gitignore line for `entry`: a folder as `name/`, the `*.log` pattern
/// as it is (`*.log/` would match folders only).
fn line(entry: &str) -> String {
    if entry == LOG_FILES {
        entry.to_owned()
    } else {
        format!("{entry}/")
    }
}

/// Covered when git, reading `gitignore` at the repo root, ignores `entry`
/// (a folder or file name, or a repo-relative path): globs, anchors and
/// negations count as git counts them (ADR 0051; it was a name match).
pub(crate) fn is_covered(gitignore: &str, entry: &str) -> bool {
    firescale_core::walk::gitignore_covers(gitignore, entry)
}

fn missing_entries(ctx: &RuleContext, gitignore: &str) -> Result<Vec<String>> {
    let facts = detect_facts(ctx.root())?;
    let kinds: Vec<(&str, &str)> = facts
        .projects
        .iter()
        .map(|project| (project.root.as_str(), project.kind.as_str()))
        .collect();
    let mut missing = Vec::new();
    for entry in required_entries(&facts.languages, &kinds) {
        // `*.log` must ignore files: a folder-only `*.log/` does not count.
        let covered = if entry == LOG_FILES {
            firescale_core::walk::gitignore_covers_file(gitignore, &entry)
        } else {
            is_covered(gitignore, &entry)
        };
        if covered {
            continue;
        }
        // A subfolder project's own .gitignore counts for its build output.
        if let Some((folder, name)) = entry.rsplit_once('/') {
            let own = ctx
                .read(&format!("{folder}/.gitignore"))?
                .unwrap_or_default();
            if is_covered(&own, name) {
                continue;
            }
        }
        missing.push(entry);
    }
    Ok(missing)
}

impl BuiltinRule for GitignoreBasics {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Consistency,
            description: ".gitignore exists and covers the detected stack.".to_owned(),
            // The stack comes from every project `fscale facts` finds, so the
            // marker files count wherever they are.
            inputs: [
                ".gitignore",
                "package.json",
                "Cargo.toml",
                "pyproject.toml",
                "requirements.txt",
                "tsconfig.json",
                "**/package.json",
                "**/Cargo.toml",
                "**/pyproject.toml",
                "**/requirements.txt",
                "**/tsconfig.json",
                "**/setup.py",
                "**/Pipfile",
                "**/go.mod",
                "go.mod",
                "setup.py",
                "Pipfile",
                // What decides whether a folder is searched, and whether a
                // requirements.txt in it makes a Python project.
                "**/.gitignore",
                "pnpm-workspace.yaml",
                "**/*.py",
                "**/pytest.ini",
                "**/tox.ini",
                "**/setup.cfg",
                "**/ruff.toml",
                "**/.ruff.toml",
                "**/mypy.ini",
                "**/alembic.ini",
            ]
            .iter()
            .chain(firescale_core::ecosystems::MARKER_GLOBS.iter())
            .map(|name| (*name).to_owned())
            .collect(),
            has_fix: true,
            after: Vec::new(),
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        if !ctx.git().is_repo()? {
            return Ok(Vec::new());
        }
        let finding = |message: String| Finding {
            message,
            file: GITIGNORE.to_owned(),
            range: None,
            project: None,
            fixable: Fixable::Deterministic,
            severity: None,
        };
        let Some(current) = ctx.read(GITIGNORE)? else {
            return Ok(vec![finding(
                "There is no .gitignore at the repo root. Create one that ignores build output and dependencies; `fscale fix` can do it."
                    .to_owned(),
            )]);
        };
        Ok(missing_entries(ctx, &current)?
            .into_iter()
            .map(|entry| {
                finding(format!(
                    ".gitignore does not ignore \"{entry}\". Add \"{}\" to .gitignore so generated files are not committed.",
                    line(&entry)
                ))
            })
            .collect())
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let existing = ctx.read(GITIGNORE)?;
        let current = existing.clone().unwrap_or_default();
        let missing = missing_entries(ctx, &current)?;
        if existing.is_some() && missing.is_empty() {
            return Ok(FixResult::default());
        }
        let eol = if current.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let separator = if current.is_empty() || current.ends_with('\n') {
            ""
        } else {
            eol
        };
        // The header once: a first pass with nothing to list (no package.json
        // yet) already wrote it.
        let header = "# Generated files and dependencies";
        let mut block = Vec::new();
        if !current.lines().any(|line| line.trim() == header) {
            block.push(header.to_owned());
        }
        block.extend(missing.iter().map(|entry| line(entry)));
        ctx.write(
            GITIGNORE,
            &format!("{current}{separator}{}{eol}", block.join(eol)),
        )?;
        Ok(FixResult {
            changed_files: vec![GITIGNORE.to_owned()],
            notes: Vec::new(),
        })
    }
}
