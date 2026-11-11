//! Tests for `core/readme-setup`.

use firescale_core::model::{Category, Fixable, Severity};
use firescale_core::rule::{fix_rule_with, BuiltinRule, Change, Options, RuleContext};
use firescale_core::testing::{Fixture, Track};

use super::{ReadmeSetup, ID};

const GOOD: &str =
    "# demo\n\n## Install\n\n```sh\npnpm install\n```\n\n## Usage\n\n```sh\npnpm dev\n```\n";

fn check(files: &[(&str, &str)]) -> Vec<firescale_core::model::Diagnostic> {
    let f = Fixture::new(files, Track::All).unwrap();
    f.check(&ReadmeSetup, &Options::new()).unwrap()
}

mod passing_repo {
    use super::*;

    #[test]
    fn readme_with_install_and_usage_sections_passes() {
        assert!(check(&[("README.md", GOOD)]).is_empty());
    }

    #[test]
    fn commands_alone_satisfy_both_parts() {
        let text = "# demo\n\nRun `npm ci` first, then:\n\n    npm start\n";
        assert!(check(&[("README.md", text)]).is_empty());
    }

    #[test]
    fn getting_started_heading_satisfies_both_parts() {
        assert!(check(&[("readme.md", "# demo\n\n## Getting started\n\nHello.\n")]).is_empty());
    }

    #[test]
    fn rst_underlined_headings_count() {
        let text = "demo\n====\n\nInstallation\n------------\n\nx\n\nUsage\n-----\n\nx\n";
        assert!(check(&[("README.rst", text)]).is_empty());
    }

    #[test]
    fn whole_word_headings_satisfy_the_steps() {
        for text in [
            "# demo\n\n## Running the app\n\n## Installation notes\n",
            "# demo\n\n## Getting Started\n",
            "demo\n====\n\nQuick start\n-----------\n\nx\n",
            "# demo\n\n## Set up\n\n## Examples\n",
        ] {
            assert!(check(&[("README.md", text)]).is_empty(), "{text}");
        }
    }

    #[test]
    fn works_outside_a_git_repo() {
        let f = Fixture::new(&[("README.md", GOOD)], Track::NoGit).unwrap();
        assert!(f.check(&ReadmeSetup, &Options::new()).unwrap().is_empty());
    }
}

mod failing_repo {
    use super::*;

    #[test]
    fn missing_readme_is_reported_on_readme_md() {
        let d = check(&[("package.json", "{}\n")]);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].rule_id, ID);
        assert_eq!(d[0].file, "README.md");
        assert_eq!(d[0].fixable, Fixable::Deterministic);
        assert!(d[0].message.contains("Add a README.md"));
        assert!(d[0].message.contains("`fscale fix`"));
    }

    #[test]
    fn readme_without_either_part_gets_two_findings() {
        let d = check(&[(
            "README.md",
            "# demo\n\nA small tool. Make sure you like it.\n",
        )]);
        assert_eq!(d.len(), 2);
        assert!(d.iter().all(|x| x.file == "README.md"));
        assert!(d[0].message.contains("install") || d[1].message.contains("install"));
        assert!(d.iter().any(|x| x.message.contains("run the project")));
    }

    #[test]
    fn substring_headings_do_not_count() {
        for heading in ["Truncation", "Restart policy", "Reinstallation"] {
            let text = format!("# demo\n\n## {heading}\n\nx\n");
            let d = check(&[("README.md", text.as_str())]);
            assert_eq!(d.len(), 2, "{heading}");
        }
    }

    #[test]
    fn install_only_readme_is_missing_run() {
        let d = check(&[("README.md", "# demo\n\n## Install\n\nnpm install\n")]);
        assert_eq!(d.len(), 1);
        assert!(d[0].message.contains("run the project"));
    }

    #[test]
    fn finding_uses_the_actual_readme_name() {
        let d = check(&[("Readme.txt", "hello\n")]);
        assert_eq!(d.len(), 2);
        assert!(d.iter().all(|x| x.file == "Readme.txt"));
    }

    #[test]
    fn a_readme_in_a_subfolder_does_not_count() {
        let d = check(&[("docs/README.md", GOOD)]);
        assert_eq!(d[0].file, "README.md");
    }
}

mod metadata {
    use super::*;

    #[test]
    fn meta_matches_the_spec() {
        let meta = ReadmeSetup.meta();
        assert_eq!(meta.id, ID);
        assert_eq!(meta.severity, Severity::Low);
        assert_eq!(meta.category, Category::Maintainability);
        assert!(meta.has_fix);
        assert_eq!(meta.inputs, vec!["[Rr][Ee][Aa][Dd][Mm][Ee]*"]);
    }

