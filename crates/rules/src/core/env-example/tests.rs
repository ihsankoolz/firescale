//! Tests for `core/env-example`.

use firescale_core::model::{Category, Diagnostic, Fixable, Severity};
use firescale_core::rule::{fix_rule_with, BuiltinRule, Options};
use firescale_core::testing::{Fixture, Track};
use firescale_core::Error;
use serde_json::json;

use super::{EnvExample, ID};

const APP: &str =
    "const a = process.env.EXAMPLE_API_URL;\nconst b = process.env[\"EXAMPLE_TOKEN_NAME\"];\n";

fn repo(files: &[(&str, &str)]) -> Fixture {
    Fixture::new(files, Track::All).unwrap()
}

fn check(f: &Fixture) -> Vec<Diagnostic> {
    f.check(&EnvExample, &Options::new()).unwrap()
}

fn options(value: serde_json::Value) -> Options {
    value.as_object().cloned().unwrap_or_default()
}

mod passing_repo {
    use super::*;

    #[test]
    fn all_names_listed_passes() {
        let f = repo(&[
            ("src/app.js", APP),
            (
                ".env.example",
                "EXAMPLE_API_URL=\nexport EXAMPLE_TOKEN_NAME=\n",
            ),
        ]);
        assert!(check(&f).is_empty());
    }

    #[test]
    fn no_env_reads_passes_without_an_example_file() {
        let f = repo(&[("src/app.js", "console.log(1);\n")]);
        assert!(check(&f).is_empty());
    }

    #[test]
    fn built_in_names_lowercase_names_and_skipped_folders_are_ignored() {
        let f = repo(&[
            (
                "src/a.ts",
                "process.env.NODE_ENV; process.env.GITHUB_TOKEN; process.env.fooBar; process.env.npm_x;\n",
            ),
            ("node_modules/x/index.js", "process.env.EXAMPLE_VENDORED;\n"),
            ("dist/out.js", "process.env.EXAMPLE_BUILT;\n"),
            ("notes.txt", "process.env.EXAMPLE_TEXT\n"),
        ]);
        assert!(check(&f).is_empty());
    }

    #[test]
    fn ignore_option_adds_names() {
        let f = repo(&[("a.py", "import os\nos.environ.get(\"EXAMPLE_SKIP\")\n")]);
        assert!(!check(&f).is_empty());
        let o = options(json!({ "ignore": ["EXAMPLE_SKIP"] }));
        assert!(f.check(&EnvExample, &o).unwrap().is_empty());
    }

    #[test]
    fn invalid_ignore_option_is_a_user_error_naming_the_option() {
        let f = repo(&[("a.js", APP)]);
        let err = f
            .check(&EnvExample, &options(json!({ "ignore": "nope" })))
            .unwrap_err();
        assert!(matches!(err, Error::User(_)));
        assert!(err.message().contains("ignore"));
    }

    #[test]
    fn untracked_sources_are_not_scanned() {
        let f = Fixture::new(&[("a.js", APP)], Track::None).unwrap();
        assert!(check(&f).is_empty());
    }
}

mod failing_repo {
    use super::*;

