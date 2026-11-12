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

mod segments_and_comments {
    use super::*;

    const READ: &str = "const v = process.env.EXAMPLE_SEG_VAR;\n";

    fn names(f: &Fixture, opts: serde_json::Value) -> Vec<Diagnostic> {
        f.check(&EnvExample, &options(opts)).unwrap()
    }

    #[test]
    fn exclude_paths_match_whole_segments() {
        let f = repo(&[("src/ab/y.ts", READ)]);
        assert_eq!(names(&f, json!({ "excludePaths": ["src/a"] })).len(), 1);
    }

    #[test]
    fn exclude_paths_with_and_without_trailing_slash_exclude_the_folder() {
        let f = repo(&[("src/a/x.ts", READ)]);
        assert!(names(&f, json!({ "excludePaths": ["src/a/"] })).is_empty());
        assert!(names(&f, json!({ "excludePaths": ["src/a"] })).is_empty());
        assert!(names(&f, json!({ "excludePaths": ["src/a/x.ts"] })).is_empty());
    }

    #[test]
    fn commented_reads_are_ignored_but_the_next_real_read_is_found() {
        let f = repo(&[
            (".env.example", "UNRELATED=\n"),
            (
                "app.py",
                "# os.environ[\"EXAMPLE_PY_COMMENT\"]\nx = os.environ[\"EXAMPLE_PY_REAL\"]\n",
            ),
            (
                "app.js",
                "// process.env.EXAMPLE_JS_COMMENT\n/* process.env.EXAMPLE_JS_BLOCK */\n * process.env.EXAMPLE_JS_STAR\nconst a = process.env.EXAMPLE_JS_REAL;\n",
            ),
        ]);
        let d = check(&f);
        let all: String = d.iter().map(|x| x.message.clone()).collect();
        assert!(all.contains("EXAMPLE_PY_REAL"), "{all}");
        assert!(all.contains("EXAMPLE_JS_REAL"), "{all}");
        assert!(!all.contains("COMMENT") && !all.contains("BLOCK") && !all.contains("STAR"));
        assert_eq!(d.len(), 2);
    }
}

mod well_known_variables {
    use super::super::well_known::{is_well_known, NAMES, PREFIXES};
    use super::*;

    const DOCS: &str = include_str!("../../../../../docs/rules.md");

    /// The text of the "always ignored" section: from its heading to the next heading.
    fn ignored_section(text: &str) -> &str {
        let heading = "### Variables that are always ignored";
        let start = text.find(heading).expect("the section exists") + heading.len();
        let rest = &text[start..];
        &rest[..rest.find("\n#").unwrap_or(rest.len())]
    }

    /// Every `` `CODE` `` span in the text.
    fn code_spans(text: &str) -> Vec<&str> {
        text.split('`').skip(1).step_by(2).collect()
    }

    #[test]
    fn system_and_tooling_variables_are_not_findings() {
        let f = repo(&[
            (
                "src/a.ts",
                "process.env.DOCKER_HOST; process.env.PATH; process.env.HOME; process.env.CI; process.env.NODE_ENV;\nprocess.env.XDG_CONFIG_HOME; process.env.VITEST; process.env.VITEST_WORKER_ID; process.env.USERPROFILE;\n",
            ),
            (
                "tools/run.py",
                "os.environ[\"PYTEST_CURRENT_TEST\"]\nos.getenv(\"PYTHONPATH\")\nos.getenv(\"HTTPS_PROXY\")\nos.getenv(\"LC_ALL\")\n",
            ),
        ]);
        assert!(check(&f).is_empty());
    }