    #[test]
    fn the_inputs_glob_matches_every_letter_case_at_the_root_only() {
        let matcher = firescale_core::cache::input_matcher(ID, &ReadmeSetup.meta().inputs).unwrap();
        for name in [
            "README.md",
            "readme",
            "ReadMe.MD",
            "README.adoc",
            "Readme.txt",
        ] {
            assert!(matcher.is_match(name), "{name}");
        }
        for name in ["docs/README.md", "NOTREADME.md", "read.me"] {
            assert!(!matcher.is_match(name), "{name}");
        }
    }
}

#[test]
fn a_readme_that_is_not_utf8_does_not_abort_the_run() {
    let f = Fixture::new(&[("README.md", "x")], Track::All).unwrap();
    let mut bytes = GOOD.as_bytes().to_vec();
    bytes.extend_from_slice(&[0xff, 0xfe, 0xfd]);
    std::fs::write(f.root().join("README.md"), bytes).unwrap();
    assert!(f.check(&ReadmeSetup, &Options::new()).unwrap().is_empty());
}

const NOTE: &str =
    "README.md is a skeleton. Describe the project and complete the Install and Usage sections.";
const COMMENT: &str =
    "<!-- A skeleton created by firescale (core/readme-setup). It is your team's to edit. -->";

/// CRLF, no trailing newline, non-ASCII: nothing a fix would ever write.
const ODD: &str = "# \u{e9}t\u{e9} \u{2603}\r\n\r\nNothing about setting things going.\r\nfin";

fn run_check(fixture: &Fixture) -> Vec<firescale_core::model::Diagnostic> {
    fixture.check(&ReadmeSetup, &Options::new()).unwrap()
}

fn bytes(fixture: &Fixture, file: &str) -> Vec<u8> {
    std::fs::read(fixture.root().join(file)).unwrap()
}

/// Names at the fixture root except `.git`, sorted.
fn root_names(fixture: &Fixture) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(fixture.root())
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name != ".git")
        .collect();
    names.sort();
    names
}

mod untracked_or_unusual_readme {
    use super::*;

    #[test]
    fn an_untracked_readme_is_manual_and_uses_its_real_name() {
        for name in ["README.md", "ReadMe.MD", "readme.rst", "README"] {
            let f = Fixture::new(&[(name, GOOD)], Track::None).unwrap();
            let d = run_check(&f);
            assert_eq!(d.len(), 1, "{name}");
            assert_eq!(d[0].file, name);
            assert_eq!(d[0].fixable, Fixable::Manual, "{name}");
            assert!(d[0].message.contains(&format!("git add {name}")), "{name}");
        }
    }

    #[test]
    fn an_unrecognised_readme_name_is_left_to_an_agent() {
        for track in [Track::All, Track::None, Track::NoGit] {
            for name in ["README.adoc", "readme.markdown", "README/index.md"] {
                let f = Fixture::new(&[(name, GOOD)], track).unwrap();
                let d = run_check(&f);
                assert_eq!(d.len(), 1, "{name}");
                assert_eq!(d[0].file, "README.md");
                assert_eq!(d[0].fixable, Fixable::Agent, "{name}");
                assert_eq!(
                    d[0].message,
                    "No README found at the repo root. Add a README.md that explains how to install and run the project."
                );
            }
        }
    }

    #[test]
    fn a_tracked_readme_missing_from_disk_is_manual_and_its_staged_content_survives() {
        let f = Fixture::new(&[("README.md", GOOD), ("package.json", "{}\n")], Track::All).unwrap();
        // The index holds the only copy of this text.
        let staged = git_out(&f, &["ls-files", "--stage", "--", "README.md"]);
        std::fs::remove_file(f.root().join("README.md")).unwrap();

        let d = run_check(&f);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].file, "README.md");
        assert_eq!(d[0].fixable, Fixable::Manual);
        assert!(
            d[0].message.contains("git restore README.md"),
            "{}",
            d[0].message
        );

        let outcome = fix_rule_with(&ReadmeSetup, f.root(), &Options::new(), false).unwrap();
        assert!(outcome.changes.is_empty());
        let ctx = RuleContext::for_fix(f.root(), Options::new(), false);
        assert!(ReadmeSetup.fix(&ctx, &[]).unwrap().changed_files.is_empty());
        assert!(!f.root().join("README.md").exists());
        assert_eq!(
            git_out(&f, &["ls-files", "--stage", "--", "README.md"]),
            staged
        );

        // What the message says to do brings the person's text back.
        git_out(&f, &["restore", "README.md"]);
        assert_eq!(f.read("README.md").as_deref(), Some(GOOD));
        assert!(run_check(&f).is_empty());
    }

    #[test]
    fn a_readme_that_gitignore_ignores_is_manual_and_nothing_is_created() {
        let f = Fixture::new(&[(".gitignore", "README.md\n")], Track::All).unwrap();
        let d = run_check(&f);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].fixable, Fixable::Manual);
        assert!(d[0].message.contains(".gitignore rule ignores README.md"));
        let outcome = fix_rule_with(&ReadmeSetup, f.root(), &Options::new(), false).unwrap();
        assert!(outcome.changes.is_empty());
        assert!(!f.root().join("README.md").exists());
    }
}

