//! `core/no-secrets-in-source`: no obvious keys or tokens are in tracked files.
//!
//! Only well-known, high-confidence formats are detected. A line that holds
//! the marker `firescale:allow-secret` is never reported.

use std::fs;
use std::path::Path;

use firescale_core::model::{Category, Finding, Fixable, LineRange, RuleMeta, Severity};
use firescale_core::rule::{BuiltinRule, Options, RuleContext};
use firescale_core::{Error, Result};
use serde_json::Value;

#[cfg(test)]
mod tests;

pub const ID: &str = "core/no-secrets-in-source";

/// Put this marker on a line to tell the rule that it is not a secret.
pub const ALLOW_MARKER: &str = "firescale:allow-secret";

const MAX_BYTES: u64 = 1_000_000;

const LOCKFILES: [&str; 10] = [
    "pnpm-lock.yaml",
    "package-lock.json",
    "yarn.lock",
    "Cargo.lock",
    "bun.lock",
    "poetry.lock",
    "uv.lock",
    "go.sum",
    "composer.lock",
    "Gemfile.lock",
];

const SKIP_SEGMENTS: [&str; 5] = ["node_modules", "dist", "build", "target", "vendor"];

const SKIP_SUFFIXES: [&str; 12] = [
    ".min.js", ".map", ".svg", ".png", ".jpg", ".jpeg", ".gif", ".ico", ".pdf", ".woff", ".woff2",
    ".snap",
];

const PLACEHOLDER_WORDS: [&str; 6] = [
    "EXAMPLE",
    "example",
    "xxxx",
    "XXXX",
    "0000000000",
    "placeholder",
];

const KEY_BEGIN: &str = "-----BEGIN ";
const KEY_END: &str = "PRIVATE KEY-----";

pub struct NoSecretsInSource;

#[derive(Clone, Copy)]
enum Class {
    Alnum,
    AlnumUnderscore,
    AlnumDash,
    AlnumUnderscoreDash,
    UpperAlnum,
}

impl Class {
    fn has(self, b: u8) -> bool {
        match self {
            Class::Alnum => b.is_ascii_alphanumeric(),
            Class::AlnumUnderscore => b.is_ascii_alphanumeric() || b == b'_',
            Class::AlnumDash => b.is_ascii_alphanumeric() || b == b'-',
            Class::AlnumUnderscoreDash => b.is_ascii_alphanumeric() || b == b'_' || b == b'-',
            Class::UpperAlnum => b.is_ascii_uppercase() || b.is_ascii_digit(),
        }
    }
}

/// How long the run after the prefix must be.
#[derive(Clone, Copy)]
enum Len {
    AtLeast(usize),
    Exactly(usize),
}

struct Format {
    kind: &'static str,
    prefixes: &'static [&'static str],
    class: Class,
    len: Len,
}

const FORMATS: [Format; 7] = [
    Format {
        kind: "an AWS access key ID",
        prefixes: &["AKIA", "ASIA"],
        class: Class::UpperAlnum,
        len: Len::Exactly(16),
    },
    Format {
        kind: "a GitHub token",
        prefixes: &["ghp_", "gho_", "ghu_", "ghs_", "ghr_"],
        class: Class::Alnum,
        len: Len::AtLeast(36),
    },
    Format {
        kind: "a GitHub token",
        prefixes: &["github_pat_"],
        class: Class::AlnumUnderscore,
        len: Len::AtLeast(60),
    },
    Format {
        kind: "a Slack token",
        prefixes: &["xoxb-", "xoxp-", "xoxa-", "xoxr-", "xoxs-"],
        class: Class::AlnumDash,
        len: Len::AtLeast(20),
    },
    Format {
        kind: "a Stripe live key",
        prefixes: &["sk_live_", "rk_live_"],
        class: Class::Alnum,
        len: Len::AtLeast(20),
    },
    Format {
        kind: "a Google API key",
        prefixes: &["AIza"],
        class: Class::AlnumUnderscoreDash,
        len: Len::Exactly(35),
    },
    Format {
        kind: "an npm token",
        prefixes: &["npm_"],
        class: Class::Alnum,
        len: Len::Exactly(36),
    },
];

fn is_placeholder(after_prefix: &str) -> bool {
    let mut chars = after_prefix.chars();
    let single_char = match chars.next() {
        Some(first) => chars.all(|c| c == first),
        None => true,
    };
    single_char
        || PLACEHOLDER_WORDS
            .iter()
            .any(|word| after_prefix.contains(word))
}

/// Tries every format at byte offset `at`. Returns the kind and the end offset.
fn match_at(line: &str, at: usize) -> Option<(&'static str, usize)> {
    let bytes = line.as_bytes();
    let rest = line.get(at..)?;
    for format in &FORMATS {
        for prefix in format.prefixes {
            if !rest.starts_with(prefix) {
                continue;
            }
            let start = at + prefix.len();
            let run = bytes[start..]
                .iter()
                .take_while(|b| format.class.has(**b))
                .count();
            let ok = match format.len {
                Len::AtLeast(n) => run >= n,
                Len::Exactly(n) => run == n,
            };
            if !ok {
                continue;
            }
            let end = start + run;
            if matches!(format.len, Len::Exactly(_))
                && bytes
                    .get(end)
                    .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
            {
                continue;
            }
            if is_placeholder(&line[start..end]) {
                continue;
            }
            return Some((format.kind, end));
        }
    }
    None
}

