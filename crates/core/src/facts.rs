//! Repo facts for `fscale facts`: stack detection from marker files and a
//! simple project graph built from workspace files (see docs/facts.md).
//!
//! There is no TOML or YAML parser here. Small line-based readers handle the
//! common forms, and the limits are documented. Everything is read from disk;
//! a missing or unreadable optional file is not an error, and a malformed
//! marker file becomes a warning.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::error::{Error, Result};

/// A project of the repo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub name: String,
    /// Repo-relative, forward slashes, `.` for the repo root.
    pub root: String,
    /// `node` or `rust`.
    pub kind: String,
    pub depends_on: Vec<String>,
}

/// Commands to run, by purpose. Absent when the repo has no such script.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Commands {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub typecheck: Option<String>,
}

/// Everything `fscale facts` reports.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoFacts {
    pub schema_version: u32,
    pub package_manager: Option<String>,
    pub lockfiles: Vec<String>,
    pub languages: Vec<String>,
    pub tools: Vec<String>,
    pub commands: Commands,
    pub projects: Vec<Project>,
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------- readers

fn read_text(root: &Path, file: &str) -> Option<String> {
    let path = file
        .split('/')
        .fold(root.to_path_buf(), |path, part| path.join(part));
    fs::read_to_string(path).ok()
}

fn exists(root: &Path, file: &str) -> bool {
    file.split('/')
        .fold(root.to_path_buf(), |path, part| path.join(part))
        .exists()
}

fn read_json(root: &Path, file: &str, warnings: &mut Vec<String>) -> Option<Value> {
    let text = read_text(root, file)?;
    match serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')) {
        Ok(value) => Some(value),
        Err(error) => {
            warnings.push(format!(
                "{file} is not valid JSON ({error}). Fix the syntax so its facts can be read."
            ));
            None
        }
    }
}

/// Removes a `#` comment that is not inside quotes.
fn strip_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    for (index, c) in line.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (None, '"' | '\'') => quote = Some(c),
            (None, '#') => return &line[..index],
            _ => {}
        }
    }
    line
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    for q in ['"', '\''] {
        if value.len() >= 2 && value.starts_with(q) && value.ends_with(q) {
            return value[1..value.len() - 1].to_owned();
        }
    }
    value.to_owned()
}

/// Every quoted string inside an array value such as `["a", 'b']`.
fn quoted_strings(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '"' || c == '\'' {
            let mut item = String::new();
            for d in chars.by_ref() {
                if d == c {
                    break;
                }
                item.push(d);
            }
            out.push(item);
        }
    }
    out
}

/// One `key = value` line of a TOML file with the section it is under.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TomlEntry {
    section: String,
    key: String,
    value: String,
}

fn bracket_balance(text: &str) -> i32 {
    let mut quote: Option<char> = None;
    let mut balance = 0;
    for c in text.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (None, '"' | '\'') => quote = Some(c),
            (None, '[' | '{') => balance += 1,
            (None, ']' | '}') => balance -= 1,
            _ => {}
        }
    }
    balance
}

/// Reads section headers and `key = value` lines. Arrays that span lines are
/// joined. Inline tables are kept as raw text in `value`.
fn parse_toml(text: &str) -> Vec<TomlEntry> {
    let mut entries = Vec::new();
    let mut section = String::new();
    let mut lines = text.lines();
    while let Some(raw) = lines.next() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            section = line
                .trim_start_matches('[')
                .trim_end_matches(']')
                .trim()
                .to_owned();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let mut value = value.trim().to_owned();
        while bracket_balance(&value) > 0 {
            let Some(next) = lines.next() else { break };
            value.push(' ');
            value.push_str(strip_comment(next).trim());
        }
        entries.push(TomlEntry {
            section: section.clone(),
            key: unquote(key.trim()),
            value,
        });
    }
    entries
}

/// The `packages:` list of a pnpm-workspace.yaml.
fn parse_pnpm_packages(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut inside = false;
    for raw in text.lines() {
        let line = strip_comment(raw).trim_end();
        if line.trim().is_empty() {
            continue;
        }
        let indented = line.starts_with(' ') || line.starts_with('\t');
        if !indented {
            inside = line.trim() == "packages:";
            continue;
        }
        if inside {
            if let Some(item) = line.trim().strip_prefix('-') {
                out.push(unquote(item));
            }
        }
    }
    out
}

