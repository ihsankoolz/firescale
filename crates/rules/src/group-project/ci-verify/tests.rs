//! Tests for `group-project/ci-verify`.

use firescale_core::model::{Category, Diagnostic, Fixable, Severity};
use firescale_core::rule::{BuiltinRule, Options};
use firescale_core::testing::{Fixture, Track};

use super::{verifies_pull_requests, CiVerify, ID};
use crate::core::ci_exists::{CiExists, STARTER};
const TAMPER: &str = ".github/workflows/tamper-guard.yml";

fn check(fixture: &Fixture) -> Vec<Diagnostic> {
    fixture.check(&CiVerify, &Options::new()).unwrap()
}

const HAND_WRITTEN: &str = "# Our CI. Do not touch.\r\nname: tests\r\non: [pull_request]\r\njobs:\r\n  test:\r\n    runs-on: ubuntu-latest\r\n    steps:\r\n      - run: npm ci\r\n      - run: npm test\r\n";

#[test]
fn meta_matches_the_spec() {
    let meta = CiVerify.meta();
    assert_eq!(meta.id, ID);
    assert_eq!(meta.severity, Severity::Medium);
    assert_eq!(meta.category, Category::Reliability);
    assert!(meta.has_fix);
    assert_eq!(meta.after, ["core/ci-exists"]);
}

mod passing_repo {
    use super::*;

    #[test]
    fn the_ways_a_workflow_runs_verify_count() {
        for run in [
            "      - run: npx firescale verify",
            "      - run: pnpm fscale verify --json",
            "      - run: npx --yes firescale@0.1.2 verify",
            "      - run: |\n          npm ci\n          npx firescale verify",
            "      - run: ./node_modules/.bin/fscale verify",
            "      - run: npm ci && npx firescale verify",
            "      - run: CI=1 npx --yes firescale --cwd app verify",
            "      - name: Check\n        run: \"npx firescale verify\"",
            "      - run: >\n          npx firescale\n          verify",
        ] {
            let text = format!("on:\n  pull_request:\njobs:\n  a:\n    steps:\n{run}\n");
            assert!(verifies_pull_requests(&text), "{run}");
        }
    }

    #[test]
    fn a_workflow_that_runs_verify_on_pull_requests_passes() {
        let f = Fixture::new(
            &[(".github/workflows/ci.yml", "on: [push, pull_request]\njobs:\n  v:\n    steps:\n      - run: npx firescale verify\n")],
            Track::All,
        )
        .unwrap();
        assert!(check(&f).is_empty());
    }
}

mod failing_repo {
    use super::*;

    #[test]
    fn what_does_not_count() {
        for text in [
            // verify only on push
            "on: [push]\njobs:\n  a:\n    steps:\n      - run: npx firescale verify\n",
            // only in a comment
            "on: [pull_request]\njobs:\n  a:\n    steps:\n      # - run: npx firescale verify\n      - run: npm test\n",
            // another command
            "on: [pull_request]\njobs:\n  a:\n    steps:\n      - run: npx firescale check\n",
            "on: [pull_request]\njobs:\n  a:\n    steps:\n      - run: npm run verify\n",
            // only in a step's name, an echo, or a later command
            "on: [pull_request]\njobs:\n  a:\n    steps:\n      - name: Run fscale verify\n        run: npm test\n",
            "on: [pull_request]\njobs:\n  a:\n    steps:\n      - run: echo \"run npx firescale verify locally\"\n",
            "on: [pull_request]\njobs:\n  a:\n    steps:\n      - run: npx fscale check && echo verify\n",
            // a block that ended before the line
            "on: [pull_request]\njobs:\n  a:\n    steps:\n      - run: |\n          npm ci\n    env:\n      X: npx firescale verify\n",
        ] {
            assert!(!verifies_pull_requests(text), "{text}");
        }
    }

    #[test]
    fn a_hand_written_workflow_without_verify_is_agent_fixable_and_never_rewritten() {
        let file = ".github/workflows/tests.yml";
        let f = Fixture::new(&[(file, HAND_WRITTEN)], Track::All).unwrap();
        let found = check(&f);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].file, file);
        assert_eq!(found[0].fixable, Fixable::Agent);
        assert!(found[0].message.contains("npx firescale verify"));
        let result = f.fix(&CiVerify, &Options::new()).unwrap();
        assert!(result.changed_files.is_empty());
        assert_eq!(f.read(file).as_deref(), Some(HAND_WRITTEN));
        assert_eq!(f.read(STARTER), None);
    }

    #[test]
    fn ci_of_another_system_is_a_manual_finding() {
        let f = Fixture::new(&[(".gitlab-ci.yml", "test:\n  script: make\n")], Track::All).unwrap();
        let found = check(&f);
        assert_eq!(found[0].fixable, Fixable::Manual);
        assert!(f
            .fix(&CiVerify, &Options::new())
            .unwrap()
            .changed_files
            .is_empty());
    }

    #[test]
    fn no_ci_at_all_is_deterministic() {
        let f = Fixture::new(&[("README.md", "x\n")], Track::All).unwrap();
        let found = check(&f);
        assert_eq!(found[0].fixable, Fixable::Deterministic);
        assert_eq!(found[0].file, STARTER);
    }
}

mod fixed_repo {
    use super::*;

    #[test]
    fn the_fix_creates_the_starter_and_the_check_passes_then_nothing_changes() {
        let f = Fixture::new(&[("README.md", "x\n")], Track::All).unwrap();
        let result = f.fix(&CiVerify, &Options::new()).unwrap();
        assert_eq!(result.changed_files, [STARTER, TAMPER]);
        assert!(check(&f).is_empty());
        let before = f.read(STARTER);
        assert!(f
            .fix(&CiVerify, &Options::new())
            .unwrap()
            .changed_files
            .is_empty());
        assert_eq!(f.read(STARTER), before);
    }

    #[test]
    fn after_core_ci_exists_has_written_the_starter_there_is_nothing_left_to_do() {
        let f = Fixture::new(&[("README.md", "x\n")], Track::All).unwrap();
        assert_eq!(
            f.fix(&CiExists, &Options::new()).unwrap().changed_files,
            [STARTER, TAMPER]
        );
        assert!(check(&f).is_empty());
        assert!(f
            .fix(&CiVerify, &Options::new())
            .unwrap()
            .changed_files
            .is_empty());
    }
}
