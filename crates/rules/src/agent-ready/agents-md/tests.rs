//! Tests for `agent-ready/agents-md`.

use firescale_core::model::{Category, Diagnostic, Fixable, Severity};
use firescale_core::rule::{BuiltinRule, Options};
use firescale_core::testing::{Fixture, Track};

use super::{AgentsMdExists, CLAUDE_MD, ID};

fn check(fixture: &Fixture) -> Vec<Diagnostic> {
    fixture.check(&AgentsMdExists, &Options::new()).unwrap()
}

fn fix(fixture: &Fixture) -> Vec<String> {
    fixture
        .fix(&AgentsMdExists, &Options::new())
        .unwrap()
        .changed_files
}

const PACKAGE: &str =
    "{\"name\":\"shop\",\"scripts\":{\"build\":\"vite build\",\"test\":\"vitest run\",\"lint\":\"eslint .\"}}\n";

#[test]
fn meta_matches_the_spec() {
    let meta = AgentsMdExists.meta();
    assert_eq!(meta.id, ID);
    assert_eq!(meta.severity, Severity::Medium);
    assert_eq!(meta.category, Category::Consistency);
    assert!(meta.has_fix);
    for input in [
        "AGENTS.md",
        "CLAUDE.md",
        "package.json",
        "**/pyproject.toml",
    ] {
        assert!(meta.inputs.contains(&input.to_owned()), "{input}");
    }
}

mod passing_repo {
    use super::*;

    #[test]
    fn a_file_with_a_current_block_passes() {
        let f = Fixture::new(&[("package.json", PACKAGE)], Track::None).unwrap();
        fix(&f);
        assert!(check(&f).is_empty());
    }

    #[test]
    fn a_claude_md_that_imports_agents_md_itself_passes() {
        let f = Fixture::new(
            &[
                ("package.json", PACKAGE),
                (CLAUDE_MD, "# Notes\n\n@AGENTS.md\n"),
            ],
            Track::None,
        )
        .unwrap();
        fix(&f);
        assert_eq!(f.read(CLAUDE_MD).unwrap(), "# Notes\n\n@AGENTS.md\n");
        assert!(check(&f).is_empty());
    }
}

mod failing_repo {
    use super::*;

    #[test]
    fn a_missing_file_is_a_deterministic_finding() {
        let f = Fixture::new(&[("README.md", "x\n")], Track::All).unwrap();
        let found = check(&f);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].file, "AGENTS.md");
        assert_eq!(found[0].fixable, Fixable::Deterministic);
    }

    #[test]
    fn a_file_without_the_block_is_reported() {
        let f = Fixture::new(
            &[("AGENTS.md", "# Agents\n\nRun `make test`.\n")],
            Track::None,
        )
        .unwrap();
        let found = check(&f);
        assert_eq!(found.len(), 1);
        assert!(found[0].message.contains("has no firescale block"));
        assert_eq!(found[0].fixable, Fixable::Deterministic);
    }

    #[test]
    fn a_hand_edit_inside_the_block_is_reported() {
        let f = Fixture::new(&[("package.json", PACKAGE)], Track::None).unwrap();
        fix(&f);
        let text = f
            .read("AGENTS.md")
            .unwrap()
            .replace("- Test: `npm run test`", "- Test: `npm test --watch`");
        std::fs::write(f.root().join("AGENTS.md"), text).unwrap();
        let found = check(&f);
        assert_eq!(found.len(), 1);
        assert!(found[0].message.contains("out of date"));
    }

    #[test]
    fn a_damaged_block_is_left_for_a_person() {
        let text = "# Agents\n<!-- BEGIN firescale: x -->\nno end line\n";
        let f = Fixture::new(&[("AGENTS.md", text)], Track::None).unwrap();
        let found = check(&f);
        assert_eq!(found[0].fixable, Fixable::Manual);
        assert!(fix(&f).is_empty());
        assert_eq!(f.read("AGENTS.md").unwrap(), text);
    }

    #[test]
    fn a_folder_named_agents_md_is_manual() {
        let f = Fixture::new(&[("AGENTS.md/inner.txt", "x")], Track::NoGit).unwrap();
        let found = check(&f);
        assert_eq!(found[0].fixable, Fixable::Manual);
    }

    #[test]
    fn a_claude_md_without_the_import_is_reported() {
        let f = Fixture::new(&[(CLAUDE_MD, "# Ours\n")], Track::None).unwrap();
        let files: Vec<String> = check(&f).into_iter().map(|d| d.file).collect();
        assert_eq!(files, ["AGENTS.md", "CLAUDE.md"]);
    }
}