// ------------------------------------------------------------ workspaces

/// Node-style workspace patterns: `dir/*`, literal paths, `!negated`.
/// Returns repo-relative directories that exist and contain `marker`.
fn expand_patterns(
    root: &Path,
    patterns: &[String],
    marker: &str,
    origin: &str,
    warnings: &mut Vec<String>,
) -> Vec<String> {
    let mut included: BTreeSet<String> = BTreeSet::new();
    let mut excluded: Vec<String> = Vec::new();
    for pattern in patterns {
        let (negated, body) = match pattern.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, pattern.as_str()),
        };
        let body = body.trim_start_matches("./").trim_end_matches('/');
        let (parent, wildcard) = match body.strip_suffix("/*") {
            Some(parent) => (parent, true),
            None if body == "*" => ("", true),
            None => (body, false),
        };
        if parent.contains(['*', '?', '{', '[']) || body.is_empty() || body.starts_with("..") {
            warnings.push(format!(
                "{origin}: pattern \"{pattern}\" is not supported. Use `dir/*` or a literal path."
            ));
            continue;
        }
        let dirs = if wildcard {
            list_subdirs(root, parent)
        } else {
            vec![parent.to_owned()]
        };
        if negated {
            excluded.extend(dirs);
        } else {
            included.extend(dirs);
        }
    }
    included
        .into_iter()
        .filter(|dir| !excluded.contains(dir) && exists(root, &format!("{dir}/{marker}")))
        .collect()
}

fn list_subdirs(root: &Path, parent: &str) -> Vec<String> {
    let dir = if parent.is_empty() {
        root.to_path_buf()
    } else {
        parent
            .split('/')
            .fold(root.to_path_buf(), |path, part| path.join(part))
    };
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.') && name != "node_modules" && name != "target")
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|name| {
            if parent.is_empty() {
                name
            } else {
                format!("{parent}/{name}")
            }
        })
        .collect()
}

struct Raw {
    name: String,
    root: String,
    kind: &'static str,
    deps: Vec<String>,
}

fn node_deps(json: &Value) -> Vec<String> {
    let mut deps = Vec::new();
    for field in [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ] {
        if let Some(map) = json.get(field).and_then(Value::as_object) {
            deps.extend(map.keys().cloned());
        }
    }
    deps
}

fn node_projects(root: &Path, root_json: Option<&Value>, warnings: &mut Vec<String>) -> Vec<Raw> {
    let mut out = Vec::new();
    let mut patterns: Vec<String> = Vec::new();
    let mut origin = "package.json";
    if let Some(text) = read_text(root, "pnpm-workspace.yaml") {
        patterns = parse_pnpm_packages(&text);
        origin = "pnpm-workspace.yaml";
    }
    if patterns.is_empty() {
        if let Some(workspaces) = root_json.and_then(|json| json.get("workspaces")) {
            let list = match workspaces {
                Value::Array(items) => Some(items),
                other => other.get("packages").and_then(Value::as_array),
            };
            patterns = list
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
        }
    }
    if let Some(json) = root_json {
        if let Some(name) = json.get("name").and_then(Value::as_str) {
            out.push(Raw {
                name: name.to_owned(),
                root: ".".to_owned(),
                kind: "node",
                deps: node_deps(json),
            });
        }
    }
    for dir in expand_patterns(root, &patterns, "package.json", origin, warnings) {
        let file = format!("{dir}/package.json");
        let Some(json) = read_json(root, &file, warnings) else {
            continue;
        };
        if let Some(name) = json.get("name").and_then(Value::as_str) {
            out.push(Raw {
                name: name.to_owned(),
                root: dir,
                kind: "node",
                deps: node_deps(&json),
            });
        }
    }
    out
}

fn cargo_deps(entries: &[TomlEntry]) -> Vec<String> {
    let mut deps = Vec::new();
    for entry in entries {
        let section = entry.section.as_str();
        if matches!(
            section,
            "dependencies" | "dev-dependencies" | "build-dependencies"
        ) {
            // `x = ...`, `x.workspace = true` and `x.path = "..."` all start with the name.
            let name = entry.key.split('.').next().unwrap_or_default();
            deps.push(unquote(name));
        } else if let Some(name) = ["dependencies.", "dev-dependencies.", "build-dependencies."]
            .iter()
            .find_map(|prefix| section.strip_prefix(prefix))
        {
            deps.push(unquote(name));
        }
    }
    deps
}

