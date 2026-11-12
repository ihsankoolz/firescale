//! Tests for `group-project/contributing`.

use firescale_core::facts::detect_facts_with;
use firescale_core::model::{Category, Diagnostic, Fixable, Severity};
use firescale_core::rule::{BuiltinRule, Options};
use firescale_core::testing::{course_project_files, Fixture, Track};
use serde_json::json;

use super::{guide, Contributing, GUIDE, ID};
use crate::group_project::DEFAULT_BRANCHES;

fn check(fixture: &Fixture) -> Vec<Diagnostic> {
    fixture.check(&Contributing, &Options::new()).unwrap()
}

fn defaults() -> Vec<String> {
    DEFAULT_BRANCHES.iter().map(|b| (*b).to_owned()).collect()
}

#[test]
fn meta_matches_the_spec() {
    let meta = Contributing.meta();
    assert_eq!(meta.id, ID);
    assert_eq!(meta.severity, Severity::Medium);
    assert_eq!(meta.category, Category::Maintainability);
    assert!(meta.has_fix);
}

mod passing_repo {
    use super::*;

    #[test]
    fn a_guide_in_any_of_githubs_places_and_letter_cases_passes() {
        for file in [
            "CONTRIBUTING.md",
            "contributing.md",
            ".github/CONTRIBUTING.md",
            "docs/CONTRIBUTING.rst",
            "CONTRIBUTING",
        ] {
            let f = Fixture::new(&[(file, "# How we work\n")], Track::All).unwrap();
            assert!(check(&f).is_empty(), "{file}");
        }
    }
}

mod failing_repo {
    use super::*;

    #[test]
    fn no_guide_is_one_deterministic_finding() {
        let f = Fixture::new(&[("README.md", "x\n")], Track::All).unwrap();
        let found = check(&f);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].file, GUIDE);
        assert_eq!(found[0].fixable, Fixable::Deterministic);
    }

    #[test]
    fn a_guide_elsewhere_or_untracked_does_not_count() {
        let f = Fixture::new(&[("src/CONTRIBUTING.md", "x\n")], Track::All).unwrap();
        assert_eq!(check(&f)[0].fixable, Fixable::Deterministic);
        let f = Fixture::new(&[("CONTRIBUTING.md", "x\n")], Track::None).unwrap();
        let found = check(&f);
        assert_eq!(found[0].fixable, Fixable::Manual);
        assert!(f
            .fix(&Contributing, &Options::new())
            .unwrap()
            .changed_files
            .is_empty());
        assert_eq!(f.read(GUIDE).as_deref(), Some("x\n"));
    }

    #[test]
    fn a_bad_branches_option_is_a_user_error() {
        let f = Fixture::new(&[("CONTRIBUTING.md", "x\n")], Track::All).unwrap();
        let opts = json!({ "branches": [] }).as_object().cloned().unwrap();
        let error = f.check(&Contributing, &opts).unwrap_err();
        assert!(error
            .message()
            .contains("group-project/contributing: option \"branches\""));
    }
}

mod fixed_repo {
    use super::*;

    #[test]
    fn the_fix_writes_the_guide_from_the_facts_and_a_second_run_changes_nothing() {
        let f = Fixture::new(
            &[
                (
                    "package.json",
                    r#"{"name":"app","scripts":{"lint":"eslint .","test":"vitest run"}}"#,
                ),
                ("pnpm-lock.yaml", ""),
            ],
            Track::All,
        )
        .unwrap();
        let result = f.fix(&Contributing, &Options::new()).unwrap();
        assert_eq!(result.changed_files, [GUIDE]);
        assert!(f.tracked().unwrap().contains(&GUIDE.to_owned()));
        let text = f.read(GUIDE).unwrap();
        assert!(
            text.contains("- In the repo root: `pnpm install`\n"),
            "{text}"
        );
        assert!(
            text.contains("| repo root | `pnpm run lint` | - | `pnpm run test` |\n"),
            "{text}"
        );
        assert!(text.contains("npx firescale verify"), "{text}");
        assert!(text.contains("- `feat/*`\n"), "{text}");
        assert!(!text.contains('\r'));
        assert!(check(&f).is_empty());
        assert!(f
            .fix(&Contributing, &Options::new())
            .unwrap()
            .changed_files
            .is_empty());
        assert_eq!(f.read(GUIDE), Some(text));
    }

