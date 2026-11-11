//! `core/ci-exists`: A CI workflow exists and runs on pull requests.

use std::fs;

use firescale_core::model::{Category, Finding, Fixable, RuleMeta, Severity};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

#[cfg(test)]
mod tests;

pub const ID: &str = "core/ci-exists";

const WORKFLOWS: &str = ".github/workflows";

/// Configs of other CI systems. The rule only understands GitHub Actions
/// triggers, so any of these at the root means there is nothing to report.
const OTHER_CI: [&str; 14] = [
    ".gitlab-ci.yml",
    ".circleci/config.yml",
    "azure-pipelines.yml",
    "bitbucket-pipelines.yml",
    "Jenkinsfile",
    ".travis.yml",
    ".gitlab-ci.yaml",
    ".circleci/config.yaml",
    ".drone.yml",
    ".woodpecker.yml",
    ".buildkite/pipeline.yml",
    ".buildkite/pipeline.yaml",
    "appveyor.yml",
    ".appveyor.yml",
];

pub struct CiExists;

fn is_workflow(file: &str) -> bool {
    file.strip_prefix(".github/workflows/").is_some_and(|rest| {
        !rest.contains('/') && (rest.ends_with(".yml") || rest.ends_with(".yaml"))
    })
}

/// Tracked files in a git repo; otherwise a plain listing of the folders that matter.
fn candidate_files(ctx: &RuleContext) -> Result<Vec<String>> {
    if ctx.git().is_repo()? {
        return ctx.git().tracked_files();
    }
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(ctx.root().join(".github").join("workflows")) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                files.push(format!("{WORKFLOWS}/{name}"));
            }
        }
    }
    for other in OTHER_CI {
        let path = other
            .split('/')
            .fold(ctx.root().to_path_buf(), |p, part| p.join(part));
        if path.is_file() {
            files.push(other.to_owned());
        }
    }
    files.sort();
    Ok(files)
}

/// Removes a `#` comment: a `#` at the start of the text or after whitespace.
fn strip_comment(line: &str) -> &str {
    let mut prev_space = true;
    for (i, c) in line.char_indices() {
        if c == '#' && prev_space {
            return &line[..i];
        }
        prev_space = c.is_whitespace();
    }
    line
}

fn unquote(s: &str) -> &str {
    let s = s.trim();
    for q in ['"', '\''] {
        if let Some(inner) = s.strip_prefix(q).and_then(|r| r.strip_suffix(q)) {
            return inner.trim();
        }
    }
    s
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Splits `key: value` at the first colon that ends the key.
fn key_of(entry: &str) -> &str {
    let key = entry.split(':').next().unwrap_or(entry);
    unquote(key)
}

/// The trigger names listed under the top-level `on` key.
///
/// Reads these shapes: scalar, flow list, flow mapping, block list and block
/// mapping (first nesting level only).
fn triggers(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().map(strip_comment).collect();
    let mut out = Vec::new();
    let Some(start) = lines.iter().position(|l| {
        let t = l.trim_end();
        indent_of(t) == 0 && ["on:", "\"on\":", "'on':"].iter().any(|p| t.starts_with(p))
    }) else {
        return out;
    };
    let head = lines[start].trim_end();
    let rest = head[head.find(':').map_or(0, |i| i + 1)..].trim();
    if !rest.is_empty() {
        if let Some(inner) = rest.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            out.extend(inner.split(',').map(|i| unquote(i).to_owned()));
        } else if let Some(inner) = rest.strip_prefix('{').and_then(|r| r.strip_suffix('}')) {
            out.extend(inner.split(',').map(|i| key_of(i).to_owned()));
        } else {
            out.push(unquote(rest).to_owned());
        }
        return out;
    }
    let mut level: Option<usize> = None;
    for line in &lines[start + 1..] {
        if line.trim().is_empty() {
            continue;
        }
        let indent = indent_of(line);
        let t = line.trim();
        if indent == 0 && !t.starts_with('-') {
            break;
        }
        let lvl = *level.get_or_insert(indent);
        if indent != lvl {
            continue;
        }
        match t.strip_prefix('-') {
            Some(item) => out.push(unquote(item).to_owned()),
            None => out.push(key_of(t).to_owned()),
        }
    }
    out
}

/// True when the workflow's `on` key lists a pull request trigger.
fn mentions_pull_request(text: &str) -> bool {
    triggers(text)
        .iter()
        .any(|t| t == "pull_request" || t == "pull_request_target")
}

impl BuiltinRule for CiExists {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Reliability,
            description: "A CI workflow exists and runs on pull requests.".to_owned(),
            inputs: [".github/workflows/*.yml", ".github/workflows/*.yaml"]
                .iter()
                .chain(OTHER_CI.iter())
                .map(|s| (*s).to_owned())
                .collect(),
            has_fix: false,
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        let files = candidate_files(ctx)?;
        let workflows: Vec<&String> = files.iter().filter(|f| is_workflow(f)).collect();
        if workflows.is_empty() {
            if files.iter().any(|f| OTHER_CI.contains(&f.as_str())) {
                return Ok(Vec::new());
            }
            return Ok(vec![Finding {
                message: "No CI workflow found in .github/workflows. Add a workflow that runs your build and tests on pull requests (see SKILL.md).".to_owned(),
                file: WORKFLOWS.to_owned(),
                range: None,
                project: None,
                fixable: Fixable::Agent,
            }]);
        }
        for file in &workflows {
            if let Some(text) = ctx.read(file)? {
                if mentions_pull_request(&text) {
                    return Ok(Vec::new());
                }
            }
        }
        let first = workflows[0].clone();
        Ok(vec![Finding {
            message: format!(
                "No workflow in .github/workflows runs on pull requests. Add a `pull_request` trigger under `on:` in \"{first}\" so changes are checked before they merge."
            ),
            file: first,
            range: None,
            project: None,
            fixable: Fixable::Agent,
        }])
    }
}
