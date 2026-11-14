//! The `expo` stack pack (ADR 0050): rules whose fixes create and repair
//! the files an Expo (React Native) app needs, so `fscale init --packs
//! core,expo` builds a working starter. Opt-in: add "expo" to `packs`.
//!
//! What the pack assumes was read on 2026-11-14 from Expo's docs source and
//! the CLI at SDK 58, and from the `expo-template-blank-typescript` 58.0.34
//! template; the versions are in [`VERSIONS`].
//!
//! A fix only creates a file that is missing, or adds missing keys or lines.
//! It never replaces a file a person wrote.

use firescale_core::jsonedit::{holds, insert_merge, merged};
use firescale_core::model::{Finding, FixResult, Fixable};
use firescale_core::rule::{BuiltinPack, RuleContext};
use firescale_core::Result;
use serde_json::{json, Map, Value};

#[path = "app-config/mod.rs"]
pub mod app_config;
pub mod entry;
pub mod eslint;
pub mod gitignore;
#[path = "package-json/mod.rs"]
pub mod package_json;
#[path = "public-env-secret/mod.rs"]
pub mod public_env_secret;
pub mod tsconfig;

#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod tests;

/// The packages a new app gets: SDK 58's versions from
/// `expo/bundledNativeModules.json` (expo 58.0.25) and the blank TypeScript
/// template 58.0.34, not npm's latest (TypeScript 7, ESLint 10 and React
/// 19.3.1 do not match the SDK). ESLint `^9.22.0`: `expo lint` installs
/// `^9.0.0`, but its config's `defineConfig` needs 9.22.
pub const VERSIONS: [(&str, &str, bool); 8] = [
    ("expo", "~58.0.25", false),
    ("expo-status-bar", "~58.0.5", false),
    ("react", "19.3.0", false),
    ("react-native", "0.88.1", false),
    ("@types/react", "~19.3.0", true),
    ("eslint", "^9.22.0", true),
    ("eslint-config-expo", "~58.0.4", true),
    ("typescript", "~6.0.3", true),
];

/// The scripts an Expo app runs (the template's, and `expo lint`'s).
pub const SCRIPTS: [(&str, &str); 5] = [
    ("start", "expo start"),
    ("android", "expo start --android"),
    ("ios", "expo start --ios"),
    ("web", "expo start --web"),
    ("lint", "expo lint"),
];

pub(crate) fn finding(file: &str, message: String, fixable: Fixable) -> Finding {
    Finding {
        message,
        file: file.to_owned(),
        range: None,
        project: None,
        fixable,
        severity: None,
    }
}

/// The root package.json, parsed. `Err(())` when it exists but is not JSON.
pub(crate) fn package_json(ctx: &RuleContext) -> Result<std::result::Result<Option<Value>, ()>> {
    let Some(text) = ctx.read("package.json")? else {
        return Ok(Ok(None));
    };
    Ok(serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map(Some)
        .map_err(|_| ()))
}

/// True when package.json lists the package in any dependency list.
pub(crate) fn depends_on(json: &Value, package: &str) -> bool {
    ["dependencies", "devDependencies", "peerDependencies"]
        .iter()
        .any(|field| json.get(field).and_then(|deps| deps.get(package)).is_some())
}

/// True when the pack applies to the repo root: no package.json yet (a new
/// app), or one that depends on `expo`, unless the root is a workspace with
/// no app of its own (its app is in a subfolder; only `expo/package-json`
/// says so).
pub(crate) fn applies(ctx: &RuleContext) -> Result<bool> {
    Ok(match package_json(ctx)? {
        Ok(None) => true,
        Ok(Some(json)) => depends_on(&json, "expo") && !workspace_without_app(ctx, &json),
        Err(()) => false,
    })
}

