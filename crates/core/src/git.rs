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
    // firescale only reads: never refresh the index (`git status` would
    // rewrite `.git/index` and take its lock while an agent commits).
    command.env("GIT_OPTIONAL_LOCKS", "0");
    // A commit id means what it says: `git replace` refs, which would let
    // anyone who can write the repo make one commit read as another (the
    // base the tamper guard compares with, ADR 0045), are never followed.
    command.env("GIT_NO_REPLACE_OBJECTS", "1");
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
    run_git_env(cwd, args, &[])
}

/// The git subcommand of `args`: the first word after the `-c <name=value>`
/// pairs and other options.
fn subcommand<'a>(args: &[&'a str]) -> Option<&'a str> {
    let mut words = args.iter();
    while let Some(word) = words.next() {
        match *word {
            "-c" | "-C" => {
                words.next();
            }
            option if option.starts_with('-') => {}
            command => return Some(command),
        }
    }
    None
}

/// A private copy of the repo's index, removed when dropped.
struct IndexCopy(PathBuf);

impl IndexCopy {
    /// Copies the index of the repo at `cwd`, if it has one.
    fn of(cwd: &Path) -> Option<Self> {
        let out = git_command()
            .args(["rev-parse", "--git-path", "index"])
            .current_dir(cwd)
            .stdin(Stdio::null())
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let path = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        let index = if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        };
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.subsec_nanos());
        let copy =
            std::env::temp_dir().join(format!("firescale-index-{}-{nanos}", std::process::id()));
        std::fs::copy(&index, &copy).ok()?;
        let copied = Self(copy);
        // Keep the index's own time: git takes an entry whose time is not
        // older than the index file as "racily clean" and compares its
        // content. A fresh time on the copy would hide a file rewritten,
        // same size, in the same second as the last commit.
        let modified = std::fs::metadata(&index)
            .and_then(|meta| meta.modified())
            .ok()?;
        std::fs::OpenOptions::new()
            .write(true)
            .open(&copied.0)
            .and_then(|file| file.set_modified(modified))
            .ok()?;
        Some(copied)
    }
}

impl Drop for IndexCopy {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let mut lock = self.0.clone().into_os_string();
        lock.push(".lock");
        let _ = std::fs::remove_file(lock);
    }
}

