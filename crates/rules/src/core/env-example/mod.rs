//! `core/env-example`: If code reads env vars, .env.example lists them.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, LineRange, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::{Error, Result};

#[cfg(test)]
mod tests;

pub const ID: &str = "core/env-example";

const EXAMPLE: &str = ".env.example";
const EXTENSIONS: [&str; 12] = [
    "js", "jsx", "mjs", "cjs", "ts", "tsx", "mts", "cts", "py", "rs", "go", "rb",
];
const SKIP_DIRS: [&str; 6] = ["node_modules", "dist", "build", "target", "vendor", ".git"];
const MAX_BYTES: u64 = 1_000_000;

/// Names that are never project secrets or config.
const BUILT_IN_IGNORE: [&str; 14] = [
    "NODE_ENV", "CI", "PATH", "HOME", "PWD", "USER", "SHELL", "TERM", "TMPDIR", "TEMP", "TMP",
    "LANG", "TZ", "DEBUG",
];
const IGNORE_PREFIXES: [&str; 7] = [
    "npm_", "GITHUB_", "RUNNER_", "FSCALE_", "CARGO_", "RUSTC_", "RUSTUP_",
];

/// Patterns where the name follows directly and ends at a non-identifier character.
const BARE_PATTERNS: [&str; 2] = ["process.env.", "import.meta.env."];

/// Patterns where the name follows directly and must be closed by a quote.
const QUOTED_PATTERNS: [&str; 18] = [
    "process.env[\"",
    "process.env['",
    "os.environ[\"",
    "os.environ['",
    "os.environ.get(\"",
    "os.environ.get('",
    "os.getenv(\"",
    "os.getenv('",
    "env::var(\"",
    "env::var_os(\"",
    "os.Getenv(\"",
    "ENV[\"",
    "ENV['",
    "ENV.fetch(\"",
    "ENV.fetch('",
    "import.meta.env[\"",
    "import.meta.env['",
    "os.environ.setdefault(\"",
];

pub struct EnvExample;

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Reads `[A-Z][A-Z0-9_]*` from the start of `rest`. `quoted` requires a closing quote after it.
fn parse_name(rest: &str, quoted: bool) -> Option<&str> {
    let first = rest.chars().next()?;
    if !first.is_ascii_uppercase() {
        return None;
    }
    let len = rest
        .find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
        .unwrap_or(rest.len());
    let after = rest[len..].chars().next();
    let ok = if quoted {
        matches!(after, Some('"' | '\''))
    } else {
        after.is_none_or(|c| !is_ident(c))
    };
    ok.then(|| &rest[..len])
}

/// True when the pattern is not the tail of a longer identifier or member path.
fn boundary_ok(before: &str, pattern: &str) -> bool {
    let Some(prev) = before.chars().next_back() else {
        return true;
    };
    if prev.is_ascii_alphanumeric() || prev == '_' || prev == '$' {
        return false;
    }
    let starts_with_ident = pattern.starts_with(|c: char| c.is_ascii_alphabetic());
    // `std::env::var` is reached through a path, so `::` is fine; a `.` is not.
    !(starts_with_ident && prev == '.')
}

fn names_in_line(line: &str) -> Vec<&str> {
    let mut found = Vec::new();
    for (patterns, quoted) in [(&BARE_PATTERNS[..], false), (&QUOTED_PATTERNS[..], true)] {
        for pattern in patterns {
            let mut from = 0;
            while let Some(at) = line[from..].find(pattern) {
                let begin = from + at;
                let start = begin + pattern.len();
                if !boundary_ok(&line[..begin], pattern) {
                    from = start;
                    continue;
                }
                if let Some(name) = parse_name(&line[start..], quoted) {
                    found.push(name);
                }
                from = start;
            }
        }
    }
    found
}

fn ignore_list(ctx: &RuleContext) -> Result<BTreeSet<String>> {
    let mut ignore: BTreeSet<String> = BUILT_IN_IGNORE.iter().map(|s| (*s).to_owned()).collect();
    if let Some(option) = ctx.options().get("ignore") {
        let names: Option<Vec<String>> = option.as_array().and_then(|items| {
            items
                .iter()
                .map(|item| item.as_str().map(str::to_owned))
                .collect()
        });
        let Some(names) = names else {
            return Err(Error::user(
                "core/env-example: option \"ignore\" must be a list of variable names, for example [\"EXTRA_VAR\"]. Fix it in your firescale config.",
            ));
        };
        ignore.extend(names);
    }
    Ok(ignore)
}

fn is_ignored(name: &str, ignore: &BTreeSet<String>) -> bool {
    ignore.contains(name) || IGNORE_PREFIXES.iter().any(|p| name.starts_with(p))
}

fn is_source(file: &str) -> bool {
    let mut parts: Vec<&str> = file.split('/').collect();
    let name = parts.pop().unwrap_or("");
    if parts.iter().any(|p| SKIP_DIRS.contains(p)) {
        return false;
    }
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| EXTENSIONS.contains(&ext))
}

