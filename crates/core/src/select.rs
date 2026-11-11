//! Picking the rules to run for `--rules <glob>`.

use crate::config::ConfiguredRule;
use crate::error::{Error, Result};
use crate::model::Status;

/// Matches a rule id against a glob. `*` matches within one id segment,
/// `**` matches across `/`. Everything else is literal.
#[must_use]
pub fn glob_match(glob: &str, id: &str) -> bool {
    fn matches(glob: &[char], id: &[char]) -> bool {
        match glob {
            [] => id.is_empty(),
            ['*', '*', rest @ ..] => (0..=id.len()).any(|skip| matches(rest, &id[skip..])),
            ['*', rest @ ..] => {
                let segment = id.iter().position(|c| *c == '/').unwrap_or(id.len());
                (0..=segment).any(|skip| matches(rest, &id[skip..]))
            }
            [literal, rest @ ..] => id.first() == Some(literal) && matches(rest, &id[1..]),
        }
    }
    let glob: Vec<char> = glob.chars().collect();
    let id: Vec<char> = id.chars().collect();
    matches(&glob, &id)
}

/// The rules to run, plus notes to show the user.
pub struct Selection<'a> {
    pub rules: Vec<ConfiguredRule<'a>>,
    pub notes: Vec<String>,
}

/// Picks the rules to run.
///
/// Without `--rules`: every rule that is not off. With it: rules matching one
/// of the comma-separated globs. A rule that is off still runs when `--rules`
/// names its id exactly; it then runs as report-only, with a note.
pub fn select_rules<'a>(
    rules: Vec<ConfiguredRule<'a>>,
    rules_glob: Option<&str>,
) -> Result<Selection<'a>> {
    let Some(rules_glob) = rules_glob else {
        let rules = rules
            .into_iter()
            .filter(|rule| rule.status != Status::Off)
            .collect();
        return Ok(Selection {
            rules,
            notes: Vec::new(),
        });
    };
    let globs: Vec<&str> = rules_glob
        .split(',')
        .map(str::trim)
        .filter(|glob| !glob.is_empty())
        .collect();
    if globs.is_empty() {
        return Err(Error::user(
            "--rules needs a rule id or glob, for example --rules \"core/*\".",
        ));
    }

    let known = if rules.is_empty() {
        "(none)".to_owned()
    } else {
        rules
            .iter()
            .map(|rule| rule.meta.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut selected = Vec::new();
    let mut notes = Vec::new();
    let mut off_matches = Vec::new();
    for mut rule in rules {
        if !globs.iter().any(|glob| glob_match(glob, &rule.meta.id)) {
            continue;
        }
        if rule.status == Status::Off {
            if globs.contains(&rule.meta.id.as_str()) {
                notes.push(format!(
                    "note: {} is off in your firescale config. Running it because --rules names it. Its violations are report-only.",
                    rule.meta.id
                ));
                rule.status = Status::Evaluated;
            } else {
                off_matches.push(rule.meta.id.clone());
                continue;
            }
        }
        selected.push(rule);
    }

    if selected.is_empty() {
        if !off_matches.is_empty() {
            return Err(Error::user(format!(
                "--rules \"{rules_glob}\" only matches rules that are off: {}. Name a rule's full id to run it anyway, or set it to \"evaluated\" or \"enforced\" in your firescale config.",
                off_matches.join(", ")
            )));
        }
        return Err(Error::user(format!(
            "--rules \"{rules_glob}\" matches no rule. Available rules: {known}."
        )));
    }
    Ok(Selection {
        rules: selected,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::{glob_match, select_rules};
    use crate::config::{ConfiguredRule, RuleSource};
    use crate::model::{Category, Finding, RuleMeta, Severity, Status};
    use crate::rule::{BuiltinRule, RuleContext};
    use crate::Result;

    struct StubRule;

    impl BuiltinRule for StubRule {
        fn meta(&self) -> RuleMeta {
            RuleMeta {
                id: "stub/stub".to_owned(),
                severity: Severity::Low,
                category: Category::Consistency,
                description: "stub".to_owned(),
                inputs: Vec::new(),
                has_fix: false,
            }
        }
        fn skill(&self) -> &'static str {
            ""
        }
        fn check(&self, _ctx: &RuleContext) -> Result<Vec<Finding>> {
            Ok(Vec::new())
        }
    }

    static STUB: StubRule = StubRule;

    fn rule(id: &str, status: Status) -> ConfiguredRule<'static> {
        let mut meta = STUB.meta();
        meta.id = id.to_owned();
        ConfiguredRule {
            meta,
            status,
            options: serde_json::Map::new(),
            source: RuleSource::Builtin(&STUB),
        }
    }

    fn sample() -> Vec<ConfiguredRule<'static>> {
        vec![
            rule("core/a-rule", Status::Evaluated),
            rule("core/b-rule", Status::Enforced),
            rule("core/c-off", Status::Off),
            rule("team/d-rule", Status::Evaluated),
            rule("team/e-off", Status::Off),
        ]
    }

    fn ids(rules: &[ConfiguredRule<'_>]) -> Vec<String> {
        rules.iter().map(|r| r.meta.id.clone()).collect()
    }

    fn error_of(glob: &str) -> String {
        select_rules(sample(), Some(glob))
            .err()
            .expect("selection should fail")
            .message()
            .to_owned()
    }

    #[test]
    fn without_a_glob_off_rules_are_dropped_and_there_are_no_notes() {
        let selection = select_rules(sample(), None).unwrap();
        assert_eq!(
            ids(&selection.rules),
            vec!["core/a-rule", "core/b-rule", "team/d-rule"]
        );
        assert!(selection.notes.is_empty());
    }

    #[test]
    fn a_glob_can_match_several_rules_and_keeps_their_status() {
        let selection = select_rules(sample(), Some("core/*")).unwrap();
        assert_eq!(ids(&selection.rules), vec!["core/a-rule", "core/b-rule"]);
        assert_eq!(selection.rules[0].status, Status::Evaluated);
        assert_eq!(selection.rules[1].status, Status::Enforced);
        assert!(selection.notes.is_empty());
    }

    #[test]
    fn comma_separated_globs_with_spaces_are_all_used() {
        let selection = select_rules(sample(), Some(" core/a-* , team/d-rule ,, ")).unwrap();
        assert_eq!(ids(&selection.rules), vec!["core/a-rule", "team/d-rule"]);
    }

    #[test]
    fn an_empty_rules_value_is_rejected() {
        for glob in ["", "  ", ",", " , "] {
            let message = error_of(glob);
            assert!(
                message.contains("--rules needs a rule id or glob"),
                "{message}"
            );
        }
    }

    #[test]
    fn no_match_is_an_error_listing_the_available_rules() {
        let message = error_of("nope/*");
        assert!(message.contains("matches no rule"), "{message}");
        assert!(
            message.contains(
                "Available rules: core/a-rule, core/b-rule, core/c-off, team/d-rule, team/e-off"
            ),
            "{message}"
        );
    }

    #[test]
    fn no_match_with_no_rules_says_none_are_available() {
        let message = select_rules(Vec::new(), Some("core/*"))
            .err()
            .expect("fails")
            .message()
            .to_owned();
        assert!(message.contains("Available rules: (none)"), "{message}");
    }

    #[test]
    fn a_glob_that_only_matches_off_rules_is_an_error() {
        let message = error_of("*/*-off");
        assert!(
            message.contains("only matches rules that are off"),
            "{message}"
        );
        assert!(message.contains("core/c-off"), "{message}");
        assert!(message.contains("team/e-off"), "{message}");
    }

    #[test]
    fn an_exact_id_of_an_off_rule_runs_it_as_evaluated_with_a_note() {
        let selection = select_rules(sample(), Some("core/c-off")).unwrap();
        assert_eq!(ids(&selection.rules), vec!["core/c-off"]);
        assert_eq!(selection.rules[0].status, Status::Evaluated);
        assert_eq!(selection.notes.len(), 1);
        assert!(
            selection.notes[0].contains("core/c-off"),
            "{:?}",
            selection.notes
        );
        assert!(
            selection.notes[0].contains("is off"),
            "{:?}",
            selection.notes
        );
    }

    #[test]
    fn an_exact_id_and_a_glob_together_select_both() {
        let selection = select_rules(sample(), Some("core/*, team/e-off")).unwrap();
        assert_eq!(
            ids(&selection.rules),
            vec!["core/a-rule", "core/b-rule", "team/e-off"]
        );
        // core/c-off matched only by the glob, so it stays out and gets no note.
        assert_eq!(selection.notes.len(), 1);
        assert!(selection.notes[0].contains("team/e-off"));
        assert_eq!(selection.rules[2].status, Status::Evaluated);
    }

    #[test]
    fn star_stays_inside_one_segment() {
        assert!(glob_match("core/*", "core/no-committed-env"));
        assert!(glob_match("*/no-*", "core/no-committed-env"));
        assert!(glob_match("core/no-committed-env", "core/no-committed-env"));
        assert!(!glob_match("*", "core/no-committed-env"));
        assert!(!glob_match("core", "core/no-committed-env"));
        assert!(!glob_match("core/*", "other/no-committed-env"));
    }

    #[test]
    fn double_star_crosses_segments() {
        assert!(glob_match("**", "core/no-committed-env"));
        assert!(glob_match("**/no-committed-env", "core/no-committed-env"));
        assert!(glob_match("c**env", "core/no-committed-env"));
        assert!(!glob_match("**x", "core/no-committed-env"));
    }

    #[test]
    fn other_characters_are_literal() {
        assert!(!glob_match(
            "core/no.committed-env",
            "core/no-committed-env"
        ));
        assert!(!glob_match(
            "core/no-committed-en?",
            "core/no-committed-env"
        ));
    }
}
