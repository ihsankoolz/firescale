//! Tests for `core/gitignore-basics`.

use firescale_core::model::Fixable;
use firescale_core::rule::{fix_rule_with, BuiltinRule, Options};
use firescale_core::testing::{Fixture, Track};

use super::{GitignoreBasics, ID};

const PKG: &str = "{\"name\":\"demo\"}\n";
const CARGO: &str = "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n";

fn messages(fixture: &Fixture) -> Vec<String> {
    fixture
        .check(&GitignoreBasics, &Options::new())
        .unwrap()
        .into_iter()
        .map(|d| d.message)
        .collect()
}

#[test]
fn meta_matches_the_spec() {
    let meta = GitignoreBasics.meta();
    assert_eq!(meta.id, ID);
    assert!(meta.has_fix);
}

mod passing_repo {
    use super::*;

    #[test]
    fn accepts_all_equivalent_spellings() {
        for line in [
            "node_modules",
            "node_modules/",
            "/node_modules",
            "**/node_modules/",
            "node_modules/**",
        ] {
            let f = Fixture::new(
                &[
                    ("package.json", PKG),
                    (".gitignore", &format!("{line}\n*.log\n")),
                ],
                Track::All,
            )
            .unwrap();
            assert!(messages(&f).is_empty(), "{line} should count");
        }
    }

    #[test]
    fn accepts_rust_and_python_entries() {
        let f = Fixture::new(
            &[
                ("Cargo.toml", CARGO),
                ("requirements.txt", "requests\n"),
                (".gitignore", "# c\n/target\n__pycache__/\n.venv\n*.log\n"),
            ],
            Track::All,
        )
        .unwrap();
        assert!(messages(&f).is_empty());
    }

    #[test]
    fn reports_nothing_outside_a_git_repo() {
        let f = Fixture::new(&[("package.json", PKG)], Track::NoGit).unwrap();
        assert!(messages(&f).is_empty());
    }
}

mod failing_repo {
    use super::*;

    #[test]
    fn reports_a_missing_gitignore() {
        let f = Fixture::new(&[("package.json", PKG)], Track::All).unwrap();
        let found = f.check(&GitignoreBasics, &Options::new()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].file, ".gitignore");
        assert_eq!(found[0].fixable, Fixable::Deterministic);
    }

    #[test]
    fn reports_each_missing_entry() {
        let f = Fixture::new(
            &[
                ("Cargo.toml", CARGO),
                ("package.json", PKG),
                (".gitignore", "# node_modules\n.env\n"),
            ],
            Track::All,
        )
        .unwrap();
        let found = messages(&f);
        assert_eq!(found.len(), 3);
        assert!(found.iter().any(|m| m.contains("node_modules")));
        assert!(found.iter().any(|m| m.contains("target")));
        assert!(found.iter().any(|m| m.contains("*.log")));
    }
}

mod fixed_repo {
    use super::*;

    #[test]
    fn creates_the_file() {
        let f = Fixture::new(&[("Cargo.toml", CARGO)], Track::All).unwrap();
        let result = f.fix(&GitignoreBasics, &Options::new()).unwrap();
        assert_eq!(result.changed_files, [".gitignore"]);
        assert!(f.read(".gitignore").unwrap().contains("target/"));
        assert!(messages(&f).is_empty());
    }

    #[test]
    fn appends_and_keeps_crlf() {
        let f = Fixture::new(
            &[("package.json", PKG), (".gitignore", "dist\r\n.env")],
            Track::All,
        )
        .unwrap();
        f.fix(&GitignoreBasics, &Options::new()).unwrap();
        let text = f.read(".gitignore").unwrap();
        assert!(text.starts_with("dist\r\n.env\r\n# "));
        assert!(text.ends_with("node_modules/\r\n*.log\r\n"));
        assert!(!text.replace("\r\n", "").contains('\n'));
        assert!(messages(&f).is_empty());
    }

    #[test]
    fn fix_twice_is_idempotent() {
        let f = Fixture::new(&[("package.json", PKG), ("Cargo.toml", CARGO)], Track::All).unwrap();
        assert!(!f
            .fix(&GitignoreBasics, &Options::new())
            .unwrap()
            .changed_files
            .is_empty());
        let after_first = f.read(".gitignore");
        let second = f.fix(&GitignoreBasics, &Options::new()).unwrap();
        assert!(second.changed_files.is_empty());
        assert_eq!(f.read(".gitignore"), after_first);
    }

    #[test]
    fn dry_run_changes_nothing() {
        let f = Fixture::new(&[("package.json", PKG)], Track::All).unwrap();
        let outcome = fix_rule_with(&GitignoreBasics, f.root(), &Options::new(), true).unwrap();
        assert_eq!(outcome.changes.len(), 1);
        assert!(f.read(".gitignore").is_none());
    }
}

mod negation {
    use super::*;

    #[test]
    fn a_later_negation_uncovers_the_entry_and_a_later_ignore_covers_it_again() {
        let f = Fixture::new(
            &[
                ("package.json", PKG),
                (".gitignore", "*.log\nnode_modules/\n!node_modules/\n"),
            ],
            Track::All,
        )
        .unwrap();
        assert_eq!(messages(&f).len(), 1);
        let g = Fixture::new(
            &[
                ("package.json", PKG),
                (".gitignore", "!node_modules\nnode_modules/\n*.log\n"),
            ],
            Track::All,
        )
        .unwrap();
        assert!(messages(&g).is_empty());
    }