    #[test]
    fn the_guide_uses_the_branches_option() {
        let f = Fixture::new(&[("README.md", "x\n")], Track::All).unwrap();
        let opts = json!({ "branches": ["main", "story/*"] })
            .as_object()
            .cloned()
            .unwrap();
        f.fix(&Contributing, &opts).unwrap();
        let text = f.read(GUIDE).unwrap();
        assert!(
            text.contains("- `story/*`\n\nThe long-lived branches are `main`.\n"),
            "{text}"
        );
        assert!(!text.contains("feat/"), "{text}");
    }

    #[test]
    fn a_guide_a_person_wrote_is_never_touched() {
        let own = "# Rules\r\n\r\nAsk Sam.\r\n";
        let f = Fixture::new(&[(".github/CONTRIBUTING.md", own)], Track::All).unwrap();
        assert!(f
            .fix(&Contributing, &Options::new())
            .unwrap()
            .changed_files
            .is_empty());
        assert_eq!(f.read(".github/CONTRIBUTING.md").as_deref(), Some(own));
        assert_eq!(f.read(GUIDE), None);
    }
}

mod guide_text {
    use super::*;

    fn course() -> Fixture {
        let files = course_project_files();
        let borrowed: Vec<(&str, &str)> = files
            .iter()
            .map(|(file, content)| (file.as_str(), content.as_str()))
            .collect();
        Fixture::new(&borrowed, Track::NoGit).unwrap()
    }

    #[test]
    fn a_repo_with_projects_in_subfolders_lists_each_one() {
        let f = course();
        let facts = detect_facts_with(f.root(), None).unwrap();
        let text = guide(&facts, &defaults());
        assert!(
            text.contains(
                "- In `backend/audit_service`: `python -m pip install -r requirements-dev.txt`\n"
            ),
            "{text}"
        );
        assert!(text.contains("- In `frontend`: `npm install`\n"), "{text}");
        assert!(
            text.contains(
                "| `backend/auth_service` | `python -m ruff check .` | - | `python -m pytest` |\n"
            ),
            "{text}"
        );
        assert!(
            text.contains("| `frontend` | `npx --no oxlint` | `npx --no tsc --noEmit` | - |\n"),
            "{text}"
        );
        assert!(text.contains("python -m venv .venv"), "{text}");
        assert!(text.contains("tests in every project"), "{text}");
    }

    #[test]
    fn a_venv_on_this_machine_does_not_leak_into_the_guide() {
        let f = course();
        let venv = if cfg!(windows) {
            "backend/audit_service/.venv/Scripts/python.exe"
        } else {
            "backend/audit_service/.venv/bin/python"
        };
        f.write(venv, "").unwrap();
        let facts = detect_facts_with(f.root(), None).unwrap();
        let text = guide(&facts, &defaults());
        assert!(
            !text.contains(".venv/bin") && !text.contains("Scripts"),
            "{text}"
        );
        assert!(
            text.contains(
                "- In `backend/audit_service`: `python -m pip install -r requirements-dev.txt`\n"
            ),
            "{text}"
        );
    }

    #[test]
    fn an_empty_repo_gets_placeholders_not_invented_commands() {
        let f = Fixture::new(&[], Track::NoGit).unwrap();
        let facts = detect_facts_with(f.root(), None).unwrap();
        let text = guide(&facts, &defaults());
        assert!(
            text.contains("Describe how to install the project here.\n"),
            "{text}"
        );
        assert!(!text.contains("| Folder |"), "{text}");
        assert!(!text.contains("in every project"), "{text}");
    }
}