    #[test]
    fn names_that_look_like_project_config_are_still_findings() {
        let names = [
            "PORT",
            "HOST",
            "DATABASE_URL",
            "EXAMPLE_API_KEY",
            "EXAMPLE_TOKEN",
            "EXAMPLE_SECRET",
            "VITE_EXAMPLE_URL",
            "NEXT_PUBLIC_EXAMPLE_URL",
            "DOCKER_EXAMPLE_REGISTRY",
        ];
        let source: String = names
            .iter()
            .map(|name| format!("process.env.{name};\n"))
            .collect();
        let f = repo(&[("src/a.ts", &source), (".env.example", "OTHER=\n")]);
        let d = check(&f);
        assert_eq!(d.len(), names.len());
        for name in names {
            assert!(!is_well_known(name), "{name}");
            let start = format!("{name} is read here");
            assert!(d.iter().any(|x| x.message.starts_with(&start)), "{name}");
        }
    }

    #[test]
    fn the_ignore_option_adds_to_the_built_in_list() {
        let f = repo(&[(
            "src/a.ts",
            "process.env.DOCKER_HOST; process.env.EXAMPLE_EXTRA;\n",
        )]);
        assert_eq!(check(&f).len(), 1);
        let o = options(json!({ "ignore": ["EXAMPLE_EXTRA"] }));
        assert!(f.check(&EnvExample, &o).unwrap().is_empty());
    }

    #[test]
    fn the_list_has_no_duplicates_and_no_name_a_prefix_already_covers() {
        let mut seen = std::collections::BTreeSet::new();
        for entry in NAMES.iter().chain(PREFIXES) {
            assert!(seen.insert(*entry), "{entry} is listed twice");
            assert!(
                entry.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                "{entry}"
            );
        }
        for name in NAMES {
            assert!(
                !PREFIXES.iter().any(|prefix| name.starts_with(prefix)),
                "{name} is already covered by a prefix"
            );
        }
    }

    #[test]
    fn the_skill_and_the_docs_list_exactly_the_names_and_prefixes_in_the_rust_list() {
        let mut expected: Vec<String> = NAMES.iter().map(|name| (*name).to_owned()).collect();
        expected.extend(PREFIXES.iter().map(|prefix| format!("{prefix}*")));
        expected.sort();
        for (label, text) in [("SKILL.md", EnvExample.skill()), ("docs/rules.md", DOCS)] {
            // Paths and the option name are code spans too; they are not variable names.
            let mut documented: Vec<String> = code_spans(ignored_section(text))
                .into_iter()
                .filter(|span| !span.contains(['/', '.']) && *span != "ignore")
                .map(str::to_owned)
                .collect();
            documented.sort();
            assert_eq!(
                documented, expected,
                "{label} and well_known.rs list different names"
            );
        }
    }
}

mod test_only_variables {
    use super::super::is_test_path;
    use super::*;

    const TEST_READ: &str = "const u = process.env.EXAMPLE_TEST_USER;\n";

    #[test]
    fn the_definition_of_test_code() {
        for file in [
            "test/a.py",
            "tests/a.rs",
            "src/__tests__/a.js",
            "backend/auth_service/unit_test/helpers.py",
            "svc/unit_tests/a.py",
            "svc/integration_test/a.py",
            "svc/integration_tests/a.py",
            "frontend/e2e/login.ts",
            "spec/models/user_spec.rb",
            "cypress/support/a.js",
            "conftest.py",
            "svc/conftest.py",
            "svc/test_api.py",
            "svc/api_test.py",
            "pkg/api_test.go",
            "src/api.test.ts",
            "src/api.spec.tsx",
            "src/api.test.d.ts",
        ] {
            assert!(is_test_path(file), "{file}");
        }
        for file in [
            "src/api.ts",
            "src/latest/api.ts",
            "src/contest.py",
            "src/testing.py",
            "src/test.ts",
            "src/spec.ts",
            "src/api_test.ts",
            "src/attest.py",
            "pkg/test_api.go",
            "src/Tests/api.ts",
            "protest/a.py",
            // Folders that often hold product code (review of M10): a spec
            // loader, test helpers that ship, a Playwright service wrapper.
            "src/spec/loader.ts",
            "specs/a.rb",
            "pkg/testing/a.go",
            "playwright/a.ts",
        ] {
            assert!(!is_test_path(file), "{file}");
        }
    }