    #[test]
    fn fix_appends_after_the_negation_and_is_idempotent() {
        let f = Fixture::new(
            &[
                ("package.json", PKG),
                (".gitignore", "*.log\nnode_modules/\n!node_modules/\n"),
            ],
            Track::All,
        )
        .unwrap();
        let first = f.fix(&GitignoreBasics, &Options::new()).unwrap();
        assert_eq!(first.changed_files, [".gitignore"]);
        assert!(f.read(".gitignore").unwrap().ends_with("node_modules/\n"));
        assert!(messages(&f).is_empty());
        let after = f.read(".gitignore");
        let second = f.fix(&GitignoreBasics, &Options::new()).unwrap();
        assert!(second.changed_files.is_empty());
        assert_eq!(f.read(".gitignore"), after);
    }
}

mod projects_in_subfolders {
    use super::*;

    const SERVICE: [(&str, &str); 2] = [
        ("backend/api/requirements.txt", "pytest\n"),
        ("backend/api/main.py", "app = None\n"),
    ];

    #[test]
    fn a_python_service_in_a_subfolder_needs_the_python_entries() {
        let mut files = SERVICE.to_vec();
        files.push((".gitignore", "node_modules\n*.log\n"));
        let f = Fixture::new(&files, Track::All).unwrap();
        let found = messages(&f);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].contains(".venv") && found[1].contains("__pycache__"));
    }

    #[test]
    fn it_passes_once_they_are_listed() {
        let mut files = SERVICE.to_vec();
        files.push((".gitignore", "__pycache__/\n.venv/\n*.log\n"));
        let f = Fixture::new(&files, Track::All).unwrap();
        assert!(messages(&f).is_empty());
    }
}

// The 0.5.1 run on the course project (docs/reviews/cs464-0.5.1.md,
// finding 5): an untracked verify.log at the root made `verify --affected`
// run every project. The owner decided that the rule ignores `*.log`.
mod log_files {
    use super::*;
    use firescale_core::walk::gitignore_covers_file;

    const ADOPTED: &str = "# Generated files and dependencies\nnode_modules/\n";

    #[test]
    fn an_adopted_gitignore_without_log_files_is_reported() {
        let f = Fixture::new(
            &[("package.json", PKG), (".gitignore", ADOPTED)],
            Track::All,
        )
        .unwrap();
        assert_eq!(
            messages(&f),
            [".gitignore does not ignore \"*.log\". Add \"*.log\" to .gitignore so generated files are not committed."]
        );
    }

    #[test]
    fn fix_adds_the_line_to_the_existing_block_and_git_then_ignores_verify_log() {
        let f = Fixture::new(
            &[("package.json", PKG), (".gitignore", ADOPTED)],
            Track::All,
        )
        .unwrap();
        let first = f.fix(&GitignoreBasics, &Options::new()).unwrap();
        assert_eq!(first.changed_files, [".gitignore"]);
        let text = f.read(".gitignore").unwrap();
        assert_eq!(text, format!("{ADOPTED}*.log\n"));
        // `*.log` has no slash: git reads it in every folder.
        assert!(gitignore_covers_file(&text, "verify.log"));
        assert!(gitignore_covers_file(
            &text,
            "backend/audit_service/verify.log"
        ));
        assert!(messages(&f).is_empty());
        let second = f.fix(&GitignoreBasics, &Options::new()).unwrap();
        assert!(second.changed_files.is_empty());
        assert_eq!(f.read(".gitignore").unwrap(), text);
    }

    #[test]
    fn a_new_gitignore_lists_log_files_after_the_folders() {
        let f = Fixture::new(&[("Cargo.toml", CARGO)], Track::All).unwrap();
        f.fix(&GitignoreBasics, &Options::new()).unwrap();
        assert_eq!(
            f.read(".gitignore").unwrap(),
            "# Generated files and dependencies\ntarget/\n*.log\n"
        );
    }

    #[test]
    fn a_repo_with_no_detected_language_still_ignores_log_files() {
        let f = Fixture::new(
            &[("README.md", "# demo\n"), (".gitignore", ".env\n")],
            Track::All,
        )
        .unwrap();
        assert_eq!(messages(&f).len(), 1);
        f.fix(&GitignoreBasics, &Options::new()).unwrap();
        assert_eq!(
            f.read(".gitignore").unwrap(),
            ".env\n# Generated files and dependencies\n*.log\n"
        );
    }

    #[test]
    fn any_line_that_ignores_log_files_at_the_root_counts_and_a_negation_does_not() {
        for line in ["*.log", "/*.log", "**/*.log", "*.log\n!important.log"] {
            let f = Fixture::new(
                &[
                    ("package.json", PKG),
                    (".gitignore", &format!("node_modules/\n{line}\n")),
                ],
                Track::All,
            )
            .unwrap();
            assert!(messages(&f).is_empty(), "{line} should count");
        }
        let f = Fixture::new(
            &[
                ("package.json", PKG),
                (".gitignore", "node_modules/\n*.log\n!*.log\n"),
            ],
            Track::All,
        )
        .unwrap();
        assert_eq!(messages(&f).len(), 1);
    }
}
