//! The `nextjs` stack pack (ADR 0029): rules whose fixes create and repair
//! the files a Next.js App Router project needs, so `fscale init --packs
//! core,nextjs` builds a working starter. Opt-in: add "nextjs" to `packs`.
//!
//! What the pack assumes was read from Next.js's docs and create-next-app's
//! default template on 2026-11-12; the versions are in [`VERSIONS`].
//!
//! A fix only creates a file that is missing, or adds missing keys to
//! package.json as new lines (`jsonedit`). It never replaces a file a person
//! wrote.

use firescale_core::rule::{BuiltinPack, RuleContext};
use firescale_core::Result;
use serde_json::{json, Map, Value};

pub(crate) use crate::stackkit::{
    any_exists, create, depends_on, finding, package_json, package_json_with,
};

#[path = "app-router/mod.rs"]
pub mod app_router;
#[path = "eslint/mod.rs"]
pub mod eslint;
#[path = "gitignore/mod.rs"]
pub mod gitignore;
#[path = "next-config/mod.rs"]
pub mod next_config;
#[path = "package-json/mod.rs"]
pub mod package_json;
#[path = "tsconfig/mod.rs"]
pub mod tsconfig;

#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod tests;

/// The packages a new project gets, with the versions create-next-app
/// wrote on 2026-11-12: exact for Next.js and React, ranges for the rest.
pub const VERSIONS: [(&str, &str, bool); 9] = [
    ("next", "16.4.6", false),
    ("react", "19.3.1", false),
    ("react-dom", "19.3.1", false),
    ("@types/node", "^20", true),
    ("@types/react", "^19", true),
    ("@types/react-dom", "^19", true),
    ("eslint", "^9", true),
    ("eslint-config-next", "16.4.6", true),
    ("typescript", "^5", true),
];

/// The scripts a Next.js project runs.
pub const SCRIPTS: [(&str, &str); 4] = [
    ("dev", "next dev"),
    ("build", "next build"),
    ("start", "next start"),
    ("lint", "eslint"),
];

/// True when the pack applies to the repo root: there is no package.json
/// yet (a new app), or it depends on `next`. A workspace root whose Next.js
/// app lives in a subfolder is left alone (review of M15): only
/// `nextjs/package-json` says so, as a manual finding.
pub(crate) fn applies(ctx: &RuleContext) -> Result<bool> {
    Ok(match package_json(ctx)? {
        Ok(None) => true,
        Ok(Some(json)) => depends_on(&json, "next"),
        Err(()) => false,
    })
}

/// True when the app is written in TypeScript: it depends on it, or has no
/// package.json yet (a new app is TypeScript).
pub(crate) fn uses_typescript(ctx: &RuleContext) -> Result<bool> {
    Ok(match package_json(ctx)? {
        Ok(Some(json)) => depends_on(&json, "typescript"),
        Ok(None) => true,
        Err(()) => false,
    })
}

/// The major version a dependency is declared with: `^16.1.0` gives 16.
pub(crate) fn declared_major(json: &Value, package: &str) -> Option<u64> {
    crate::stackkit::declared_version(json, package).map(|(major, _)| major)
}

/// The package.json of a new project.
#[must_use]
pub fn new_package_json() -> String {
    let mut scripts = Map::new();
    for (name, command) in SCRIPTS {
        scripts.insert(name.to_owned(), json!(command));
    }
    let mut deps = Map::new();
    let mut dev = Map::new();
    for (name, version, is_dev) in VERSIONS {
        if is_dev {
            dev.insert(name.to_owned(), json!(version));
        } else {
            deps.insert(name.to_owned(), json!(version));
        }
    }
    let value = json!({
        "name": "next-app",
        "version": "0.1.0",
        "private": true,
        "engines": { "node": ">=20.9.0" },
        "scripts": scripts,
        "dependencies": deps,
        "devDependencies": dev,
    });
    format!(
        "{}\n",
        serde_json::to_string_pretty(&value).unwrap_or_default()
    )
}

#[must_use]
pub fn pack() -> BuiltinPack {
    BuiltinPack {
        name: "nextjs",
        rules: vec![
            Box::new(package_json::PackageJson),
            Box::new(app_router::AppRouter),
            Box::new(next_config::NextConfig),
            Box::new(tsconfig::TsConfig),
            Box::new(eslint::EslintConfig),
            Box::new(gitignore::NextGitignore),
        ],
    }
}
