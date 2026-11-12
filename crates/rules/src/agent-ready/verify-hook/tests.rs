//! Tests for `agent-ready/verify-hook`.

use firescale_core::hooks::{install_hook, Agent};
use firescale_core::model::{Diagnostic, Fixable};
use firescale_core::rule::{BuiltinRule, Options};
use firescale_core::testing::{Fixture, Track};
use serde_json::json;

use super::{VerifyHook, ID};

fn options(value: serde_json::Value) -> Options {
    value.as_object().cloned().unwrap_or_default()
}

fn check(fixture: &Fixture, options: &Options) -> Vec<Diagnostic> {
    fixture.check(&VerifyHook, options).unwrap()
}

#[test]
fn meta_lists_the_three_hook_files_as_inputs() {
    let meta = VerifyHook.meta();
    assert_eq!(meta.id, ID);
    assert!(meta.has_fix);
    assert_eq!(
        meta.inputs,
        [
            ".claude/settings.json",
            ".codex/hooks.json",
            ".cursor/hooks.json"
        ]
    );
}

mod passing_repo {
    use super::*;

    #[test]
    fn any_one_agent_is_enough_by_default() {
        for agent in [Agent::Claude, Agent::Codex, Agent::Cursor] {
            let f = Fixture::new(&[], Track::None).unwrap();
            install_hook(f.root(), agent, "npx firescale").unwrap();
            assert!(check(&f, &Options::new()).is_empty(), "{}", agent.name());
        }
    }

    #[test]
    fn every_listed_agent_has_the_hook() {
        let f = Fixture::new(&[], Track::None).unwrap();
        install_hook(f.root(), Agent::Claude, "fscale").unwrap();
        install_hook(f.root(), Agent::Codex, "fscale").unwrap();
        assert!(check(
            &f,
            &options(json!({"agents": ["claude", "codex", "claude"]}))
        )
        .is_empty());
    }
}

mod failing_repo {
    use super::*;

    #[test]
    fn no_hook_anywhere_is_one_deterministic_finding() {
        let f = Fixture::new(&[("README.md", "x\n")], Track::All).unwrap();
        let found = check(&f, &Options::new());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].rule_id, ID);
        assert_eq!(found[0].file, ".claude/settings.json");
        assert_eq!(found[0].fixable, Fixable::Deterministic);
        assert!(found[0].message.contains("fscale sync"));
        assert!(found[0].message.contains("fscale hooks install --all"));
    }

    #[test]
    fn a_settings_file_the_hook_cannot_go_into_is_manual() {
        for text in ["{ nope", "{\"hooks\":{}}"] {
            let f = Fixture::new(&[(".claude/settings.json", text)], Track::All).unwrap();
            let found = check(&f, &Options::new());
            assert_eq!(found[0].fixable, Fixable::Manual, "{text}");
        }
    }

    #[test]
    fn settings_without_our_hook_or_with_broken_json_do_not_count() {
        for text in [
            "{\"hooks\":{\"Stop\":[{\"hooks\":[{\"type\":\"command\",\"command\":\"echo bye\"}]}]}}",
            "{ nope",
            "",
        ] {
            let f = Fixture::new(&[(".claude/settings.json", text)], Track::All).unwrap();
            assert_eq!(check(&f, &Options::new()).len(), 1, "{text:?}");
        }
    }

    #[test]
    fn each_listed_agent_without_the_hook_is_reported() {
        let f = Fixture::new(&[], Track::None).unwrap();
        install_hook(f.root(), Agent::Claude, "fscale").unwrap();
        let found = check(
            &f,
            &options(json!({"agents": ["claude", "codex", "cursor"]})),
        );
        let files: Vec<&str> = found.iter().map(|d| d.file.as_str()).collect();
        assert_eq!(files, [".codex/hooks.json", ".cursor/hooks.json"]);
        assert_eq!(found[0].fixable, Fixable::Deterministic);
        assert!(found[0].message.contains("`fscale sync` adds it"));
    }

    #[test]
    fn a_bad_agents_option_is_an_error_that_says_what_to_do() {
        let f = Fixture::new(&[], Track::None).unwrap();
        for bad in [json!("claude"), json!([]), json!(["vim"]), json!([1])] {
            let error = f
                .check(&VerifyHook, &options(json!({ "agents": bad })))
                .unwrap_err();
            assert!(
                error
                    .message()
                    .contains("option \"agents\" must be a list of agent names"),
                "{}",
                error.message()
            );
        }
    }
}

