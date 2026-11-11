//! Git access. Git is always run without a shell.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::{Error, Result};

/// How many paths go into one `git rm` call. Keeps the command line under
/// the Windows length limit.
const UNTRACK_BATCH: usize = 100;

/// The result of one git command.
#[derive(Debug, Clone)]
pub struct GitOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Environment variables with which git tells its hooks, or a user tells
/// git, which repository and index to use instead of the one found from the
/// current folder.
const REPO_ENV: [&str; 7] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_PREFIX",
];

static IGNORE_INHERITED_REPO: AtomicBool = AtomicBool::new(false);

/// From now on, every git command of this process finds its repository from
/// the folder it runs in, and ignores `GIT_DIR`, `GIT_INDEX_FILE` and the
/// like from the environment.
///
/// A dry run calls this before it works in its scratch copy. Without it, a
/// dry run started from a git hook, where git sets those variables, would
/// run its git commands against the real repository and its real index.
pub fn ignore_inherited_repo() {
    IGNORE_INHERITED_REPO.store(true, Ordering::SeqCst);
}

fn git_command() -> Command {
    let mut command = Command::new("git");
    if IGNORE_INHERITED_REPO.load(Ordering::SeqCst) {
        for name in REPO_ENV {
            command.env_remove(name);
        }
    }
    command
}

/// Runs git in `cwd`. A non-zero exit is returned, not treated as an error;
/// only failing to start git is an error.
pub fn run_git(cwd: &Path, args: &[&str]) -> Result<GitOutput> {
    let output = git_command()
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| {
            let reason = if error.kind() == ErrorKind::NotFound {
                "git was not found on PATH".to_owned()
            } else {
                error.to_string()
            };
            Error::user(format!(
                "Could not run git ({reason}). Install git and make sure it is on your PATH, then try again."
            ))
        })?;
    Ok(GitOutput {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// Like `run_git`, with `input` written to git's stdin.
pub fn run_git_with_input(cwd: &Path, args: &[&str], input: &[u8]) -> Result<GitOutput> {
    use std::io::Write;

    let failed = |error: std::io::Error| {
        let reason = if error.kind() == ErrorKind::NotFound {
            "git was not found on PATH".to_owned()
        } else {
            error.to_string()
        };
        Error::user(format!(
            "Could not run git ({reason}). Install git and make sure it is on your PATH, then try again."
        ))
    };
    let mut child = git_command()
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(failed)?;
    // Write from another thread: git may fill its output pipe before it has
    // read all of its input, and would then wait on us while we wait on it.
    let stdin = child.stdin.take();
    let output = std::thread::scope(|scope| {
        scope.spawn(move || {
            if let Some(mut stdin) = stdin {
                // A git that exits early closes the pipe; its exit code says why.
                let _ = stdin.write_all(input);
            }
        });
        child.wait_with_output()
    })
    .map_err(failed)?;
    Ok(GitOutput {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// Git access for one repo root.
#[derive(Debug, Clone)]
pub struct Git {
    root: PathBuf,
}

impl Git {
    #[must_use]
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    /// False when the root is not inside a git work tree.
    pub fn is_repo(&self) -> Result<bool> {
        let result = run_git(&self.root, &["rev-parse", "--is-inside-work-tree"])?;
        Ok(result.code == 0 && result.stdout.trim() == "true")
    }

    /// Repo-relative paths in the git index, sorted. Empty outside a git repo.
    pub fn tracked_files(&self) -> Result<Vec<String>> {
        if !self.is_repo()? {
            return Ok(Vec::new());
        }
        // -z gives NUL-separated, unquoted paths, so odd file names survive.
        let result = run_git(&self.root, &["ls-files", "-z", "--cached"])?;
        if result.code != 0 {
            return Err(Error::user(format!(
                "git ls-files failed in {}: {}. Check that the repo is not corrupted (try `git status`).",
                self.root.display(),
                result.stderr.trim()
            )));
        }
        let mut files: Vec<String> = result
            .stdout
            .split('\0')
            .filter(|entry| !entry.is_empty())
            .map(str::to_owned)
            .collect();
        files.sort();
        Ok(files)
    }

    /// Adds one file to the index.
    pub fn track(&self, file: &str) -> Result<()> {
        let result = run_git(&self.root, &["--literal-pathspecs", "add", "--", file])?;
        if result.code != 0 {
            return Err(Error::user(format!(
                "git add failed for {file}: {}. If a .gitignore rule ignores the file, remove that rule, then run this again.",
                result.stderr.trim()
            )));
        }
        Ok(())
    }

    /// True when the index holds `file` with the content it has on disk.
    pub fn is_staged_as_on_disk(&self, file: &str) -> Result<bool> {
        let args = ["--literal-pathspecs", "ls-files", "--cached", "--", file];
        if run_git(&self.root, &args)?.stdout.trim().is_empty() {
            return Ok(false);
        }
        // Exit code 1 means the file differs from the index.
        let diff = run_git(
            &self.root,
            &["--literal-pathspecs", "diff-files", "--quiet", "--", file],
        )?;
        Ok(diff.code == 0)
    }

    /// Removes paths from the index and keeps the files on disk.
    pub fn untrack(&self, paths: &[String]) -> Result<()> {
        for batch in paths.chunks(UNTRACK_BATCH) {
            // --literal-pathspecs: file names are names, never globs or pathspec magic.
            let mut args = vec!["--literal-pathspecs", "rm", "--cached", "--quiet", "--"];
            args.extend(batch.iter().map(String::as_str));
            let result = run_git(&self.root, &args)?;
            if result.code != 0 {
                return Err(Error::user(format!(
                    "git rm --cached failed: {}. Fix the git error above, then run the fix again.",
                    result.stderr.trim()
                )));
            }
        }
        Ok(())
    }
}
