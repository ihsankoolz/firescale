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

/// What a walked entry is. A link is never followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Folder,
    Link,
}

/// Lists every file under `root` that git would not ignore, as sorted
/// repo-relative paths with forward slashes. Dotfiles are included. Runs on
/// all available cores.
pub fn walk_files(root: &Path) -> Result<Vec<String>> {
    Ok(walk_entries(root)?
        .into_iter()
        .filter(|(_, kind)| *kind == EntryKind::File)
        .map(|(path, _)| path)
        .collect())
}

/// Like `walk_files`, and also lists folders and symbolic links. A dry run's
/// scratch copy needs them: an empty folder or a link changes what a fix does.
pub fn walk_entries(root: &Path) -> Result<Vec<(String, EntryKind)>> {
    let entries: Mutex<Vec<(String, EntryKind)>> = Mutex::new(Vec::new());
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
            let entries = &entries;
            let failure = &failure;
            Box::new(move |entry| {
                match entry {
                    Ok(entry) => {
                        let kind = entry.file_type().and_then(|kind| {
                            if kind.is_symlink() {
                                Some(EntryKind::Link)
                            } else if kind.is_file() {
                                Some(EntryKind::File)
                            } else if kind.is_dir() {
                                Some(EntryKind::Folder)
                            } else {
                                None
                            }
                        });
                        if let (Some(kind), Some(path)) = (kind, to_relative(root, entry.path())) {
                            if let Ok(mut entries) = entries.lock() {
                                entries.push((path, kind));
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
    let mut entries = entries.into_inner().unwrap_or_default();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(entries)
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

/// True when the lines of a `.gitignore` at the repo root ignore `path`
/// (repo-relative, `/`-separated) as git would: globs (`[Bb]in/`,
/// `/build*/`), anchors (`/.temp` is only the root's), directory-only
/// lines and negations, with the last matching line deciding. A path that
/// could be a folder or a file is ignored when either is, or when
/// everything in it is.
#[must_use]
pub fn gitignore_covers(text: &str, path: &str) -> bool {
    let root = Path::new("/");
    let mut builder = ignore::gitignore::GitignoreBuilder::new(root);
    for line in text.trim_start_matches('\u{feff}').lines() {
        // A line git cannot read is skipped, as git skips it.
        let _ = builder.add_line(None, line.trim_end_matches('\r'));
    }
    let Ok(matcher) = builder.build() else {
        return false;
    };
    let path = root.join(path.trim_start_matches('/'));
    // `node_modules/**` ignores what is in the folder, not the folder:
    // as good, so a file inside it is asked too.
    [true, false].into_iter().any(|is_dir| {
        matcher
            .matched_path_or_any_parents(&path, is_dir)
            .is_ignore()
    }) || matcher
        .matched_path_or_any_parents(path.join("fscale-probe"), false)
        .is_ignore()
}

#[cfg(test)]
mod covers_tests {
    use super::gitignore_covers;

    #[test]
    fn reads_gitignore_lines_as_git_does() {
        assert!(gitignore_covers("[Bb]in/\n[Oo]bj/\n", "bin"));
        assert!(gitignore_covers("/build*/\n", "build"));
        assert!(gitignore_covers("**/node_modules/**\n", "node_modules"));
        assert!(gitignore_covers("target\n", "target"));
        assert!(gitignore_covers("\u{feff}.venv/\r\n", ".venv"));
        // Anchored to the root: not a folder below it.
        assert!(!gitignore_covers("/.temp\n", "supabase/.temp"));
        assert!(gitignore_covers("supabase/.temp\n", "supabase/.temp"));
        // The last matching line decides.
        assert!(!gitignore_covers("build/\n!build/\n", "build"));
        assert!(!gitignore_covers("# target\n", "target"));
        assert!(gitignore_covers("*.jks\n", "release.jks"));
    }
}