mod fixed_repo {
    use super::*;

    #[test]
    fn the_block_lists_the_commands_and_the_definition_of_done() {
        let f = Fixture::new(
            &[("package.json", PACKAGE), ("package-lock.json", "{}")],
            Track::All,
        )
        .unwrap();
        assert_eq!(fix(&f), ["AGENTS.md"]);
        let text = f.read("AGENTS.md").unwrap();
        for needle in [
            "<!-- BEGIN firescale",
            "- Build: `npm run build`",
            "- Test: `npm run test`",
            "- Lint: `npm run lint`",
            "run `npx firescale verify` from the repo root",
            "<!-- END firescale -->",
        ] {
            assert!(text.contains(needle), "{needle}: {text}");
        }
        assert!(check(&f).is_empty());
    }

    #[test]
    fn text_outside_the_block_survives_byte_for_byte() {
        let before = "\u{feff}# Agents\r\n\r\nNever touch `legacy/`.\r\n<!-- firescale:managed -->look-alike<!-- /firescale:managed -->\r\n\tTabs   \r\nlast line";
        let f = Fixture::new(
            &[("AGENTS.md", before), ("package.json", PACKAGE)],
            Track::None,
        )
        .unwrap();
        fix(&f);
        let after = std::fs::read_to_string(f.root().join("AGENTS.md")).unwrap();
        assert!(after.starts_with(before), "{after:?}");
        assert!(
            !after.replace("\r\n", "").contains('\n'),
            "line endings kept: {after:?}"
        );
        // A hand edit inside the block is repaired, and only the block changes.
        let edited = after.replace("- Test: `npm run test`", "- Test: `whatever`");
        std::fs::write(f.root().join("AGENTS.md"), &edited).unwrap();
        fix(&f);
        assert_eq!(
            std::fs::read_to_string(f.root().join("AGENTS.md")).unwrap(),
            after
        );
    }

    #[test]
    fn claude_md_gets_a_small_import_block_at_its_end() {
        let claude = "# Big project notes\n\nLine two, no final newline";
        let f = Fixture::new(&[(CLAUDE_MD, claude)], Track::None).unwrap();
        assert_eq!(fix(&f), ["AGENTS.md", "CLAUDE.md"]);
        let after = f.read(CLAUDE_MD).unwrap();
        assert!(after.starts_with(claude));
        assert!(
            after.ends_with("\n@AGENTS.md\n\n<!-- END firescale -->\n"),
            "{after}"
        );
        assert!(check(&f).is_empty());
    }

    #[test]
    fn fix_twice_changes_nothing_the_second_time() {
        let f = Fixture::new(
            &[("package.json", PACKAGE), (CLAUDE_MD, "# x\n")],
            Track::None,
        )
        .unwrap();
        assert!(!fix(&f).is_empty());
        assert!(fix(&f).is_empty());
    }

    #[test]
    fn a_python_service_is_listed_with_python_dash_m_on_every_machine() {
        let f = Fixture::new(
            &[
                ("backend/api/requirements.txt", "pytest\n"),
                ("backend/api/pytest.ini", "[pytest]\n"),
                ("backend/api/main.py", ""),
            ],
            Track::None,
        )
        .unwrap();
        fix(&f);
        let text = f.read("AGENTS.md").unwrap();
        assert!(text.contains("### `backend/api`"), "{text}");
        assert!(text.contains("- Test: `python -m pytest`"), "{text}");
        assert!(
            !text.contains(".venv/") && !text.contains(".venv\\"),
            "{text}"
        );
    }
}
