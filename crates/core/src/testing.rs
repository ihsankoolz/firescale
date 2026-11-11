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
