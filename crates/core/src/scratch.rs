//! A throwaway copy of a repo, for dry runs of the fix loop (ADR 0018).
//!
//! A dry run of several fixes has to let each fix see what the fixes before
//! it would have written. Rules read the repo in many ways: through their
//! context, through git, and straight from disk. The only view that is right
//! for all of them is a real folder, so a dry run copies the repo to a
//! scratch folder, runs the real loop there, and reports what changed. The
//! user's working tree and git index are never touched.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result};
use crate::git::{run_git, run_git_with_input, Git};
use crate::walk::walk_files;

/// A scratch copy of a repo root. The folder is deleted when this is dropped.
#[derive(Debug)]
pub struct Scratch {
    root: PathBuf,
}

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn failed(what: &str, error: &std::io::Error) -> Error {
    Error::user(format!(
        "Could not prepare the dry run ({what}: {error}). Check that the temp folder is writable and has free space, or run `fscale fix` without --dry-run on a clean git tree and review the result with `git diff`."
    ))
}

fn join(root: &Path, file: &str) -> PathBuf {
    file.split('/')
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

/// Makes a new, empty folder in the OS temp folder.
fn create_scratch_dir() -> Result<PathBuf> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos());
    let mut last = None;
    for _ in 0..16 {
        let name = format!(
            "firescale-dry-run-{}-{nanos}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let dir = std::env::temp_dir().join(name);
        match fs::create_dir(&dir) {
            // Canonical, like every other root: macOS temp folders are
            // symlinks and Windows may hand out 8.3 names.
            Ok(()) => {
                return dir
                    .canonicalize()
                    .map_err(|error| failed("temp folder", &error))
            }
            Err(error) => last = Some(error),
        }
    }
    let error = last.unwrap_or_else(|| std::io::Error::other("no attempt was made"));
    Err(failed("temp folder", &error))
}

fn git_ok(root: &Path, args: &[&str], input: Option<&[u8]>) -> Result<String> {
    // The copy must not depend on the machine's line-ending settings.
    let mut full = vec!["-c", "core.autocrlf=false", "-c", "core.safecrlf=false"];
    full.extend_from_slice(args);
    let result = match input {
        Some(input) => run_git_with_input(root, &full, input)?,
        None => run_git(root, &full)?,
    };
    if result.code == 0 {
        Ok(result.stdout)
    } else {
        Err(Error::user(format!(
            "Could not prepare the dry run (git {} failed: {}). Run `fscale fix` without --dry-run on a clean git tree and review the result with `git diff`.",
            args.first().copied().unwrap_or_default(),
            result.stderr.trim()
        )))
    }
}