fn git_out(fixture: &Fixture, args: &[&str]) -> String {
    // Like the fixtures: independent of the machine's line-ending settings.
    let mut full = vec!["-c", "core.autocrlf=false", "-c", "core.safecrlf=false"];
    full.extend_from_slice(args);
    let result = firescale_core::git::run_git(fixture.root(), &full).unwrap();
    assert_eq!(result.code, 0, "git {args:?}: {}", result.stderr);
    result.stdout
}

mod fixed_repo {
    use super::*;

    #[test]
    fn an_empty_repo_gets_the_skeleton_and_it_is_tracked() {
        let f = Fixture::new(&[], Track::All).unwrap();
        let result = f.fix(&ReadmeSetup, &Options::new()).unwrap();
        assert_eq!(result.changed_files, ["README.md"]);
        assert_eq!(result.notes, [NOTE]);
        assert_eq!(
            f.read("README.md").unwrap(),
            format!("# Project name\n\n{COMMENT}\n\n## Install\n\nDescribe how to install the project here.\n\n## Usage\n\nDescribe how to run the project here.\n")
        );
        assert_eq!(f.tracked().unwrap(), ["README.md"]);
        assert!(run_check(&f).is_empty());
    }

    #[test]
    fn a_pnpm_project_gets_its_name_and_commands() {
        let f = Fixture::new(
            &[
                (
                    "package.json",
                    "{\"name\":\"@acme/demo\",\"scripts\":{\"build\":\"tsc\",\"test\":\"vitest run\",\"lint\":\"eslint .\"}}\n",
                ),
                ("pnpm-lock.yaml", "lockfileVersion: '9.0'\n"),
            ],
            Track::All,
        )
        .unwrap();
        f.fix(&ReadmeSetup, &Options::new()).unwrap();
        assert_eq!(
            f.read("README.md").unwrap(),
            format!("# @acme/demo\n\n{COMMENT}\n\n## Install\n\n```sh\npnpm install\n```\n\n## Usage\n\n```sh\npnpm run build\npnpm run test\n```\n")
        );
        assert!(run_check(&f).is_empty());
    }

    #[test]
    fn each_package_manager_gets_its_install_command() {
        for (lock, command) in [
            ("package-lock.json", "npm install"),
            ("yarn.lock", "yarn install"),
            ("bun.lock", "bun install"),
        ] {
            let f = Fixture::new(
                &[
                    (
                        "package.json",
                        "{\"name\":\"demo\",\"scripts\":{\"test\":\"node t.js\"}}\n",
                    ),
                    (lock, "{}\n"),
                ],
                Track::All,
            )
            .unwrap();
            f.fix(&ReadmeSetup, &Options::new()).unwrap();
            let manager = command.split(' ').next().unwrap();
            assert_eq!(
                f.read("README.md").unwrap(),
                format!("# demo\n\n{COMMENT}\n\n## Install\n\n```sh\n{command}\n```\n\n## Usage\n\n```sh\n{manager} run test\n```\n")
            );
            assert!(run_check(&f).is_empty(), "{command}");
        }
    }

    #[test]
    fn a_rust_project_installs_with_cargo_build() {
        let f = Fixture::new(
            &[(
                "Cargo.toml",
                "[package]\nname = \"rusty\"\nversion = \"0.1.0\"\n",
            )],
            Track::All,
        )
        .unwrap();
        f.fix(&ReadmeSetup, &Options::new()).unwrap();
        assert_eq!(
            f.read("README.md").unwrap(),
            format!("# rusty\n\n{COMMENT}\n\n## Install\n\n```sh\ncargo build\n```\n\n## Usage\n\nDescribe how to run the project here.\n")
        );
        assert!(run_check(&f).is_empty());
    }

    #[test]
    fn the_heading_never_comes_from_the_folder_name() {
        // A dry run works in a scratch copy: two folders, one file.
        let files = [("package.json", "{\"scripts\":{\"build\":\"tsc\"}}\n")];
        let a = Fixture::new(&files, Track::All).unwrap();
        let b = Fixture::new(&files, Track::NoGit).unwrap();
        a.fix(&ReadmeSetup, &Options::new()).unwrap();
        b.fix(&ReadmeSetup, &Options::new()).unwrap();
        let text = a.read("README.md").unwrap();
        assert!(text.starts_with("# Project name\n"));
        assert_eq!(b.read("README.md").unwrap(), text);
        assert!(!text.contains('\r'));
    }