    #[test]
    fn a_name_read_only_by_tests_is_a_low_severity_finding_that_says_so() {
        let f = repo(&[
            ("frontend/e2e/login.spec.ts", TEST_READ),
            ("src/app.js", APP),
            (".env.example", "EXAMPLE_API_URL=\nEXAMPLE_TOKEN_NAME=\n"),
        ]);
        let d = check(&f);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].file, "frontend/e2e/login.spec.ts");
        assert_eq!(d[0].severity, Some(Severity::Low));
        assert_eq!(d[0].fixable, Fixable::Deterministic);
        assert_eq!(
            d[0].message,
            "EXAMPLE_TEST_USER is read here, only by test code, and is not listed in .env.example. Add a line \"EXAMPLE_TEST_USER=\" to .env.example."
        );
    }

    #[test]
    fn a_name_read_by_tests_and_by_other_code_is_a_normal_finding_at_the_other_read() {
        let f = repo(&[
            ("a/tests/first.ts", TEST_READ),
            ("src/z.ts", "\nconst u = process.env.EXAMPLE_TEST_USER;\n"),
            (".env.example", "OTHER=\n"),
        ]);
        let d = check(&f);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].file, "src/z.ts");
        assert_eq!(d[0].range.unwrap().start_line, 2);
        assert_eq!(d[0].severity, None);
        assert!(!d[0].message.contains("test code"), "{}", d[0].message);
    }

    #[test]
    fn a_listed_test_only_name_is_not_a_finding() {
        let f = repo(&[
            (
                "svc/unit_test/conftest.py",
                "os.getenv(\"EXAMPLE_TEST_USER\")\n",
            ),
            (".env.example", "EXAMPLE_TEST_USER=\n"),
        ]);
        assert!(check(&f).is_empty());
    }

    #[test]
    fn without_an_example_file_the_summary_is_low_only_when_every_name_is_test_only() {
        let only_tests = repo(&[("e2e/login.spec.ts", TEST_READ)]);
        let d = check(&only_tests);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].file, ".env.example");
        assert_eq!(d[0].severity, Some(Severity::Low));
        assert!(d[0].message.contains("EXAMPLE_TEST_USER"));
        assert!(d[0].message.contains("Only test code"), "{}", d[0].message);

        let mixed = repo(&[("e2e/login.spec.ts", TEST_READ), ("src/app.js", APP)]);
        let d = check(&mixed);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].severity, None);
        for name in ["EXAMPLE_API_URL", "EXAMPLE_TEST_USER", "EXAMPLE_TOKEN_NAME"] {
            assert!(d[0].message.contains(name), "{name}");
        }
    }

    #[test]
    fn the_fix_writes_test_only_names_under_their_own_comment() {
        let f = repo(&[
            ("e2e/login.spec.ts", TEST_READ),
            ("src/app.js", APP),
            (".env.example", "EXAMPLE_API_URL=\n"),
        ]);
        f.fix(&EnvExample, &Options::new()).unwrap();
        assert_eq!(
            f.read(".env.example").as_deref(),
            Some(
                "EXAMPLE_API_URL=\n# Variables read by the code. Add example values or leave empty.\nEXAMPLE_TOKEN_NAME=\n# Test-only variables: read only by test code.\nEXAMPLE_TEST_USER=\n"
            )
        );
        assert!(check(&f).is_empty());
    }

    #[test]
    fn a_new_file_keeps_test_only_names_apart_too() {
        let f = repo(&[("e2e/login.spec.ts", TEST_READ), ("src/app.js", APP)]);
        f.fix(&EnvExample, &Options::new()).unwrap();
        assert_eq!(
            f.read(".env.example").as_deref(),
            Some(
                "EXAMPLE_API_URL=\nEXAMPLE_TOKEN_NAME=\n# Test-only variables: read only by test code.\nEXAMPLE_TEST_USER=\n"
            )
        );
    }

    #[test]
    fn the_fix_keeps_crlf_in_the_test_only_block_and_a_second_run_changes_nothing() {
        let f = repo(&[
            ("e2e/login.spec.ts", TEST_READ),
            (".env.example", "OTHER=1\r\n"),
        ]);
        f.fix(&EnvExample, &Options::new()).unwrap();
        let text = f.read(".env.example").unwrap();
        assert_eq!(
            text,
            "OTHER=1\r\n# Test-only variables: read only by test code.\r\nEXAMPLE_TEST_USER=\r\n"
        );
        let second = f.fix(&EnvExample, &Options::new()).unwrap();
        assert!(second.changed_files.is_empty());
        assert_eq!(f.read(".env.example"), Some(text));
    }

    #[test]
    fn a_test_only_name_that_other_code_starts_to_read_is_not_added_again() {
        let f = repo(&[
            ("e2e/login.spec.ts", TEST_READ),
            (".env.example", "OTHER=\n"),
        ]);
        f.fix(&EnvExample, &Options::new()).unwrap();
        let after_first = f.read(".env.example").unwrap();
        f.write("src/app.ts", TEST_READ).unwrap();
        f.track(&["src/app.ts"]).unwrap();
        assert!(check(&f).is_empty());
        let second = f.fix(&EnvExample, &Options::new()).unwrap();
        assert!(second.changed_files.is_empty());
        let text = f.read(".env.example").unwrap();
        assert_eq!(text, after_first);
        assert_eq!(text.matches("EXAMPLE_TEST_USER").count(), 1);
    }

    #[test]
    fn a_later_test_only_name_joins_the_block_without_a_second_comment() {
        let f = repo(&[
            ("e2e/login.spec.ts", TEST_READ),
            (".env.example", "OTHER=\n"),
        ]);
        f.fix(&EnvExample, &Options::new()).unwrap();
        f.write("e2e/more.spec.ts", "process.env.EXAMPLE_TEST_EXTRA;\n")
            .unwrap();
        f.track(&["e2e/more.spec.ts"]).unwrap();
        f.fix(&EnvExample, &Options::new()).unwrap();
        assert_eq!(
            f.read(".env.example").as_deref(),
            Some(
                "OTHER=\n# Test-only variables: read only by test code.\nEXAMPLE_TEST_USER=\nEXAMPLE_TEST_EXTRA=\n"
            )
        );
    }
}

