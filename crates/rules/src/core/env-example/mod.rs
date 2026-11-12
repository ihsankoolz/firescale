//! `core/env-example`: If code reads env vars, .env.example lists them.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, LineRange, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::{Error, Result};

#[cfg(test)]
mod tests;
mod well_known;

pub const ID: &str = "core/env-example";

const EXAMPLE: &str = ".env.example";
const EXTENSIONS: [&str; 12] = [
    "js", "jsx", "mjs", "cjs", "ts", "tsx", "mts", "cts", "py", "rs", "go", "rb",
];
const SKIP_DIRS: [&str; 6] = ["node_modules", "dist", "build", "target", "vendor", ".git"];
const MAX_BYTES: u64 = 1_000_000;

/// Folder names that hold test code, wherever they sit in the path.
const TEST_DIRS: [&str; 13] = [
    "test",
    "tests",
    "__tests__",
    "unit_test",
    "unit_tests",
    "integration_test",
    "integration_tests",
    "e2e",
    "spec",
    "specs",
    "testing",
    "cypress",
    "playwright",
];

const CODE_COMMENT: &str = "# Variables read by the code. Add example values or leave empty.";
const TEST_COMMENT: &str = "# Test-only variables: read only by test code.";

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
    let mut ignore = BTreeSet::new();
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
    well_known::is_well_known(name) || ignore.contains(name)
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

/// True when a repo-relative path is test code. This is the rule's one
/// definition of "test code"; SKILL.md describes it in words.
///
/// Test code is a file below a folder named in `TEST_DIRS`, a file named
/// `conftest.py`, a Python `test_*.py` or `*_test.py`, a Go `*_test.go`, and
/// any `*.test.*` or `*.spec.*` file.
fn is_test_path(file: &str) -> bool {
    let mut folders: Vec<&str> = file.split('/').collect();
    let name = folders.pop().unwrap_or("");
    if folders.iter().any(|folder| TEST_DIRS.contains(folder)) || name == "conftest.py" {
        return true;
    }
    let (stem, extension) = name.rsplit_once('.').unwrap_or((name, ""));
    let by_language = match extension {
        "py" => stem.starts_with("test_") || stem.ends_with("_test"),
        "go" => stem.ends_with("_test"),
        _ => false,
    };
    // `api.test.ts`, `login.spec.js`: `test` or `spec` between two dots.
    let pieces: Vec<&str> = name.split('.').collect();
    let by_infix = pieces.len() >= 3
        && pieces[1..pieces.len() - 1]
            .iter()
            .any(|piece| *piece == "test" || *piece == "spec");
    by_language || by_infix
}

/// The folder part of a repo-relative path. Empty for a file in the root.
fn folder_of(file: &str) -> &str {
    file.rsplit_once('/').map_or("", |(folder, _)| folder)
}

/// The folder above `folder`, or `None` when `folder` is the repo root.
fn parent_of(folder: &str) -> Option<&str> {
    (!folder.is_empty()).then(|| folder_of(folder))
}

/// The `.env.example` path that belongs to a folder.
fn example_in(folder: &str) -> String {
    if folder.is_empty() {
        EXAMPLE.to_owned()
    } else {
        format!("{folder}/{EXAMPLE}")
    }
}

fn disk_path(ctx: &RuleContext, file: &str) -> PathBuf {
    file.split('/')
        .fold(ctx.root().to_path_buf(), |path, part| path.join(part))
}

/// A file and a 1-based line.
type Place = (String, u32);

/// Where one name is read: the first read in other code and the first read in test code.
#[derive(Default)]
struct Reads {
    code: Option<Place>,
    test: Option<Place>,
}

impl Reads {
    fn add(&mut self, place: Place, test: bool) {
        let slot = if test { &mut self.test } else { &mut self.code };
        if slot.as_ref().is_none_or(|first| place < *first) {
            *slot = Some(place);
        }
    }

    fn merge(&mut self, other: Self) {
        if let Some(place) = other.code {
            self.add(place, false);
        }
        if let Some(place) = other.test {
            self.add(place, true);
        }
    }
}