/// Files that make the root an app: a config or an entry.
const ROOT_APP_FILES: [&str; 24] = [
    "app.json",
    "app.config.json",
    "app.config.ts",
    "app.config.js",
    "app.config.mts",
    "app.config.cts",
    "app.config.mjs",
    "app.config.cjs",
    "index.ts",
    "index.tsx",
    "index.js",
    "index.jsx",
    "index.mjs",
    "index.cjs",
    "App.ts",
    "App.tsx",
    "App.js",
    "App.jsx",
    "app/_layout.tsx",
    "app/_layout.js",
    "app/index.tsx",
    "app/index.js",
    "src/app/_layout.tsx",
    "src/app/index.tsx",
];

/// True when the root is a workspace (`workspaces`, or
/// `pnpm-workspace.yaml`) and has no app config or entry of its own: a
/// monorepo listing `expo` at the root for its apps in subfolders. A
/// workspace that is itself the app (a config or an entry at the root)
/// is not one.
pub(crate) fn workspace_without_app(ctx: &RuleContext, json: &Value) -> bool {
    let workspace = json.get("workspaces").is_some() || any_exists(ctx, &["pnpm-workspace.yaml"]);
    workspace && !any_exists(ctx, &ROOT_APP_FILES) && json.get("main").is_none()
}

/// What every rule of the pack reads besides its own files, for the cache:
/// where the pack applies depends on package.json, a workspace file and the
/// root's app files.
pub(crate) fn inputs(own: &[&str]) -> Vec<String> {
    let mut all: Vec<String> = own.iter().map(|s| (*s).to_owned()).collect();
    for shared in ["package.json", "pnpm-workspace.yaml"]
        .iter()
        .chain(ROOT_APP_FILES.iter())
    {
        if !all.iter().any(|a| a == shared) {
            all.push((*shared).to_owned());
        }
    }
    all
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

/// True when any of `names` exists at the root.
pub(crate) fn any_exists(ctx: &RuleContext, names: &[&str]) -> bool {
    names.iter().any(|name| ctx.root().join(name).exists())
}

/// Creates a file firescale owns, or does nothing when anything is there.
pub(crate) fn create(
    ctx: &RuleContext,
    file: &str,
    content: &str,
    result: &mut FixResult,
) -> Result<()> {
    if ctx.root().join(file).symlink_metadata().is_ok() {
        return Ok(());
    }
    ctx.create_tracked(file, content)?;
    result.changed_files.push(file.to_owned());
    Ok(())
}

/// The package.json of a new app: the blank TypeScript template's, with
/// `expo lint`'s ESLint and script. The name is fixed, never the folder's,
/// so a dry run in a scratch copy writes the same file.
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
        "name": "expo-app",
        "version": "1.0.0",
        "private": true,
        "main": "index.ts",
        "engines": { "node": ">=22.13.0" },
        "scripts": scripts,
        "dependencies": deps,
        "devDependencies": dev,
    });
    format!(
        "{}\n",
        serde_json::to_string_pretty(&value).unwrap_or_default()
    )
}

/// What a JSON file would become with `add` merged in as new lines, or
/// `None` when that would change a line (or the file is not JSON).
pub(crate) fn json_with(ctx: &RuleContext, file: &str, add: &Value) -> Result<Option<String>> {
    let Some(text) = ctx.read(file)? else {
        return Ok(None);
    };
    let Ok(have) = serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')) else {
        return Ok(None);
    };
    let target = merged(&have, add);
    if !holds(&target, add) {
        return Ok(None);
    }
    Ok(insert_merge(&text, &target))
}

#[must_use]
pub fn pack() -> BuiltinPack {
    BuiltinPack {
        name: "expo",
        rules: vec![
            Box::new(package_json::PackageJson),
            Box::new(app_config::AppConfig),
            Box::new(entry::Entry),
            Box::new(tsconfig::TsConfig),
            Box::new(eslint::EslintConfig),
            Box::new(gitignore::ExpoGitignore),
            Box::new(public_env_secret::PublicEnvSecret),
        ],
    }
}
