//! `nextjs/next-config`: the project has a Next.js config file.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{any_exists, create, finding};

pub const ID: &str = "nextjs/next-config";

pub struct NextConfig;

pub const NAMES: [&str; 4] = [
    "next.config.ts",
    "next.config.mjs",
    "next.config.js",
    "next.config.cjs",
];

pub const CONFIG: &str = "import type { NextConfig } from 'next';\n\nconst nextConfig: NextConfig = {\n  /* config options here */\n};\n\nexport default nextConfig;\n";

impl BuiltinRule for NextConfig {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Low,
            category: Category::Consistency,
            description:
                "A Next.js config file (next.config.ts, .mjs, .js or .cjs) is at the root."
                    .to_owned(),
            inputs: NAMES.iter().map(|name| (*name).to_owned()).collect(),
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
        Ok(vec![finding(
            "next.config.ts",
            "No Next.js config file. `fscale sync` creates next.config.ts, typed, with no options set, so settings have one place to go.".to_owned(),
            Fixable::Deterministic,
        )])
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        if !any_exists(ctx, &NAMES) {
            create(ctx, "next.config.ts", CONFIG, &mut result)?;
        }
        Ok(result)
    }
}
