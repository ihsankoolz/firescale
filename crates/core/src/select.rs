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
    use super::glob_match;

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
