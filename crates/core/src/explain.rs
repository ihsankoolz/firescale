//! `fscale explain <rule>`: a rule's metadata followed by its SKILL.md body.

use std::fs;

use crate::config::{ConfiguredRule, ResolvedConfig, RuleSource};
use crate::error::{Error, Result};

/// Finds a rule by full id, or by its short name when that is unambiguous.
pub fn find_rule<'r, 'a>(
    config: &'r ResolvedConfig<'a>,
    query: &str,
) -> Result<&'r ConfiguredRule<'a>> {
    if let Some(exact) = config.rules.iter().find(|rule| rule.meta.id == query) {
        return Ok(exact);
    }
    let by_short_name: Vec<&ConfiguredRule<'a>> = config
        .rules
        .iter()
        .filter(|rule| rule.meta.id.split('/').nth(1) == Some(query))
        .collect();
    if let [only] = by_short_name.as_slice() {
        return Ok(only);
    }
    let known = if config.rules.is_empty() {
        "(none)".to_owned()
    } else {
        config
            .rules
            .iter()
            .map(|rule| rule.meta.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    Err(Error::user(format!(
        "Unknown rule \"{query}\". Available rules: {known}."
    )))
}

/// Returns a SKILL.md without its frontmatter, with LF line endings.
#[must_use]
pub fn skill_body(text: &str) -> String {
    let normalized = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let body = normalized
        .strip_prefix("---\n")
        .and_then(|rest| rest.find("\n---").map(|end| &rest[end + "\n---".len()..]))
        .map(|rest| rest.strip_prefix('\n').unwrap_or(rest));
    body.unwrap_or(&normalized).trim().to_owned()
}

/// Builds the text for `fscale explain`.
/// A rule's SKILL.md, as written.
///
/// # Errors
///
/// When a custom rule's SKILL.md cannot be read.
pub fn rule_skill(rule: &ConfiguredRule<'_>) -> Result<String> {
    match &rule.source {
        RuleSource::Builtin(builtin) => Ok(builtin.skill().to_owned()),
        RuleSource::Custom { skill_file, .. } => fs::read_to_string(skill_file).map_err(|error| {
            Error::user(format!(
                "Could not read the SKILL.md for \"{}\" at {} ({error}). Every rule needs a SKILL.md next to its rule file.",
                rule.meta.id,
                skill_file.display()
            ))
        }),
    }
}

pub fn explain_rule(rule: &ConfiguredRule<'_>) -> Result<String> {
    let meta = &rule.meta;
    let skill = match &rule.source {
        RuleSource::Builtin(builtin) => builtin.skill().to_owned(),
        RuleSource::Custom { skill_file, .. } => fs::read_to_string(skill_file).map_err(|error| {
            Error::user(format!(
                "Could not read the SKILL.md for \"{}\" at {} ({error}). Every rule needs a SKILL.md next to its rule file.",
                meta.id,
                skill_file.display()
            ))
        })?,
    };
    let fix = if meta.has_fix {
        "deterministic fix available"
    } else {
        "no deterministic fix"
    };
    Ok(format!(
        "Rule:        {}\nSeverity:    {}\nCategory:    {}\nStatus:      {}\nFix:         {fix}\nDescription: {}\n\n{}\n",
        meta.id,
        meta.severity.as_str(),
        meta.category.as_str(),
        rule.status.as_str(),
        meta.description,
        skill_body(&skill)
    ))
}

#[cfg(test)]
mod tests {
    use super::skill_body;

    #[test]
    fn strips_frontmatter_bom_and_crlf() {
        let text = "\u{feff}---\r\nname: x\r\ndescription: y\r\n---\r\n\r\n# Title\r\n\r\nBody\r\n";
        assert_eq!(skill_body(text), "# Title\n\nBody");
    }

    #[test]
    fn keeps_text_without_frontmatter() {
        assert_eq!(skill_body("# Title\n\nBody\n"), "# Title\n\nBody");
    }
}
