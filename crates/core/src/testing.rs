//! Fixture repos for rule tests. Enabled by the `testing` feature.
//!
//! A fixture is a throwaway folder in the OS temp directory, usually a git
//! repo with chosen files in the index. Fixtures are built at test time
//! because a fixture needs its own git index (ADR 0006).

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::error::{Error, Result};
use crate::git::{run_git, Git};
use crate::model::{Diagnostic, FixResult};
use crate::rule::{check_rule, fix_rule, BuiltinRule, Options};

/// Which files of a fixture go into the git index.
#[derive(Debug, Clone, Copy)]
pub enum Track<'a> {
    /// Every file, even ones a `.gitignore` would skip.
    All,
    /// A git repo with an empty index.
    None,
    /// A plain folder that is not a git repo.
    NoGit,
    /// Only these paths.
    Only(&'a [&'a str]),
}

/// A throwaway repo. Deleted when dropped.
pub struct Fixture {
    dir: TempDir,
    root: PathBuf,
}

fn git(root: &Path, args: &[&str]) -> Result<()> {
    // Fixtures must not depend on the machine's line-ending settings.
    let mut full = vec!["-c", "core.autocrlf=false", "-c", "core.safecrlf=false"];
    full.extend_from_slice(args);
    let result = run_git(root, &full)?;
    if result.code == 0 {
        Ok(())
    } else {
        Err(Error::internal(format!(
            "fixture setup: git {} failed: {}",
            args.join(" "),
            result.stderr.trim()
        )))
    }
}

impl Fixture {
    /// Creates a fixture with the given `(path, content)` files.
    pub fn new(files: &[(&str, &str)], track: Track<'_>) -> Result<Self> {
        let failed = |error: std::io::Error| Error::internal(format!("fixture setup: {error}"));
        let dir = tempfile::Builder::new()
            .prefix("firescale-fixture-")
            .tempdir()
            .map_err(failed)?;
        // Canonical path: macOS temp folders are symlinks and Windows may hand out 8.3 names.
        let root = dir.path().canonicalize().map_err(failed)?;
        let fixture = Self { dir, root };
        for (file, content) in files {
            fixture.write(file, content)?;
        }
        if !matches!(track, Track::NoGit) {
            git(&fixture.root, &["init", "--quiet"])?;
        }
        match track {
            Track::All => {
                fixture.track(&files.iter().map(|(file, _)| *file).collect::<Vec<_>>())?
            }
            Track::Only(paths) => fixture.track(paths)?,
            Track::None | Track::NoGit => {}
        }
        Ok(fixture)
    }

    /// Absolute, canonical path of the fixture root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The temp folder handle, for tests that need its original path.
    #[must_use]
    pub fn dir(&self) -> &Path {
        self.dir.path()
    }

    fn resolve(&self, file: &str) -> PathBuf {
        file.split('/')
            .fold(self.root.clone(), |path, part| path.join(part))
    }

    pub fn write(&self, file: &str, content: &str) -> Result<()> {
        let failed =
            |error: std::io::Error| Error::internal(format!("fixture write {file}: {error}"));
        let target = self.resolve(file);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(failed)?;
        }
        fs::write(target, content).map_err(failed)
    }

    /// Reads a file, or returns `None` when it does not exist.
    #[must_use]
    pub fn read(&self, file: &str) -> Option<String> {
        fs::read_to_string(self.resolve(file)).ok()
    }

    /// Adds paths to the git index, including ignored ones.
    pub fn track(&self, paths: &[&str]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let mut args = vec!["--literal-pathspecs", "add", "--force", "--"];
        args.extend_from_slice(paths);
        git(&self.root, &args)
    }

    /// Repo-relative paths currently in the git index, sorted.
    pub fn tracked(&self) -> Result<Vec<String>> {
        Git::new(&self.root).tracked_files()
    }

    /// Runs a rule's check against the fixture.
    pub fn check(&self, rule: &dyn BuiltinRule, options: &Options) -> Result<Vec<Diagnostic>> {
        check_rule(rule, &self.root, options)
    }

    /// Runs a rule's deterministic fix against the fixture.
    pub fn fix(&self, rule: &dyn BuiltinRule, options: &Options) -> Result<FixResult> {
        fix_rule(rule, &self.root, options)
    }
}