fn package_name(entries: &[TomlEntry]) -> Option<String> {
    entries
        .iter()
        .find(|entry| entry.section == "package" && entry.key == "name")
        .map(|entry| unquote(&entry.value))
}

fn rust_projects(root: &Path, cargo: &[TomlEntry], warnings: &mut Vec<String>) -> Vec<Raw> {
    let mut out = Vec::new();
    if let Some(name) = package_name(cargo) {
        out.push(Raw {
            name,
            root: ".".to_owned(),
            kind: "rust",
            deps: cargo_deps(cargo),
        });
    }
    let members: Vec<String> = cargo
        .iter()
        .filter(|entry| entry.section == "workspace" && entry.key == "members")
        .flat_map(|entry| quoted_strings(&entry.value))
        .collect();
    for dir in expand_patterns(root, &members, "Cargo.toml", "Cargo.toml", warnings) {
        let Some(text) = read_text(root, &format!("{dir}/Cargo.toml")) else {
            continue;
        };
        let entries = parse_toml(&text);
        if let Some(name) = package_name(&entries) {
            out.push(Raw {
                name,
                root: dir,
                kind: "rust",
                deps: cargo_deps(&entries),
            });
        }
    }
    out
}

fn finish_projects(raw: Vec<Raw>) -> Vec<Project> {
    let mut projects: Vec<Project> = raw
        .iter()
        .map(|project| {
            let mut depends_on: Vec<String> = project
                .deps
                .iter()
                .filter(|dep| {
                    **dep != project.name
                        && raw
                            .iter()
                            .any(|other| other.kind == project.kind && other.name == **dep)
                })
                .cloned()
                .collect();
            depends_on.sort();
            depends_on.dedup();
            Project {
                name: project.name.clone(),
                root: project.root.clone(),
                kind: project.kind.to_owned(),
                depends_on,
            }
        })
        .collect();
    projects.sort_by(|a, b| (&a.name, &a.kind, &a.root).cmp(&(&b.name, &b.kind, &b.root)));
    projects.dedup();
    projects
}

// ---------------------------------------------------------------- facts

