//! Tests for `core/no-committed-env`, ported from the TypeScript rule's test file
//! and extended with letter-case variants.

use firescale_core::git::run_git;
use firescale_core::model::Fixable;
use firescale_core::rule::{BuiltinRule, Options, RuleContext};
use firescale_core::testing::{Fixture, Track};
use firescale_core::Error;
use serde_json::json;

use super::{NoCommittedEnv, ID};

const EXAMPLE: &str = "EXAMPLE=1\n";

fn options(value: serde_json::Value) -> Options {
    value.as_object().cloned().unwrap_or_default()
}

fn repo(files: &[(&str, &str)]) -> Fixture {
    Fixture::new(files, Track::All).unwrap()
}

fn check_files(fixture: &Fixture, options: &Options) -> Vec<String> {
    fixture
        .check(&NoCommittedEnv, options)
        .unwrap()
        .into_iter()
        .map(|d| d.file)
        .collect()
}

fn lines(fixture: &Fixture, file: &str) -> Vec<String> {
    fixture
        .read(file)
        .unwrap_or_default()
        .split('\n')
        .map(str::to_owned)
        .collect()
}

fn has(lines: &[String], wanted: &str) -> bool {
    lines.iter().any(|line| line == wanted)
}

fn user_message(error: &Error) -> &str {
    error.message()
}

mod passing_repo {
    use super::*;

    #[test]
    fn reports_nothing_when_only_env_example_is_tracked() {
        let f = repo(&[
            (".env.example", EXAMPLE),
            ("README.md", "# demo\n"),
            ("apps/web/.env.example", EXAMPLE),
        ]);
        assert!(check_files(&f, &Options::new()).is_empty());
    }

    #[test]
    fn reports_nothing_for_an_env_file_on_disk_that_is_not_tracked() {
        let files = [
            (".env", EXAMPLE),
            (".gitignore", ".env\n"),
            ("README.md", "# demo\n"),
        ];
        let f = Fixture::new(&files, Track::Only(&[".gitignore", "README.md"])).unwrap();
        assert!(check_files(&f, &Options::new()).is_empty());
    }

    #[test]
    fn does_not_flag_envrc_or_similarly_named_files() {
        let f = repo(&[
            (".envrc", "EXAMPLE=1\n"),
            (".environment", "x\n"),
            ("env.txt", "x\n"),
        ]);
        assert!(check_files(&f, &Options::new()).is_empty());
    }

    #[test]
    fn reports_nothing_outside_a_git_repo() {
        let f = Fixture::new(&[(".env", EXAMPLE)], Track::NoGit).unwrap();
        assert!(check_files(&f, &Options::new()).is_empty());
    }

    #[test]
    fn honours_the_allow_option() {
        let f = repo(&[(".env.template", EXAMPLE), (".env.example", EXAMPLE)]);
        let allow = options(json!({ "allow": [".env.example", ".env.template"] }));
        assert!(check_files(&f, &allow).is_empty());
        // With the default allow list only .env.example is exempt.
        assert_eq!(check_files(&f, &Options::new()), vec![".env.template"]);
    }

    #[test]
    fn rejects_an_invalid_allow_option() {
        let f = repo(&[(".env", EXAMPLE)]);
        for bad in [
            json!({ "allow": "nope" }),
            json!({ "allow": [1] }),
            json!({ "allow": ["package.json"] }),
        ] {
            let error = f.check(&NoCommittedEnv, &options(bad)).unwrap_err();
            assert!(matches!(error, Error::User(_)), "got {error:?}");
        }
    }

    #[test]
    fn case_variants_of_example_and_envrc_are_not_flagged() {
        let f = repo(&[(".ENV.EXAMPLE", EXAMPLE), (".envrc", EXAMPLE)]);
        assert!(check_files(&f, &Options::new()).is_empty());
    }
}

mod failing_repo {
    use super::*;

