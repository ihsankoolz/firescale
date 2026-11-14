//! The `supabase` stack pack (ADR 0049): rules whose fixes create and repair
//! the files a project using the Supabase CLI needs, so `fscale init --packs
//! core,supabase` builds a working starter. Opt-in: add "supabase" to
//! `packs`.
//!
//! What the pack assumes was read from Supabase's docs and the CLI's source
//! at v3.1.0 on 2026-11-14, and checked against the CLI 3.1.0 itself where
//! it runs without Docker; the versions are in [`VERSIONS`].
//!
//! A fix only creates a file that is missing, adds lines, renames the one
//! table header CLI v3 refuses, or adds a new migration. It never edits a
//! migration or replaces a file a person wrote.

use firescale_core::rule::{BuiltinPack, RuleContext};
use firescale_core::Result;
use serde_json::{json, Map};

pub(crate) use crate::stackkit::{
    create, declared_version, depends_on, finding, package_json, package_json_with,
};

#[path = "config/mod.rs"]
pub mod config;
#[path = "db-tests/mod.rs"]
pub mod db_tests;
#[path = "gitignore/mod.rs"]
pub mod gitignore;
#[path = "local-smtp/mod.rs"]
pub mod local_smtp;
#[path = "migration-names/mod.rs"]
pub mod migration_names;
#[path = "package-json/mod.rs"]
pub mod package_json;
#[path = "public-secret-key/mod.rs"]
pub mod public_secret_key;
#[path = "rls/mod.rs"]
pub mod rls;
pub mod sql;

#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod tests;

/// The packages a new project gets, from the npm registry on 2026-11-14:
/// the CLI as a dev dependency (so `npx supabase` runs the repo's version)
/// and the JavaScript client.
pub const VERSIONS: [(&str, &str, bool); 2] = [
    ("@supabase/supabase-js", "2.119.0", false),
    ("supabase", "3.1.0", true),
];

/// The scripts a new project gets.
pub const SCRIPTS: [(&str, &str); 3] = [
    ("db:start", "supabase start"),
    ("db:reset", "supabase db reset"),
    ("test:db", "supabase test db"),
];

/// The folder the CLI works in.
pub const DIR: &str = "supabase";
pub const CONFIG: &str = "supabase/config.toml";
pub const MIGRATIONS: &str = "supabase/migrations";

/// True when the pack works at the repo root: the root has a `supabase/`
/// folder, or no Supabase project is in a subfolder (the pack is on, so the
/// root is where it goes: a new project, or one beside another stack's
/// package.json). Dependencies do not count: a workspace often lists the
/// CLI or the client at its root while its project is in a subfolder.
pub(crate) fn applies(ctx: &RuleContext) -> Result<bool> {
    if ctx.root().join(DIR).is_dir() {
        return Ok(true);
    }
    Ok(!matches!(package_json(ctx)?, Err(())) && elsewhere(ctx).is_none())
}

/// What every rule of the pack reads besides its own files, for the cache:
/// where the pack applies depends on package.json, the root `supabase/`
/// config and any Supabase project in a subfolder.
pub(crate) fn inputs(own: &[&str]) -> Vec<String> {
    let mut all: Vec<String> = own.iter().map(|s| (*s).to_owned()).collect();
    for shared in ["package.json", CONFIG, "**/supabase/config.toml"] {
        if !all.iter().any(|a| a == shared) {
            all.push(shared.to_owned());
        }
    }
    all
}

/// A folder below the root, at most three deep, that holds a Supabase
/// project (`<folder>/supabase/config.toml`): a workspace whose Supabase
/// project is not at the root. Dependency, build and hidden folders are
/// skipped.
pub(crate) fn elsewhere(ctx: &RuleContext) -> Option<String> {
    fn walk(dir: &std::path::Path, rel: &str, depth: usize) -> Option<String> {
        if depth > 3 {
            return None;
        }
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .ok()?
            .filter_map(std::result::Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| e.file_name().to_str().map(str::to_owned))
            .filter(|name| {
                !name.starts_with('.')
                    && !matches!(
                        name.as_str(),
                        "node_modules" | "target" | "dist" | "build" | "vendor"
                    )
            })
            .collect();
        entries.sort();
        for name in entries {
            let path = dir.join(&name);
            let rel = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            if depth > 0 && name == DIR && path.join("config.toml").is_file() {
                return Some(rel);
            }
            if let Some(found) = walk(&path, &rel, depth + 1) {
                return Some(found);
            }
        }
        None
    }
    walk(ctx.root(), "", 0)
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
        "name": "supabase-app",
        "version": "0.1.0",
        "private": true,
        "type": "module",
        "engines": { "node": ">=22" },
        "scripts": scripts,
        "dependencies": deps,
        "devDependencies": dev,
    });
    format!(
        "{}\n",
        serde_json::to_string_pretty(&value).unwrap_or_default()
    )
}