/// Detects the facts of the repo at `root`.
pub fn detect_facts(root: &Path) -> Result<RepoFacts> {
    if !root.is_dir() {
        return Err(Error::user(format!(
            "Folder \"{}\" does not exist. Run fscale from a repo folder or pass --cwd.",
            root.display()
        )));
    }
    let mut warnings = Vec::new();
    let package_json = read_json(root, "package.json", &mut warnings);
    let has_package_json = exists(root, "package.json");

    let lockfile_names = [
        "Cargo.lock",
        "bun.lock",
        "bun.lockb",
        "package-lock.json",
        "pnpm-lock.yaml",
        "poetry.lock",
        "uv.lock",
        "yarn.lock",
    ];
    let lockfiles: Vec<String> = lockfile_names
        .iter()
        .filter(|name| exists(root, name))
        .map(|name| (*name).to_owned())
        .collect();

    let declared = package_json
        .as_ref()
        .and_then(|json| json.get("packageManager"))
        .and_then(Value::as_str)
        .map(|value| value.split('@').next().unwrap_or_default().to_owned())
        .filter(|name| matches!(name.as_str(), "pnpm" | "npm" | "yarn" | "bun"));
    let from_lock = [
        ("pnpm-lock.yaml", "pnpm"),
        ("package-lock.json", "npm"),
        ("yarn.lock", "yarn"),
        ("bun.lock", "bun"),
        ("bun.lockb", "bun"),
    ]
    .iter()
    .find(|(file, _)| lockfiles.iter().any(|lock| lock == file))
    .map(|(_, name)| (*name).to_owned());
    let package_manager = declared.or(from_lock);

    let cargo_text = read_text(root, "Cargo.toml");
    let cargo = cargo_text.as_deref().map(parse_toml).unwrap_or_default();

    let mut languages = BTreeSet::new();
    if has_package_json {
        languages.insert("javascript");
    }
    let ts_dep = package_json.as_ref().is_some_and(|json| {
        ["dependencies", "devDependencies"].iter().any(|field| {
            json.get(field)
                .and_then(|deps| deps.get("typescript"))
                .is_some()
        })
    });
    if exists(root, "tsconfig.json") || ts_dep {
        languages.insert("typescript");
    }
    if cargo_text.is_some() {
        languages.insert("rust");
    }
    if exists(root, "pyproject.toml") || exists(root, "requirements.txt") {
        languages.insert("python");
    }

    let mut tools = BTreeSet::new();
    for (file, tool) in [
        ("nx.json", "nx"),
        ("turbo.json", "turbo"),
        ("pnpm-workspace.yaml", "pnpm-workspace"),
    ] {
        if exists(root, file) {
            tools.insert(tool);
        }
    }
    if cargo.iter().any(|entry| entry.section == "workspace") {
        tools.insert("cargo-workspace");
    }
    let has_workflow = fs::read_dir(root.join(".github").join("workflows"))
        .map(|read| {
            read.filter_map(|entry| entry.ok()).any(|entry| {
                let name = entry.file_name().to_string_lossy().to_lowercase();
                name.ends_with(".yml") || name.ends_with(".yaml")
            })
        })
        .unwrap_or(false);
    if has_workflow {
        tools.insert("github-actions");
    }

    let pm = package_manager.as_deref().unwrap_or("npm");
    let script = |name: &str| {
        package_json
            .as_ref()
            .and_then(|json| json.get("scripts"))
            .and_then(|scripts| scripts.get(name))
            .is_some()
            .then(|| format!("{pm} run {name}"))
    };
    let commands = Commands {
        build: script("build"),
        test: script("test"),
        lint: script("lint"),
        typecheck: script("typecheck"),
    };

    let mut raw = node_projects(root, package_json.as_ref(), &mut warnings);
    if cargo_text.is_some() {
        raw.extend(rust_projects(root, &cargo, &mut warnings));
    }

    Ok(RepoFacts {
        schema_version: 1,
        package_manager,
        lockfiles,
        languages: languages.into_iter().map(str::to_owned).collect(),
        tools: tools.into_iter().map(str::to_owned).collect(),
        commands,
        projects: finish_projects(raw),
        warnings,
    })
}

/// The facts as pretty JSON (2 spaces) with a trailing newline.
#[must_use]
pub fn format_facts_json(facts: &RepoFacts) -> String {
    let mut text = serde_json::to_string_pretty(facts).unwrap_or_else(|_| "{}".to_owned());
    text.push('\n');
    text
}

fn list_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_owned()
    } else {
        items.join(", ")
    }
}

