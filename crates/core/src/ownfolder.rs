//! `.firescale`, firescale's own folder in a repo.
//!
//! A repo can hold a symbolic link where firescale expects its folder, or a
//! folder or file in it. Whatever firescale wrote there would land where the
//! link points, outside the repo. So nothing is read, written or removed
//! through one: each writer asks [`linked`] first and refuses (ADR 0060).
//! The step cache (`stepcache.rs`) and the stop hook's counter (`hooks.rs`)
//! make the same check in place (ADR 0056, bug 17).

use std::fs;
use std::path::Path;

/// The folder's name, at the repo root.
pub const DIR: &str = ".firescale";

/// True when the path itself is a symbolic link, also one that points
/// nowhere. On Windows a junction counts too.
fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

/// The first symbolic link on the way to `.firescale/<below…>`: the folder
/// itself, then each part below it. Named from the repo root with forward
/// slashes (`.firescale/cache`). `None` when no part is a link.
#[must_use]
pub fn linked(root: &Path, below: &[&str]) -> Option<String> {
    let mut path = root.join(DIR);
    let mut name = DIR.to_owned();
    if is_symlink(&path) {
        return Some(name);
    }
    for part in below {
        path.push(part);
        name = format!("{name}/{part}");
        if is_symlink(&path) {
            return Some(name);
        }
    }
    None
}

/// What a writer says when it refuses `link` (a name from [`linked`]).
#[must_use]
pub fn refusal(link: &str) -> String {
    format!(
        "{link} is a symbolic link, and firescale reads and writes nothing through one. Remove the link: a real folder or file is made in its place."
    )
}

#[cfg(test)]
mod tests {
    // Making a symbolic link needs a privilege on Windows.
    #[cfg(unix)]
    mod links {
        use std::fs;
        use std::path::Path;

        use crate::agentrun::{append_ledger, LedgerEntry};
        use crate::cache::Cache;
        use crate::testing::{Fixture, Track};

        /// The names in a folder, sorted.
        fn names_in(dir: &Path) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(dir)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }

        /// A repo whose `linked` (a path below the root) is a link to a folder
        /// outside it that holds `keep.txt`.
        fn linked_out(linked: &str) -> (Fixture, tempfile::TempDir) {
            let fixture = Fixture::new(&[("a.md", "one\n")], Track::NoGit).unwrap();
            let outside = tempfile::tempdir().unwrap();
            fs::write(outside.path().join("keep.txt"), "kept\n").unwrap();
            let link = linked
                .split('/')
                .fold(fixture.root().to_path_buf(), |path, part| path.join(part));
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(outside.path(), &link).unwrap();
            (fixture, outside)
        }

        fn entry() -> LedgerEntry {
            LedgerEntry {
                schema: 1,
                time: "2026-11-15T00:00:00Z".to_owned(),
                rule: "core/env-example".to_owned(),
                agent: "fake".to_owned(),
                result: "pass".to_owned(),
                reason: None,
                files_changed: 1,
                lines_changed: 1,
                duration_ms: 1,
                cost_usd: None,
                turns: None,
            }
        }

        #[test]
        fn a_rule_result_is_never_stored_or_read_through_a_symlinked_folder() {
            for linked in [".firescale", ".firescale/cache"] {
                let (fixture, outside) = linked_out(linked);
                let cache = Cache::new(fixture.root());
                let error = cache.put("core/x", "key", &[]).unwrap_err();
                assert_eq!(names_in(outside.path()), ["keep.txt"], "{linked}");
                assert_eq!(
                error.message(),
                format!("{linked} is a symbolic link, and firescale reads and writes nothing through one. Remove the link: a real folder or file is made in its place."),
            );
                // A real cache elsewhere is not read through the link.
                let other = Fixture::new(&[("a.md", "one\n")], Track::NoGit).unwrap();
                Cache::new(other.root()).put("core/x", "key", &[]).unwrap();
                let target = if linked == ".firescale" {
                    other.root().join(".firescale")
                } else {
                    other.root().join(".firescale").join("cache")
                };
                let link = fixture.root().join(linked);
                fs::remove_file(&link).unwrap();
                std::os::unix::fs::symlink(&target, &link).unwrap();
                assert!(Cache::new(other.root()).get("core/x", "key").is_some());
                assert!(cache.get("core/x", "key").is_none(), "{linked}");
            }
        }