impl Scratch {
    /// Copies the repo at `root`: every file git does not ignore, plus every
    /// tracked file. When `root` is in a git repo, the copy is a git repo too,
    /// with the same files tracked, so rules that ask git get the same answers.
    ///
    /// Ignored, untracked files (`node_modules`, build output) are not copied.
    pub fn copy_of(root: &Path) -> Result<Self> {
        let scratch = Self {
            root: create_scratch_dir()?,
        };
        let git = Git::new(root);
        let is_repo = git.is_repo()?;
        let tracked = if is_repo {
            git.tracked_files()?
        } else {
            Vec::new()
        };

        let mut files = walk_files(root)?;
        files.extend(tracked.iter().cloned());
        files.sort();
        files.dedup();

        let mut missing: Vec<&str> = Vec::new();
        let mut present: Vec<&str> = Vec::new();
        for file in &files {
            let source = join(root, file);
            // A tracked file can be gone from disk, and anything can vanish
            // while we copy. Neither is an error for a preview.
            if !source.is_file() {
                continue;
            }
            let target = join(&scratch.root, file);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|error| failed(file, &error))?;
            }
            match fs::copy(&source, &target) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(failed(file, &error)),
            }
        }
        for file in &tracked {
            if join(&scratch.root, file).is_file() {
                present.push(file);
            } else {
                missing.push(file);
            }
        }

        if is_repo {
            git_ok(&scratch.root, &["init", "--quiet"], None)?;
            if !present.is_empty() {
                let mut list = Vec::new();
                for file in &present {
                    list.extend_from_slice(file.as_bytes());
                    list.push(0);
                }
                git_ok(
                    &scratch.root,
                    &["update-index", "--add", "-z", "--stdin"],
                    Some(&list),
                )?;
            }
            if !missing.is_empty() {
                // Tracked, but not on disk: keep them tracked in the copy, as
                // empty files in the index only.
                let empty = git_ok(&scratch.root, &["hash-object", "-w", "--stdin"], Some(b""))?;
                let empty = empty.trim();
                let mut list = Vec::new();
                for file in &missing {
                    list.extend_from_slice(format!("100644 {empty}\t{file}").as_bytes());
                    list.push(0);
                }
                git_ok(
                    &scratch.root,
                    &["update-index", "--add", "-z", "--index-info"],
                    Some(&list),
                )?;
            }
        }
        Ok(scratch)
    }

    /// Absolute, canonical path of the copy.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Fixture, Track};

    fn read(root: &Path, file: &str) -> Option<String> {
        fs::read_to_string(join(root, file)).ok()
    }

    #[test]
    fn copies_files_and_what_git_tracks_but_not_ignored_files() {
        let fixture = Fixture::new(
            &[
                (".gitignore", "dist/\n.env\n"),
                ("src/a.txt", "a\n"),
                ("untracked.txt", "u\n"),
                ("dist/out.js", "built\n"),
                (".env", "EXAMPLE=1\n"),
                ("nested/dir/deep.txt", "d\r\n"),
            ],
            // .env is ignored, but tracked on purpose.
            Track::Only(&[".gitignore", "src/a.txt", ".env", "nested/dir/deep.txt"]),
        )
        .unwrap();
        let scratch = Scratch::copy_of(fixture.root()).unwrap();
        let copy = scratch.root().to_path_buf();
        assert_ne!(copy, fixture.root());

        assert_eq!(read(&copy, "src/a.txt").as_deref(), Some("a\n"));
        assert_eq!(read(&copy, "untracked.txt").as_deref(), Some("u\n"));
        assert_eq!(read(&copy, ".env").as_deref(), Some("EXAMPLE=1\n"));
        // Line endings are copied byte for byte.
        assert_eq!(read(&copy, "nested/dir/deep.txt").as_deref(), Some("d\r\n"));
        assert_eq!(read(&copy, "dist/out.js"), None);

        assert_eq!(
            Git::new(&copy).tracked_files().unwrap(),
            fixture.tracked().unwrap()
        );
    }

    #[test]
    fn changes_in_the_copy_do_not_reach_the_repo_and_the_copy_is_removed() {
        let fixture = Fixture::new(&[("a.txt", "a\n"), (".env", "X=1\n")], Track::All).unwrap();
        let copy = {
            let scratch = Scratch::copy_of(fixture.root()).unwrap();
            let copy = scratch.root().to_path_buf();
            fs::write(copy.join("a.txt"), "changed\n").unwrap();
            fs::write(copy.join("new.txt"), "new\n").unwrap();
            Git::new(&copy).untrack(&[".env".to_owned()]).unwrap();
            assert_eq!(Git::new(&copy).tracked_files().unwrap(), ["a.txt"]);
            copy
        };
        assert!(!copy.exists(), "the scratch folder is deleted on drop");
        assert_eq!(fixture.read("a.txt").as_deref(), Some("a\n"));
        assert_eq!(fixture.read("new.txt"), None);
        assert_eq!(fixture.tracked().unwrap(), [".env", "a.txt"]);
    }

    #[test]
    fn a_folder_that_is_not_a_git_repo_is_copied_as_plain_files() {
        let fixture = Fixture::new(&[("a.txt", "a\n")], Track::NoGit).unwrap();
        let scratch = Scratch::copy_of(fixture.root()).unwrap();
        assert_eq!(read(scratch.root(), "a.txt").as_deref(), Some("a\n"));
        assert!(!scratch.root().join(".git").exists());
    }

    #[test]
    fn a_tracked_file_that_is_gone_from_disk_stays_tracked_in_the_copy() {
        let fixture = Fixture::new(&[("a.txt", "a\n"), (".env", "X=1\n")], Track::All).unwrap();
        fs::remove_file(fixture.root().join(".env")).unwrap();
        let scratch = Scratch::copy_of(fixture.root()).unwrap();
        assert!(!scratch.root().join(".env").exists());
        assert_eq!(
            Git::new(scratch.root()).tracked_files().unwrap(),
            [".env", "a.txt"]
        );
    }

    #[test]
    fn odd_file_names_survive() {
        let fixture = Fixture::new(
            &[("with space.txt", "s\n"), ("dir/ünï.txt", "u\n")],
            Track::All,
        )
        .unwrap();
        let scratch = Scratch::copy_of(fixture.root()).unwrap();
        assert_eq!(
            Git::new(scratch.root()).tracked_files().unwrap(),
            fixture.tracked().unwrap()
        );
    }
}