mod nearest_example_file {
    use firescale_core::cache::input_matcher;

    use super::*;

    const AUTH: &str = "import os\nissuer = os.environ[\"EXAMPLE_AUTH_ISSUER\"]\n";

    #[test]
    fn a_sibling_folders_example_file_does_not_satisfy_a_read() {
        let f = repo(&[
            ("backend/auth_service/main.py", AUTH),
            ("backend/ledger_service/main.py", AUTH),
            (
                "backend/auth_service/.env.example",
                "EXAMPLE_AUTH_ISSUER=\n",
            ),
            (".env.example", "OTHER=\n"),
        ]);
        let d = check(&f);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].file, "backend/ledger_service/main.py");
        assert_eq!(d[0].range.unwrap().start_line, 2);
        assert_eq!(d[0].severity, None);
        assert_eq!(
            d[0].message,
            "EXAMPLE_AUTH_ISSUER is read here but not listed in .env.example. Add a line \"EXAMPLE_AUTH_ISSUER=\" to .env.example."
        );
    }

    #[test]
    fn any_example_file_at_or_above_the_reading_file_satisfies_a_read() {
        for example in [
            "backend/auth_service/.env.example",
            "backend/.env.example",
            ".env.example",
        ] {
            let f = repo(&[
                ("backend/auth_service/main.py", AUTH),
                (example, "EXAMPLE_AUTH_ISSUER=\n"),
            ]);
            assert!(check(&f).is_empty(), "{example}");
        }
    }

    #[test]
    fn a_name_listed_only_above_the_nearest_file_is_still_satisfied() {
        let f = repo(&[
            ("backend/auth_service/main.py", AUTH),
            ("backend/auth_service/.env.example", "OTHER=\n"),
            (".env.example", "EXAMPLE_AUTH_ISSUER=\n"),
        ]);
        assert!(check(&f).is_empty());
    }

    #[test]
    fn the_finding_names_the_nearest_example_file() {
        let f = repo(&[
            ("backend/auth_service/routes/login.py", AUTH),
            ("backend/auth_service/.env.example", "OTHER=\n"),
            (".env.example", "OTHER=\n"),
        ]);
        let d = check(&f);
        assert_eq!(d.len(), 1);
        assert_eq!(
            d[0].message,
            "EXAMPLE_AUTH_ISSUER is read here but not listed in backend/auth_service/.env.example. Add a line \"EXAMPLE_AUTH_ISSUER=\" to backend/auth_service/.env.example."
        );
    }

    #[test]
    fn one_name_can_be_a_finding_for_two_example_files_and_once_per_file() {
        let f = repo(&[
            ("a/one.py", AUTH),
            ("a/deep/two.py", AUTH),
            ("b/three.py", AUTH),
            ("c/four.py", AUTH),
            ("a/.env.example", "OTHER=\n"),
            ("b/.env.example", "OTHER=\n"),
            ("c/.env.example", "EXAMPLE_AUTH_ISSUER=\n"),
        ]);
        let d = check(&f);
        let files: Vec<&str> = d.iter().map(|x| x.file.as_str()).collect();
        assert_eq!(files, vec!["a/deep/two.py", "b/three.py"]);
        assert!(d[0].message.contains("in a/.env.example"));
        assert!(d[1].message.contains("in b/.env.example"));
    }

    #[test]
    fn nested_names_get_their_own_findings_next_to_the_summary_for_a_missing_root_file() {
        let f = repo(&[
            ("svc/main.py", AUTH),
            ("svc/.env.example", "OTHER=\n"),
            ("src/app.js", APP),
        ]);
        let d = check(&f);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].file, ".env.example");
        assert!(d[0].message.contains("EXAMPLE_API_URL"));
        assert!(!d[0].message.contains("EXAMPLE_AUTH_ISSUER"));
        assert_eq!(d[1].file, "svc/main.py");
    }

    #[test]
    fn an_untracked_example_file_counts_in_the_root_and_in_a_nested_folder() {
        for example in [".env.example", "svc/.env.example"] {
            let f = Fixture::new(
                &[("svc/main.py", AUTH), (example, "EXAMPLE_AUTH_ISSUER=\n")],
                Track::Only(&["svc/main.py"]),
            )
            .unwrap();
            assert!(check(&f).is_empty(), "{example}");
        }
    }

    #[test]
    fn a_nested_folder_named_env_example_counts_as_absent() {
        let f = repo(&[
            ("svc/main.py", AUTH),
            ("svc/.env.example/placeholder.txt", "x\n"),
            (".env.example", "OTHER=\n"),
        ]);
        let d = check(&f);
        assert_eq!(d.len(), 1);
        assert!(d[0].message.contains("listed in .env.example"));
        let r = f.fix(&EnvExample, &Options::new()).unwrap();
        assert_eq!(r.changed_files, vec![".env.example"]);
        assert!(check(&f).is_empty());
    }

    #[test]
    fn the_fix_adds_to_the_nearest_file_and_leaves_the_others_alone() {
        let f = repo(&[
            ("backend/auth_service/main.py", AUTH),
            ("backend/auth_service/.env.example", "OTHER=1\r\n"),
            (".env.example", "ROOT=\n"),
        ]);
        let r = f.fix(&EnvExample, &Options::new()).unwrap();
        assert_eq!(r.changed_files, vec!["backend/auth_service/.env.example"]);
        assert_eq!(f.read(".env.example").as_deref(), Some("ROOT=\n"));
        assert_eq!(
            f.read("backend/auth_service/.env.example").as_deref(),
            Some(
                "OTHER=1\r\n# Variables read by the code. Add example values or leave empty.\r\nEXAMPLE_AUTH_ISSUER=\r\n"
            )
        );
        assert!(check(&f).is_empty());
        let second = f.fix(&EnvExample, &Options::new()).unwrap();
        assert!(second.changed_files.is_empty());
    }

    #[test]
    fn the_fix_creates_the_root_file_when_no_example_file_is_above_the_read() {
        let f = repo(&[
            ("backend/ledger_service/main.py", AUTH),
            ("backend/auth_service/.env.example", "OTHER=\n"),
        ]);
        let r = f.fix(&EnvExample, &Options::new()).unwrap();
        assert_eq!(r.changed_files, vec![".env.example"]);
        assert_eq!(
            f.read(".env.example").as_deref(),
            Some("EXAMPLE_AUTH_ISSUER=\n")
        );
        assert_eq!(
            f.read("backend/auth_service/.env.example").as_deref(),
            Some("OTHER=\n")
        );
        assert_eq!(f.read("backend/ledger_service/.env.example"), None);
        assert!(check(&f).is_empty());
    }

    #[test]
    fn a_dry_run_reports_the_nested_write_and_touches_nothing() {
        let f = repo(&[("svc/main.py", AUTH), ("svc/.env.example", "OTHER=\n")]);
        let outcome = fix_rule_with(&EnvExample, f.root(), &Options::new(), true).unwrap();
        assert_eq!(outcome.changes.len(), 1);
        assert_eq!(f.read("svc/.env.example").as_deref(), Some("OTHER=\n"));
    }

    #[test]
    fn nested_example_files_are_cache_inputs() {
        let meta = EnvExample.meta();
        assert!(meta.inputs.contains(&"**/.env.example".to_owned()));
        let matcher = input_matcher(&meta.id, &meta.inputs).unwrap();
        for file in [
            ".env.example",
            "frontend/.env.example",
            "backend/auth_service/.env.example",
            "backend/auth_service/main.py",
        ] {
            assert!(matcher.is_match(file), "{file}");
        }
        assert!(!matcher.is_match("backend/auth_service/.env"));
        assert!(!matcher.is_match("backend/auth_service/README.md"));
    }
}

