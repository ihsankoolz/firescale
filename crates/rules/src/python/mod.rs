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
pub(crate) const INPUTS: [&str; 12] = [
    "**/requirements*.txt",
    "**/requirements/*.txt",
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

    pub(crate) fn exists(&self, ctx: &RuleContext, file: &str) -> bool {
        on_disk(ctx.root(), &self.path(file)).exists()
    }

    pub(crate) fn read(&self, ctx: &RuleContext, file: &str) -> Option<String> {
        std::fs::read_to_string(on_disk(ctx.root(), &self.path(file))).ok()
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

/// Every Python project, or the repo root as a new starter when there is none.
pub(crate) fn targets(ctx: &RuleContext) -> Result<Vec<Target>> {
    let facts = detect_facts_with(ctx.root(), None)?;
    let projects: Vec<Target> = facts
        .projects
        .into_iter()
        .filter(|project| project.kind == "python")
        .map(|project| Target {
            new: project.root == "." && !has_python_code(ctx.root()),
            folder: project.root.clone(),
            project: Some(project),
        })
        .collect();
    if projects.is_empty() {
        return Ok(vec![Target {
            folder: ".".to_owned(),
            project: None,
            new: true,
        }]);
    }
    Ok(projects)
}

/// True when the folder, or a folder right below it, has a `.py` file
/// (virtual environments and hidden folders aside).
fn has_python_code(dir: &Path) -> bool {
    let is_py = |entry: &std::fs::DirEntry| entry.file_name().to_string_lossy().ends_with(".py");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.filter_map(std::result::Result::ok).any(|entry| {
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().is_dir() {
            !name.starts_with('.')
                && name != "venv"
                && name != "node_modules"
                && std::fs::read_dir(entry.path()).is_ok_and(|inner| {
                    inner.filter_map(std::result::Result::ok).any(|e| is_py(&e))
                })
        } else {
            is_py(&entry)
        }
    })
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
pub(crate) fn create(ctx: &RuleContext, file: &str, content: &str, result: &mut FixResult) -> Result<()> {
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
