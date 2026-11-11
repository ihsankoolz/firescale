//! `core/readme-setup`: README has install and run steps.

use std::fs;

use firescale_core::model::{Category, Finding, Fixable, RuleMeta, Severity};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

#[cfg(test)]
mod tests;

pub const ID: &str = "core/readme-setup";

/// Heading words and phrases, matched as whole words.
const INSTALL_HEADINGS: [&str; 13] = [
    "install",
    "installation",
    "installing",
    "setup",
    "set up",
    "getting started",
    "quick start",
    "quickstart",
    "development",
    "dev setup",
    "prerequisites",
    "build",
    "building",
];

const RUN_HEADINGS: [&str; 14] = [
    "usage",
    "run",
    "running",
    "start",
    "starting",
    "getting started",
    "quick start",
    "quickstart",
    "development",
    "examples",
    "example",
    "commands",
    "test",
    "testing",
];

const INSTALL_COMMANDS: [&str; 16] = [
    "npm install",
    "npm ci",
    "pnpm install",
    "pnpm i",
    "yarn install",
    "yarn",
    "bun install",
    "pip install",
    "uv sync",
    "poetry install",
    "cargo build",
    "cargo install",
    "go mod download",
    "bundle install",
    "make install",
    "npm i",
];

const RUN_COMMANDS: [&str; 17] = [
    "npm start",
    "npm run",
    "pnpm run",
    "pnpm dev",
    "pnpm start",
    "pnpm build",
    "pnpm test",
    "yarn dev",
    "yarn start",
    "cargo run",
    "cargo test",
    "python ",
    "uv run",
    "go run",
    "make",
    "docker compose up",
    "npx ",
];

pub struct ReadmeSetup;

/// The root README's name: tracked files in a git repo, a folder listing otherwise.
fn find_readme(ctx: &RuleContext) -> Result<Option<String>> {
    let names: Vec<String> = if ctx.git().is_repo()? {
        ctx.git()
            .tracked_files()?
            .into_iter()
            .filter(|f| !f.contains('/'))
            .collect()
    } else {
        match fs::read_dir(ctx.root()) {
            Ok(entries) => entries
                .flatten()
                .filter(|e| e.path().is_file())
                .filter_map(|e| e.file_name().to_str().map(str::to_owned))
                .collect(),
            Err(_) => Vec::new(),
        }
    };
    let mut readmes: Vec<String> = names
        .into_iter()
        .filter(|name| {
            let lower = name.to_lowercase();
            matches!(
                lower.as_str(),
                "readme.md" | "readme" | "readme.rst" | "readme.txt"
            )
        })
        .collect();
    readmes.sort();
    // Prefer Markdown when there are several.
    let preferred = readmes.iter().find(|n| n.to_lowercase() == "readme.md");
    Ok(preferred.or_else(|| readmes.first()).cloned())
}

fn is_underline(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 3 && t.chars().all(|c| matches!(c, '=' | '-' | '~' | '^' | '*'))
}

/// Heading texts, lower case: Markdown `#` lines and underlined lines.
fn headings(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut fenced = false;
    for (i, line) in lines.iter().enumerate() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let t = line.trim();
        if t.starts_with('#') {
            out.push(t.trim_start_matches('#').trim().to_lowercase());
        } else if !t.is_empty() && lines.get(i + 1).is_some_and(|next| is_underline(next)) {
            out.push(t.to_lowercase());
        }
    }
    out
}

/// Text that looks like commands: fenced or indented code, inline code, and `$ ` lines.
fn code_segments(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut fenced = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        let indented = line.starts_with("    ") || line.starts_with('\t');
        let t = line.trim();
        if fenced || indented || t.starts_with("$ ") {
            out.push(t.trim_start_matches("$ ").to_lowercase());
        }
        let mut parts = line.split('`');
        parts.next();
        for (n, part) in parts.enumerate() {
            if n % 2 == 0 {
                out.push(part.trim().to_lowercase());
            }
        }
    }
    out
}

/// True when `command` appears at a word start and ends at a word end.
fn contains_command(segment: &str, command: &str) -> bool {
    let mut from = 0;
    while let Some(found) = segment[from..].find(command) {
        let start = from + found;
        let end = start + command.len();
        let before_ok = segment[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '-' || c == '_' || c == '/'));
        let after_ok = command.ends_with(' ')
            || segment[end..]
                .chars()
                .next()
                .is_none_or(|c| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '`'));
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

/// Lower-case words of `text`; a word is a maximal run of letters.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphabetic())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// True when the words of `phrase` appear in `heading` as whole, adjacent words.
fn heading_has_phrase(heading: &[String], phrase: &str) -> bool {
    let wanted = words(phrase);
    !wanted.is_empty()
        && heading
            .windows(wanted.len())
            .any(|w| w == wanted.as_slice())
}

fn satisfied(text: &str, heading_keys: &[&str], commands: &[&str]) -> bool {
    if headings(text).iter().any(|h| {
        let heading = words(h);
        heading_keys.iter().any(|k| heading_has_phrase(&heading, k))
    }) {
        return true;
    }
    code_segments(text)
        .iter()
        .any(|seg| commands.iter().any(|c| contains_command(seg, c)))
}

impl BuiltinRule for ReadmeSetup {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Low,
            category: Category::Maintainability,
            description: "README has install and run steps.".to_owned(),
            inputs: vec!["README*".to_owned(), "readme*".to_owned()],
            has_fix: false,
            after: Vec::new(),
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        let Some(file) = find_readme(ctx)? else {
            return Ok(vec![Finding {
                message: "No README found at the repo root. Add a README.md that explains how to install and run the project.".to_owned(),
                file: "README.md".to_owned(),
                range: None,
                project: None,
                fixable: Fixable::Agent,
            }]);
        };
        // Bytes plus a lossy decode: a README in another encoding must not abort the run.
        let path = file
            .split('/')
            .fold(ctx.root().to_path_buf(), |p, part| p.join(part));
        let text = fs::read(&path)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default();
        let mut findings = Vec::new();
        let mut missing = |part: &str, hint: &str| {
            findings.push(Finding {
                message: format!(
                    "\"{file}\" does not show how to {part}. Add {hint} (a heading or a command in a code block)."
                ),
                file: file.clone(),
                range: None,
                project: None,
                fixable: Fixable::Agent,
            });
        };
        if !satisfied(&text, &INSTALL_HEADINGS, &INSTALL_COMMANDS) {
            missing(
                "install the project",
                "an \"Install\" section with the install command",
            );
        }
        if !satisfied(&text, &RUN_HEADINGS, &RUN_COMMANDS) {
            missing(
                "run the project",
                "a \"Usage\" section with the command that runs it",
            );
        }
        Ok(findings)
    }
}