/// A repo shaped like a small course project: compose file and example file in
/// the root, Python services under `backend/`, a Vite app under `frontend/`.
mod course_project {
    use super::*;

    fn project() -> Fixture {
        repo(&[
            ("docker-compose.yml", "services: {}\n"),
            ("README.md", "# Example\n"),
            (".env.example", "EXAMPLE_COMPOSE_PORT=8080\r\n"),
            (
                "backend/auth_service/main.py",
                "import os\nissuer = os.environ[\"EXAMPLE_AUTH_ISSUER\"]\nport = os.getenv(\"EXAMPLE_COMPOSE_PORT\")\nttl = os.getenv(\"EXAMPLE_AUTH_TTL\")\ndocker = os.getenv(\"DOCKER_HOST\")\n",
            ),
            ("backend/auth_service/.env.example", "EXAMPLE_AUTH_ISSUER=\n"),
            (
                "backend/auth_service/unit_test/conftest.py",
                "import os\nos.environ.setdefault(\"EXAMPLE_TEST_DB\", \"sqlite://\")\nttl = os.getenv(\"EXAMPLE_AUTH_TTL\")\n",
            ),
            (
                "backend/ledger_service/main.py",
                "import os\nissuer = os.environ[\"EXAMPLE_AUTH_ISSUER\"]\n",
            ),
            (
                "frontend/src/api.ts",
                "export const base = import.meta.env.EXAMPLE_API_BASE;\nexport const port = process.env.EXAMPLE_COMPOSE_PORT;\nexport const mode = process.env.NODE_ENV;\n",
            ),
            (
                "frontend/e2e/login.spec.ts",
                "const user = process.env.EXAMPLE_E2E_USER;\nconst ci = process.env.CI;\n",
            ),
        ])
    }