/// The facts for people.
#[must_use]
pub fn format_facts_human(facts: &RepoFacts) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Package manager: {}\n",
        facts.package_manager.as_deref().unwrap_or("none")
    ));
    out.push_str(&format!("Lockfiles: {}\n", list_or_none(&facts.lockfiles)));
    out.push_str(&format!("Languages: {}\n", list_or_none(&facts.languages)));
    out.push_str(&format!("Tools: {}\n", list_or_none(&facts.tools)));
    out.push_str("Commands:\n");
    let commands = [
        ("build", &facts.commands.build),
        ("test", &facts.commands.test),
        ("lint", &facts.commands.lint),
        ("typecheck", &facts.commands.typecheck),
    ];
    let mut any = false;
    for (name, command) in commands {
        if let Some(command) = command {
            out.push_str(&format!("  {name}: {command}\n"));
            any = true;
        }
    }
    if !any {
        out.push_str("  none\n");
    }
    out.push_str(&format!("Projects ({}):\n", facts.projects.len()));
    for project in &facts.projects {
        out.push_str(&format!(
            "  {} [{}] {}\n",
            project.name, project.kind, project.root
        ));
        if !project.depends_on.is_empty() {
            out.push_str(&format!(
                "    depends on: {}\n",
                project.depends_on.join(", ")
            ));
        }
    }
    if !facts.warnings.is_empty() {
        out.push_str("Warnings:\n");
        for warning in &facts.warnings {
            out.push_str(&format!("  {warning}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Fixture, Track};

    fn facts(files: &[(&str, &str)]) -> RepoFacts {
        let fixture = Fixture::new(files, Track::NoGit).unwrap();
        detect_facts(fixture.root()).unwrap()
    }

    #[test]
    fn empty_folder_has_no_facts() {
        let facts = facts(&[]);
        assert_eq!(facts.schema_version, 1);
        assert_eq!(facts.package_manager, None);
        assert!(facts.lockfiles.is_empty() && facts.languages.is_empty());
        assert!(facts.tools.is_empty() && facts.projects.is_empty());
        assert_eq!(facts.commands, Commands::default());
        let json = format_facts_json(&facts);
        assert!(json.starts_with("{\n  \"schemaVersion\": 1,\n  \"packageManager\": null,"));
        assert!(json.ends_with("}\n"));
    }

    #[test]
    fn pnpm_monorepo_with_a_dependency_between_packages() {
        let facts = facts(&[
            (
                "package.json",
                r#"{"name":"root","packageManager":"pnpm@9.1.0","scripts":{"build":"x","test":"y"},"devDependencies":{"typescript":"5"}}"#,
            ),
            ("pnpm-lock.yaml", ""),
            (
                "pnpm-workspace.yaml",
                "packages:\n  - 'packages/*'\n  - \"apps/web\"\n  - tools/cli # comment\n  - '!packages/skip'\nother: 1\n",
            ),
            ("packages/a/package.json", r#"{"name":"@x/a"}"#),
            (
                "packages/b/package.json",
                r#"{"name":"@x/b","dependencies":{"@x/a":"workspace:*","left-pad":"1"}}"#,
            ),
            ("packages/skip/package.json", r#"{"name":"@x/skip"}"#),
            ("apps/web/package.json", r#"{"name":"web","devDependencies":{"@x/b":"*"}}"#),
        ]);
        assert_eq!(facts.package_manager.as_deref(), Some("pnpm"));
        assert_eq!(facts.lockfiles, ["pnpm-lock.yaml"]);
        assert_eq!(facts.languages, ["javascript", "typescript"]);
        assert_eq!(facts.tools, ["pnpm-workspace"]);
        assert_eq!(facts.commands.build.as_deref(), Some("pnpm run build"));
        assert_eq!(facts.commands.test.as_deref(), Some("pnpm run test"));
        assert_eq!(facts.commands.lint, None);
        let names: Vec<_> = facts.projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["@x/a", "@x/b", "root", "web"]);
        let b = &facts.projects[1];
        assert_eq!((b.root.as_str(), b.kind.as_str()), ("packages/b", "node"));
        assert_eq!(b.depends_on, ["@x/a"]);
        assert_eq!(facts.projects[3].depends_on, ["@x/b"]);
        assert_eq!(facts.projects[2].root, ".");
        assert!(facts.warnings.is_empty());
    }

    #[test]
    fn npm_workspaces_array_and_object_forms() {
        let array = facts(&[
            ("package.json", r#"{"workspaces":["libs/*"]}"#),
            ("package-lock.json", "{}"),
            ("libs/one/package.json", r#"{"name":"one"}"#),
            (
                "libs/two/package.json",
                r#"{"name":"two","dependencies":{"one":"1"}}"#,
            ),
        ]);
        assert_eq!(array.package_manager.as_deref(), Some("npm"));
        assert_eq!(array.projects.len(), 2);
        assert_eq!(array.projects[1].depends_on, ["one"]);
        let object = facts(&[
            (
                "package.json",
                r#"{"workspaces":{"packages":["libs/one"]}}"#,
            ),
            ("libs/one/package.json", r#"{"name":"one"}"#),
            ("libs/two/package.json", r#"{"name":"two"}"#),
        ]);
        assert_eq!(object.projects.len(), 1);
        assert_eq!(object.projects[0].root, "libs/one");
    }

    #[test]
    fn complex_patterns_warn() {
        let facts = facts(&[("package.json", r#"{"workspaces":["a/**","b/*/c"]}"#)]);
        assert_eq!(facts.warnings.len(), 2);
        assert!(facts.warnings[0].contains("not supported"));
    }

    #[test]
    fn cargo_workspace_with_path_and_workspace_dependencies() {
        let facts = facts(&[
            ("Cargo.toml", "[workspace]\nmembers = [\n  \"crates/*\", # all\n  \"tools/x\",\n]\n\n[workspace.dependencies]\ncore = { path = \"crates/core\" }\n"),
            ("Cargo.lock", ""),
            ("crates/core/Cargo.toml", "[package]\nname = \"my-core\"\n"),
            ("crates/cli/Cargo.toml", "[package]\nname = \"my-cli\"\n\n[dependencies]\nmy-core = { path = \"../core\" }\nserde.workspace = true\n\n[dev-dependencies.tools-x]\npath = \"../../tools/x\"\n"),
            ("tools/x/Cargo.toml", "[package]\nname = 'tools-x'\n\n[dependencies]\nmy-core.workspace = true\n"),
        ]);
        assert_eq!(facts.languages, ["rust"]);
        assert_eq!(facts.tools, ["cargo-workspace"]);
        assert_eq!(facts.lockfiles, ["Cargo.lock"]);
        let by: Vec<_> = facts
            .projects
            .iter()
            .map(|p| (p.name.as_str(), p.root.as_str(), p.depends_on.join(",")))
            .collect();
        assert_eq!(
            by,
            [
                ("my-cli", "crates/cli", "my-core,tools-x".to_owned()),
                ("my-core", "crates/core", String::new()),
                ("tools-x", "tools/x", "my-core".to_owned()),
            ]
        );
    }

    #[test]
    fn mixed_repo() {
        let facts = facts(&[
            (
                "package.json",
                r#"{"name":"app","scripts":{"lint":"x","typecheck":"y"}}"#,
            ),
            ("tsconfig.json", "{}"),
            ("yarn.lock", ""),
            ("uv.lock", ""),
            ("pyproject.toml", ""),
            ("nx.json", "{}"),
            ("turbo.json", "{}"),
            (".github/workflows/ci.yml", ""),
            ("Cargo.toml", "[package]\nname = \"app\"\n"),
        ]);
        assert_eq!(facts.package_manager.as_deref(), Some("yarn"));
        assert_eq!(facts.lockfiles, ["uv.lock", "yarn.lock"]);
        assert_eq!(
            facts.languages,
            ["javascript", "python", "rust", "typescript"]
        );
        assert_eq!(facts.tools, ["github-actions", "nx", "turbo"]);
        assert_eq!(facts.commands.lint.as_deref(), Some("yarn run lint"));
        assert_eq!(facts.projects.len(), 2);
        // Same name, different kinds: no dependency between them.
        assert!(facts.projects.iter().all(|p| p.depends_on.is_empty()));
    }

    #[test]
    fn malformed_package_json_is_a_warning() {
        let facts = facts(&[("package.json", "{ nope")]);
        assert_eq!(facts.languages, ["javascript"]);
        assert_eq!(facts.warnings.len(), 1);
        assert!(facts.warnings[0].starts_with("package.json is not valid JSON"));
        assert!(facts.projects.is_empty());
    }

    #[test]
    fn roots_use_forward_slashes() {
        let facts = facts(&[
            ("package.json", r#"{"workspaces":["a/b/*"]}"#),
            ("a/b/c/package.json", r#"{"name":"c"}"#),
        ]);
        assert_eq!(facts.projects[0].root, "a/b/c");
        assert!(!format_facts_json(&facts).contains('\\'));
    }

    #[test]
    fn human_output_lists_projects() {
        let facts = facts(&[
            ("package.json", r#"{"workspaces":["p/*"]}"#),
            ("p/a/package.json", r#"{"name":"a"}"#),
            (
                "p/b/package.json",
                r#"{"name":"b","dependencies":{"a":"1"}}"#,
            ),
        ]);
        let text = format_facts_human(&facts);
        assert!(text.contains("Projects (2):\n  a [node] p/a\n  b [node] p/b\n    depends on: a\n"));
    }

    #[test]
    fn toml_reader_handles_multiline_arrays_and_comments() {
        let entries = parse_toml("[a]\nx = [\n \"1\", # c\n \"2\"\n]\ny = \"v # not\"\n");
        assert_eq!(quoted_strings(&entries[0].value), ["1", "2"]);
        assert_eq!(unquote(&entries[1].value), "v # not");
    }
}
