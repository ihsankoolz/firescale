//! The `python` stack pack (ADR 0030): rules whose fixes create and repair
//! what a Python project needs, so `fscale init --packs core,python` builds
//! a starter that installs with pip into a virtual environment and passes
//! `python -m pytest` and `python -m ruff check .`. Opt-in: add "python" to
//! `packs`.
//!
//! The pack works **per Python project**, as `fscale facts` finds them: a
//! repo with several services, each with its own `requirements.txt`,
//! `requirements-dev.txt`, `pytest.ini` and `.venv`, is checked service by
//! service, and nothing ever suggests one root `pyproject.toml`. Only a repo
//! with no Python project at all gets a starter at its root.
//!
//! A fix only creates a missing file or appends to `.gitignore`; it never
//! edits a requirements file or a `pyproject.toml` a person wrote.

use std::path::Path;

use firescale_core::facts::{detect_facts_with, Project};
use firescale_core::model::{Finding, FixResult, Fixable};
use firescale_core::rule::{BuiltinPack, RuleContext};
use firescale_core::Result;

#[path = "gitignore/mod.rs"]
pub mod gitignore;
#[path = "pytest-config/mod.rs"]
pub mod pytest_config;
#[path = "requirements/mod.rs"]
pub mod requirements;
#[path = "ruff-config/mod.rs"]
pub mod ruff_config;
#[path = "test-tools/mod.rs"]
pub mod test_tools;
#[path = "tests/mod.rs"]
pub mod tests_present;

#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod pack_tests;

/// The test and lint tools a new project pins, as on PyPI on 2026-11-12.
pub const PYTEST: &str = "pytest==9.2.0";
pub const RUFF: &str = "ruff==0.17.5";

/// What every Python rule's result depends on.
pub(crate) const INPUTS: [&str; 19] = [
    "**/*requirements*.txt",
    "**/requirements/*.txt",
    "**/pytest.toml",
    "**/.pytest.toml",
    "**/mypy.ini",
    "**/alembic.ini",
    "**/tests/**",
    "**/test/**",
    ".gitignore",
    "**/pyproject.toml",
    "**/setup.py",
    "**/setup.cfg",
    "**/Pipfile",
    "**/pytest.ini",
    "**/tox.ini",
    "**/ruff.toml",
    "**/.ruff.toml",
    "**/*.py",
    "package.json",
];

/// A folder the pack works on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    /// Repo-relative, `.` for the root.
    pub(crate) folder: String,
    /// The Python project facts found there, or `None` for the starter of a
    /// repo with no Python project yet.
    pub(crate) project: Option<Project>,
    /// True for the starter: no Python project yet, or one at the root that
    /// has no Python code yet (the starter's own files are being written).
    pub(crate) new: bool,
}

impl Target {
    /// `folder/file`, or `file` at the root.
    pub(crate) fn path(&self, file: &str) -> String {
        if self.folder == "." {
            file.to_owned()
        } else {
            format!("{}/{file}", self.folder)
        }
    }

    /// True when anything is at the path, a dangling link included, so a
    /// check never promises a fix that `create` will not make.
    pub(crate) fn exists(&self, ctx: &RuleContext, file: &str) -> bool {
        on_disk(ctx.root(), &self.path(file))
            .symlink_metadata()
            .is_ok()
    }

    /// True when something is at the path that is not a regular file.
    pub(crate) fn blocked(&self, ctx: &RuleContext, file: &str) -> bool {
        on_disk(ctx.root(), &self.path(file))
            .symlink_metadata()
            .is_ok_and(|meta| !meta.is_file())
    }

    /// True for the starter of a repo that has no Python project yet.
    pub(crate) fn is_new(&self) -> bool {
        self.new
    }
}