fn is_base64_body(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 40
        && t.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
}

/// The label between `-----BEGIN ` and `PRIVATE KEY-----` when `line` is a header at its start.
fn private_key_header(line: &str) -> Option<&str> {
    let rest = line.trim_start().strip_prefix(KEY_BEGIN)?;
    let at = rest.find(KEY_END)?;
    Some(&rest[..at])
}

/// True when a header at `lines[index]` is followed by key material: a base64
/// body line within 3 lines, or the matching END line within 200 lines.
fn has_key_material(lines: &[&str], index: usize, label: &str) -> bool {
    let end_line = format!("-----END {label}{KEY_END}");
    let body = lines
        .iter()
        .skip(index + 1)
        .take(3)
        .any(|l| is_base64_body(l));
    body || lines
        .iter()
        .skip(index + 1)
        .take(200)
        .any(|l| l.trim_start().starts_with(&end_line))
}

/// Finds secrets in a text. Returns `(1-based line, kind)` for each match.
/// The secret text itself is never returned.
#[must_use]
pub fn find_secrets(text: &str) -> Vec<(usize, &'static str)> {
    let mut found = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    for (index, line) in lines.iter().copied().enumerate() {
        if line.contains(ALLOW_MARKER) {
            continue;
        }
        let number = index + 1;
        if let Some(label) = private_key_header(line) {
            if has_key_material(&lines, index, label) {
                found.push((number, "a private key"));
            }
        }
        let bytes = line.as_bytes();
        let mut at = 0;
        while at < bytes.len() {
            let boundary =
                at == 0 || !(bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_');
            if boundary && line.is_char_boundary(at) {
                if let Some((kind, end)) = match_at(line, at) {
                    found.push((number, kind));
                    at = end;
                    continue;
                }
            }
            at += 1;
        }
    }
    found
}

fn is_skipped_path(file: &str) -> bool {
    let name = file.rsplit('/').next().unwrap_or(file);
    LOCKFILES.contains(&name)
        || file.split('/').any(|part| SKIP_SEGMENTS.contains(&part))
        || SKIP_SUFFIXES
            .iter()
            .any(|suffix| name.to_ascii_lowercase().ends_with(suffix))
}

fn allow_files(options: &Options) -> Result<Vec<String>> {
    let Some(value) = options.get("allowFiles") else {
        return Ok(Vec::new());
    };
    let files: Option<Vec<String>> = value.as_array().and_then(|items| {
        items
            .iter()
            .map(|item| match item {
                Value::String(path)
                    if !path.is_empty()
                        && !path.contains('\\')
                        && !path.starts_with('/')
                        && !path.ends_with('/')
                        && !path.split('/').any(|p| p.is_empty() || p == "..") =>
                {
                    Some(path.clone())
                }
                _ => None,
            })
            .collect()
    });
    files.ok_or_else(|| {
        Error::user(
            "core/no-secrets-in-source: option \"allowFiles\" must be a list of file paths relative to the repo root, written with forward slashes, for example [\"docs/sample-config.txt\"]. Fix it in your firescale config.",
        )
    })
}

/// Reads a file as text. `None` when it is too big, binary, not UTF-8 or unreadable.
fn read_text(root: &Path, file: &str) -> Option<String> {
    let path = file
        .split('/')
        .fold(root.to_path_buf(), |p, part| p.join(part));
    let meta = fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return None;
    }
    let bytes = fs::read(&path).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

impl BuiltinRule for NoSecretsInSource {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Blocker,
            category: Category::Security,
            description: "No obvious keys or tokens are in tracked files.".to_owned(),
            inputs: vec!["**/*".to_owned()],
            has_fix: false,
            after: Vec::new(),
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        let allowed = allow_files(ctx.options())?;
        let mut findings = Vec::new();
        for file in ctx.git().tracked_files()? {
            if is_skipped_path(&file) || allowed.contains(&file) {
                continue;
            }
            let Some(text) = read_text(ctx.root(), &file) else {
                continue;
            };
            for (line, kind) in find_secrets(&text) {
                let number = u32::try_from(line).unwrap_or(u32::MAX);
                findings.push(Finding {
                    message: format!(
                        "\"{file}\" line {line} contains what looks like {kind}. Rotate it at the provider now, then remove it from the file and from git history. If it is not a secret, add the marker {ALLOW_MARKER} to that line or list the file in the \"allowFiles\" option."
                    ),
                    file: file.clone(),
                    range: Some(LineRange {
                        start_line: number,
                        end_line: number,
                    }),
                    project: None,
                    fixable: Fixable::Manual,
                    severity: None,
                });
            }
        }
        Ok(findings)
    }
}
