//! `nextjs/eslint`: ESLint is set up with Next.js's rules.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{any_exists, create, declared_major, depends_on, finding, package_json};

#[cfg(test)]
mod tests;

pub const ID: &str = "nextjs/eslint";

pub struct EslintConfig;

pub const NAMES: [&str; 12] = [
    "eslint.config.mjs",
    "eslint.config.cts",
    "eslint.config.js",
    "eslint.config.cjs",
    "eslint.config.ts",
    "eslint.config.mts",
    ".eslintrc.json",
    ".eslintrc.js",
    ".eslintrc.cjs",
    ".eslintrc.yml",
    ".eslintrc.yaml",
    ".eslintrc",
];

/// Whether the flat config can be written for this app.
#[derive(Debug, PartialEq, Eq)]
enum Ready {
    Yes,
    MissingDeps,
    /// eslint before 9 or eslint-config-next before 16 cannot load it.
    OldVersions,
}

fn ready(json: &serde_json::Value) -> Ready {
    if !depends_on(json, "eslint-config-next") || !depends_on(json, "eslint") {
        return Ready::MissingDeps;
    }
    let new_enough = declared_major(json, "eslint").is_none_or(|major| major >= 9)
        && declared_major(json, "eslint-config-next").is_none_or(|major| major >= 16);
    if new_enough {
        Ready::Yes
    } else {
        Ready::OldVersions
    }
}

/// The config for a JavaScript app: no TypeScript rules.
pub const CONFIG_JS: &str = "import { defineConfig, globalIgnores } from 'eslint/config';\nimport nextVitals from 'eslint-config-next/core-web-vitals';\n\nconst eslintConfig = defineConfig([\n  ...nextVitals,\n  globalIgnores(['.next/**', 'out/**', 'build/**', 'next-env.d.ts']),\n]);\n\nexport default eslintConfig;\n";

pub const CONFIG: &str = "import { defineConfig, globalIgnores } from 'eslint/config';\nimport nextVitals from 'eslint-config-next/core-web-vitals';\nimport nextTs from 'eslint-config-next/typescript';\n\nconst eslintConfig = defineConfig([\n  ...nextVitals,\n  ...nextTs,\n  globalIgnores(['.next/**', 'out/**', 'build/**', 'next-env.d.ts']),\n]);\n\nexport default eslintConfig;\n";

impl BuiltinRule for EslintConfig {
    fn meta(&self) -> RuleMeta {
        let mut inputs: Vec<String> = NAMES.iter().map(|name| (*name).to_owned()).collect();
        inputs.push("package.json".to_owned());
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Consistency,
            description: "ESLint is set up with eslint-config-next.".to_owned(),
            inputs,
            has_fix: true,
            after: vec![super::package_json::ID.to_owned()],
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        if any_exists(ctx, &NAMES) {
            return Ok(Vec::new());
        }
        // With no package.json yet there is nothing to lint; nextjs/package-json
        // writes one, and the next pass of the fix loop comes back here.
        let Ok(Some(json)) = package_json(ctx)? else {
            return Ok(Vec::new());
        };
        if !depends_on(&json, "next") {
            return Ok(Vec::new());
        }
        Ok(vec![match ready(&json) {
            Ready::Yes => finding(
                "eslint.config.mjs",
                "No ESLint config. `fscale sync` creates eslint.config.mjs with Next.js's core-web-vitals rules (and its TypeScript rules for a TypeScript app).".to_owned(),
                Fixable::Deterministic,
            ),
            Ready::MissingDeps => finding(
                "eslint.config.mjs",
                "No ESLint config, and package.json does not depend on eslint and eslint-config-next. Add them as dev dependencies (`npm install -D eslint eslint-config-next`), then run `fscale sync` to write eslint.config.mjs.".to_owned(),
                Fixable::Agent,
            ),
            Ready::OldVersions => finding(
                "eslint.config.mjs",
                "No ESLint config. The flat config firescale writes needs eslint 9 or later and eslint-config-next 16 or later; this app has older ones. Upgrade them, or write the config by hand as their docs describe.".to_owned(),
                Fixable::Agent,
            ),
        }])
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        let Ok(Some(json)) = package_json(ctx)? else {
            return Ok(result);
        };
        if any_exists(ctx, &NAMES) || !depends_on(&json, "next") || ready(&json) != Ready::Yes {
            return Ok(result);
        }
        let content = if depends_on(&json, "typescript") {
            CONFIG
        } else {
            CONFIG_JS
        };
        create(ctx, "eslint.config.mjs", content, &mut result)?;
        Ok(result)
    }
}