fn on_disk(root: &Path, file: &str) -> std::path::PathBuf {
    file.split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

/// Folders the Python pack never looks into.
const SKIPPED: [&str; 6] = [
    "node_modules",
    "venv",
    "env",
    "__pycache__",
    "target",
    "dist",
];

/// How deep the pack looks for Python folders, as facts does (ADR 0020).
const DEPTH: usize = 4;

/// What a walk of the repo found: folders with a requirements file or a
/// pyproject.toml, and whether there is any `.py` file at all.
struct Walk {
    declaring: Vec<String>,
    any_code: bool,
}

fn walk(root: &Path) -> Walk {
    let mut out = Walk {
        declaring: Vec::new(),
        any_code: false,
    };
    let mut stack = vec![(root.to_path_buf(), String::from("."), 0usize)];
    while let Some((dir, rel, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut declares = false;
        for entry in entries.filter_map(std::result::Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                if depth < DEPTH && !name.starts_with('.') && !SKIPPED.contains(&name.as_str()) {
                    let child = if rel == "." {
                        name.clone()
                    } else {
                        format!("{rel}/{name}")
                    };
                    stack.push((entry.path(), child, depth + 1));
                }
                continue;
            }
            let lower = name.to_lowercase();
            if lower.ends_with(".py") {
                out.any_code = true;
            }
            if (lower.starts_with("requirements") && lower.ends_with(".txt"))
                || lower == "pyproject.toml"
            {
                declares = true;
            }
        }
        if declares {
            out.declaring.push(rel);
        }
    }
    out
}

/// Every Python project: those `fscale facts` finds, and folders with a
/// requirements file or a pyproject.toml that it does not (a service with
/// no pytest.ini yet). Only a repo with no Python code at all is a new
/// starter at its root (review of M16).
pub(crate) fn targets(ctx: &RuleContext) -> Result<Vec<Target>> {
    let facts = detect_facts_with(ctx.root(), None)?;
    let found = walk(ctx.root());
    let mut targets: Vec<Target> = facts
        .projects
        .into_iter()
        .filter(|project| project.kind == "python")
        .map(|project| Target {
            folder: project.root.clone(),
            project: Some(project),
            new: false,
        })
        .collect();
    for folder in &found.declaring {
        // A folder inside a project facts found belongs to that project.
        let inside = targets.iter().any(|target| {
            target.folder == *folder
                || (target.folder != "." && folder.starts_with(&format!("{}/", target.folder)))
        });
        if !inside {
            targets.push(Target {
                folder: folder.clone(),
                project: None,
                new: false,
            });
        }
    }
    // No Python code anywhere, and nothing but the root: a starter still
    // being built, whatever requirements files it has so far.
    if !found.any_code && targets.iter().all(|target| target.folder == ".") {
        return Ok(vec![Target {
            folder: ".".to_owned(),
            project: None,
            new: true,
        }]);
    }
    if targets.is_empty() {
        targets.push(Target {
            folder: ".".to_owned(),
            project: None,
            new: false,
        });
    }
    targets.sort_by(|a, b| a.folder.cmp(&b.folder));
    Ok(targets)
}

/// The folders of all targets, to tell where one project ends.
pub(crate) fn target_folders(ctx: &RuleContext) -> Result<Vec<String>> {
    Ok(targets(ctx)?
        .into_iter()
        .map(|target| target.folder)
        .collect())
}

pub(crate) fn finding(target: &Target, file: &str, message: String, fixable: Fixable) -> Finding {
    Finding {
        message,
        file: target.path(file),
        range: None,
        project: (target.folder != ".").then(|| target.folder.clone()),
        fixable,
        severity: None,
    }
}

/// Creates a file firescale owns, or does nothing when anything is there.
pub(crate) fn create(
    ctx: &RuleContext,
    file: &str,
    content: &str,
    result: &mut FixResult,
) -> Result<()> {
    if on_disk(ctx.root(), file).symlink_metadata().is_ok() {
        return Ok(());
    }
    ctx.create_tracked(file, content)?;
    result.changed_files.push(file.to_owned());
    Ok(())
}

#[must_use]
pub fn pack() -> BuiltinPack {
    BuiltinPack {
        name: "python",
        rules: vec![
            Box::new(requirements::Requirements),
            Box::new(test_tools::TestTools),
            Box::new(tests_present::TestsPresent),
            Box::new(pytest_config::PytestConfig),
            Box::new(ruff_config::RuffConfig),
            Box::new(gitignore::PythonGitignore),
        ],
    }
}