    #[test]
    fn each_read_is_checked_against_the_example_files_above_it() {
        let d = check(&project());
        let summary: Vec<(&str, u32, Option<Severity>, &str)> = d
            .iter()
            .map(|x| {
                (
                    x.file.as_str(),
                    x.range.unwrap().start_line,
                    x.severity,
                    x.message.as_str(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                (
                    "backend/auth_service/main.py",
                    4,
                    None,
                    "EXAMPLE_AUTH_TTL is read here but not listed in backend/auth_service/.env.example. Add a line \"EXAMPLE_AUTH_TTL=\" to backend/auth_service/.env.example."
                ),
                (
                    "backend/auth_service/unit_test/conftest.py",
                    2,
                    Some(Severity::Low),
                    "EXAMPLE_TEST_DB is read here, only by test code, and is not listed in backend/auth_service/.env.example. Add a line \"EXAMPLE_TEST_DB=\" to backend/auth_service/.env.example."
                ),
                (
                    "backend/ledger_service/main.py",
                    2,
                    None,
                    "EXAMPLE_AUTH_ISSUER is read here but not listed in .env.example. Add a line \"EXAMPLE_AUTH_ISSUER=\" to .env.example."
                ),
                (
                    "frontend/e2e/login.spec.ts",
                    1,
                    Some(Severity::Low),
                    "EXAMPLE_E2E_USER is read here, only by test code, and is not listed in .env.example. Add a line \"EXAMPLE_E2E_USER=\" to .env.example."
                ),
                (
                    "frontend/src/api.ts",
                    1,
                    None,
                    "EXAMPLE_API_BASE is read here but not listed in .env.example. Add a line \"EXAMPLE_API_BASE=\" to .env.example."
                ),
            ]
        );
        assert!(d.iter().all(|x| x.fixable == Fixable::Deterministic));
    }

    #[test]
    fn the_fix_fills_each_example_file_and_a_second_run_changes_nothing() {
        let f = project();
        let first = f.fix(&EnvExample, &Options::new()).unwrap();
        assert_eq!(
            first.changed_files,
            vec![".env.example", "backend/auth_service/.env.example"]
        );
        let root = f.read(".env.example").unwrap();
        assert_eq!(
            root,
            "EXAMPLE_COMPOSE_PORT=8080\r\n# Variables read by the code. Add example values or leave empty.\r\nEXAMPLE_API_BASE=\r\nEXAMPLE_AUTH_ISSUER=\r\n# Test-only variables: read only by test code.\r\nEXAMPLE_E2E_USER=\r\n"
        );
        let auth = f.read("backend/auth_service/.env.example").unwrap();
        assert_eq!(
            auth,
            "EXAMPLE_AUTH_ISSUER=\n# Variables read by the code. Add example values or leave empty.\nEXAMPLE_AUTH_TTL=\n# Test-only variables: read only by test code.\nEXAMPLE_TEST_DB=\n"
        );
        assert!(check(&f).is_empty());

        let second = f.fix(&EnvExample, &Options::new()).unwrap();
        assert!(second.changed_files.is_empty());
        assert_eq!(f.read(".env.example"), Some(root));
        assert_eq!(f.read("backend/auth_service/.env.example"), Some(auth));
    }
}

mod review_findings {
    use super::*;

    #[test]
    fn product_code_in_a_folder_named_spec_or_testing_is_not_test_code() {
        let f = repo(&[
            (".env.example", ""),
            (
                "src/spec/loader.ts",
                "const a = process.env.EXAMPLE_SPEC_URL;\n",
            ),
            (
                "src/testing/helpers.ts",
                "const b = process.env.EXAMPLE_HELPER_URL;\n",
            ),
            ("src/e2e/run.ts", "const c = process.env.EXAMPLE_E2E_URL;\n"),
        ]);
        let found = check(&f);
        let severity = |name: &str| {
            found
                .iter()
                .find(|d| d.message.starts_with(name))
                .unwrap_or_else(|| panic!("{name} not reported: {found:?}"))
                .severity
        };
        assert_eq!(severity("EXAMPLE_SPEC_URL"), None);
        assert_eq!(severity("EXAMPLE_HELPER_URL"), None);
        assert_eq!(severity("EXAMPLE_E2E_URL"), Some(Severity::Low));
    }

    #[test]
    fn names_a_project_could_define_are_not_on_the_ignore_list() {
        let f = repo(&[
            (".env.example", ""),
            (
                "app.py",
                "import os\nos.getenv(\"USERNAME\")\nos.getenv(\"HOSTNAME\")\nos.getenv(\"COMPOSE_PROJECT_NAME\")\nos.getenv(\"PLAYWRIGHT_SERVICE_TOKEN\")\nos.getenv(\"DOCKER_HOST\")\n",
            ),
        ]);
        let mut names: Vec<String> = check(&f)
            .iter()
            .map(|d| d.message.split(' ').next().unwrap_or_default().to_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "COMPOSE_PROJECT_NAME",
                "HOSTNAME",
                "PLAYWRIGHT_SERVICE_TOKEN",
                "USERNAME"
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_example_file_is_a_manual_fix_and_is_never_written() {
        let outside =
            Fixture::new(&[("shared.env.example", "EXAMPLE_KEPT=\n")], Track::NoGit).unwrap();
        let real = outside.root().join("shared.env.example");
        let f = repo(&[
            (".env.example", ""),
            ("svc/main.py", "import os\nos.getenv(\"EXAMPLE_SVC_URL\")\n"),
            ("app.py", "import os\nos.getenv(\"EXAMPLE_ROOT_URL\")\n"),
        ]);
        std::os::unix::fs::symlink(&real, f.root().join("svc").join(".env.example")).unwrap();
        let found = check(&f);
        let svc = found
            .iter()
            .find(|d| d.message.starts_with("EXAMPLE_SVC_URL"))
            .unwrap();
        assert_eq!(svc.fixable, Fixable::Manual);
        assert!(svc.message.contains("symbolic link"));
        // The fix still does the rest, and leaves the link's target alone.
        let result = f.fix(&EnvExample, &Options::new()).unwrap();
        assert_eq!(result.changed_files, [".env.example"]);
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "EXAMPLE_KEPT=\n");
        assert!(f
            .fix(&EnvExample, &Options::new())
            .unwrap()
            .changed_files
            .is_empty());
    }
}

mod test_runner_config {
    use super::super::is_test_path;
    use super::*;

    #[test]
    fn test_runner_config_files_are_test_code() {
        for file in [
            "frontend/playwright.config.ts",
            "playwright.config.js",
            "web/vitest.config.mts",
            "vitest.workspace.ts",
            "jest.config.cjs",
            "app/cypress.config.ts",
            "playwright-ct.config.ts",
            "vitest.e2e.config.ts",
            "jest.setup.js",
            "vitest.setup.ts",
            "Playwright.Config.TS",
        ] {
            assert!(is_test_path(file), "{file}");
        }
        // Build and app config is not test code, and neither is a file that
        // only mentions a runner in its name.
        for file in [
            "frontend/vite.config.ts",
            "next.config.js",
            "src/playwright.ts",
            "src/jest.setup-config.ts",
            "src/my-vitest.config.ts",
        ] {
            assert!(!is_test_path(file), "{file}");
        }
    }

    #[test]
    fn a_variable_read_only_by_a_playwright_config_is_low_severity() {
        let f = repo(&[
            (".env.example", ""),
            (
                "frontend/playwright.config.ts",
                "export default { use: { baseURL: process.env.EXAMPLE_E2E_BASE_URL } };\n",
            ),
            (
                "frontend/vitest.config.ts",
                "export default { test: { env: { A: process.env.EXAMPLE_VITEST_SEED } } };\n",
            ),
        ]);
        let found = check(&f);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found.iter().all(|d| d.severity == Some(Severity::Low)));
        assert!(found
            .iter()
            .all(|d| d.message.contains("only by test code")));
    }

    #[test]
    fn a_variable_read_by_the_vite_config_is_a_normal_finding() {
        let f = repo(&[
            (".env.example", ""),
            (
                "frontend/vite.config.ts",
                "export default { server: { port: Number(process.env.EXAMPLE_DEV_PORT) } };\n",
            ),
        ]);
        let found = check(&f);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].severity, None);
    }
}