/// Every env read in tracked source, keyed by name and by the folder of the reading file.
fn scan(ctx: &RuleContext) -> Result<BTreeMap<(String, String), Reads>> {
    let ignore = ignore_list(ctx)?;
    let excluded = excluded_paths(ctx)?;
    let mut found: BTreeMap<(String, String), Reads> = BTreeMap::new();
    for file in ctx.git().tracked_files()? {
        if !is_source(&file) || excluded.iter().any(|entry| is_under(&file, entry)) {
            continue;
        }
        let path = disk_path(ctx, &file);
        match fs::metadata(&path) {
            Ok(meta) if meta.is_file() && meta.len() <= MAX_BYTES => {}
            _ => continue,
        }
        // Binary or unreadable files are skipped rather than failing the run.
        let Ok(bytes) = fs::read(&path) else { continue };
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        let test = is_test_path(&file);
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
                    .entry((name.to_owned(), folder_of(&file).to_owned()))
                    .or_default()
                    .add((file.clone(), line_number), test);
            }
        }
    }
    Ok(found)
}

/// The names each `.env.example` lists, read from disk once per folder.
struct Examples<'a> {
    ctx: &'a RuleContext,
    seen: BTreeMap<String, Option<BTreeSet<String>>>,
}

impl<'a> Examples<'a> {
    fn new(ctx: &'a RuleContext) -> Self {
        Self {
            ctx,
            seen: BTreeMap::new(),
        }
    }

    /// The names listed by the `.env.example` in `folder`. `None` when that
    /// folder has no such file. A folder or other non-file with that name
    /// counts as absent.
    fn names(&mut self, folder: &str) -> Result<Option<&BTreeSet<String>>> {
        if !self.seen.contains_key(folder) {
            let file = example_in(folder);
            let names = if fs::metadata(disk_path(self.ctx, &file)).is_ok_and(|m| m.is_file()) {
                self.ctx.read(&file)?.as_deref().map(listed_names)
            } else {
                None
            };
            self.seen.insert(folder.to_owned(), names);
        }
        Ok(self.seen.get(folder).and_then(Option::as_ref))
    }
}

/// One name that an example file should list and does not.
struct Missing {
    /// The `.env.example` the name belongs in.
    target: String,
    name: String,
    /// The read to point at: the first read outside tests, else the first read in tests.
    place: Place,
    test_only: bool,
}

struct Unlisted {
    missing: Vec<Missing>,
    root_exists: bool,
}

/// Checks every read against the example files in its folder and the folders
/// above it. A read is satisfied when any of them lists the name. A name that
/// none lists belongs in the nearest one, or in the root file when there is none.
fn unlisted(ctx: &RuleContext, used: BTreeMap<(String, String), Reads>) -> Result<Unlisted> {
    let mut examples = Examples::new(ctx);
    let mut by_target: BTreeMap<(String, String), Reads> = BTreeMap::new();
    for ((name, folder), reads) in used {
        let mut nearest: Option<String> = None;
        let mut satisfied = false;
        let mut at = Some(folder.as_str());
        while let Some(current) = at {
            if let Some(names) = examples.names(current)? {
                if names.contains(&name) {
                    satisfied = true;
                    break;
                }
                nearest.get_or_insert_with(|| example_in(current));
            }
            at = parent_of(current);
        }
        if !satisfied {
            let target = nearest.unwrap_or_else(|| EXAMPLE.to_owned());
            by_target.entry((target, name)).or_default().merge(reads);
        }
    }
    let root_exists = examples.names("")?.is_some();
    let missing = by_target
        .into_iter()
        .filter_map(|((target, name), reads)| {
            let test_only = reads.code.is_none();
            let place = reads.code.or(reads.test)?;
            Some(Missing {
                target,
                name,
                place,
                test_only,
            })
        })
        .collect();
    Ok(Unlisted {
        missing,
        root_exists,
    })
}

/// True when the root `.env.example` exists but is a directory or other non-file.
fn example_is_not_a_file(ctx: &RuleContext) -> bool {
    fs::metadata(ctx.root().join(EXAMPLE)).is_ok_and(|m| !m.is_file())
}