    #[test]
    fn missing_example_file_gives_one_finding_listing_names() {
        let f = repo(&[("src/app.js", APP)]);
        let d = check(&f);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].rule_id, ID);
        assert_eq!(d[0].file, ".env.example");
        assert_eq!(d[0].fixable, Fixable::Deterministic);
        assert!(d[0].message.contains("EXAMPLE_API_URL"));
        assert!(d[0].message.contains("EXAMPLE_TOKEN_NAME"));
    }

    #[test]
    fn one_finding_per_missing_name_at_its_first_read() {
        let f = repo(&[
            ("src/app.js", APP),
            ("lib/z.rs", "fn f() {\n    let _ = std::env::var(\"EXAMPLE_API_URL\");\n    let _ = env::var_os(\"EXAMPLE_RUST\");\n}\n"),
            (".env.example", "# comment\nEXAMPLE_TOKEN_NAME=\n"),
        ]);
        let d = check(&f);
        let summary: Vec<(String, u32, u32)> = d
            .iter()
            .map(|x| {
                let r = x.range.unwrap();
                (x.file.clone(), r.start_line, r.end_line)
            })
            .collect();
        assert_eq!(
            summary,
            vec![("lib/z.rs".to_owned(), 2, 2), ("lib/z.rs".to_owned(), 3, 3)]
        );
        assert!(d.iter().all(|x| x.fixable == Fixable::Deterministic));
    }

    #[test]
    fn recognises_each_language_pattern() {
        let f = repo(&[
            (
                "a.py",
                "os.environ[\"EXAMPLE_PY1\"]\nos.getenv('EXAMPLE_PY2')\n",
            ),
            ("b.go", "os.Getenv(\"EXAMPLE_GO\")\n"),
            ("c.rb", "ENV[\"EXAMPLE_RB1\"]\nENV.fetch(\"EXAMPLE_RB2\")\n"),
            ("d.ts", "import.meta.env.EXAMPLE_VITE\n"),
        ]);
        let d = check(&f);
        assert_eq!(d.len(), 1);
        for name in [
            "EXAMPLE_PY1",
            "EXAMPLE_PY2",
            "EXAMPLE_GO",
            "EXAMPLE_RB1",
            "EXAMPLE_RB2",
            "EXAMPLE_VITE",
        ] {
            assert!(d[0].message.contains(name), "{name}");
        }
    }
}

mod fixed_repo {
    use super::*;

    #[test]
    fn creates_the_example_file_with_sorted_empty_names() {
        let f = repo(&[("src/app.js", APP)]);
        let r = f.fix(&EnvExample, &Options::new()).unwrap();
        assert_eq!(r.changed_files, vec![".env.example"]);
        assert_eq!(
            f.read(".env.example").as_deref(),
            Some("EXAMPLE_API_URL=\nEXAMPLE_TOKEN_NAME=\n")
        );
        f.track(&[".env.example"]).unwrap();
        assert!(check(&f).is_empty());
    }

    #[test]
    fn appends_under_a_comment_and_keeps_existing_content() {
        let f = repo(&[
            ("src/app.js", APP),
            (".env.example", "EXAMPLE_API_URL=http://localhost"),
        ]);
        f.fix(&EnvExample, &Options::new()).unwrap();
        let text = f.read(".env.example").unwrap();
        assert!(text.starts_with("EXAMPLE_API_URL=http://localhost\n# "));
        assert!(text.ends_with("EXAMPLE_TOKEN_NAME=\n"));
        assert_eq!(text.matches("EXAMPLE_API_URL").count(), 1);
        assert!(check(&f).is_empty());
    }

    #[test]
    fn keeps_crlf_line_endings() {
        let f = repo(&[
            ("src/app.js", APP),
            (".env.example", "EXAMPLE_API_URL=\r\n"),
        ]);
        f.fix(&EnvExample, &Options::new()).unwrap();
        let text = f.read(".env.example").unwrap();
        assert!(!text.replace("\r\n", "").contains('\n'));
        assert!(text.ends_with("EXAMPLE_TOKEN_NAME=\r\n"));
    }
}

mod fix_run_twice {
    use super::*;

    #[test]
    fn the_second_run_changes_nothing() {
        let f = repo(&[("src/app.js", APP), (".env.example", "OTHER=\n")]);
        let first = f.fix(&EnvExample, &Options::new()).unwrap();
        assert_eq!(first.changed_files, vec![".env.example"]);
        let after = f.read(".env.example");
        let second = f.fix(&EnvExample, &Options::new()).unwrap();
        assert!(second.changed_files.is_empty());
        assert_eq!(f.read(".env.example"), after);
    }
}

mod dry_run {
    use super::*;

    #[test]
    fn a_dry_run_reports_the_write_and_touches_nothing() {
        let f = repo(&[("src/app.js", APP)]);
        let outcome = fix_rule_with(&EnvExample, f.root(), &Options::new(), true).unwrap();
        assert_eq!(outcome.changes.len(), 1);
        assert_eq!(f.read(".env.example"), None);
        assert_eq!(check(&f).len(), 1);
    }
}

