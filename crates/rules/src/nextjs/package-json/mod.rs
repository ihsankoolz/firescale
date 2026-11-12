//! `nextjs/package-json`: package.json depends on next, react and react-dom
//! and has the dev, build, start and lint scripts.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;
use serde_json::{Map, Value};

use super::{
    create, depends_on, finding, new_package_json, package_json, package_json_with, SCRIPTS,
};

pub const ID: &str = "nextjs/package-json";

pub struct PackageJson;

/// The scripts package.json lacks. `lint` counts only once ESLint for
/// Next.js is a dependency: the script would fail without it.
fn missing_scripts(json: &Value) -> Map<String, Value> {
    let mut missing = Map::new();
    for (name, command) in SCRIPTS {
        if name == "lint" && !depends_on(json, "eslint-config-next") {
            continue;
        }
        if json
            .get("scripts")
            .and_then(|scripts| scripts.get(name))
            .is_none()
        {
            missing.insert(name.to_owned(), Value::from(command));
        }
    }
    missing
}

impl BuiltinRule for PackageJson {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::High,
            category: Category::Reliability,
            description: "package.json depends on next, react and react-dom, and has the dev, build, start and lint scripts.".to_owned(),
            inputs: vec!["package.json".to_owned()],
            has_fix: true,
            after: Vec::new(),
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        let json = match package_json(ctx)? {
            Ok(Some(json)) => json,
            Ok(None) => {
                return Ok(vec![finding(
                    "package.json",
                    "No package.json. `fscale sync` creates one for a Next.js app with next, react, react-dom, TypeScript and ESLint, and the dev, build, start and lint scripts.".to_owned(),
                    Fixable::Deterministic,
                )])
            }
            Err(()) => {
                return Ok(vec![finding(
                    "package.json",
                    "package.json is not valid JSON. Fix it by hand, then run `fscale sync`.".to_owned(),
                    Fixable::Manual,
                )])
            }
        };
        let mut findings = Vec::new();
        let missing: Vec<&str> = ["next", "react", "react-dom"]
            .into_iter()
            .filter(|package| !depends_on(&json, package))
            .collect();
        if !missing.is_empty() {
            findings.push(finding(
                "package.json",
                format!(
                    "package.json does not depend on {}. Install them with your package manager (for example `npm install {}`), so the versions match your lockfile.",
                    missing.join(", "),
                    missing.join(" ")
                ),
                Fixable::Agent,
            ));
        }
        let scripts = missing_scripts(&json);
        if !scripts.is_empty() {
            let names: Vec<&str> = scripts.keys().map(String::as_str).collect();
            let add = serde_json::json!({ "scripts": scripts });
            let fixable = if package_json_with(ctx, &add)?.is_some() {
                Fixable::Deterministic
            } else {
                Fixable::Manual
            };
            findings.push(finding(
                "package.json",
                format!(
                    "package.json has no {} script{} for Next.js. `fscale sync` adds {}: `next dev`, `next build`, `next start`, `eslint`.",
                    names.join(", "),
                    if names.len() == 1 { "" } else { "s" },
                    if names.len() == 1 { "it" } else { "them" }
                ),
                fixable,
            ));
        }
        Ok(findings)
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        let json = match package_json(ctx)? {
            Ok(Some(json)) => json,
            Ok(None) => {
                create(ctx, "package.json", &new_package_json(), &mut result)?;
                if !result.changed_files.is_empty() {
                    result.notes.push("package.json names Next.js and React; run your package manager's install (for example `npm install`) to fetch them.".to_owned());
                }
                return Ok(result);
            }
            Err(()) => return Ok(result),
        };
        let scripts = missing_scripts(&json);
        if scripts.is_empty() {
            return Ok(result);
        }
        if let Some(updated) = package_json_with(ctx, &serde_json::json!({ "scripts": scripts }))? {
            ctx.write("package.json", &updated)?;
            result.changed_files.push("package.json".to_owned());
        }
        Ok(result)
    }
}