    #[test]
    fn fix_twice_changes_nothing_the_second_time() {
        let f = Fixture::new(&[("package.json", "{\"name\":\"demo\"}\n")], Track::All).unwrap();
        let first = fix_rule_with(&ReadmeSetup, f.root(), &Options::new(), false).unwrap();
        assert_eq!(first.changes.len(), 2);
        assert!(
            matches!(&first.changes[0], Change::Write { file, before: None, .. } if file == "README.md")
        );
        assert_eq!(
            first.changes[1],
            Change::Track {
                file: "README.md".to_owned()
            }
        );
        let after_first = bytes(&f, "README.md");
        let second = fix_rule_with(&ReadmeSetup, f.root(), &Options::new(), false).unwrap();
        assert!(second.changes.is_empty());
        assert!(second.result.changed_files.is_empty());
        assert_eq!(bytes(&f, "README.md"), after_first);
    }

    #[test]
    fn dry_run_reports_the_changes_and_touches_nothing() {
        let f = Fixture::new(&[("package.json", "{\"name\":\"demo\"}\n")], Track::All).unwrap();
        let outcome = fix_rule_with(&ReadmeSetup, f.root(), &Options::new(), true).unwrap();
        assert_eq!(outcome.changes.len(), 2);
        assert_eq!(root_names(&f), ["package.json"]);
        assert_eq!(f.tracked().unwrap(), ["package.json"]);
    }

    #[test]
    fn outside_git_the_fix_creates_the_file_and_the_check_passes() {
        let f = Fixture::new(&[("package.json", "{\"name\":\"demo\"}\n")], Track::NoGit).unwrap();
        assert_eq!(run_check(&f)[0].fixable, Fixable::Deterministic);
        let outcome = fix_rule_with(&ReadmeSetup, f.root(), &Options::new(), false).unwrap();
        assert_eq!(outcome.result.changed_files, ["README.md"]);
        assert_eq!(outcome.changes.len(), 1, "nothing to stage outside git");
        assert!(run_check(&f).is_empty());
        let again = fix_rule_with(&ReadmeSetup, f.root(), &Options::new(), false).unwrap();
        assert!(again.changes.is_empty());
    }
}

mod never_overwrites {
    use super::*;

    /// Runs the fix every way it can be reached and checks nothing moved.
    fn assert_untouched(f: &Fixture, file: &str, expected: &[u8]) {
        let names = root_names(f);
        let tracked = f.tracked().unwrap();
        let outcome = fix_rule_with(&ReadmeSetup, f.root(), &Options::new(), false).unwrap();
        assert!(outcome.changes.is_empty(), "{file}");
        assert!(outcome.result.changed_files.is_empty(), "{file}");
        // Even when called directly, without a deterministic finding.
        let ctx = RuleContext::for_fix(f.root(), Options::new(), false);
        assert!(ReadmeSetup.fix(&ctx, &[]).unwrap().changed_files.is_empty());
        assert!(ctx.changes().is_empty(), "{file}");
        assert_eq!(bytes(f, file), expected, "{file}");
        assert_eq!(root_names(f), names, "{file}");
        assert_eq!(f.tracked().unwrap(), tracked, "{file}");
    }

    #[test]
    fn an_existing_readme_of_any_name_is_byte_for_byte_unchanged() {
        for track in [Track::None, Track::All, Track::NoGit] {
            for name in [
                "README.md",
                "ReadMe.MD",
                "README.adoc",
                "readme.markdown",
                "README",
            ] {
                let f = Fixture::new(&[(name, ODD)], track).unwrap();
                assert_untouched(&f, name, ODD.as_bytes());
                assert_eq!(root_names(&f), [name], "no second README appears");
            }
        }
    }

    #[test]
    fn a_readme_that_is_not_utf8_is_unchanged() {
        let raw = [0xff, 0xfe, b'#', 0x00, b'\r', b'\n', 0xe9];
        for name in ["README.md", "README.adoc"] {
            let f = Fixture::new(&[(name, "x")], Track::None).unwrap();
            std::fs::write(f.root().join(name), raw).unwrap();
            assert_untouched(&f, name, &raw);
        }
    }

    #[test]
    fn a_readme_folder_blocks_the_skeleton() {
        let f = Fixture::new(&[("README/index.md", ODD)], Track::All).unwrap();
        assert_untouched(&f, "README/index.md", ODD.as_bytes());
        assert_eq!(root_names(&f), ["README"]);
    }

    #[test]
    fn a_tracked_readme_without_steps_is_never_edited() {
        let f = Fixture::new(&[("README.md", ODD)], Track::All).unwrap();
        let d = run_check(&f);
        assert_eq!(d.len(), 2);
        assert!(d.iter().all(|x| x.fixable == Fixable::Agent));
        assert_untouched(&f, "README.md", ODD.as_bytes());
    }
}
