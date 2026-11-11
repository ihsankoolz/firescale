//! Tests for `core/ci-exists`.

use firescale_core::model::{Category, Fixable, Severity};
use firescale_core::rule::{fix_rule_with, BuiltinRule, Change, Options};
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
        assert_eq!(d[0].fixable, Fixable::Deterministic);
        assert!(d[0].message.contains("Add a workflow"));
        assert!(d[0].message.contains("`fscale fix`"));
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
        assert!(meta.has_fix);
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

const STARTER: &str = ".github/workflows/ci.yml";

const HEAD: &str = "# Created by firescale (core/ci-exists). This file belongs to your team:\n# edit it freely. firescale will not rewrite it.\nname: CI\n\non:\n  pull_request:\n  push:\n    branches: [main, master]\n\npermissions:\n  contents: read\n\njobs:\n  verify:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1\n      - uses: actions/setup-node@949feb2413d6458794dcd2491c4babbbce0c15c1 # v7.1.0\n        with:\n          node-version: 24\n";
const TAIL: &str =
    "      # Runs check, lint, typecheck and test.\n      - run: npx firescale verify\n";

/// CRLF, no trailing newline, non-ASCII: nothing a fix would ever write.
const ODD: &str = "name: \u{e9}t\u{e9} \u{2603}\r\non: push\r\njobs: {}";

fn fix(fixture: &Fixture) -> firescale_core::model::FixResult {
    fixture.fix(&CiExists, &Options::new()).unwrap()
}

fn bytes(fixture: &Fixture, file: &str) -> Vec<u8> {
    let path = file
        .split('/')
        .fold(fixture.root().to_path_buf(), |p, part| p.join(part));
    std::fs::read(path).unwrap()
}

/// Every file under the fixture except `.git`, repo-relative, sorted.
fn files_on_disk(fixture: &Fixture) -> Vec<String> {
    fn walk(dir: &std::path::Path, prefix: &str, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if prefix.is_empty() && name == ".git" {
                continue;
            }
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if entry.path().is_dir() {
                walk(&entry.path(), &rel, out);
            } else {
                out.push(rel);
            }
        }
    }
    let mut out = Vec::new();
    walk(fixture.root(), "", &mut out);
    out.sort();
    out
}

mod untracked_ci {
    use super::*;

    #[test]
    fn an_untracked_workflow_is_manual_and_names_the_file() {
        let f = Fixture::new(
            &[(".github/workflows/build.yaml", PR_WORKFLOW)],
            Track::None,
        )
        .unwrap();
        let d = check(&f);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].file, ".github/workflows");
        assert_eq!(d[0].fixable, Fixable::Manual);
        assert!(d[0]
            .message
            .contains("git add .github/workflows/build.yaml"));
    }

    #[test]
    fn an_untracked_config_of_another_ci_system_is_manual() {
        let f = Fixture::new(
            &[(".gitlab-ci.yml", "test:\n  script: make\n")],
            Track::None,
        )
        .unwrap();
        let d = check(&f);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].fixable, Fixable::Manual);
        assert!(d[0].message.contains("git add .gitlab-ci.yml"));
        assert!(fix(&f).changed_files.is_empty());
        assert_eq!(files_on_disk(&f), [".gitlab-ci.yml"]);
    }

    #[test]
    fn a_tracked_workflow_missing_from_disk_is_manual_and_its_staged_content_survives() {
        let f = Fixture::new(&[(STARTER, PR_WORKFLOW)], Track::All).unwrap();
        let stage = |f: &Fixture| {
            firescale_core::git::run_git(f.root(), &["ls-files", "--stage"])
                .unwrap()
                .stdout
        };
        let staged = stage(&f);
        std::fs::remove_file(f.root().join(".github").join("workflows").join("ci.yml")).unwrap();
        let d = check(&f);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].file, ".github/workflows");
        assert_eq!(d[0].fixable, Fixable::Manual);
        assert!(d[0].message.contains(&format!("git restore {STARTER}")));
        // Nothing is created, and the index still holds the person's workflow.
        assert!(fix(&f).changed_files.is_empty());
        assert!(files_on_disk(&f).is_empty());
        assert_eq!(stage(&f), staged);
    }

    #[test]
    fn a_workflow_path_that_gitignore_ignores_is_manual_and_nothing_is_created() {
        let f = Fixture::new(&[(".gitignore", ".github/\n")], Track::All).unwrap();
        let d = check(&f);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].fixable, Fixable::Manual);
        assert!(d[0].message.contains(".gitignore rule ignores"));
        assert!(fix(&f).changed_files.is_empty());
        assert_eq!(files_on_disk(&f), [".gitignore"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_github_folder_is_never_written_through() {
        let outside = Fixture::new(&[], Track::NoGit).unwrap();
        let f = Fixture::new(&[("package.json", "{}\n")], Track::All).unwrap();
        std::os::unix::fs::symlink(outside.root(), f.root().join(".github")).unwrap();
        let d = check(&f);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].fixable, Fixable::Agent);
        assert!(fix(&f).changed_files.is_empty());
        assert_eq!(std::fs::read_dir(outside.root()).unwrap().count(), 0);
    }

    #[test]
    fn a_workflow_with_an_upper_case_extension_is_left_to_an_agent() {
        // Not a workflow for the check, but too close to one to add a second file.
        let f = Fixture::new(&[(".github/workflows/CI.YML", PR_WORKFLOW)], Track::All).unwrap();
        let d = check(&f);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].fixable, Fixable::Agent);
        assert!(d[0].message.contains("Add a workflow"));
        assert!(fix(&f).changed_files.is_empty());
        assert_eq!(files_on_disk(&f), [".github/workflows/CI.YML"]);
    }
}

