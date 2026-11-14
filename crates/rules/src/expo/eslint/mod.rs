//! `expo/eslint`: ESLint is set up with Expo's config, so `expo lint` runs
//! without installing anything or writing files first.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{any_exists, applies, create, depends_on, finding, package_json};

#[cfg(test)]
mod tests;

pub const ID: &str = "expo/eslint";

pub struct EslintConfig;

/// The config files `expo lint` looks for (SDK 58, `ESlintPrerequisite`).
pub const NAMES: [&str; 12] = [
    // ESLint 8 reads it, though `expo lint` does not list it.
    ".eslintrc",
    "eslint.config.js",
    "eslint.config.mjs",
    "eslint.config.cjs",
    "eslint.config.ts",
    "eslint.config.mts",
    "eslint.config.cts",
    ".eslintrc.js",
    ".eslintrc.cjs",
    ".eslintrc.yaml",
    ".eslintrc.yml",
    ".eslintrc.json",
];

/// What `npx expo lint` writes (`@expo/cli/static/template/eslint.config.js`,
/// SDK 58), exactly.
pub const CONFIG: &str = "// https://docs.expo.dev/guides/using-eslint/\nconst { defineConfig } = require('eslint/config');\nconst expoConfig = require('eslint-config-expo/flat');\n\nmodule.exports = defineConfig([\n  expoConfig,\n  {\n    ignores: ['dist/*'],\n  },\n]);\n";

/// True when the app lints already: an ESLint config, Biome's config, or a
/// `lint` script that runs something other than ESLint (review on a real
/// app that lints with Biome).
fn configured(ctx: &RuleContext, json: &serde_json::Value) -> bool {
    let other_lint = json
        .get("scripts")
        .and_then(|s| s.get("lint"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|script| !script.contains("expo lint") && !script.contains("eslint"));
    any_exists(ctx, &NAMES)
        || any_exists(ctx, &["biome.json", "biome.jsonc"])
        || json.get("eslintConfig").is_some()
        || other_lint
}

/// The version a dependency is declared with, as (major, minor).
fn declared(json: &serde_json::Value, package: &str) -> Option<(u64, u64)> {
    let spec = ["dependencies", "devDependencies"]
        .iter()
        .find_map(|field| json.get(field)?.get(package)?.as_str())?;
    let rest = spec.trim_start_matches(|c: char| !c.is_ascii_digit());
    let mut parts = rest.split(|c: char| !c.is_ascii_digit());
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().and_then(|m| m.parse().ok()).unwrap_or(0);
    Some((major, minor))
}

/// True when the config `expo lint` writes can load: ESLint 9.22 or later
/// (`eslint/config`'s `defineConfig`) and an eslint-config-expo with the
/// flat config (9 or later; SDK-numbered versions such as 58 too). A
/// declaration that is not a version (a tag, a URL) is trusted.
fn flat_ready(json: &serde_json::Value) -> bool {
    if !depends_on(json, "eslint") || !depends_on(json, "eslint-config-expo") {
        return false;
    }
    declared(json, "eslint").is_none_or(|v| v >= (9, 22))
        && declared(json, "eslint-config-expo").is_none_or(|(major, _)| major >= 9)
}

impl BuiltinRule for EslintConfig {
    fn meta(&self) -> RuleMeta {
        let mut inputs: Vec<String> = NAMES.iter().map(|name| (*name).to_owned()).collect();
        inputs.extend([
            "package.json".to_owned(),
            "biome.json".to_owned(),
            "biome.jsonc".to_owned(),
        ]);
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Consistency,
            description: "ESLint is set up with eslint-config-expo, so `expo lint` runs as it is."
                .to_owned(),
            inputs: super::inputs(&inputs.iter().map(String::as_str).collect::<Vec<_>>()),
            has_fix: true,
            after: vec![super::package_json::ID.to_owned()],
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        if !applies(ctx)? {
            return Ok(Vec::new());
        }
        // With no package.json yet, expo/package-json writes one with
        // ESLint, and the next pass of the fix loop comes back here.
        let Ok(Some(json)) = package_json(ctx)? else {
            return Ok(Vec::new());
        };
        if configured(ctx, &json) {
            return Ok(Vec::new());
        }
        Ok(vec![if flat_ready(&json) {
            finding(
                    "eslint.config.js",
                    "No ESLint config. `fscale sync` creates eslint.config.js as `npx expo lint` writes it, with eslint-config-expo's flat config.".to_owned(),
                    Fixable::Deterministic,
                )
        } else {
            finding(
                    "eslint.config.js",
                    "No ESLint config, and package.json does not depend on eslint 9.22 or later and a flat-config eslint-config-expo, so `expo lint` would install them and write files first. Add or update them (`npx expo install eslint eslint-config-expo -- --save-dev`), then run `fscale sync`.".to_owned(),
                    Fixable::Agent,
                )
        }])
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        if !applies(ctx)? {
            return Ok(result);
        }
        let Ok(Some(json)) = package_json(ctx)? else {
            return Ok(result);
        };
        if !configured(ctx, &json) && flat_ready(&json) {
            create(ctx, "eslint.config.js", CONFIG, &mut result)?;
        }
        Ok(result)
    }
}
