//! `core/scripts-present`: standard scripts exist: lint, test and build.

use firescale_core::model::{Category, Finding, Fixable, RuleMeta, Severity};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::{Error, Result};
use serde_json::Value;

#[cfg(test)]
mod tests;

pub const ID: &str = "core/scripts-present";

const PACKAGE_JSON: &str = "package.json";
const DEFAULT_REQUIRED: [&str; 3] = ["lint", "test", "build"];

pub struct ScriptsPresent;

fn required(ctx: &RuleContext) -> Result<Vec<String>> {
    let Some(option) = ctx.options().get("required") else {
        return Ok(DEFAULT_REQUIRED.iter().map(|s| (*s).to_owned()).collect());
    };
    let names: Option<Vec<String>> = option.as_array().and_then(|items| {
        items
            .iter()
            .map(|item| match item {
                Value::String(name) if !name.trim().is_empty() => Some(name.clone()),
                _ => None,
            })
            .collect()
    });
    names.ok_or_else(|| {
        Error::user(
            "core/scripts-present: option \"required\" must be a list of script names, for example [\"lint\", \"test\", \"build\"]. Fix it in your firescale config.",
        )
    })
}

/// True when the script is missing, empty, or npm's default "no test specified" placeholder.
fn is_missing(scripts: Option<&Value>, name: &str) -> bool {
    match scripts.and_then(|s| s.get(name)).and_then(Value::as_str) {
        None => true,
        Some(command) => {
            let command = command.trim();
            command.is_empty() || (name == "test" && command.contains("no test specified"))
        }
    }
}

impl BuiltinRule for ScriptsPresent {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Consistency,
            description: "Standard scripts exist: lint, test and build.".to_owned(),
            inputs: vec![PACKAGE_JSON.to_owned()],
            has_fix: false,
            after: Vec::new(),
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        let required = required(ctx)?;
        let Some(text) = ctx.read(PACKAGE_JSON)? else {
            return Ok(Vec::new());
        };
        let Ok(package) = serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')) else {
            return Ok(vec![Finding {
                message: "package.json is not valid JSON. Fix the syntax error, then run the check again."
                    .to_owned(),
                file: PACKAGE_JSON.to_owned(),
                range: None,
                project: None,
                fixable: Fixable::Manual,
            }]);
        };
        let shape_problem = if !package.is_object() {
            Some("the top level is not an object")
        } else if package.get("scripts").is_some_and(|s| !s.is_object()) {
            Some("\"scripts\" is not an object")
        } else {
            None
        };
        if let Some(problem) = shape_problem {
            return Ok(vec![Finding {
                message: format!(
                    "package.json has an unexpected shape: {problem}. Expected a JSON object with a \"scripts\" object that maps script names to commands, for example {{\"scripts\": {{\"test\": \"vitest run\"}}}}."
                ),
                file: PACKAGE_JSON.to_owned(),
                range: None,
                project: None,
                fixable: Fixable::Manual,
            }]);
        }
        let scripts = package.get("scripts");
        Ok(required
            .into_iter()
            .filter(|name| is_missing(scripts, name))
            .map(|name| Finding {
                message: format!(
                    "package.json has no \"{name}\" script. Add a \"{name}\" entry to \"scripts\" that runs this project's {name} command."
                ),
                file: PACKAGE_JSON.to_owned(),
                range: None,
                project: None,
                fixable: Fixable::Agent,
            })
            .collect())
    }
}
