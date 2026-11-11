//! Tests for `core/single-lockfile`.

use firescale_core::model::Fixable;
use firescale_core::rule::{BuiltinRule, Options};
use firescale_core::testing::{Fixture, Track};

use super::{SingleLockfile, ID};

const PKG: &str = "{\"name\":\"demo\"}\n";
const PKG_PNPM: &str = "{\"name\":\"demo\",\"packageManager\":\"pnpm@9.15.0\"}\n";
const PKG_PNPM_DEPS: &str =
    "{\"name\":\"demo\",\"packageManager\":\"pnpm@9.15.0\",\"dependencies\":{\"left-pad\":\"1.3.0\"}}\n";
const LOCK: &str = "lock\n";

fn files(fixture: &Fixture) -> Vec<String> {
    fixture
        .check(&SingleLockfile, &Options::new())
        .unwrap()
        .into_iter()
        .map(|d| d.file)
        .collect()
}

#[test]
fn meta_has_no_fix() {
    let meta = SingleLockfile.meta();
    assert_eq!(meta.id, ID);
    assert!(!meta.has_fix);
}

mod passing_repo {
    use super::*;

    #[test]
    fn one_lockfile_without_declared_manager() {
        let f = Fixture::new(&[("package.json", PKG), ("yarn.lock", LOCK)], Track::All).unwrap();
        assert!(files(&f).is_empty());
    }

    #[test]
    fn one_lockfile_matching_declared_manager() {
        let f = Fixture::new(
            &[("package.json", PKG_PNPM), ("pnpm-lock.yaml", LOCK)],
            Track::All,
        )
        .unwrap();
        assert!(files(&f).is_empty());
    }

    #[test]
    fn no_package_json_means_no_findings() {
        let f = Fixture::new(&[("Cargo.toml", "[package]\n")], Track::All).unwrap();
        assert!(files(&f).is_empty());
    }
}

mod failing_repo {
    use super::*;

    #[test]
    fn reports_missing_lockfile_on_package_json() {
        let f = Fixture::new(&[("package.json", PKG_PNPM_DEPS)], Track::All).unwrap();
        let found = f.check(&SingleLockfile, &Options::new()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].file, "package.json");
        assert_eq!(found[0].fixable, Fixable::Manual);
        assert!(found[0].message.contains("pnpm install"));
    }

    #[test]
    fn flags_only_extra_lockfiles_when_manager_declared() {
        let f = Fixture::new(
            &[
                ("package.json", PKG_PNPM),
                ("pnpm-lock.yaml", LOCK),
                ("package-lock.json", LOCK),
                ("yarn.lock", LOCK),
            ],
            Track::All,
        )
        .unwrap();
        assert_eq!(files(&f), ["package-lock.json", "yarn.lock"]);
    }

    #[test]
    fn flags_every_lockfile_when_nothing_declared() {
        let f = Fixture::new(
            &[
                ("package.json", PKG),
                ("pnpm-lock.yaml", LOCK),
                ("yarn.lock", LOCK),
            ],
            Track::All,
        )
        .unwrap();
        let found = f.check(&SingleLockfile, &Options::new()).unwrap();
        assert_eq!(found.len(), 2);
        assert!(found[0].message.contains("packageManager"));
    }

    #[test]
    fn flags_lockfile_of_a_different_manager() {
        let f = Fixture::new(
            &[("package.json", PKG_PNPM), ("package-lock.json", LOCK)],
            Track::All,
        )
        .unwrap();
        assert_eq!(files(&f), ["package-lock.json"]);
    }

    #[test]
    fn wrong_manager_message_says_declared_lockfile_is_missing() {
        let f = Fixture::new(
            &[("package.json", PKG_PNPM), ("package-lock.json", LOCK)],
            Track::All,
        )
        .unwrap();
        let found = f.check(&SingleLockfile, &Options::new()).unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].message.contains("unexpected"));
        assert!(found[0].message.contains("pnpm lockfile is missing"));
    }

    #[test]
    fn counts_binary_bun_lockfile() {
        let f = Fixture::new(
            &[
                ("package.json", PKG),
                ("bun.lockb", LOCK),
                ("bun.lock", LOCK),
            ],
            Track::All,
        )
        .unwrap();
        assert_eq!(files(&f), ["bun.lock", "bun.lockb"]);
    }
}