/// [`run_git`] with extra environment variables for this one command.
///
/// `git diff` refreshes the index and writes it back whenever a file's
/// stat data is stale, `GIT_OPTIONAL_LOCKS=0` or not. So a `diff` runs
/// against a private copy of the index: the user's `.git/index` is never
/// written (ADR 0037).
pub fn run_git_env(cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<GitOutput> {
    let mut command = git_command();
    let private = (subcommand(args) == Some("diff")
        && !env.iter().any(|(name, _)| *name == "GIT_INDEX_FILE"))
    .then(|| IndexCopy::of(cwd))
    .flatten();
    if let Some(copy) = &private {
        command.env("GIT_INDEX_FILE", &copy.0);
    }
    for (name, value) in env {
        command.env(name, value);
    }
    let output = command
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
    let (code, stdout, stderr) = run_git_bytes(cwd, args, input)?;
    Ok(GitOutput {
        code,
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

/// Like [`run_git_with_input`], with git's output as bytes: the exit code,
/// stdout and stderr.
pub fn run_git_bytes(cwd: &Path, args: &[&str], input: &[u8]) -> Result<(i32, Vec<u8>, Vec<u8>)> {
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
    Ok((
        output.status.code().unwrap_or(-1),
        output.stdout,
        output.stderr,
    ))
}

/// The bytes of each of `paths` (relative to `cwd`, forward slashes) as
/// `commit` has them, from one `git cat-file --batch`. A path the commit
/// does not have is left out. A path with a line break is read with
/// `git show`.
#[must_use]
pub fn files_at(
    cwd: &Path,
    commit: &str,
    paths: &[&str],
) -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut found = std::collections::BTreeMap::new();
    if paths.is_empty() {
        return found;
    }
    // `<commit>:<path>` in a batch is relative to the repo's top, so add
    // the folder `cwd` is in.
    let prefix = run_git(cwd, &["rev-parse", "--show-prefix"])
        .ok()
        .filter(|out| out.code == 0)
        .map(|out| out.stdout.trim_end_matches(['\r', '\n']).to_owned())
        .unwrap_or_default();
    let (plain, odd): (Vec<&str>, Vec<&str>) =
        paths.iter().partition(|path| !path.contains(['\n', '\r']));
    for path in odd {
        if let Ok((0, bytes, _)) = run_git_bytes(cwd, &["show", &format!("{commit}:./{path}")], &[])
        {
            found.insert(path.to_owned(), bytes);
        }
    }
    let input: String = plain
        .iter()
        .map(|path| format!("{commit}:{prefix}{path}\n"))
        .collect();
    let Ok((0, out, _)) = run_git_bytes(cwd, &["cat-file", "--batch"], input.as_bytes()) else {
        return found;
    };
    // Each answer is `<oid> <type> <size>\n<content>\n`, or `<name> missing\n`.
    let mut at = 0;
    for path in plain {
        let Some(end) = out[at..]
            .iter()
            .position(|b| *b == b'\n')
            .map(|end| at + end)
        else {
            break;
        };
        let header = String::from_utf8_lossy(&out[at..end]).into_owned();
        at = end + 1;
        let words: Vec<&str> = header.rsplitn(3, ' ').collect();
        let (Some(size), Some(kind)) = (words.first(), words.get(1)) else {
            continue;
        };
        let Ok(size) = size.parse::<usize>() else {
            // `missing`, `ambiguous`: no content follows.
            continue;
        };
        let Some(content) = out.get(at..at + size) else {
            break;
        };
        if *kind == "blob" {
            found.insert(path.to_owned(), content.to_vec());
        }
        at += size + 1;
    }
    found
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

    /// The name of the checked-out branch, also before its first commit.
    /// `None` outside a git repo and with a detached HEAD.
    pub fn current_branch(&self) -> Result<Option<String>> {
        if !self.is_repo()? {
            return Ok(None);
        }
        let result = run_git(&self.root, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
        let name = result.stdout.trim();
        Ok((result.code == 0 && !name.is_empty()).then(|| name.to_owned()))
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

    /// True when the index has an entry for `file`.
    pub fn is_tracked(&self, file: &str) -> Result<bool> {
        let args = ["--literal-pathspecs", "ls-files", "--cached", "--", file];
        Ok(!run_git(&self.root, &args)?.stdout.trim().is_empty())
    }

    /// True when a `.gitignore` rule (or another exclude) ignores `file`.
    pub fn is_ignored(&self, file: &str) -> Result<bool> {
        // check-ignore takes plain paths and does not accept pathspec magic.
        // Exit code 0: ignored. 1: not ignored. Anything else: git could not
        // say, which must not be read as "free to create".
        let result = run_git(&self.root, &["check-ignore", "--quiet", "--", file])?;
        match result.code {
            0 => Ok(true),
            1 => Ok(false),
            _ => Err(Error::user(format!(
                "git check-ignore failed for {file}: {}. Fix the git error above, then run this again.",
                result.stderr.trim()
            ))),
        }
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

/// True when the repo at `cwd` is a shallow clone: its history stops
/// before the first commit, so a merge base may be missing (ADR 0041).
#[must_use]
pub fn is_shallow(cwd: &Path) -> bool {
    run_git(cwd, &["rev-parse", "--is-shallow-repository"])
        .is_ok_and(|out| out.code == 0 && out.stdout.trim() == "true")
}

/// True when `rev` names a commit in the repo at `cwd`.
#[must_use]
pub fn resolves(cwd: &Path, rev: &str) -> bool {
    run_git(
        cwd,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{commit}}"),
        ],
    )
    .is_ok_and(|out| out.code == 0)
}

/// True when `base` names a commit, but this shallow clone does not hold
/// its merge base with `HEAD`. A name that does not resolve is a typo, not
/// a shallow clone.
#[must_use]
pub fn shallow_without_merge_base(cwd: &Path, base: &str) -> bool {
    is_shallow(cwd) && resolves(cwd, base)
}

/// The note for a base that a shallow clone cannot compare with.
#[must_use]
pub fn shallow_note(base: &str) -> String {
    format!(
        "this clone is shallow, so it does not hold the merge base with {base}; fetch the full history (in GitHub Actions: actions/checkout with fetch-depth: 0)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Fixture, Track};

    #[test]
    fn the_subcommand_is_found_after_config_pairs() {
        assert_eq!(
            subcommand(&["-c", "core.quotepath=off", "diff", "HEAD"]),
            Some("diff")
        );
        assert_eq!(subcommand(&["--no-pager", "status"]), Some("status"));
        assert_eq!(subcommand(&["-c", "x=diff", "ls-files"]), Some("ls-files"));
    }

    #[test]
    fn git_diff_never_writes_the_index_even_when_a_file_s_time_changed() {
        let fixture = Fixture::new(&[("a.txt", "one\n"), ("b.txt", "two\n")], Track::All).unwrap();
        let root = fixture.root();
        let out = run_git(
            root,
            &[
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--quiet",
                "--no-verify",
                "-m",
                "base",
            ],
        )
        .unwrap();
        assert_eq!(out.code, 0, "{}", out.stderr);
        // Same content, a later time: git diff would refresh the index.
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(30);
        std::fs::OpenOptions::new()
            .append(true)
            .open(root.join("a.txt"))
            .unwrap()
            .set_modified(later)
            .unwrap();
        let index = root.join(".git").join("index");
        let before = std::fs::read(&index).unwrap();
        let out = run_git(root, &["diff", "--name-only", "-z", "HEAD"]).unwrap();
        assert_eq!(out.code, 0);
        assert_eq!(out.stdout, "", "the content is the same");
        assert_eq!(std::fs::read(&index).unwrap(), before);
    }

    #[test]
    fn git_diff_still_sees_a_racily_clean_change() {
        let fixture = Fixture::new(&[("a.txt", "one\n")], Track::All).unwrap();
        let root = fixture.root();
        let out = run_git(
            root,
            &[
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--quiet",
                "--no-verify",
                "-m",
                "base",
            ],
        )
        .unwrap();
        assert_eq!(out.code, 0, "{}", out.stderr);
        let file = root.join("a.txt");
        let index = root.join(".git").join("index");
        let set = |path: &std::path::Path, when: std::time::SystemTime| {
            std::fs::OpenOptions::new()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(when)
                .unwrap();
        };
        // An entry that is clean and not smudged: the file is older than
        // the index git writes now.
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        set(&file, past);
        let refreshed = run_git_env(
            root,
            &["update-index", "--refresh"],
            &[("GIT_OPTIONAL_LOCKS", "1")],
        )
        .unwrap();
        assert_eq!(refreshed.code, 0, "{}", refreshed.stderr);
        // Same size, same time as the entry, and the index no newer than it:
        // only git's racy check, which reads the index file's own time, can
        // tell the file changed.
        std::fs::write(&file, "two\n").unwrap();
        set(&file, past);
        set(&index, past);
        // Compare stat data as a git built without nanoseconds does: size
        // and whole seconds only.
        let out = run_git(
            root,
            &[
                "-c",
                "core.checkStat=minimal",
                "-c",
                "core.trustctime=false",
                "diff",
                "--name-only",
                "HEAD",
            ],
        )
        .unwrap();
        assert_eq!(out.stdout.trim(), "a.txt");
    }
}
