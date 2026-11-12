//! Tests for `group-project/pr-template`.

use firescale_core::model::{Category, Diagnostic, Fixable, Severity};
use firescale_core::rule::{BuiltinRule, Options};
use firescale_core::testing::{Fixture, Track};

use super::{PrTemplate, ID, TEMPLATE, TEMPLATE_TEXT};

fn check(fixture: &Fixture) -> Vec<Diagnostic> {
    fixture.check(&PrTemplate, &Options::new()).unwrap()
}

#[test]
fn meta_matches_the_spec() {
    let meta = PrTemplate.meta();
    assert_eq!(meta.id, ID);
    assert_eq!(meta.severity, Severity::Low);
    assert_eq!(meta.category, Category::Consistency);
    assert!(meta.has_fix);
    assert!(meta.after.is_empty());
}

mod passing_repo {
    use super::*;

    #[test]
    fn a_template_in_any_of_githubs_places_and_letter_cases_passes() {
        for file in [
            ".github/pull_request_template.md",
            "pull_request_template.md",
            "docs/pull_request_template.md",
            ".github/PULL_REQUEST_TEMPLATE.md",
            "docs/Pull_Request_Template.txt",
            ".github/PULL_REQUEST_TEMPLATE/feature.md",
            "PULL_REQUEST_TEMPLATE/bugfix.md",
        ] {
            let f = Fixture::new(&[(file, "## What\n")], Track::All).unwrap();
            assert!(check(&f).is_empty(), "{file}");
        }
    }

    #[test]
    fn outside_git_a_file_on_disk_counts() {
        let f = Fixture::new(&[(TEMPLATE, "x\n")], Track::NoGit).unwrap();
        assert!(check(&f).is_empty());
    }
}

mod failing_repo {
    use super::*;

    #[test]
    fn no_template_is_one_deterministic_finding() {
        let f = Fixture::new(&[("README.md", "x\n")], Track::All).unwrap();
        let found = check(&f);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].rule_id, ID);
        assert_eq!(found[0].file, TEMPLATE);
        assert_eq!(found[0].fixable, Fixable::Deterministic);
    }

    #[test]
    fn a_template_somewhere_else_does_not_count() {
        for file in [
            "src/pull_request_template.md",
            ".github/templates/pull_request_template.md",
            ".github/pull_request.md",
        ] {
            let f = Fixture::new(&[(file, "x\n")], Track::All).unwrap();
            assert_eq!(check(&f).len(), 1, "{file}");
        }
    }

    #[test]
    fn an_untracked_template_is_a_manual_finding_and_the_fix_leaves_it() {
        let f = Fixture::new(&[("docs/pull_request_template.md", "x\n")], Track::None).unwrap();
        let found = check(&f);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].fixable, Fixable::Manual);
        assert!(found[0]
            .message
            .contains("git add docs/pull_request_template.md"));
        assert!(f
            .fix(&PrTemplate, &Options::new())
            .unwrap()
            .changed_files
            .is_empty());
        assert_eq!(f.read(TEMPLATE), None);
    }

    #[test]
    fn an_ignored_path_is_a_manual_finding_and_nothing_is_written() {
        let f = Fixture::new(&[(".gitignore", ".github/\n")], Track::All).unwrap();
        let found = check(&f);
        assert_eq!(found[0].fixable, Fixable::Manual);
        assert!(found[0].message.contains(".gitignore"));
        assert!(f
            .fix(&PrTemplate, &Options::new())
            .unwrap()
            .changed_files
            .is_empty());
        assert_eq!(f.read(TEMPLATE), None);
    }

    #[test]
    fn a_file_named_github_blocks_the_fix() {
        let f = Fixture::new(&[(".github", "not a folder\n")], Track::All).unwrap();
        let found = check(&f);
        assert_eq!(found[0].fixable, Fixable::Manual);
        assert!(f
            .fix(&PrTemplate, &Options::new())
            .unwrap()
            .changed_files
            .is_empty());
    }
}

mod fixed_repo {
    use super::*;

    #[test]
    fn the_fix_creates_and_stages_the_template_then_the_check_passes() {
        let f = Fixture::new(&[("README.md", "x\n")], Track::All).unwrap();
        let result = f.fix(&PrTemplate, &Options::new()).unwrap();
        assert_eq!(result.changed_files, [TEMPLATE]);
        assert!(!result.notes.is_empty());
        assert_eq!(f.read(TEMPLATE).as_deref(), Some(TEMPLATE_TEXT));
        assert!(f.tracked().unwrap().contains(&TEMPLATE.to_owned()));
        assert!(check(&f).is_empty());
        assert!(!TEMPLATE_TEXT.contains('\r'));
    }

    #[test]
    fn the_second_fix_changes_nothing() {
        let f = Fixture::new(&[("README.md", "x\n")], Track::All).unwrap();
        f.fix(&PrTemplate, &Options::new()).unwrap();
        let second = f.fix(&PrTemplate, &Options::new()).unwrap();
        assert!(second.changed_files.is_empty());
        assert_eq!(f.read(TEMPLATE).as_deref(), Some(TEMPLATE_TEXT));
    }

    #[test]
    fn a_template_a_person_wrote_is_never_touched() {
        let own = "# Our own template\r\n\r\nSay what you did.\r\n";
        let f = Fixture::new(&[("docs/PULL_REQUEST_TEMPLATE.md", own)], Track::All).unwrap();
        assert!(f
            .fix(&PrTemplate, &Options::new())
            .unwrap()
            .changed_files
            .is_empty());
        assert_eq!(
            f.read("docs/PULL_REQUEST_TEMPLATE.md").as_deref(),
            Some(own)
        );
        assert_eq!(f.read(TEMPLATE), None);
    }
}
