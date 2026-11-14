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
