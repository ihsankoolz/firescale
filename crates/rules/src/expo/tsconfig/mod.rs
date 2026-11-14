//! `expo/tsconfig`: a TypeScript app has a tsconfig.json.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{applies, create, finding, uses_typescript};

#[cfg(test)]
mod tests;

pub const ID: &str = "expo/tsconfig";

pub struct TsConfig;

/// The blank TypeScript template's tsconfig.json (SDK 58), exactly: Expo's
/// base sets everything else (`noEmit` among it, so `tsc --noEmit` works).
pub const TSCONFIG: &str =
    "{\n  \"extends\": \"expo/tsconfig.base\",\n  \"compilerOptions\": {\n    \"strict\": true\n  }\n}\n";

impl BuiltinRule for TsConfig {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Consistency,
            description: "A TypeScript Expo app has tsconfig.json.".to_owned(),
            inputs: super::inputs(&["tsconfig.json"]),
            has_fix: true,
            after: vec![super::package_json::ID.to_owned()],
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        if ctx.root().join("tsconfig.json").exists() || !applies(ctx)? || !uses_typescript(ctx)? {
            return Ok(Vec::new());
        }
        Ok(vec![finding(
            "tsconfig.json",
            "A TypeScript Expo app with no tsconfig.json. `fscale sync` creates the template's, which extends expo/tsconfig.base with strict on.".to_owned(),
            Fixable::Deterministic,
        )])
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        if applies(ctx)? && uses_typescript(ctx)? {
            create(ctx, "tsconfig.json", TSCONFIG, &mut result)?;
        }
        Ok(result)
    }
}
