//! `nextjs/app-router`: the App Router has its root layout, and a new app
//! a first page.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{any_exists, applies, create, finding, uses_typescript};

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

/// True when the app folder has a root layout, or each route group has its
/// own ("multiple root layouts", review of M15).
fn has_layout(ctx: &RuleContext, dir: &str) -> bool {
    let layout_in = |folder: &std::path::Path| {
        EXTENSIONS
            .iter()
            .any(|ext| folder.join(format!("layout.{ext}")).is_file())
    };
    let app = ctx.root().join(dir);
    if layout_in(&app) {
        return true;
    }
    std::fs::read_dir(&app).is_ok_and(|entries| {
        entries.filter_map(std::result::Result::ok).any(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.starts_with('(') && name.ends_with(')') && layout_in(&entry.path())
        })
    })
}

/// The layout and page of a JavaScript app, for one with no TypeScript.
pub const LAYOUT_JS: &str = "export const metadata = {\n  title: 'Next.js app',\n  description: 'Started with firescale.',\n};\n\nexport default function RootLayout({ children }) {\n  return (\n    <html lang=\"en\">\n      <body>{children}</body>\n    </html>\n  );\n}\n";

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
        if !applies(ctx)? {
            return Ok(Vec::new());
        }
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
        if !applies(ctx)? {
            return Ok(result);
        }
        // A JavaScript app gets .jsx files: a .tsx file would make
        // `next build` set the app up for TypeScript.
        let (ext, layout) = if uses_typescript(ctx)? {
            ("tsx", LAYOUT)
        } else {
            ("jsx", LAYOUT_JS)
        };
        match app_dir(ctx) {
            Some(dir) if !has_layout(ctx, dir) => {
                create(ctx, &format!("{dir}/layout.{ext}"), layout, &mut result)?;
            }
            None if !pages_router(ctx) => {
                create(ctx, &format!("app/layout.{ext}"), layout, &mut result)?;
                create(ctx, &format!("app/page.{ext}"), PAGE, &mut result)?;
            }
            _ => {}
        }
        Ok(result)
    }
}