    #[test]
    fn reports_tracked_env_files_sorted_with_forward_slash_paths() {
        let f = repo(&[
            ("apps/web/.env.local", EXAMPLE),
            (".env", EXAMPLE),
            (".env.example", EXAMPLE),
            ("README.md", "# demo\n"),
        ]);
        let diagnostics = f.check(&NoCommittedEnv, &Options::new()).unwrap();
        let files: Vec<&str> = diagnostics.iter().map(|d| d.file.as_str()).collect();
        assert_eq!(files, vec![".env", "apps/web/.env.local"]);
        for d in &diagnostics {
            assert_eq!(d.rule_id, ID);
            assert_eq!(d.fixable, Fixable::Deterministic);
            assert!(!d.file.contains('\\'));
            assert!(d.message.contains(&d.file));
        }
    }

    #[test]
    fn reports_a_tracked_env_file_even_when_gitignore_lists_it() {
        let f = repo(&[(".env", EXAMPLE), (".gitignore", ".env\n")]);
        assert_eq!(check_files(&f, &Options::new()), vec![".env"]);
    }

    #[test]
    fn flags_upper_and_mixed_case_env_files() {
        // One fixture per name: case-insensitive file systems cannot hold two
        // names that differ only by case in one folder.
        for name in [".ENV", ".Env.Local", "sub/.ENV.PRODUCTION"] {
            let f = repo(&[(name, EXAMPLE)]);
            assert_eq!(check_files(&f, &Options::new()), vec![name], "{name}");
        }
    }
}

mod fixed_repo {
    use super::*;

    fn failing(gitignore: Option<&str>) -> Fixture {
        let mut files = vec![
            (".env", "EXAMPLE=1\n"),
            ("apps/web/.env.local", "EXAMPLE=2\n"),
            (".env.example", EXAMPLE),
        ];
        if let Some(text) = gitignore {
            files.push((".gitignore", text));
        }
        repo(&files)
    }

    #[test]
    fn untracks_the_files_keeps_them_on_disk_and_ignores_them() {
        let f = failing(None);
        let result = f.fix(&NoCommittedEnv, &Options::new()).unwrap();
        assert_eq!(
            result.changed_files,
            vec![".env", ".gitignore", "apps/web/.env.local"]
        );
        assert!(check_files(&f, &Options::new()).is_empty());
        assert_eq!(f.read(".env").as_deref(), Some("EXAMPLE=1\n"));
        assert_eq!(
            f.read("apps/web/.env.local").as_deref(),
            Some("EXAMPLE=2\n")
        );

        let tracked = f.tracked().unwrap();
        assert!(!tracked.contains(&".env".to_owned()));
        assert!(!tracked.contains(&"apps/web/.env.local".to_owned()));
        assert!(tracked.contains(&".env.example".to_owned()));

        let ls = lines(&f, ".gitignore");
        for wanted in [".env", ".env.*", "!.env.example"] {
            assert!(has(&ls, wanted), "missing {wanted} in {ls:?}");
        }
    }

    #[test]
    fn uses_the_allow_option_for_the_negation_lines() {
        let f = failing(None);
        let allow = options(json!({ "allow": [".env.example", ".env.template"] }));
        f.fix(&NoCommittedEnv, &allow).unwrap();
        let ls = lines(&f, ".gitignore");
        assert!(has(&ls, "!.env.example"));
        assert!(has(&ls, "!.env.template"));
    }

    #[test]
    fn preserves_existing_gitignore_content_and_only_adds_missing_lines() {
        let f = failing(Some("node_modules\n.env\n"));
        f.fix(&NoCommittedEnv, &Options::new()).unwrap();
        let text = f.read(".gitignore").unwrap();
        assert!(text.starts_with("node_modules\n.env\n"));
        let ls = lines(&f, ".gitignore");
        assert_eq!(ls.iter().filter(|l| *l == ".env").count(), 1);
        assert!(has(&ls, ".env.*"));
        assert!(has(&ls, "!.env.example"));
    }

    #[test]
    fn handles_a_gitignore_without_a_trailing_newline() {
        let f = failing(Some("node_modules"));
        f.fix(&NoCommittedEnv, &Options::new()).unwrap();
        let text = f.read(".gitignore").unwrap();
        assert!(text.starts_with("node_modules\n"));
        assert!(has(&lines(&f, ".gitignore"), ".env.*"));
    }

