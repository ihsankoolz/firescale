//! `core/readme-setup`: README has install and run steps.

use std::fs;

use std::path::Path;

use firescale_core::facts::{detect_facts, RepoFacts};
use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::install::install_command;

#[cfg(test)]
mod tests;

pub const ID: &str = "core/readme-setup";

/// The only file the fix ever creates.
const SKELETON: &str = "README.md";

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

fn is_recognised(name: &str) -> bool {
    matches!(
        name.to_lowercase().as_str(),
        "readme.md" | "readme" | "readme.rst" | "readme.txt"
    )
}

/// Markdown first when there are several, then by name.
fn preferred(mut readmes: Vec<String>) -> Option<String> {
    readmes.sort();
    let markdown = readmes.iter().find(|n| n.to_lowercase() == "readme.md");
    markdown.or_else(|| readmes.first()).cloned()
}

/// Names of everything at the repo root on disk: files, folders and links.
fn root_entries(root: &Path) -> Vec<String> {
    fs::read_dir(root)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// Root entries on disk that are, or could be meant as, a README.
fn readme_like_entries(root: &Path) -> Vec<String> {
    root_entries(root)
        .into_iter()
        .filter(|name| name.to_lowercase().starts_with("readme"))
        .collect()
}

/// The root README's name. In a git repo it must be tracked and on disk;
/// otherwise a folder listing decides.
fn find_readme(ctx: &RuleContext) -> Result<Option<String>> {
    let names: Vec<String> = if ctx.git().is_repo()? {
        ctx.git()
            .tracked_files()?
            .into_iter()
            .filter(|f| !f.contains('/') && is_recognised(f))
            .filter(|f| ctx.root().join(f).is_file())
            .collect()
    } else {
        root_entries(ctx.root())
            .into_iter()
            .filter(|name| is_recognised(name) && ctx.root().join(name).is_file())
            .collect()
    };
    Ok(preferred(names))
}

/// The heading of the skeleton: the root project's name. Never the folder
/// name, so a dry run in a scratch copy writes the same file.
fn project_name(facts: &RepoFacts) -> String {
    facts
        .projects
        .iter()
        .filter(|project| project.root == ".")
        .map(|project| project.name.trim())
        .find(|name| !name.is_empty() && !name.chars().any(char::is_control))
        .unwrap_or("Project name")
        .to_owned()
}

fn fenced(commands: &[&str]) -> String {
    format!("```sh\n{}\n```\n", commands.join("\n"))
}

/// The skeleton README. LF line endings.
fn skeleton(facts: &RepoFacts) -> String {
    let install = match facts.package_manager.as_deref() {
        Some(manager) => install_command(manager),
        None if facts.languages.iter().any(|language| language == "rust") => Some("cargo build"),
        None => None,
    };
    let install = install.map_or_else(
        || "Describe how to install the project here.\n".to_owned(),
        |command| fenced(&[command]),
    );
    let commands: Vec<&str> = [&facts.commands.build, &facts.commands.test]
        .into_iter()
        .filter_map(|command| command.as_deref())
        .collect();
    let usage = if commands.is_empty() {
        "Describe how to run the project here.\n".to_owned()
    } else {
        fenced(&commands)
    };
    format!(
        "# {}\n\n<!-- A skeleton created by firescale (core/readme-setup). It is your team's to edit. -->\n\n## Install\n\n{install}\n## Usage\n\n{usage}",
        project_name(facts)
    )
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
            // One glob for every letter case of the name.
            inputs: vec!["[Rr][Ee][Aa][Dd][Mm][Ee]*".to_owned()],
            has_fix: true,
            after: Vec::new(),
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        let Some(file) = find_readme(ctx)? else {
            let finding = |message: String, file: &str, fixable: Fixable| Finding {
                message,
                file: file.to_owned(),
                range: None,
                project: None,
                fixable,
            };
            let like = readme_like_entries(ctx.root());
            // Only reached in a git repo: outside one, a README on disk counts.
            let untracked: Vec<String> = like
                .iter()
                .filter(|name| is_recognised(name) && ctx.root().join(name).is_file())
                .cloned()
                .collect();
            if let Some(name) = preferred(untracked) {
                return Ok(vec![finding(
                    format!(
                        "\"{name}\" exists but is not tracked by git, so it does not count as the README. Run `git add {name}` and commit it."
                    ),
                    &name,
                    Fixable::Manual,
                )]);
            }
            if !like.is_empty() {
                // Something README-like is there; a second README beside it would be wrong.
                return Ok(vec![finding(
                    "No README found at the repo root. Add a README.md that explains how to install and run the project.".to_owned(),
                    SKELETON,
                    Fixable::Agent,
                )]);
            }
            return Ok(vec![finding(
                "No README found at the repo root. Add a README.md that explains how to install and run the project; `fscale fix` can create a skeleton.".to_owned(),
                SKELETON,
                Fixable::Deterministic,
            )]);
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

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        // Never trust the diagnostics alone: look at the disk again, and only
        // create a file where nothing README-like exists.
        if !readme_like_entries(ctx.root()).is_empty() {
            return Ok(FixResult::default());
        }
        let facts = detect_facts(ctx.root())?;
        ctx.write(SKELETON, &skeleton(&facts))?;
        ctx.track(SKELETON)?;
        Ok(FixResult {
            changed_files: vec![SKELETON.to_owned()],
            notes: vec![format!(
                "{SKELETON} is a skeleton. Describe the project and complete the Install and Usage sections."
            )],
        })
    }
}