/// True when `file` equals `entry` or lies under it as a folder (whole segments only).
fn is_under(file: &str, entry: &str) -> bool {
    let entry = entry.trim_end_matches('/');
    file == entry
        || file
            .strip_prefix(entry)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// A cheap, conservative comment filter on the trimmed line.
fn is_comment(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with('#') || t.starts_with("//") || t.starts_with('*') || t.starts_with("/*")
}

/// Reads the `excludePaths` option: repo-relative paths (whole segments) that
/// are not scanned, for example fixtures that contain env reads on purpose.
fn excluded_paths(ctx: &RuleContext) -> Result<Vec<String>> {
    let Some(option) = ctx.options().get("excludePaths") else {
        return Ok(Vec::new());
    };
    let paths: Option<Vec<String>> = option.as_array().and_then(|items| {
        items
            .iter()
            .map(|item| {
                item.as_str()
                    .filter(|path| {
                        !path.is_empty() && !path.starts_with('/') && !path.contains('\\')
                    })
                    .map(str::to_owned)
            })
            .collect()
    });
    paths.ok_or_else(|| {
        Error::user(
            "core/env-example: option \"excludePaths\" must be a list of repo-relative folders or files with forward slashes, for example [\"tests/fixtures\"] or [\"tests/fixtures/\"]. Fix it in your firescale config.",
        )
    })
}

fn scan(ctx: &RuleContext) -> Result<BTreeMap<String, (String, u32)>> {
    let ignore = ignore_list(ctx)?;
    let excluded = excluded_paths(ctx)?;
    let mut found: BTreeMap<String, (String, u32)> = BTreeMap::new();
    for file in ctx.git().tracked_files()? {
        if !is_source(&file) || excluded.iter().any(|entry| is_under(&file, entry)) {
            continue;
        }
        let path = file
            .split('/')
            .fold(ctx.root().to_path_buf(), |p, part| p.join(part));
        match fs::metadata(&path) {
            Ok(meta) if meta.is_file() && meta.len() <= MAX_BYTES => {}
            _ => continue,
        }
        // Binary or unreadable files are skipped rather than failing the run.
        let Ok(bytes) = fs::read(&path) else { continue };
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            if is_comment(line) {
                continue;
            }
            for name in names_in_line(line) {
                if is_ignored(name, &ignore) {
                    continue;
                }
                let line_number = u32::try_from(index + 1).unwrap_or(u32::MAX);
                found
                    .entry(name.to_owned())
                    .or_insert_with(|| (file.clone(), line_number));
            }
        }
    }
    Ok(found)
}

/// Names listed in `.env.example` text.
fn listed_names(text: &str) -> BTreeSet<String> {
    text.trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
            line.split_once('=').map(|(name, _)| name.trim().to_owned())
        })
        .collect()
}

/// True when `.env.example` exists but is a directory or other non-file.
fn example_is_not_a_file(ctx: &RuleContext) -> bool {
    fs::metadata(ctx.root().join(EXAMPLE)).is_ok_and(|m| !m.is_file())
}

impl BuiltinRule for EnvExample {
    fn meta(&self) -> RuleMeta {
        let mut inputs: Vec<String> = EXTENSIONS.iter().map(|e| format!("**/*.{e}")).collect();
        inputs.push(EXAMPLE.to_owned());
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Reliability,
            description: "If code reads env vars, .env.example lists them.".to_owned(),
            inputs,
            has_fix: true,
            after: Vec::new(),
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        let used = scan(ctx)?;
        if used.is_empty() {
            return Ok(Vec::new());
        }
        if example_is_not_a_file(ctx) {
            return Ok(vec![Finding {
                message: ".env.example must be a file, but it is not. Replace it with a file that lists one NAME= line per variable."
                    .to_owned(),
                file: EXAMPLE.to_owned(),
                range: None,
                project: None,
                fixable: Fixable::Manual,
            }]);
        }
        let Some(example) = ctx.read(EXAMPLE)? else {
            let mut names: Vec<&str> = used.keys().map(String::as_str).take(10).collect();
            let more = used.len() - names.len();
            let mut list = names.join(", ");
            if more > 0 {
                list.push_str(&format!(" and {more} more"));
            }
            names.clear();
            return Ok(vec![Finding {
                message: format!(
                    "Code reads env vars but there is no .env.example: {list}. Create .env.example with one NAME= line per variable (no values)."
                ),
                file: EXAMPLE.to_owned(),
                range: None,
                project: None,
                fixable: Fixable::Deterministic,
            }]);
        };
        let listed = listed_names(&example);
        Ok(used
            .into_iter()
            .filter(|(name, _)| !listed.contains(name))
            .map(|(name, (file, line))| Finding {
                message: format!(
                    "{name} is read here but not listed in .env.example. Add a line \"{name}=\" to .env.example."
                ),
                file,
                range: Some(LineRange {
                    start_line: line,
                    end_line: line,
                }),
                project: None,
                fixable: Fixable::Deterministic,
            })
            .collect())
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        // Work from the current state, so a stale diagnostic list cannot cause a second change.
        if example_is_not_a_file(ctx) {
            return Ok(FixResult::default());
        }
        let used = scan(ctx)?;
        let current = ctx.read(EXAMPLE)?;
        let listed = current.as_deref().map(listed_names).unwrap_or_default();
        let missing: Vec<&String> = used.keys().filter(|n| !listed.contains(*n)).collect();
        if missing.is_empty() {
            return Ok(FixResult::default());
        }
        let text = current.unwrap_or_default();
        let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let mut out = text.clone();
        if !text.is_empty() {
            if !text.ends_with('\n') {
                out.push_str(eol);
            }
            out.push_str("# Variables read by the code. Add example values or leave empty.");
            out.push_str(eol);
        }
        for name in missing {
            out.push_str(name);
            out.push('=');
            out.push_str(eol);
        }
        ctx.write(EXAMPLE, &out)?;
        Ok(FixResult {
            changed_files: vec![EXAMPLE.to_owned()],
            notes: Vec::new(),
        })
    }
}
