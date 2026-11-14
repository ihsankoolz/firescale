//! What the stack packs share (ADR 0051): reading the root package.json,
//! asking what it depends on and at which version, adding keys to a JSON
//! file as new lines, creating a file only when nothing is there, and the
//! shape of a finding. One copy here instead of one per pack.

use firescale_core::jsonedit::{holds, insert_merge, merged};
use firescale_core::model::{Finding, FixResult, Fixable};
use firescale_core::rule::RuleContext;
use firescale_core::Result;
use serde_json::Value;

/// A finding of a stack-pack rule: a file, a message, who can fix it.
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

/// The version a dependency is declared with, as (major, minor): `^16.1.0`
/// gives (16, 1). `None` for a tag, a URL or no declaration.
pub(crate) fn declared_version(json: &Value, package: &str) -> Option<(u64, u64)> {
    let spec = ["dependencies", "devDependencies"]
        .iter()
        .find_map(|field| json.get(field)?.get(package)?.as_str())?;
    let rest = spec.trim_start_matches(|c: char| !c.is_ascii_digit());
    let mut parts = rest.split(|c: char| !c.is_ascii_digit());
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().and_then(|m| m.parse().ok()).unwrap_or(0);
    Some((major, minor))
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

/// [`json_with`] for the root package.json.
pub(crate) fn package_json_with(ctx: &RuleContext, add: &Value) -> Result<Option<String>> {
    json_with(ctx, "package.json", add)
}

/// Root manifests of every stack a pack knows, for [`other_stack`]: a
/// pack's starter is written only into a repo that has none but its own.
pub(crate) const MANIFESTS: [&str; 22] = [
    "package.json",
    "deno.json",
    "deno.jsonc",
    "pyproject.toml",
    "requirements.txt",
    "setup.py",
    "setup.cfg",
    "Pipfile",
    "Cargo.toml",
    "go.mod",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "build.sbt",
    "CMakeLists.txt",
    "meson.build",
    "Package.swift",
    "pubspec.yaml",
    "Gemfile",
    "composer.json",
    "mix.exs",
    "settings.gradle.kts",
];

/// Source file extensions of every language a pack knows, for
/// [`other_stack`].
pub(crate) const CODE: [&str; 26] = [
    "py", "js", "jsx", "mjs", "cjs", "ts", "tsx", "rs", "go", "java", "kt", "kts", "scala", "c",
    "h", "cc", "cpp", "hpp", "cs", "fs", "swift", "rb", "php", "dart", "ex", "exs",
];

/// True for a path no tool builds from: in a folder whose name starts with
/// `.` or `_`, or named `testdata`, `vendor` or `node_modules`.
pub(crate) fn set_aside(path: &str) -> bool {
    let mut parts: Vec<&str> = path.split('/').collect();
    parts.pop();
    parts.iter().any(|folder| {
        folder.starts_with(['.', '_']) || matches!(*folder, "testdata" | "vendor" | "node_modules")
    })
}

/// True when the root holds another stack: a manifest that is not one of
/// `own` (the pack's own), or source code in a language whose extension is
/// not one of `code` (the pack's), anywhere outside folders set aside. A
/// pack writes its starter only into a repo that is new to it: found in
/// the go pack's review, where a Python project with only `setup.py`
/// would have been given a Go module.
pub(crate) fn other_stack(ctx: &RuleContext, own: &[&str], code: &[&str]) -> Result<bool> {
    let foreign: Vec<&str> = MANIFESTS
        .iter()
        .copied()
        .filter(|name| !own.contains(name))
        .collect();
    if any_exists(ctx, &foreign) {
        return Ok(true);
    }
    Ok(firescale_core::walk::walk_files(ctx.root())?
        .iter()
        .any(|path| {
            !set_aside(path)
                && path.rsplit_once('.').is_some_and(|(_, extension)| {
                    CODE.contains(&extension) && !code.contains(&extension)
                })
        }))
}

/// What [`other_stack`] reads, as cache inputs: every manifest at the root
/// and every source file.
pub(crate) fn other_stack_inputs() -> Vec<String> {
    MANIFESTS
        .iter()
        .map(|name| (*name).to_owned())
        .chain(CODE.iter().map(|extension| format!("**/*.{extension}")))
        .collect()
}