/// The SQL files of `supabase/migrations` the CLI applies, in its order
/// (byte-wise by name), with their text: `<digits>_<name>.sql` only.
pub(crate) fn migrations(ctx: &RuleContext) -> Vec<(String, String)> {
    let mut names: Vec<String> = migration_entries(ctx)
        .into_iter()
        .filter(|(name, is_file)| *is_file && migration_names::version(name).is_some())
        .map(|(name, _)| name)
        .collect();
    names.sort_unstable();
    let mut out = Vec::new();
    for name in names {
        // Read as bytes: a migration in another encoding is read lossily,
        // never a reason to stop the whole check.
        let path = MIGRATIONS
            .split('/')
            .fold(ctx.root().to_path_buf(), |dir, part| dir.join(part))
            .join(&name);
        if let Ok(bytes) = std::fs::read(path) {
            out.push((name, String::from_utf8_lossy(&bytes).into_owned()));
        }
    }
    out
}

/// Every entry of `supabase/migrations`, with whether it is a file.
pub(crate) fn migration_entries(ctx: &RuleContext) -> Vec<(String, bool)> {
    let dir = MIGRATIONS
        .split('/')
        .fold(ctx.root().to_path_buf(), |dir, part| dir.join(part));
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, bool)> = entries
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_owned();
            let is_file = entry.file_type().ok()?.is_file();
            Some((name, is_file))
        })
        .collect();
    out.sort();
    out
}

/// A top-level key of `supabase/config.toml`, or a key of one of its
/// tables, read line by line: `key = "value"` or `key = ["a", "b"]`. Only
/// what this pack needs; not a TOML parser.
pub(crate) fn config_value(text: &str, table: Option<&str>, key: &str) -> Option<String> {
    let mut current: Option<String> = None;
    let mut lines = text.trim_start_matches('\u{feff}').lines();
    while let Some(line) = lines.next() {
        let line = line.trim();
        if let Some(header) = table_header(line) {
            current = Some(header);
            continue;
        }
        if current.as_deref() != table {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim().trim_matches('"') != key {
            continue;
        }
        let mut value = strip_comment(value).trim().to_owned();
        // An array over several lines: read on to its closing bracket.
        if value.starts_with('[') && !value.ends_with(']') {
            for more in lines.by_ref() {
                value.push(' ');
                value.push_str(strip_comment(more).trim());
                if value.ends_with(']') {
                    break;
                }
            }
        }
        return Some(value);
    }
    None
}

/// A TOML value without a `#` comment after it (a `#` inside a string
/// stays).
fn strip_comment(value: &str) -> &str {
    let mut quote: Option<char> = None;
    for (at, c) in value.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '#') => return &value[..at],
            _ => {}
        }
    }
    value
}

/// The name of a `[table]` or `[[array]]` header, without brackets.
pub(crate) fn table_header(line: &str) -> Option<String> {
    let line = line.trim_start_matches('\u{feff}').trim();
    let line = match line.find('#') {
        Some(at) if line[..at].trim_end().ends_with(']') => line[..at].trim_end(),
        _ => line,
    };
    let inner = line
        .strip_prefix("[[")
        .and_then(|l| l.strip_suffix("]]"))
        .or_else(|| line.strip_prefix('[').and_then(|l| l.strip_suffix(']')))?;
    Some(
        inner
            .split('.')
            .map(|part| part.trim().trim_matches('"'))
            .collect::<Vec<_>>()
            .join("."),
    )
}

/// The strings of a TOML array such as `["public", "graphql_public"]`.
pub(crate) fn string_list(value: &str) -> Vec<String> {
    value
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(|item| item.trim().trim_matches('"').trim_matches('\'').to_owned())
        .filter(|item| !item.is_empty())
        .collect()
}

#[must_use]
pub fn pack() -> BuiltinPack {
    BuiltinPack {
        name: "supabase",
        rules: vec![
            Box::new(package_json::PackageJson),
            Box::new(config::Config),
            Box::new(local_smtp::LocalSmtp),
            Box::new(gitignore::SupabaseGitignore),
            Box::new(migration_names::MigrationNames),
            Box::new(rls::Rls),
            Box::new(db_tests::DbTests),
            Box::new(public_secret_key::PublicSecretKey),
        ],
    }
}