    #[test]
    fn keeps_crlf_line_endings_in_an_existing_crlf_gitignore() {
        let f = failing(Some("node_modules\r\ndist\r\n"));
        f.fix(&NoCommittedEnv, &Options::new()).unwrap();
        let text = f.read(".gitignore").unwrap();
        assert!(text.starts_with("node_modules\r\ndist\r\n"));
        assert!(!text.replace("\r\n", "").contains('\n'));
        let parts: Vec<&str> = text.split("\r\n").collect();
        for wanted in [".env", ".env.*", "!.env.example"] {
            assert!(parts.contains(&wanted), "missing {wanted} in {parts:?}");
        }
    }

    #[test]
    fn notes_that_history_still_contains_the_secrets() {
        let f = failing(None);
        let result = f.fix(&NoCommittedEnv, &Options::new()).unwrap();
        assert!(result.notes.join(" ").contains("rotate"));
    }

    #[test]
    fn upper_case_env_is_untracked_kept_and_ignored_by_git() {
        let f = repo(&[(".ENV", EXAMPLE), ("README.md", "# demo\n")]);
        f.fix(&NoCommittedEnv, &Options::new()).unwrap();
        assert!(check_files(&f, &Options::new()).is_empty());
        assert_eq!(f.read(".ENV").as_deref(), Some(EXAMPLE));
        let ignored = run_git(f.root(), &["check-ignore", "--no-index", "-q", ".ENV"]).unwrap();
        assert_eq!(ignored.code, 0, "{}", ignored.stderr);
    }
}

mod fix_run_twice {
    use super::*;

    #[test]
    fn the_second_run_changes_nothing() {
        let f = repo(&[
            (".env", EXAMPLE),
            ("apps/web/.env.local", EXAMPLE),
            (".gitignore", "dist\r\n"),
        ]);
        let first = f.fix(&NoCommittedEnv, &Options::new()).unwrap();
        assert!(!first.changed_files.is_empty());
        let gitignore_after_first = f.read(".gitignore");
        let tracked_after_first = f.tracked().unwrap();

        let second = f.fix(&NoCommittedEnv, &Options::new()).unwrap();
        assert!(second.changed_files.is_empty());
        assert_eq!(f.read(".gitignore"), gitignore_after_first);
        assert_eq!(f.tracked().unwrap(), tracked_after_first);
        assert!(check_files(&f, &Options::new()).is_empty());
    }

    #[test]
    fn does_nothing_on_a_repo_that_is_already_clean() {
        let f = repo(&[(".env.example", EXAMPLE)]);
        let result = f.fix(&NoCommittedEnv, &Options::new()).unwrap();
        assert!(result.changed_files.is_empty());
        assert!(result.notes.is_empty());
        assert_eq!(f.read(".gitignore"), None);
    }

    #[test]
    fn is_idempotent_for_upper_case_env_files() {
        let f = repo(&[(".ENV", EXAMPLE), ("sub/.Env.Local", EXAMPLE)]);
        let first = f.fix(&NoCommittedEnv, &Options::new()).unwrap();
        assert!(!first.changed_files.is_empty());
        let gitignore_after_first = f.read(".gitignore");
        let tracked_after_first = f.tracked().unwrap();

        let second = f.fix(&NoCommittedEnv, &Options::new()).unwrap();
        assert!(second.changed_files.is_empty());
        assert_eq!(f.read(".gitignore"), gitignore_after_first);
        assert_eq!(f.tracked().unwrap(), tracked_after_first);
        assert!(check_files(&f, &Options::new()).is_empty());
    }
}

mod review_regressions {
    use super::*;

    #[test]
    fn keeps_env_example_unignored_when_its_negation_sits_before_the_ignore_lines() {
        let f = repo(&[
            (".env", EXAMPLE),
            (".env.example", EXAMPLE),
            (".gitignore", "!.env.example\n.env\n"),
        ]);
        f.fix(&NoCommittedEnv, &Options::new()).unwrap();
        let ls = lines(&f, ".gitignore");
        let last = |wanted: &str| ls.iter().rposition(|l| l == wanted);
        let negation = last("!.env.example").unwrap();
        assert!(negation > last(".env.*").unwrap());
        assert!(negation > last(".env").unwrap());

        // git itself must agree: the example is not ignored, other env files are.
        let example = run_git(
            f.root(),
            &["check-ignore", "--no-index", "-q", ".env.example"],
        )
        .unwrap();
        assert_eq!(example.code, 1);
        let local = run_git(
            f.root(),
            &["check-ignore", "--no-index", "-q", ".env.local"],
        )
        .unwrap();
        assert_eq!(local.code, 0);
    }