mod fixed_repo {
    use super::*;

    #[test]
    fn an_empty_repo_gets_the_starter_workflow_and_it_is_tracked() {
        let f = Fixture::new(&[], Track::All).unwrap();
        let result = fix(&f);
        assert_eq!(result.changed_files, [STARTER]);
        assert_eq!(
            result.notes,
            ["Review .github/workflows/ci.yml: it runs `npx firescale verify`. Add anything your project needs before that step (services, secrets, other toolchains)."]
        );
        assert_eq!(f.read(STARTER).unwrap(), format!("{HEAD}{TAIL}"));
        assert_eq!(f.tracked().unwrap(), [STARTER]);
        assert!(check(&f).is_empty());
    }

    #[test]
    fn npm_with_a_lockfile_installs_with_npm_ci() {
        let f = Fixture::new(
            &[
                ("package.json", "{\"name\":\"demo\"}\n"),
                ("package-lock.json", "{}\n"),
            ],
            Track::All,
        )
        .unwrap();
        assert_eq!(fix(&f).notes.len(), 1);
        assert_eq!(
            f.read(STARTER).unwrap(),
            format!("{HEAD}      - run: npm ci\n{TAIL}")
        );
        assert!(check(&f).is_empty());
    }

    #[test]
    fn npm_without_a_lockfile_installs_with_npm_install() {
        let f = Fixture::new(
            &[(
                "package.json",
                "{\"name\":\"demo\",\"packageManager\":\"npm@11.0.0\"}\n",
            )],
            Track::All,
        )
        .unwrap();
        fix(&f);
        assert_eq!(
            f.read(STARTER).unwrap(),
            format!("{HEAD}      - run: npm install\n{TAIL}")
        );
    }

    #[test]
    fn pnpm_installs_pnpm_and_then_the_frozen_lockfile() {
        let f = Fixture::new(
            &[
                ("package.json", "{\"name\":\"demo\"}\n"),
                ("pnpm-lock.yaml", "lockfileVersion: '9.0'\n"),
            ],
            Track::All,
        )
        .unwrap();
        fix(&f);
        assert_eq!(
            f.read(STARTER).unwrap(),
            format!("{HEAD}      - run: npm install --global pnpm\n      - run: pnpm install --frozen-lockfile\n{TAIL}")
        );
        assert!(check(&f).is_empty());
    }

    #[test]
    fn another_package_manager_gets_a_placeholder_and_a_note() {
        for (lock, manager) in [("yarn.lock", "yarn"), ("bun.lock", "bun")] {
            let f = Fixture::new(
                &[("package.json", "{\"name\":\"demo\"}\n"), (lock, "")],
                Track::All,
            )
            .unwrap();
            let result = fix(&f);
            assert_eq!(result.notes.len(), 2, "{manager}");
            assert!(result.notes[1].contains("no install step"), "{manager}");
            assert_eq!(
                f.read(STARTER).unwrap(),
                format!("{HEAD}      # Add the step that installs your dependencies here.\n{TAIL}"),
                "{manager}"
            );
            assert!(check(&f).is_empty(), "{manager}");
        }
    }

    #[test]
    fn the_file_has_lf_line_endings_only() {
        let f = Fixture::new(&[("package.json", "{\"name\":\"demo\"}\r\n")], Track::All).unwrap();
        fix(&f);
        assert!(!f.read(STARTER).unwrap().contains('\r'));
    }

    #[test]
    fn fix_twice_changes_nothing_the_second_time() {
        let f = Fixture::new(&[("package.json", "{\"name\":\"demo\"}\n")], Track::All).unwrap();
        let first = fix_rule_with(&CiExists, f.root(), &Options::new(), false).unwrap();
        assert_eq!(first.changes.len(), 2);
        assert!(
            matches!(&first.changes[0], Change::Write { file, before: None, .. } if file == STARTER)
        );
        assert_eq!(
            first.changes[1],
            Change::Track {
                file: STARTER.to_owned()
            }
        );
        let after_first = bytes(&f, STARTER);
        let second = fix_rule_with(&CiExists, f.root(), &Options::new(), false).unwrap();
        assert!(second.changes.is_empty());
        assert!(second.result.changed_files.is_empty());
        assert_eq!(bytes(&f, STARTER), after_first);
    }

