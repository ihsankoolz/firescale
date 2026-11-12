//! `nextjs/eslint`: ESLint is set up with Next.js's rules.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{any_exists, create, depends_on, finding, package_json};

#[cfg(test)]
mod tests;

pub const ID: &str = "nextjs/eslint";

pub struct EslintConfig;

pub const NAMES: [&str; 11] = [
    "eslint.config.mjs",
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
        // With no package.json yet, nextjs/package-json writes one with
        // both, before this fix runs.
        let ready = match package_json(ctx)? {
            Ok(Some(json)) => {
                depends_on(&json, "eslint-config-next") && depends_on(&json, "eslint")
            }
            Ok(None) => true,
            Err(()) => false,
        };
        Ok(vec![if ready {
            finding(
                "eslint.config.mjs",
                "No ESLint config. `fscale sync` creates eslint.config.mjs with Next.js's core-web-vitals and TypeScript rules.".to_owned(),
                Fixable::Deterministic,
            )
        } else {
            finding(
                "eslint.config.mjs",
                "No ESLint config, and package.json does not depend on eslint and eslint-config-next. Add them as dev dependencies (`npm install -D eslint eslint-config-next`), then run `fscale sync` to write eslint.config.mjs.".to_owned(),
                Fixable::Agent,
            )
        }])
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        let ready = matches!(package_json(ctx)?, Ok(Some(json)) if depends_on(&json, "eslint-config-next") && depends_on(&json, "eslint"));
        if ready && !any_exists(ctx, &NAMES) {
            create(ctx, "eslint.config.mjs", CONFIG, &mut result)?;
        }
        Ok(result)
    }
}
