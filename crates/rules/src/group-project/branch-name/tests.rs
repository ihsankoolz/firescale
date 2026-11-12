//! Tests for `group-project/branch-name`.

use firescale_core::git::run_git;
use firescale_core::model::{Category, Fixable, Severity};
use firescale_core::rule::{BuiltinRule, Options, RuleContext};
use firescale_core::testing::{Fixture, Track};
use serde_json::json;

use super::{check_with, BranchName, ID};

/// A repo on `branch`, before its first commit.
fn on_branch(branch: &str) -> Fixture {
    let f = Fixture::new(&[("README.md", "x\n")], Track::All).unwrap();
    let head = format!("refs/heads/{branch}");
    assert_eq!(
        run_git(f.root(), &["symbolic-ref", "HEAD", &head])
            .unwrap()
            .code,
        0
    );
    f
}

fn options(value: serde_json::Value) -> Options {
    value.as_object().cloned().unwrap()
}

fn findings(f: &Fixture, opts: &Options, head_ref: Option<&str>) -> Vec<String> {
    let ctx = RuleContext::new(f.root(), opts.clone());
    check_with(&ctx, head_ref.map(str::to_owned))
        .unwrap()
        .into_iter()
        .map(|finding| finding.message)
        .collect()
}

#[test]
fn meta_matches_the_spec() {
    let meta = BranchName.meta();
    assert_eq!(meta.id, ID);
    assert_eq!(meta.severity, Severity::Low);
    assert_eq!(meta.category, Category::Consistency);
    assert!(!meta.has_fix);
    // Never cached: the branch is not a file.
    assert!(meta.inputs.is_empty());
}

mod passing_repo {
    use super::*;

    #[test]
    fn default_patterns_allow_main_and_kinds_of_change() {
        for branch in [
            "main",
            "master",
            "develop",
            "feat/login-page",
            "fix/token-expiry",
            "docs/readme",
        ] {
            let f = on_branch(branch);
            assert!(findings(&f, &Options::new(), None).is_empty(), "{branch}");
        }
    }

    #[test]
    fn nothing_to_check_outside_git_or_with_a_detached_head_and_no_pull_request() {
        let f = Fixture::new(&[("a.txt", "")], Track::NoGit).unwrap();
        assert!(findings(&f, &Options::new(), Some("bad")).is_empty());
        let f = Fixture::new(&[("a.txt", "")], Track::All).unwrap();
        for args in [
            vec![
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "commit",
                "-q",
                "-m",
                "x",
            ],
            vec!["checkout", "-q", "--detach"],
        ] {
            assert_eq!(run_git(f.root(), &args).unwrap().code, 0);
        }
        assert!(findings(&f, &Options::new(), None).is_empty());
        // In a pull request run, the pull request's branch is checked.
        assert!(findings(&f, &Options::new(), Some("feat/x")).is_empty());
        assert_eq!(findings(&f, &Options::new(), Some("my-stuff")).len(), 1);
    }

    #[test]
    fn the_branches_option_replaces_the_defaults() {
        let f = on_branch("story/42-login");
        let opts = options(json!({ "branches": ["main", "story/*"] }));
        assert!(findings(&f, &opts, None).is_empty());
    }
}

mod failing_repo {
    use super::*;

    #[test]
    fn a_name_outside_the_patterns_is_a_manual_finding_with_a_rename_command() {
        let f = on_branch("my-stuff");
        let found = f.check(&BranchName, &Options::new()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].fixable, Fixable::Manual);
        assert!(
            found[0]
                .message
                .starts_with("The branch \"my-stuff\" does not match"),
            "{}",
            found[0].message
        );
        assert!(
            found[0].message.contains("`git branch -m feat/my-stuff`"),
            "{}",
            found[0].message
        );
    }

    #[test]
    fn a_kind_the_patterns_do_not_list_is_a_finding() {
        let f = on_branch("wip/thing");
        assert_eq!(findings(&f, &Options::new(), None).len(), 1);
        let opts = options(json!({ "branches": ["main"] }));
        let f = on_branch("feat/x");
        assert_eq!(findings(&f, &opts, None).len(), 1);
    }

    #[test]
    fn a_bad_option_is_a_user_error_naming_it() {
        let f = on_branch("main");
        for bad in [
            json!({ "branches": [] }),
            json!({ "branches": "main" }),
            json!({ "branches": [1] }),
            json!({ "branches": ["[oops"] }),
        ] {
            let error = f.check(&BranchName, &options(bad.clone())).unwrap_err();
            assert!(
                error.message().contains("option \"branches\""),
                "{bad}: {}",
                error.message()
            );
        }
    }
}

mod fixed_repo {
    use super::*;

    #[test]
    fn renaming_the_branch_clears_the_finding() {
        let f = on_branch("my-stuff");
        assert_eq!(findings(&f, &Options::new(), None).len(), 1);
        assert_eq!(
            run_git(
                f.root(),
                &["symbolic-ref", "HEAD", "refs/heads/feat/my-stuff"]
            )
            .unwrap()
            .code,
            0
        );
        assert!(findings(&f, &Options::new(), None).is_empty());
    }
}