mod metadata {
    use super::*;

    #[test]
    fn meta_matches_the_spec() {
        let meta = EnvExample.meta();
        assert_eq!(meta.id, ID);
        assert_eq!(meta.severity, Severity::Medium);
        assert_eq!(meta.category, Category::Reliability);
        assert!(meta.has_fix);
        assert!(meta.inputs.contains(&"**/*.ts".to_owned()));
        assert!(meta.inputs.contains(&".env.example".to_owned()));
    }
}

mod regressions {
    use super::*;

    fn messages(src: &str) -> Vec<String> {
        let f = repo(&[("src/app.rb", src)]);
        check(&f).into_iter().map(|d| d.message).collect()
    }

    #[test]
    fn cargo_variables_are_ignored() {
        let f = repo(&[(
            "src/main.rs",
            "let d = std::env::var(\"CARGO_MANIFEST_DIR\");\nlet r = std::env::var(\"RUSTUP_HOME\");\nlet c = std::env::var(\"RUSTC_WRAPPER\");\n",
        )]);
        assert!(check(&f).is_empty());
    }

    #[test]
    fn a_directory_named_env_example_is_one_manual_finding_and_no_fix() {
        let f = repo(&[("src/app.js", APP), (".env.example/placeholder.txt", "x\n")]);
        let found = check(&f);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].file, ".env.example");
        assert_eq!(found[0].fixable, Fixable::Manual);
        let outcome = f.fix(&EnvExample, &Options::new()).unwrap();
        assert!(outcome.changed_files.is_empty());
    }

    #[test]
    fn longer_identifiers_and_member_paths_are_not_reads() {
        assert!(
            messages("x = MY_ENV[\"FOO\"]\ny = config.ENV[\"FOO\"]\nz = myprocess.env.FOO\n")
                .is_empty()
        );
    }

    #[test]
    fn real_reads_still_match() {
        let f = repo(&[
            ("src/a.rb", "x = ENV[\"RB_NAME\"]\n"),
            ("src/b.rs", "let x = std::env::var(\"RS_NAME\");\n"),
            ("src/c.js", "const x = process.env.JS_NAME;\n"),
            (".env.example", "OTHER=\n"),
        ]);
        assert_eq!(check(&f).len(), 3);
    }
}

mod exclude_paths {
    use firescale_core::rule::Options;
    use firescale_core::testing::{Fixture, Track};
    use serde_json::json;

    use super::super::EnvExample;

    fn options(value: serde_json::Value) -> Options {
        value.as_object().cloned().unwrap_or_default()
    }

    #[test]
    fn files_under_an_excluded_prefix_are_not_scanned() {
        let fixture = Fixture::new(
            &[
                ("src/app.js", "const url = process.env.EXAMPLE_APP_URL;\n"),
                (
                    "fixtures/sample.js",
                    "const x = process.env.EXAMPLE_FIXTURE_ONLY;\n",
                ),
            ],
            Track::All,
        )
        .unwrap();
        let all = fixture.check(&EnvExample, &Options::new()).unwrap();
        assert!(
            all[0].message.contains("EXAMPLE_FIXTURE_ONLY"),
            "{}",
            all[0].message
        );

        let narrowed = fixture
            .check(
                &EnvExample,
                &options(json!({ "excludePaths": ["fixtures/"] })),
            )
            .unwrap();
        assert_eq!(narrowed.len(), 1);
        assert!(narrowed[0].message.contains("EXAMPLE_APP_URL"));
        assert!(!narrowed[0].message.contains("EXAMPLE_FIXTURE_ONLY"));
    }

    #[test]
    fn an_invalid_exclude_paths_option_is_rejected_with_a_next_step() {
        let fixture = Fixture::new(&[("src/app.js", "x\n")], Track::All).unwrap();
        for bad in [json!("fixtures/"), json!(["/abs"]), json!([""]), json!([1])] {
            let error = fixture
                .check(&EnvExample, &options(json!({ "excludePaths": bad })))
                .unwrap_err();
            assert!(
                error.message().contains("excludePaths"),
                "{}",
                error.message()
            );
        }
    }
}