    #[test]
    fn untracks_files_whose_names_contain_spaces_unicode_and_glob_characters() {
        let names = [
            "my app/.env",
            "café/.env.local",
            "[x]/.env",
            "a b/[y] é/.env.prod",
        ];
        let files: Vec<(&str, &str)> = names.iter().map(|n| (*n, EXAMPLE)).collect();
        let f = repo(&files);
        let mut found = check_files(&f, &Options::new());
        found.sort();
        let mut expected: Vec<String> = names.iter().map(|n| (*n).to_owned()).collect();
        expected.sort();
        assert_eq!(found, expected);

        let result = f.fix(&NoCommittedEnv, &Options::new()).unwrap();
        for name in names {
            assert!(result.changed_files.contains(&name.to_owned()), "{name}");
            assert_eq!(f.read(name).as_deref(), Some(EXAMPLE));
        }
        assert!(f.tracked().unwrap().is_empty());
        assert!(check_files(&f, &Options::new()).is_empty());
    }

    #[test]
    fn treats_a_file_name_as_a_literal_not_a_glob_over_other_files() {
        let f = repo(&[
            ("[ab]/.env", EXAMPLE),
            ("a/keep.txt", "x\n"),
            ("b/keep.txt", "x\n"),
        ]);
        f.fix(&NoCommittedEnv, &Options::new()).unwrap();
        assert_eq!(f.tracked().unwrap(), vec!["a/keep.txt", "b/keep.txt"]);
    }

    #[test]
    fn rejects_allow_entries_that_contain_a_folder() {
        let f = repo(&[("apps/web/.env.local", EXAMPLE)]);
        let allow = options(json!({ "allow": ["apps/web/.env.local"] }));
        let error = f.check(&NoCommittedEnv, &allow).unwrap_err();
        assert!(user_message(&error).contains("without folders"), "{error}");
    }

    #[test]
    fn fix_itself_is_idempotent_when_called_twice_with_the_same_diagnostics() {
        let f = repo(&[(".env", EXAMPLE), (".gitignore", ".env\n")]);
        let diagnostics = f.check(&NoCommittedEnv, &Options::new()).unwrap();
        assert_eq!(diagnostics.len(), 1);
        let ctx = RuleContext::new(f.root(), Options::new());

        NoCommittedEnv.fix(&ctx, &diagnostics).unwrap();
        let after_first = f.read(".gitignore").unwrap();
        assert_eq!(
            after_first,
            ".env\n# Env files hold secrets. Commit an example file instead.\n.env.*\n!.env.example\n"
        );
        // Second direct call with the same diagnostics: already untracked, lines exist.
        let second = NoCommittedEnv.fix(&ctx, &diagnostics).unwrap();
        assert!(!second.changed_files.contains(&".gitignore".to_owned()));
        assert_eq!(f.read(".gitignore").unwrap(), after_first);
    }

    #[test]
    fn fix_called_again_with_no_diagnostics_changes_nothing() {
        let f = repo(&[(".env", EXAMPLE), (".gitignore", ".env\n")]);
        let diagnostics = f.check(&NoCommittedEnv, &Options::new()).unwrap();
        let ctx = RuleContext::new(f.root(), Options::new());
        NoCommittedEnv.fix(&ctx, &diagnostics).unwrap();
        let after_first = f.read(".gitignore").unwrap();
        let second = NoCommittedEnv.fix(&ctx, &[]).unwrap();
        assert!(second.changed_files.is_empty());
        assert_eq!(f.read(".gitignore").unwrap(), after_first);
    }

    #[test]
    fn does_not_duplicate_lines_in_a_gitignore_that_starts_with_a_bom() {
        let bom_text = "\u{feff}.env\n.env.*\n!.env.example\n";
        let files = [(".env", EXAMPLE), (".gitignore", bom_text)];
        let f = Fixture::new(&files, Track::Only(&[".env"])).unwrap();
        let result = f.fix(&NoCommittedEnv, &Options::new()).unwrap();
        assert_eq!(result.changed_files, vec![".env"]);
        assert_eq!(f.read(".gitignore").as_deref(), Some(bom_text));
    }
}
