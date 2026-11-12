//! `nextjs/app-router`: the App Router has its root layout, and a new app
//! a first page.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{any_exists, create, finding};

#[cfg(test)]
mod tests;

pub const ID: &str = "nextjs/app-router";

pub struct AppRouter;

const EXTENSIONS: [&str; 4] = ["tsx", "jsx", "ts", "js"];

pub const LAYOUT: &str = "import type { Metadata } from 'next';\nimport type { ReactNode } from 'react';\n\nexport const metadata: Metadata = {\n  title: 'Next.js app',\n  description: 'Started with firescale.',\n};\n\nexport default function RootLayout({ children }: Readonly<{ children: ReactNode }>) {\n  return (\n    <html lang=\"en\">\n      <body>{children}</body>\n    </html>\n  );\n}\n";

pub const PAGE: &str = "export default function Home() {\n  return (\n    <main>\n      <h1>Next.js app</h1>\n      <p>Edit app/page.tsx to start.</p>\n    </main>\n  );\n}\n";

/// The app folder in use: `app` or `src/app`, if any.
fn app_dir(ctx: &RuleContext) -> Option<&'static str> {
    ["app", "src/app"]
        .into_iter()
        .find(|dir| ctx.root().join(dir).is_dir())
}

fn has_layout(ctx: &RuleContext, dir: &str) -> bool {
    EXTENSIONS
        .iter()
        .any(|ext| ctx.root().join(dir).join(format!("layout.{ext}")).is_file())
}

/// True when the project uses the Pages Router instead.
fn pages_router(ctx: &RuleContext) -> bool {
    any_exists(ctx, &["pages", "src/pages"])
}

impl BuiltinRule for AppRouter {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::High,
            category: Category::Reliability,
            description:
                "The App Router has a root layout; a new app gets app/layout.tsx and app/page.tsx."
                    .to_owned(),
            inputs: ["app/**", "src/app/**", "pages/**", "src/pages/**"]
                .iter()
                .map(|glob| (*glob).to_owned())
                .collect(),
            has_fix: true,
            after: vec![super::package_json::ID.to_owned()],
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        match app_dir(ctx) {
            Some(dir) if !has_layout(ctx, dir) => Ok(vec![finding(
                &format!("{dir}/layout.tsx"),
                format!("{dir}/ has no root layout, so `next build` fails. `fscale sync` creates {dir}/layout.tsx with the <html> and <body> tags."),
                Fixable::Deterministic,
            )]),
            Some(_) => Ok(Vec::new()),
            None if pages_router(ctx) => Ok(Vec::new()),
            None => Ok(vec![finding(
                "app/page.tsx",
                "No app/ folder (and no pages/ folder), so Next.js has nothing to build. `fscale sync` creates app/layout.tsx and app/page.tsx.".to_owned(),
                Fixable::Deterministic,
            )]),
        }
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        match app_dir(ctx) {
            Some(dir) if !has_layout(ctx, dir) => {
                create(ctx, &format!("{dir}/layout.tsx"), LAYOUT, &mut result)?;
            }
            None if !pages_router(ctx) => {
                create(ctx, "app/layout.tsx", LAYOUT, &mut result)?;
                create(ctx, "app/page.tsx", PAGE, &mut result)?;
            }
            _ => {}
        }
        Ok(result)
    }
}
