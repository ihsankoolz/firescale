//! Tests for `agent-ready/agents-md`.

use firescale_core::model::{Category, Diagnostic, Fixable, Severity};
use firescale_core::rule::{BuiltinRule, Options};
use firescale_core::testing::{Fixture, Track};

use super::{AgentsMdExists, ID};

fn check(fixture: &Fixture) -> Vec<Diagnostic> {
    fixture.check(&AgentsMdExists, &Options::new()).unwrap()
}

#[test]
fn meta_matches_the_spec() {
    let meta = AgentsMdExists.meta();
    assert_eq!(meta.id, ID);
    assert_eq!(meta.severity, Severity::Medium);
    assert_eq!(meta.category, Category::Consistency);
    assert!(!meta.has_fix);
    assert!(meta.inputs.contains(&"AGENTS.md".to_owned()));
}

mod passing_repo {
    use super::*;

    #[test]
    fn a_file_with_content_passes() {
        let f = Fixture::new(
            &[("AGENTS.md", "# Agents\n\nRun `make test`.\n")],
            Track::All,
        )
        .unwrap();
        assert!(check(&f).is_empty());
    }

    #[test]
    fn works_outside_a_git_repo_and_for_an_untracked_file() {
        for track in [Track::NoGit, Track::None] {
            let f = Fixture::new(&[("AGENTS.md", "x")], track).unwrap();
            assert!(check(&f).is_empty());
        }
    }

    #[test]
    fn a_byte_order_mark_and_crlf_do_not_matter() {
        let f = Fixture::new(
            &[("AGENTS.md", "\u{feff}# Agents\r\n\r\nText.\r\n")],
            Track::All,
        )
        .unwrap();
        assert!(check(&f).is_empty());
    }

    #[test]
    fn bytes_that_are_not_utf8_still_count_as_content() {
        let f = Fixture::new(&[], Track::NoGit).unwrap();
        std::fs::write(f.root().join("AGENTS.md"), [0xff, 0xfe, b'h', b'i']).unwrap();
        assert!(check(&f).is_empty());
    }
}

mod failing_repo {
    use super::*;

    #[test]
    fn a_missing_file_is_one_agent_fixable_finding() {
        let f = Fixture::new(&[("README.md", "x\n")], Track::All).unwrap();
        let found = check(&f);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].rule_id, ID);
        assert_eq!(found[0].file, "AGENTS.md");
        assert_eq!(found[0].fixable, Fixable::Agent);
        assert!(found[0].message.contains("No AGENTS.md"));
    }

    #[test]
    fn an_agents_file_in_a_subfolder_does_not_count() {
        let f = Fixture::new(&[("docs/AGENTS.md", "# Agents\n")], Track::All).unwrap();
        assert_eq!(check(&f).len(), 1);
    }

    #[test]
    fn a_blank_file_is_reported() {
        for text in ["", "\n", "  \r\n\t\r\n", "\u{feff}", "\u{feff}\n \n"] {
            let f = Fixture::new(&[("AGENTS.md", text)], Track::All).unwrap();
            let found = check(&f);
            assert_eq!(found.len(), 1, "{text:?}");
            assert!(found[0].message.contains("blank"), "{}", found[0].message);
            assert_eq!(found[0].fixable, Fixable::Agent);
        }
    }

    #[test]
    fn a_folder_named_agents_md_is_a_manual_finding_not_an_error() {
        let f = Fixture::new(&[("AGENTS.md/notes.txt", "x\n")], Track::All).unwrap();
        let found = check(&f);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].file, "AGENTS.md");
        assert_eq!(found[0].fixable, Fixable::Manual);
        assert!(found[0].message.contains("cannot be read"));
    }
}

mod fixed_repo {
    use super::*;

    #[test]
    fn passes_once_the_file_is_written() {
        let f = Fixture::new(&[("README.md", "x\n")], Track::All).unwrap();
        assert_eq!(check(&f).len(), 1);
        f.write("AGENTS.md", "# Agents\n\nTest with `cargo test`.\n")
            .unwrap();
        assert!(check(&f).is_empty());
    }

    #[test]
    fn fix_does_nothing() {
        let f = Fixture::new(&[("README.md", "x\n")], Track::All).unwrap();
        let result = f.fix(&AgentsMdExists, &Options::new()).unwrap();
        assert!(result.changed_files.is_empty());
        assert_eq!(f.read("AGENTS.md"), None);
    }
}
