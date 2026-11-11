//! Tests for `core/ci-exists`.

use firescale_core::model::{Category, Fixable, Severity};
use firescale_core::rule::{BuiltinRule, Options};
use firescale_core::testing::{Fixture, Track};

use super::{CiExists, ID};

const PR_WORKFLOW: &str = "name: ci\non:\n  pull_request:\n  push:\n    branches: [main]\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@v4\n";
const PUSH_ONLY: &str =
    "name: ci\non:\n  push:\n    branches: [main]\n# pull_request is not enabled yet\njobs: {}\n";

fn check(fixture: &Fixture) -> Vec<firescale_core::model::Diagnostic> {
    fixture.check(&CiExists, &Options::new()).unwrap()
}

mod passing_repo {
    use super::*;

    #[test]
    fn a_workflow_with_a_pull_request_trigger_passes() {
        let f = Fixture::new(&[(".github/workflows/ci.yml", PR_WORKFLOW)], Track::All).unwrap();
        assert!(check(&f).is_empty());
    }

    #[test]
    fn yaml_extension_and_pull_request_target_pass() {
        let f = Fixture::new(
            &[(
                ".github/workflows/ci.yaml",
                "on: [pull_request_target]\njobs: {}\n",
            )],
            Track::All,
        )
        .unwrap();
        assert!(check(&f).is_empty());
    }

    #[test]
    fn one_matching_workflow_among_several_passes() {
        let f = Fixture::new(
            &[
                (".github/workflows/a-release.yml", PUSH_ONLY),
                (".github/workflows/b-ci.yml", PR_WORKFLOW),
            ],
            Track::All,
        )
        .unwrap();
        assert!(check(&f).is_empty());
    }

    #[test]
    fn another_ci_system_is_not_reported() {
        let f = Fixture::new(&[(".gitlab-ci.yml", "test:\n  script: make\n")], Track::All).unwrap();
        assert!(check(&f).is_empty());
    }

    #[test]
    fn works_outside_a_git_repo() {
        let f = Fixture::new(&[(".github/workflows/ci.yml", PR_WORKFLOW)], Track::NoGit).unwrap();
        assert!(check(&f).is_empty());
    }
}

mod failing_repo {
    use super::*;

    #[test]
    fn no_workflow_is_reported_on_the_workflows_folder() {
        let f = Fixture::new(&[("README.md", "# demo\n")], Track::All).unwrap();
        let d = check(&f);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].rule_id, ID);
        assert_eq!(d[0].file, ".github/workflows");
        assert_eq!(d[0].fixable, Fixable::Agent);
        assert!(d[0].message.contains("Add a workflow"));
    }

    #[test]
    fn untracked_workflow_does_not_count_in_a_git_repo() {
        let f = Fixture::new(&[(".github/workflows/ci.yml", PR_WORKFLOW)], Track::None).unwrap();
        assert_eq!(check(&f)[0].file, ".github/workflows");
    }

    #[test]
    fn workflows_without_a_pull_request_trigger_are_reported_on_the_first_one() {
        let f = Fixture::new(
            &[
                (".github/workflows/b.yml", PUSH_ONLY),
                (".github/workflows/a.yml", PUSH_ONLY),
            ],
            Track::All,
        )
        .unwrap();
        let d = check(&f);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].file, ".github/workflows/a.yml");
        assert_eq!(d[0].fixable, Fixable::Agent);
        assert!(d[0].message.contains("pull_request"));
    }

    #[test]
    fn a_non_workflow_file_in_the_folder_does_not_count() {
        let f = Fixture::new(
            &[(".github/workflows/notes.md", "pull_request\n")],
            Track::All,
        )
        .unwrap();
        assert_eq!(check(&f)[0].file, ".github/workflows");
    }
}

mod metadata {
    use super::*;

    #[test]
    fn meta_matches_the_spec() {
        let meta = CiExists.meta();
        assert_eq!(meta.id, ID);
        assert_eq!(meta.severity, Severity::Medium);
        assert_eq!(meta.category, Category::Reliability);
        assert!(!meta.has_fix);
        assert_eq!(meta.inputs.len(), 16);
    }
}

#[test]
fn other_ci_systems_silence_the_rule() {
    for file in [".travis.yml", ".buildkite/pipeline.yaml"] {
        let f = Fixture::new(&[(file, "script: make\n")], Track::All).unwrap();
        assert!(check(&f).is_empty(), "{file}");
    }
}

#[test]
fn meta_inputs_list_other_ci_configs() {
    let inputs = CiExists.meta().inputs;
    assert!(inputs.contains(&".drone.yml".to_owned()));
    assert!(inputs.contains(&".appveyor.yml".to_owned()));
}

mod trigger_shapes {
    use super::*;

    fn passes(on: &str) -> bool {
        let text = format!("name: ci\n{on}\njobs: {{}}\n");
        let f = Fixture::new(&[(".github/workflows/ci.yml", text.as_str())], Track::All).unwrap();
        check(&f).is_empty()
    }

    #[test]
    fn scalar_flow_list_block_list_and_mapping_pass() {
        assert!(passes("on: pull_request"));
        assert!(passes("on: [push, pull_request]"));
        assert!(passes("\"on\": [push, pull_request] # trigger"));
        assert!(passes("'on':\n  - push\n  - pull_request"));
        assert!(passes(
            "on:\n  push:\n    branches: [main]\n  pull_request_target:\n"
        ));
        assert!(passes("on:\n  pull_request:\n    branches: [main]"));
    }

    #[test]
    fn nested_or_commented_mentions_do_not_pass() {
        assert!(!passes("on:\n  push:\n    branches: [pull_request]"));
        assert!(!passes("on: push # pull_request later"));
        assert!(!passes("on: [push, workflow_dispatch]"));
    }

    #[test]
    fn echo_and_job_name_do_not_pass() {
        let text = "on: push\njobs:\n  pull_request_lint:\n    runs-on: x\n    steps:\n      - run: echo \"skip pull_request\"\n";
        let f = Fixture::new(&[(".github/workflows/ci.yml", text)], Track::All).unwrap();
        let d = check(&f);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].file, ".github/workflows/ci.yml");
    }

    #[test]
    fn reusable_workflow_next_to_a_real_pr_workflow_gives_no_finding() {
        let f = Fixture::new(
            &[
                (
                    ".github/workflows/a-reusable.yml",
                    "on:\n  workflow_call:\njobs: {}\n",
                ),
                (".github/workflows/b-ci.yml", PR_WORKFLOW),
            ],
            Track::All,
        )
        .unwrap();
        assert!(check(&f).is_empty());
    }
}
