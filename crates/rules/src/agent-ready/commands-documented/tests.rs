//! Tests for `agent-ready/commands-documented`.

use firescale_core::model::{Diagnostic, Fixable, Severity};
use firescale_core::rule::{BuiltinRule, Options};
use firescale_core::testing::{Fixture, Track};

use super::{forms, mentions, CommandsDocumented, ID};

const PNPM: &str = "{\"packageManager\":\"pnpm@9.1.0\",\"scripts\":{\"build\":\"tsc\",\"test\":\"vitest run\",\"lint\":\"eslint .\",\"typecheck\":\"tsc --noEmit\"}}\n";
const NPM: &str = "{\"scripts\":{\"test\":\"jest\",\"lint\":\"eslint .\"}}\n";

fn check(fixture: &Fixture) -> Vec<Diagnostic> {
    fixture.check(&CommandsDocumented, &Options::new()).unwrap()
}

#[test]
fn meta_has_no_fix_and_covers_what_it_reads() {
    let meta = CommandsDocumented.meta();
    assert_eq!(meta.id, ID);
    assert_eq!(meta.severity, Severity::Low);
    assert!(!meta.has_fix);
    for input in ["AGENTS.md", "package.json", "pnpm-lock.yaml"] {
        assert!(meta.inputs.contains(&input.to_owned()), "{input}");
    }
}

#[test]
fn short_forms_follow_each_package_manager() {
    assert_eq!(forms("pnpm run lint"), ["pnpm run lint", "pnpm lint"]);
    assert_eq!(forms("npm run test"), ["npm run test", "npm test"]);
    // `npm lint` is not a command, so only the long form counts.
    assert_eq!(forms("npm run lint"), ["npm run lint"]);
    assert_eq!(forms("make all"), ["make all"]);
}

#[test]
fn a_mention_is_a_whole_command() {
    assert!(mentions("Run `pnpm test`.", "pnpm test"));
    assert!(mentions("pnpm test", "pnpm test"));
    assert!(mentions("- Test: pnpm test, then push", "pnpm test"));
    assert!(!mentions("Run `pnpm test:unit`.", "pnpm test"));
    assert!(!mentions("Run `pnpm test-all`.", "pnpm test"));
    assert!(!mentions("Run `xpnpm test`.", "pnpm test"));
    assert!(mentions("`pnpm test:unit` then `pnpm test`", "pnpm test"));
}

mod passing_repo {
    use super::*;

    #[test]
    fn short_and_long_forms_both_count() {
        let agents = "# Agents\n\n- Build: `pnpm build`\n- Test: `pnpm run test`\n- Lint: `pnpm lint`\n- Types: `pnpm typecheck`\n";
        let f = Fixture::new(&[("package.json", PNPM), ("AGENTS.md", agents)], Track::All).unwrap();
        assert!(check(&f).is_empty());
    }

    #[test]
    fn silent_without_agents_md() {
        let f = Fixture::new(&[("package.json", PNPM)], Track::All).unwrap();
        assert!(check(&f).is_empty());
    }

    #[test]
    fn silent_when_no_commands_are_detected() {
        for files in [
            vec![("AGENTS.md", "# Agents\n")],
            vec![
                ("AGENTS.md", "# Agents\n"),
                ("Cargo.toml", "[package]\nname = \"x\"\n"),
            ],
            vec![("AGENTS.md", "# Agents\n"), ("package.json", "{ not json")],
        ] {
            let f = Fixture::new(&files, Track::NoGit).unwrap();
            assert!(check(&f).is_empty());
        }
    }

    #[test]
    fn crlf_and_a_byte_order_mark_do_not_matter() {
        let agents = "\u{feff}# Agents\r\n\r\nRun `npm test` and `npm run lint`.\r\n";
        let f = Fixture::new(&[("package.json", NPM), ("AGENTS.md", agents)], Track::All).unwrap();
        assert!(check(&f).is_empty());
    }
}

mod failing_repo {
    use super::*;

    #[test]
    fn one_agent_fixable_finding_per_undocumented_command() {
        let agents = "# Agents\n\nBuild with `pnpm build`. Unit tests: `pnpm test:unit`.\n";
        let f = Fixture::new(&[("package.json", PNPM), ("AGENTS.md", agents)], Track::All).unwrap();
        let found = check(&f);
        assert_eq!(found.len(), 3);
        assert!(found
            .iter()
            .all(|d| d.rule_id == ID && d.file == "AGENTS.md" && d.fixable == Fixable::Agent));
        let messages: Vec<&str> = found.iter().map(|d| d.message.as_str()).collect();
        assert!(messages
            .iter()
            .any(|m| m.contains("the test command `pnpm run test`")));
        assert!(messages
            .iter()
            .any(|m| m.contains("the lint command `pnpm run lint`")));
        assert!(messages
            .iter()
            .any(|m| m.contains("the typecheck command `pnpm run typecheck`")));
    }

    #[test]
    fn npm_lint_is_not_accepted_as_npm_run_lint() {
        let agents = "Run `npm test` and `npm lint`.\n";
        let f = Fixture::new(&[("package.json", NPM), ("AGENTS.md", agents)], Track::All).unwrap();
        let found = check(&f);
        assert_eq!(found.len(), 1);
        assert!(found[0].message.contains("`npm run lint`"));
    }

    #[test]
    fn a_watching_test_script_asks_for_its_run_once_sibling() {
        // `npm test` would start vitest in watch mode; facts names test:run.
        let package = "{\"scripts\":{\"test\":\"vitest\",\"test:run\":\"vitest run\"}}\n";
        let agents = "Run `npm test`.\n";
        let f = Fixture::new(
            &[("package.json", package), ("AGENTS.md", agents)],
            Track::All,
        )
        .unwrap();
        let found = check(&f);
        assert_eq!(found.len(), 1);
        assert!(found[0]
            .message
            .contains("the test command `npm run test:run`"));
        f.write("AGENTS.md", "Run `npm run test:run`.\n").unwrap();
        assert!(check(&f).is_empty());
    }
}

mod fixed_repo {
    use super::*;

    #[test]
    fn passes_once_the_commands_are_written_down() {
        let f = Fixture::new(
            &[("package.json", NPM), ("AGENTS.md", "# Agents\n")],
            Track::All,
        )
        .unwrap();
        assert_eq!(check(&f).len(), 2);
        f.write("AGENTS.md", "# Agents\n\n- `npm test`\n- `npm run lint`\n")
            .unwrap();
        assert!(check(&f).is_empty());
    }

    #[test]
    fn fix_does_nothing() {
        let f = Fixture::new(
            &[("package.json", NPM), ("AGENTS.md", "# Agents\n")],
            Track::All,
        )
        .unwrap();
        let result = f.fix(&CommandsDocumented, &Options::new()).unwrap();
        assert!(result.changed_files.is_empty());
        assert_eq!(f.read("AGENTS.md").as_deref(), Some("# Agents\n"));
    }
}