/// The services of the course-project fixture, under `backend/`.
pub const COURSE_SERVICES: [&str; 5] = [
    "audit_service",
    "auth_service",
    "ledger_service",
    "market_service",
    "realtime_service",
];

/// The files of a repo shaped like a real course project: Docker Compose and
/// docs at the root with no manifest, five Python services under `backend/`
/// and a Vite app under `frontend/`. Used by the facts and verify tests
/// (ADR 0020). Only `auth_service` has a ruff config, and the folders that
/// discovery must skip each hold a marker file.
#[must_use]
pub fn course_project_files() -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = [
        ("docker-compose.yml", "services:\n  auth:\n    build: ./backend/auth_service\n"),
        (".env.example", "EXAMPLE_DATABASE_URL=\n"),
        ("README.md", "# Course project\n"),
        ("CLAUDE.md", "# Notes for agents\n"),
        (".github/workflows/ci.yml", "on: [pull_request]\n"),
        (".gitignore", "generated/\n"),
        ("backend/.coveragerc", "[run]\nbranch = True\n"),
        ("backend/shared/__init__.py", ""),
        ("backend/shared/db.py", "URL = None\n"),
        ("backend/auth_service/ruff.toml", "line-length = 100\n"),
        (
            "frontend/package.json",
            r#"{"name":"frontend","private":true,"scripts":{"dev":"vite","build":"vite build","test":"vitest","test:run":"vitest run","test:e2e":"playwright test"},"devDependencies":{"typescript":"5.9.0"}}"#,
        ),
        ("frontend/package-lock.json", "{}\n"),
        ("frontend/vite.config.ts", "export default {};\n"),
        ("frontend/tsconfig.json", "{ \"compilerOptions\": { \"strict\": true } }\n"),
        ("frontend/playwright.config.ts", "export default {};\n"),
        ("frontend/.oxlintrc.json", "{}\n"),
        ("frontend/knip.jsonc", "{}\n"),
        ("frontend/src/main.ts", "export const app = 1;\n"),
        ("frontend/e2e/login.spec.ts", "export {};\n"),
        // Never searched: dependency, cache and build folders, and what
        // .gitignore ignores.
        ("frontend/node_modules/left-pad/package.json", r#"{"name":"left-pad"}"#),
        ("frontend/dist/package.json", r#"{"name":"built"}"#),
        ("node_modules/tool/package.json", r#"{"name":"tool"}"#),
        ("dist/package.json", r#"{"name":"dist"}"#),
        ("test-results/package.json", r#"{"name":"results"}"#),
        ("generated/client/package.json", r#"{"name":"generated"}"#),
        ("backend/.venv/lib/pkg/pyproject.toml", "[project]\nname = \"vendored\"\n"),
        ("backend/__pycache__/x/setup.py", ""),
        ("backend/.pytest_cache/x/pyproject.toml", ""),
        ("backend/.ruff_cache/x/pyproject.toml", ""),
    ]
    .iter()
    .map(|(file, content)| ((*file).to_owned(), (*content).to_owned()))
    .collect();
    for service in COURSE_SERVICES {
        for (file, content) in [
            ("requirements.txt", "fastapi==0.115.0\nsqlalchemy==2.0.36\n"),
            (
                "requirements-dev.txt",
                "-r requirements.txt\npytest==8.3.3\n",
            ),
            ("pytest.ini", "[pytest]\ntestpaths = unit_test\n"),
            ("Dockerfile", "FROM python:3.13-slim\n"),
            ("alembic.ini", "[alembic]\nscript_location = migrations\n"),
            ("main.py", "app = None\n"),
            (".env.example", "EXAMPLE_SERVICE_URL=\n"),
            ("unit_test/conftest.py", "import pytest\n"),
        ] {
            files.push((format!("backend/{service}/{file}"), content.to_owned()));
        }
    }
    files
}

/// A fixture holding `course_project_files`, outside git.
pub fn course_project() -> Result<Fixture> {
    let files = course_project_files();
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(file, content)| (file.as_str(), content.as_str()))
        .collect();
    Fixture::new(&borrowed, Track::NoGit)
}
