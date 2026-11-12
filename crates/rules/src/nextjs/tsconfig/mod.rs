//! `nextjs/tsconfig`: a TypeScript project has a tsconfig.json with the
//! Next.js plugin.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{create, depends_on, finding, package_json};

#[cfg(test)]
mod tests;

pub const ID: &str = "nextjs/tsconfig";

pub struct TsConfig;

pub const TSCONFIG: &str = r#"{
  "compilerOptions": {
    "target": "ES2017",
    "lib": ["dom", "dom.iterable", "esnext"],
    "allowJs": true,
    "skipLibCheck": true,
    "strict": true,
    "noEmit": true,
    "esModuleInterop": true,
    "module": "esnext",
    "moduleResolution": "bundler",
    "resolveJsonModule": true,
    "isolatedModules": true,
    "jsx": "react-jsx",
    "incremental": true,
    "plugins": [{ "name": "next" }],
    "paths": {
      "@/*": ["./*"]
    }
  },
  "include": [
    "next-env.d.ts",
    "**/*.ts",
    "**/*.tsx",
    ".next/types/**/*.ts",
    ".next/dev/types/**/*.ts",
    "**/*.mts"
  ],
  "exclude": ["node_modules"]
}
"#;

/// True when the project is written in TypeScript: it depends on it, or
/// has no package.json yet (a new app is TypeScript).
fn uses_typescript(ctx: &RuleContext) -> Result<bool> {
    Ok(match package_json(ctx)? {
        Ok(Some(json)) => depends_on(&json, "typescript"),
        Ok(None) => true,
        Err(()) => false,
    })
}

impl BuiltinRule for TsConfig {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Reliability,
            description: "A TypeScript Next.js project has a tsconfig.json.".to_owned(),
            inputs: vec!["tsconfig.json".to_owned(), "package.json".to_owned()],
            has_fix: true,
            after: vec![super::package_json::ID.to_owned()],
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        if ctx.root().join("tsconfig.json").exists() || !uses_typescript(ctx)? {
            return Ok(Vec::new());
        }
        Ok(vec![finding(
            "tsconfig.json",
            "This Next.js project uses TypeScript but has no tsconfig.json. `fscale sync` creates the one create-next-app writes, with the next plugin and the @/* path.".to_owned(),
            Fixable::Deterministic,
        )])
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        if uses_typescript(ctx)? {
            create(ctx, "tsconfig.json", TSCONFIG, &mut result)?;
        }
        Ok(result)
    }
}