    #[test]
    fn dry_run_reports_the_changes_and_touches_nothing() {
        let f = Fixture::new(&[("package.json", "{\"name\":\"demo\"}\n")], Track::All).unwrap();
        let outcome = fix_rule_with(&CiExists, f.root(), &Options::new(), true).unwrap();
        assert_eq!(outcome.changes.len(), 2);
        assert_eq!(files_on_disk(&f), ["package.json"]);
        assert_eq!(f.tracked().unwrap(), ["package.json"]);
    }

    #[test]
    fn outside_git_the_fix_creates_the_file_and_the_check_passes() {
        let f = Fixture::new(&[("package.json", "{\"name\":\"demo\"}\n")], Track::NoGit).unwrap();
        assert_eq!(check(&f)[0].fixable, Fixable::Deterministic);
        let outcome = fix_rule_with(&CiExists, f.root(), &Options::new(), false).unwrap();
        assert_eq!(outcome.result.changed_files, [STARTER]);
        assert_eq!(outcome.changes.len(), 1, "nothing to stage outside git");
        assert!(check(&f).is_empty());
        let again = fix_rule_with(&CiExists, f.root(), &Options::new(), false).unwrap();
        assert!(again.changes.is_empty());
    }
}

mod never_overwrites {
    use super::*;

    /// Runs the fix every way it can be reached and checks nothing moved.
    fn assert_untouched(f: &Fixture, files: &[&str]) {
        let before: Vec<Vec<u8>> = files.iter().map(|file| bytes(f, file)).collect();
        let listing = files_on_disk(f);
        let tracked = f.tracked().unwrap();
        let outcome = fix_rule_with(&CiExists, f.root(), &Options::new(), false).unwrap();
        assert!(outcome.changes.is_empty());
        assert!(outcome.result.changed_files.is_empty());
        // Even when called directly, without a deterministic finding.
        let ctx = firescale_core::rule::RuleContext::for_fix(f.root(), Options::new(), false);
        assert!(CiExists.fix(&ctx, &[]).unwrap().changed_files.is_empty());
        assert!(ctx.changes().is_empty());
        let after: Vec<Vec<u8>> = files.iter().map(|file| bytes(f, file)).collect();
        assert_eq!(after, before);
        assert_eq!(files_on_disk(f), listing);
        assert_eq!(f.tracked().unwrap(), tracked);
    }

    #[test]
    fn an_untracked_ci_yml_is_byte_for_byte_unchanged() {
        let f = Fixture::new(&[(STARTER, ODD)], Track::None).unwrap();
        assert_untouched(&f, &[STARTER]);
        assert_eq!(bytes(&f, STARTER), ODD.as_bytes());
    }

    #[test]
    fn an_untracked_workflow_with_another_name_blocks_the_starter() {
        let f = Fixture::new(&[(".github/workflows/build.yaml", ODD)], Track::None).unwrap();
        assert_untouched(&f, &[".github/workflows/build.yaml"]);
        assert!(f.read(STARTER).is_none());
    }

    #[test]
    fn a_ci_yml_that_is_not_utf8_is_unchanged() {
        let f = Fixture::new(&[(STARTER, "x")], Track::None).unwrap();
        let raw = [0xff, 0xfe, b'o', b'n', b':', 0x00, b'\r', b'\n'];
        std::fs::write(
            f.root().join(".github").join("workflows").join("ci.yml"),
            raw,
        )
        .unwrap();
        assert_untouched(&f, &[STARTER]);
        assert_eq!(bytes(&f, STARTER), raw);
    }

    #[test]
    fn a_repo_with_only_gitlab_ci_gets_no_workflow() {
        let f = Fixture::new(&[(".gitlab-ci.yml", "test:\r\n  script: make")], Track::All).unwrap();
        assert_untouched(&f, &[".gitlab-ci.yml"]);
        assert_eq!(files_on_disk(&f), [".gitlab-ci.yml"]);
    }

    #[test]
    fn a_hand_written_push_only_workflow_is_untouched() {
        for track in [Track::All, Track::None, Track::NoGit] {
            let f = Fixture::new(&[(".github/workflows/release.yml", PUSH_ONLY)], track).unwrap();
            assert_untouched(&f, &[".github/workflows/release.yml"]);
            assert_eq!(files_on_disk(&f), [".github/workflows/release.yml"]);
        }
        let f = Fixture::new(&[(".github/workflows/release.yml", PUSH_ONLY)], Track::All).unwrap();
        assert_eq!(check(&f)[0].fixable, Fixable::Agent);
    }

    #[test]
    fn a_push_only_ci_yml_is_untouched() {
        let f = Fixture::new(&[(STARTER, PUSH_ONLY)], Track::All).unwrap();
        assert_untouched(&f, &[STARTER]);
        assert_eq!(f.read(STARTER).unwrap(), PUSH_ONLY);
    }
}
