//! Tests for `agent-ready/agent-permissions`.

use firescale_core::model::{Category, Diagnostic, Fixable};
use firescale_core::rule::{BuiltinRule, Options};
use firescale_core::testing::{Fixture, Track};
use serde_json::Value;

use super::{codex_config, AgentPermissions, CLAUDE_SETTINGS, CODEX_CONFIG, CURSOR_CLI, ID};

fn check(fixture: &Fixture) -> Vec<Diagnostic> {
    fixture.check(&AgentPermissions, &Options::new()).unwrap()
}

fn fix(fixture: &Fixture) -> Vec<String> {
    fixture
        .fix(&AgentPermissions, &Options::new())
        .unwrap()
        .changed_files
}

fn every_line_kept(before: &str, after: &str) -> bool {
    let mut lines = after.lines();
    before
        .lines()
        .all(|line| lines.any(|candidate| candidate == line))
}

#[test]
fn meta_matches_the_spec() {
    let meta = AgentPermissions.meta();
    assert_eq!(meta.id, ID);
    assert_eq!(meta.category, Category::Security);
    assert!(meta.has_fix);
    assert!(meta.inputs.contains(&CLAUDE_SETTINGS.to_owned()));
}

mod failing_repo {
    use super::*;

    #[test]
    fn an_empty_repo_misses_all_three_files() {
        let f = Fixture::new(&[], Track::None).unwrap();
        let files: Vec<String> = check(&f).into_iter().map(|d| d.file).collect();
        assert_eq!(files, [CLAUDE_SETTINGS, CODEX_CONFIG, CURSOR_CLI]);
    }

    #[test]
    fn a_codex_config_of_your_own_is_never_edited() {
        let f = Fixture::new(&[(CODEX_CONFIG, "model = \"o4\"\n")], Track::None).unwrap();
        let codex: Vec<Diagnostic> = check(&f)
            .into_iter()
            .filter(|d| d.file == CODEX_CONFIG)
            .collect();
        assert_eq!(codex[0].fixable, Fixable::Manual);
        fix(&f);
        assert_eq!(f.read(CODEX_CONFIG).unwrap(), "model = \"o4\"\n");
    }

    #[test]
    fn a_list_on_one_line_is_left_for_a_person() {
        let text = "{\n  \"permissions\": { \"deny\": [] }\n}\n";
        let f = Fixture::new(&[(CLAUDE_SETTINGS, text)], Track::None).unwrap();
        let claude: Vec<Diagnostic> = check(&f)
            .into_iter()
            .filter(|d| d.file == CLAUDE_SETTINGS)
            .collect();
        assert_eq!(claude[0].fixable, Fixable::Manual);
        fix(&f);
        assert_eq!(f.read(CLAUDE_SETTINGS).unwrap(), text);
    }
}

mod fixed_repo {
    use super::*;

    #[test]
    fn the_fix_writes_safe_settings_and_a_second_run_changes_nothing() {
        let f = Fixture::new(
            &[(
                "package.json",
                "{\"name\":\"x\",\"scripts\":{\"test\":\"vitest run\",\"lint\":\"eslint .\"}}",
            )],
            Track::None,
        )
        .unwrap();
        assert_eq!(fix(&f), [CLAUDE_SETTINGS, CODEX_CONFIG, CURSOR_CLI]);
        let claude: Value = serde_json::from_str(&f.read(CLAUDE_SETTINGS).unwrap()).unwrap();
        let deny = claude["permissions"]["deny"].as_array().unwrap();
        assert!(deny.contains(&Value::from("Read(**/.env)")));
        assert!(!deny
            .iter()
            .any(|d| d.as_str().unwrap_or("").contains("example")));
        let allow = claude["permissions"]["allow"].as_array().unwrap();
        assert!(allow.contains(&Value::from("Bash(npx firescale verify *)")));
        assert!(allow.contains(&Value::from("Bash(npm run test *)")));
        assert_eq!(f.read(CODEX_CONFIG).unwrap(), codex_config());
        assert!(codex_config().contains("\"**/.env\" = \"deny\""));
        assert!(check(&f).is_empty());
        assert!(fix(&f).is_empty());
    }

    #[test]
    fn existing_settings_keep_every_line_and_every_setting() {
        let settings = "{\n  \"model\": \"opus\",\n  \"permissions\": {\n    \"allow\": [\n      \"Bash(make test)\"\n    ]\n  },\n  \"env\": { \"A\": \"1\" }\n}\n";
        let f = Fixture::new(&[(CLAUDE_SETTINGS, settings)], Track::None).unwrap();
        fix(&f);
        let after = f.read(CLAUDE_SETTINGS).unwrap();
        assert!(every_line_kept(settings, &after), "{after}");
        let json: Value = serde_json::from_str(&after).unwrap();
        assert_eq!(json["model"], "opus");
        assert_eq!(json["env"]["A"], "1");
        assert!(json["permissions"]["allow"]
            .as_array()
            .unwrap()
            .contains(&Value::from("Bash(make test)")));
        assert!(check(&f).is_empty());
    }
}
