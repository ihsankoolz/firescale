//! `supabase/package-json`: the Supabase CLI is a dev dependency, so
//! `npx supabase` runs the version the repo pins.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;
use serde_json::{json, Map, Value};

use super::{
    applies, create, depends_on, elsewhere, finding, new_package_json, package_json,
    package_json_with, VERSIONS,
};

pub const ID: &str = "supabase/package-json";

pub struct PackageJson;

/// The lockfiles of the package managers: with one, a dependency's version
/// belongs to the package manager, not to a fix.
const LOCKFILES: [&str; 5] = [
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lock",
    "bun.lockb",
];

fn has_lockfile(ctx: &RuleContext) -> bool {
    LOCKFILES.iter().any(|name| ctx.root().join(name).exists())
}

/// The CLI, as package.json would get it.
fn cli_dependency() -> Value {
    let mut dev = Map::new();
    for (name, version, is_dev) in VERSIONS {
        if is_dev {
            dev.insert(name.to_owned(), json!(version));
        }
    }
    json!({ "devDependencies": dev })
}

impl BuiltinRule for PackageJson {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::High,
            category: Category::Reliability,
            description: "package.json has the Supabase CLI (`supabase`) as a dev dependency, so `npx supabase` runs the repo's version.".to_owned(),
            inputs: vec!["package.json".to_owned()],
            has_fix: true,
            after: Vec::new(),
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        if !applies(ctx)? {
            // A workspace with its Supabase project in a subfolder: nothing
            // is written at the root.
            if let Some(found) = elsewhere(ctx) {
                return Ok(vec![finding(
                    "package.json",
                    format!("The Supabase project is in {found}, not at the repo root. The supabase pack checks a project at the root: run fscale in the folder above {found}, or turn the pack off here."),
                    Fixable::Manual,
                )]);
            }
            return Ok(Vec::new());
        }
        let json = match package_json(ctx)? {
            Ok(Some(json)) => json,
            Ok(None) => {
                return Ok(vec![finding(
                    "package.json",
                    "No package.json. `fscale sync` creates one with the Supabase CLI as a dev dependency, the JavaScript client, and db:start, db:reset and test:db scripts.".to_owned(),
                    Fixable::Deterministic,
                )])
            }
            Err(()) => return Ok(Vec::new()),
        };
        if depends_on(&json, "supabase") {
            return Ok(Vec::new());
        }
        let deterministic =
            !has_lockfile(ctx) && package_json_with(ctx, &cli_dependency())?.is_some();
        Ok(vec![finding(
            "package.json",
            if deterministic {
                "package.json does not list the Supabase CLI. `fscale sync` adds `supabase` as a dev dependency, so `npx supabase` runs one version for everyone.".to_owned()
            } else {
                "package.json does not list the Supabase CLI. Install it as a dev dependency with your package manager (for example `npm install --save-dev supabase`), so `npx supabase` runs one version for everyone and your lockfile records it.".to_owned()
            },
            if deterministic {
                Fixable::Deterministic
            } else {
                Fixable::Agent
            },
        )])
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        if !applies(ctx)? {
            return Ok(result);
        }
        match package_json(ctx)? {
            Ok(None) => {
                create(ctx, "package.json", &new_package_json(), &mut result)?;
                if !result.changed_files.is_empty() {
                    result.notes.push("package.json names the Supabase CLI and client; run your package manager's install (for example `npm install`) to fetch them.".to_owned());
                }
            }
            Ok(Some(json)) if !depends_on(&json, "supabase") && !has_lockfile(ctx) => {
                if let Some(updated) = package_json_with(ctx, &cli_dependency())? {
                    ctx.write("package.json", &updated)?;
                    result.changed_files.push("package.json".to_owned());
                }
            }
            _ => {}
        }
        Ok(result)
    }
}
