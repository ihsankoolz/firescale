//! Parallel, gitignore-aware file walking.

use std::path::Path;
use std::sync::Mutex;

use ignore::{WalkBuilder, WalkState};

use crate::error::{Error, Result};

/// Folders that are never walked: git's own data and firescale's cache.
const SKIPPED_DIRS: [&str; 2] = [".git", ".firescale"];

/// Converts a path relative to the root into the forward-slash form used everywhere.
fn to_relative(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let parts: Vec<String> = relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

/// Lists every file under `root` that git would not ignore, as sorted
/// repo-relative paths with forward slashes. Dotfiles are included. Runs on
/// all available cores.
pub fn walk_files(root: &Path) -> Result<Vec<String>> {
    let files: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let failure: Mutex<Option<String>> = Mutex::new(None);

    WalkBuilder::new(root)
        .hidden(false)
        .follow_links(false)
        .filter_entry(|entry| {
            entry
                .file_name()
                .to_str()
                .is_none_or(|name| !SKIPPED_DIRS.contains(&name))
        })
        .build_parallel()
        .run(|| {
            let files = &files;
            let failure = &failure;
            Box::new(move |entry| {
                match entry {
                    Ok(entry) => {
                        let is_file = entry.file_type().is_some_and(|kind| kind.is_file());
                        if let (true, Some(path)) = (is_file, to_relative(root, entry.path())) {
                            if let Ok(mut files) = files.lock() {
                                files.push(path);
                            }
                        }
                    }
                    Err(error) => {
                        if let Ok(mut failure) = failure.lock() {
                            failure.get_or_insert_with(|| error.to_string());
                        }
                    }
                }
                WalkState::Continue
            })
        });

    if let Some(reason) = failure.into_inner().ok().flatten() {
        return Err(Error::user(format!(
            "Could not read part of the repo ({reason}). Check folder permissions, or run with --no-cache to skip the file walk."
        )));
    }
    let mut files = files.into_inner().unwrap_or_default();
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::walk_files;
    use crate::testing::{Fixture, Track};

    #[test]
    fn lists_files_with_forward_slashes_and_skips_ignored_ones() {
        let fixture = Fixture::new(
            &[
                (".gitignore", "dist/\n*.log\n"),
                ("README.md", "x\n"),
                (".hidden/config", "x\n"),
                ("src/deep/file.ts", "x\n"),
                ("dist/out.js", "x\n"),
                ("debug.log", "x\n"),
                (".firescale/cache/x.json", "{}\n"),
            ],
            Track::None,
        )
        .unwrap();
        let files = walk_files(fixture.root()).unwrap();
        assert_eq!(
            files,
            [
                ".gitignore",
                ".hidden/config",
                "README.md",
                "src/deep/file.ts"
            ]
        );
    }

    #[test]
    fn walks_a_folder_that_is_not_a_git_repo() {
        let fixture = Fixture::new(&[("a/b.txt", "x\n"), ("c.txt", "x\n")], Track::NoGit).unwrap();
        assert_eq!(walk_files(fixture.root()).unwrap(), ["a/b.txt", "c.txt"]);
    }
}