/// The one finding for a repo whose root `.env.example` does not exist yet.
fn summary_finding(missing: &[&Missing]) -> Finding {
    let mut list = missing
        .iter()
        .take(10)
        .map(|item| item.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let more = missing.len().saturating_sub(10);
    if more > 0 {
        list.push_str(&format!(" and {more} more"));
    }
    let test_only = missing.iter().all(|item| item.test_only);
    let message = if test_only {
        format!(
            "Only test code reads env vars, and there is no .env.example: {list}. Create .env.example with one NAME= line per variable (no values)."
        )
    } else {
        format!(
            "Code reads env vars but there is no .env.example: {list}. Create .env.example with one NAME= line per variable (no values)."
        )
    };
    Finding {
        message,
        file: EXAMPLE.to_owned(),
        range: None,
        project: None,
        fixable: Fixable::Deterministic,
        severity: test_only.then_some(Severity::Low),
    }
}

fn name_finding(item: &Missing) -> Finding {
    let Missing { target, name, .. } = item;
    let (file, line) = item.place.clone();
    let message = if item.test_only {
        format!(
            "{name} is read here, only by test code, and is not listed in {target}. Add a line \"{name}=\" to {target}."
        )
    } else {
        format!(
            "{name} is read here but not listed in {target}. Add a line \"{name}=\" to {target}."
        )
    };
    Finding {
        message,
        file,
        range: Some(LineRange {
            start_line: line,
            end_line: line,
        }),
        project: None,
        fixable: Fixable::Deterministic,
        severity: item.test_only.then_some(Severity::Low),
    }
}

/// Appends `names` to `out` as `NAME=` lines, under `comment` when one is given.
/// The comment is left out when it is already the last comment in the text.
fn append_block(out: &mut String, comment: Option<&str>, names: &[&str], eol: &str) {
    if names.is_empty() {
        return;
    }
    if let Some(comment) = comment {
        let last_comment = out
            .lines()
            .map(str::trim)
            .rfind(|line| line.starts_with('#'));
        if last_comment != Some(comment) {
            out.push_str(comment);
            out.push_str(eol);
        }
    }
    for name in names {
        out.push_str(name);
        out.push('=');
        out.push_str(eol);
    }
}

/// The new text of one example file: the old text, then the missing names.
fn with_missing(current: &str, code: &[&str], tests: &[&str]) -> String {
    let eol = if current.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut out = current.to_owned();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push_str(eol);
    }
    // A new file needs no heading for the names the code reads.
    let code_comment = (!current.is_empty()).then_some(CODE_COMMENT);
    append_block(&mut out, code_comment, code, eol);
    append_block(&mut out, Some(TEST_COMMENT), tests, eol);
    out
}

impl BuiltinRule for EnvExample {
    fn meta(&self) -> RuleMeta {
        let mut inputs: Vec<String> = EXTENSIONS.iter().map(|e| format!("**/*.{e}")).collect();
        inputs.push(EXAMPLE.to_owned());
        inputs.push(format!("**/{EXAMPLE}"));
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
                severity: None,
            }]);
        }
        let Unlisted {
            missing,
            root_exists,
        } = unlisted(ctx, used)?;
        // Names that belong in a root file that does not exist yet share one finding.
        let (for_new_root, rest): (Vec<&Missing>, Vec<&Missing>) = missing
            .iter()
            .partition(|item| !root_exists && item.target == EXAMPLE);
        let mut findings: Vec<Finding> = rest.into_iter().map(name_finding).collect();
        if !for_new_root.is_empty() {
            findings.push(summary_finding(&for_new_root));
        }
        Ok(findings)
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        // Work from the current state, so a stale diagnostic list cannot cause a second change.
        if example_is_not_a_file(ctx) {
            return Ok(FixResult::default());
        }
        let Unlisted { missing, .. } = unlisted(ctx, scan(ctx)?)?;
        let mut by_target: BTreeMap<&str, (Vec<&str>, Vec<&str>)> = BTreeMap::new();
        for item in &missing {
            let (code, tests) = by_target.entry(item.target.as_str()).or_default();
            if item.test_only {
                tests.push(item.name.as_str());
            } else {
                code.push(item.name.as_str());
            }
        }
        let mut changed_files = Vec::new();
        for (target, (code, tests)) in by_target {
            let current = ctx.read(target)?.unwrap_or_default();
            ctx.write(target, &with_missing(&current, &code, &tests))?;
            changed_files.push(target.to_owned());
        }
        Ok(FixResult {
            changed_files,
            notes: Vec::new(),
        })
    }
}
