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
                &[("package.json", PKG), (".gitignore", &format!("{line}\n"))],
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
                (".gitignore", "# c\n/target\n__pycache__/\n.venv\n"),
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
        assert_eq!(found.len(), 2);
        assert!(found.iter().any(|m| m.contains("node_modules")));
        assert!(found.iter().any(|m| m.contains("target")));
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
        assert!(text.ends_with("node_modules/\r\n"));
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
                (".gitignore", "node_modules/\n!node_modules/\n"),
            ],
            Track::All,
        )
        .unwrap();
        assert_eq!(messages(&f).len(), 1);
        let g = Fixture::new(
            &[
                ("package.json", PKG),
                (".gitignore", "!node_modules\nnode_modules/\n"),
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
                (".gitignore", "node_modules/\n!node_modules/\n"),
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
