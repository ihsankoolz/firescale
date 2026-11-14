//! `expo/package-json`: package.json depends on expo, react and
//! react-native and has the start, android and ios scripts (web with
//! react-native-web, lint with eslint-config-expo).

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;
use serde_json::{Map, Value};

use super::{create, depends_on, finding, json_with, new_package_json, package_json, SCRIPTS};

#[cfg(test)]
mod tests;

pub const ID: &str = "expo/package-json";

pub struct PackageJson;

/// The scripts package.json lacks. `lint` counts only once ESLint for Expo
/// is a dependency: the script would install it otherwise.
fn missing_scripts(json: &Value) -> Map<String, Value> {
    let mut missing = Map::new();
    for (name, command) in SCRIPTS {
        if name == "lint" && !depends_on(json, "eslint-config-expo") {
            continue;
        }
        // `web` only for an app that runs on the web (react-native-web):
        // many apps are phone or TV only.
        if name == "web" && !depends_on(json, "react-native-web") {
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
            description: "package.json depends on expo, react and react-native, and has the start, android and ios scripts (web with react-native-web, lint with eslint-config-expo).".to_owned(),
            inputs: super::inputs(&[]),
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
                    "No package.json. `fscale sync` creates one for an Expo app (SDK 58) with expo, react, react-native, TypeScript and ESLint, and the start, android, ios, web and lint scripts.".to_owned(),
                    Fixable::Deterministic,
                )])
            }
            Err(()) => {
                return Ok(vec![finding(
                    "package.json",
                    "package.json is not valid JSON. Fix it by hand, then run `fscale sync`."
                        .to_owned(),
                    Fixable::Manual,
                )])
            }
        };
        if super::workspace_without_app(ctx, &json) {
            return Ok(vec![finding(
                "package.json",
                "The repo root is a workspace with no Expo app of its own (no app config or entry here). The expo pack checks an app at the root: run fscale in the app's folder, or turn the pack off here.".to_owned(),
                Fixable::Manual,
            )]);
        }
        if !depends_on(&json, "expo") {
            return Ok(vec![finding(
                "package.json",
                "package.json at the repo root does not depend on expo. The expo pack checks an Expo app at the repo root; if the app is in a subfolder (a workspace), run fscale there or turn the pack off. If this is meant to be the app, install Expo (`npx expo install expo react react-native`).".to_owned(),
                Fixable::Manual,
            )]);
        }
        let mut findings = Vec::new();
        let missing: Vec<&str> = ["react", "react-native"]
            .into_iter()
            .filter(|package| !depends_on(&json, package))
            .collect();
        if !missing.is_empty() {
            findings.push(finding(
                "package.json",
                format!(
                    "package.json does not depend on {}. Install the versions your SDK expects with `npx expo install {}`.",
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
            let fixable = if json_with(ctx, "package.json", &add)?.is_some() {
                Fixable::Deterministic
            } else {
                Fixable::Manual
            };
            findings.push(finding(
                "package.json",
                format!(
                    "package.json has no {} script{} for Expo. `fscale sync` adds {} (`expo start`, `expo start --android`, `--ios`, `--web`, `expo lint`).",
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
                    result.notes.push("package.json names Expo SDK 58 and its React Native; run your package manager's install (for example `npm install`) to fetch them.".to_owned());
                }
                return Ok(result);
            }
            Err(()) => return Ok(result),
        };
        if !depends_on(&json, "expo") || super::workspace_without_app(ctx, &json) {
            return Ok(result);
        }
        let scripts = missing_scripts(&json);
        if scripts.is_empty() {
            return Ok(result);
        }
        if let Some(updated) = json_with(
            ctx,
            "package.json",
            &serde_json::json!({ "scripts": scripts }),
        )? {
            ctx.write("package.json", &updated)?;
            result.changed_files.push("package.json".to_owned());
        }
        Ok(result)
    }
}