mod nothing_to_lock {
    use super::*;

    #[test]
    fn package_without_dependencies_needs_no_lockfile() {
        let f = Fixture::new(&[("package.json", PKG_PNPM)], Track::All).unwrap();
        assert!(files(&f).is_empty());
    }

    #[test]
    fn empty_dependency_maps_still_need_no_lockfile() {
        let pkg = "{\"dependencies\":{},\"devDependencies\":{}}\n";
        let f = Fixture::new(&[("package.json", pkg)], Track::All).unwrap();
        assert!(files(&f).is_empty());
    }

    #[test]
    fn dev_dependencies_need_a_lockfile() {
        let pkg = "{\"devDependencies\":{\"vitest\":\"1.0.0\"}}\n";
        let f = Fixture::new(&[("package.json", pkg)], Track::All).unwrap();
        assert_eq!(files(&f), ["package.json"]);
    }

    #[test]
    fn workspaces_field_needs_a_lockfile() {
        let pkg = "{\"workspaces\":[\"packages/*\"]}\n";
        let f = Fixture::new(&[("package.json", pkg)], Track::All).unwrap();
        assert_eq!(files(&f), ["package.json"]);
    }

    #[test]
    fn pnpm_workspace_file_needs_a_lockfile() {
        let f = Fixture::new(
            &[
                ("package.json", PKG),
                ("pnpm-workspace.yaml", "packages:\n  - a\n"),
            ],
            Track::All,
        )
        .unwrap();
        assert_eq!(files(&f), ["package.json"]);
    }
}

mod workspace_member {
    use super::*;
    use firescale_core::rule::check_rule;

    fn member_files(files: &[(&str, &str)]) -> Vec<String> {
        let f = Fixture::new(files, Track::All).unwrap();
        let root = f.root().join("packages").join("app");
        check_rule(&SingleLockfile, &root, &Options::new())
            .unwrap()
            .into_iter()
            .map(|d| d.file)
            .collect()
    }

    #[test]
    fn member_with_root_lockfile_is_fine() {
        let found = member_files(&[
            ("package.json", PKG),
            ("pnpm-lock.yaml", LOCK),
            ("packages/app/package.json", PKG_PNPM_DEPS),
        ]);
        assert!(found.is_empty());
    }

    #[test]
    fn member_without_any_lockfile_is_reported() {
        let found = member_files(&[
            ("package.json", PKG),
            ("packages/app/package.json", PKG_PNPM_DEPS),
        ]);
        assert_eq!(found, ["package.json"]);
    }
}

mod fixed_repo {
    use super::*;

    #[test]
    fn passes_after_the_extra_lockfile_is_removed() {
        let f = Fixture::new(
            &[
                ("package.json", PKG_PNPM),
                ("pnpm-lock.yaml", LOCK),
                ("yarn.lock", LOCK),
            ],
            Track::All,
        )
        .unwrap();
        assert_eq!(files(&f), ["yarn.lock"]);
        std::fs::remove_file(f.root().join("yarn.lock")).unwrap();
        assert!(files(&f).is_empty());
    }

    #[test]
    fn fix_does_nothing() {
        let f = Fixture::new(&[("package.json", PKG)], Track::All).unwrap();
        assert!(f
            .fix(&SingleLockfile, &Options::new())
            .unwrap()
            .changed_files
            .is_empty());
    }
}

#[test]
fn byte_order_mark_does_not_hide_the_declared_manager() {
    let pkg = "\u{feff}{\"name\":\"demo\",\"packageManager\":\"yarn@4.1.0\"}\n";
    let f = Fixture::new(
        &[("package.json", pkg), ("package-lock.json", LOCK)],
        Track::All,
    )
    .unwrap();
    assert_eq!(files(&f), ["package-lock.json"]);
}