        #[test]
        fn a_rule_result_is_never_read_through_a_symlinked_entry_file() {
            // A real cache elsewhere holds a result for the same rule and key.
            let other = Fixture::new(&[("a.md", "one\n")], Track::NoGit).unwrap();
            Cache::new(other.root()).put("core/x", "key", &[]).unwrap();
            let planted = other
                .root()
                .join(".firescale")
                .join("cache")
                .join("core__x.json");
            let record = fs::read_to_string(&planted).unwrap();
            // The cache folder is real; the entry in it is a link to that file.
            let fixture = Fixture::new(&[("a.md", "one\n")], Track::NoGit).unwrap();
            let dir = fixture.root().join(".firescale").join("cache");
            fs::create_dir_all(&dir).unwrap();
            let entry = dir.join("core__x.json");
            std::os::unix::fs::symlink(&planted, &entry).unwrap();
            let cache = Cache::new(fixture.root());
            assert!(
                cache.get("core/x", "key").is_none(),
                "a result was read through a linked entry file"
            );
            // A miss leaves the link as it is.
            assert!(fs::symlink_metadata(&entry)
                .unwrap()
                .file_type()
                .is_symlink());
            // The next result replaces the link with a real file. The file
            // the link pointed at is not written.
            cache.put("core/x", "key2", &[]).unwrap();
            assert!(fs::symlink_metadata(&entry).unwrap().file_type().is_file());
            assert_eq!(fs::read_to_string(&planted).unwrap(), record);
            assert!(cache.get("core/x", "key2").is_some());
            assert_eq!(names_in(&dir), ["core__x.json"]);
        }

        #[test]
        fn the_ledger_is_never_written_through_a_symlink() {
            // The folder is a link.
            let (fixture, outside) = linked_out(".firescale");
            let error = append_ledger(fixture.root(), &entry()).unwrap_err();
            assert_eq!(names_in(outside.path()), ["keep.txt"]);
            assert_eq!(
            error.message(),
            ".firescale is a symbolic link, and firescale reads and writes nothing through one. Remove the link: a real folder or file is made in its place."
        );
            // The file is a link: to a file that is there, then to one that is not.
            for target in ["keep.txt", "new.txt"] {
                let fixture = Fixture::new(&[("a.md", "one\n")], Track::NoGit).unwrap();
                let outside = tempfile::tempdir().unwrap();
                fs::write(outside.path().join("keep.txt"), "kept\n").unwrap();
                let dir = fixture.root().join(".firescale");
                fs::create_dir_all(&dir).unwrap();
                std::os::unix::fs::symlink(outside.path().join(target), dir.join("ledger.jsonl"))
                    .unwrap();
                let error = append_ledger(fixture.root(), &entry()).unwrap_err();
                assert_eq!(names_in(outside.path()), ["keep.txt"], "{target}");
                assert_eq!(
                    fs::read_to_string(outside.path().join("keep.txt")).unwrap(),
                    "kept\n"
                );
                assert!(
                    error
                        .message()
                        .starts_with(".firescale/ledger.jsonl is a symbolic link"),
                    "{}",
                    error.message()
                );
            }
            // A real folder and file: written.
            let fixture = Fixture::new(&[("a.md", "one\n")], Track::NoGit).unwrap();
            append_ledger(fixture.root(), &entry()).unwrap();
            append_ledger(fixture.root(), &entry()).unwrap();
            let text =
                fs::read_to_string(fixture.root().join(".firescale").join("ledger.jsonl")).unwrap();
            assert_eq!(text.lines().count(), 2);
        }
    }
}