mod fixed_repo {
    use super::*;

    #[test]
    fn the_fix_sets_up_every_agent_with_npx_and_a_second_run_changes_nothing() {
        let f = Fixture::new(&[], Track::None).unwrap();
        assert_eq!(check(&f, &Options::new()).len(), 1);
        let result = f.fix(&VerifyHook, &Options::new()).unwrap();
        assert_eq!(
            result.changed_files,
            [
                ".claude/settings.json",
                ".codex/hooks.json",
                ".cursor/hooks.json"
            ]
        );
        assert!(f
            .read(".codex/hooks.json")
            .unwrap()
            .contains("\"npx firescale hooks run --agent codex\""));
        assert!(check(&f, &Options::new()).is_empty());
        let again = f.fix(&VerifyHook, &Options::new()).unwrap();
        assert!(again.changed_files.is_empty());
    }

    #[test]
    fn a_repo_script_named_fscale_starts_the_hook() {
        let f = Fixture::new(
            &[
                ("package.json", "{\"name\":\"x\",\"packageManager\":\"pnpm@9.0.0\",\"scripts\":{\"fscale\":\"fscale\"}}"),
                ("pnpm-lock.yaml", ""),
            ],
            Track::None,
        )
        .unwrap();
        f.fix(&VerifyHook, &Options::new()).unwrap();
        assert!(f
            .read(".cursor/hooks.json")
            .unwrap()
            .contains("\"pnpm --silent fscale hooks run --agent cursor\""));
    }

    #[test]
    fn existing_settings_keep_every_line() {
        let settings =
            "{\n    \"model\": \"opus\",\n    \"hooks\": {\n        \"PreToolUse\": []\n    }\n}\n";
        let f = Fixture::new(&[(".claude/settings.json", settings)], Track::None).unwrap();
        f.fix(&VerifyHook, &options(json!({"agents": ["claude"]})))
            .unwrap();
        let after = f.read(".claude/settings.json").unwrap();
        let mut lines = after.lines();
        for line in settings.lines() {
            assert!(lines.any(|candidate| candidate == line), "{line}: {after}");
        }
        assert!(check(&f, &options(json!({"agents": ["claude"]}))).is_empty());
    }

    #[test]
    fn a_hook_installed_by_hand_is_enough() {
        let f = Fixture::new(&[], Track::None).unwrap();
        install_hook(f.root(), Agent::Cursor, "fscale").unwrap();
        assert!(check(&f, &Options::new()).is_empty());
        assert!(f
            .fix(&VerifyHook, &Options::new())
            .unwrap()
            .changed_files
            .is_empty());
    }
}

#[test]
fn a_setting_of_another_type_is_left_for_a_person() {
    // Review of M13: the fix loop must not be told a fix will act when it cannot.
    let text = "{\n  \"hooks\": {\n    \"Stop\": \"none\"\n  }\n}\n";
    let f = Fixture::new(&[(".claude/settings.json", text)], Track::None).unwrap();
    let options = options(json!({"agents": ["claude"]}));
    let found = check(&f, &options);
    assert_eq!(found[0].fixable, Fixable::Manual);
    assert!(f
        .fix(&VerifyHook, &options)
        .unwrap()
        .changed_files
        .is_empty());
    assert_eq!(f.read(".claude/settings.json").unwrap(), text);
}
