//! Repo facts for `fscale facts`: stack detection from marker files, projects
//! found in subfolders, and a simple project graph built from workspace files
//! (see docs/facts.md and ADR 0020).
//!
//! There is no TOML or YAML parser here. Small line-based readers handle the
//! common forms, and the limits are documented. Everything is read from disk;
//! a missing or unreadable optional file is not an error, and a malformed
//! marker file becomes a warning.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use ignore::WalkBuilder;
use serde::Serialize;
use serde_json::Value;

use crate::error::{Error, Result};

/// Version of the `fscale facts --json` format.
pub const FACTS_SCHEMA_VERSION: u32 = 2;

/// How many folder levels below the repo root are searched for projects.
/// `backend/auth_service` is level 2.
pub const DISCOVERY_DEPTH: usize = 4;

/// Folders never searched for projects: dependencies, caches and build output.
/// Folders whose name starts with a dot are skipped too, and so is everything
/// `.gitignore` ignores.
pub const DISCOVERY_SKIPPED_DIRS: [&str; 18] = [
    "node_modules",
    "bower_components",
    "vendor",
    "site-packages",
    "venv",
    "env",
    "__pycache__",
    "dist",
    "build",
    "out",
    "target",
    "coverage",
    "htmlcov",
    "test-results",
    "playwright-report",
    "tmp",
    "temp",
    "logs",
];

/// The name of a project at the repo root whose manifest gives it none.
pub const UNNAMED_ROOT: &str = ".";

/// How a project was found.
pub mod source {
    /// It is the repo root.
    pub const ROOT: &str = "root";
    /// A workspace file at the repo root lists it.
    pub const WORKSPACE: &str = "workspace";
    /// A marker file was found in a subfolder.
    pub const DISCOVERED: &str = "discovered";
}

/// A project of the repo.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub name: String,
    /// Repo-relative, forward slashes, `.` for the repo root.
    pub root: String,
    /// `node`, `rust`, `python` or `go`.
    pub kind: String,
    /// `root`, `workspace` or `discovered`.
    pub source: String,
    pub languages: Vec<String>,
    pub package_manager: Option<String>,
    /// Names of the lockfiles in the project's folder.
    pub lockfiles: Vec<String>,
    pub tools: Vec<String>,
    /// Commands to run from the project's folder.
    pub commands: Commands,
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
    /// The JavaScript package manager of the repo root.
    pub package_manager: Option<String>,
    /// Every package manager in use: the root's and each project's.
    pub package_managers: Vec<String>,
    pub lockfiles: Vec<String>,
    pub languages: Vec<String>,
    pub tools: Vec<String>,
    pub commands: Commands,
    pub projects: Vec<Project>,
    pub warnings: Vec<String>,
    /// Commands for the repo root worked out from tool config files, for the
    /// steps the root `package.json` has no script for. `commands` stays
    /// "scripts only", because rules compare it with what a repo documents.
    #[serde(skip)]
    pub root_tool_commands: Commands,
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

// ------------------------------------------------------ workspace patterns

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

// ------------------------------------------------------- project details

/// What one project's folder says about it.
#[derive(Debug, Clone, Default)]
struct Detail {
    languages: Vec<String>,
    package_manager: Option<String>,
    lockfiles: Vec<String>,
    tools: Vec<String>,
    commands: Commands,
}

const LOCKFILE_NAMES: [&str; 10] = [
    "Cargo.lock",
    "Pipfile.lock",
    "bun.lock",
    "bun.lockb",
    "go.sum",
    "package-lock.json",
    "pnpm-lock.yaml",
    "poetry.lock",
    "uv.lock",
    "yarn.lock",
];

/// `dir` is repo-relative with forward slashes, `.` for the repo root.
fn in_dir(dir: &str, file: &str) -> String {
    if dir == "." {
        file.to_owned()
    } else {
        format!("{dir}/{file}")
    }
}

/// Sorted names of the files directly in `dir`.
fn file_names(root: &Path, dir: &str) -> Vec<String> {
    let path = if dir == "." {
        root.to_path_buf()
    } else {
        dir.split('/')
            .fold(root.to_path_buf(), |path, part| path.join(part))
    };
    let Ok(read) = fs::read_dir(path) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_file())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

fn lockfiles_in(names: &[String]) -> Vec<String> {
    LOCKFILE_NAMES
        .iter()
        .filter(|lock| names.iter().any(|name| name == *lock))
        .map(|lock| (*lock).to_owned())
        .collect()
}

fn node_manager_from_locks(lockfiles: &[String]) -> Option<String> {
    [
        ("pnpm-lock.yaml", "pnpm"),
        ("package-lock.json", "npm"),
        ("yarn.lock", "yarn"),
        ("bun.lock", "bun"),
        ("bun.lockb", "bun"),
    ]
    .iter()
    .find(|(file, _)| lockfiles.iter().any(|lock| lock == file))
    .map(|(_, name)| (*name).to_owned())
}

fn declared_node_manager(json: Option<&Value>) -> Option<String> {
    json.and_then(|json| json.get("packageManager"))
        .and_then(Value::as_str)
        .map(|value| value.split('@').next().unwrap_or_default().to_owned())
        .filter(|name| matches!(name.as_str(), "pnpm" | "npm" | "yarn" | "bun"))
}

/// How a package manager runs a tool that is installed in the project.
/// `npx --no` never downloads a tool that is missing.
fn node_exec(package_manager: Option<&str>) -> &'static str {
    match package_manager {
        Some("pnpm") => "pnpm exec",
        Some("yarn") => "yarn",
        Some("bun") => "bunx",
        _ => "npx --no",
    }
}

/// A JavaScript tool, and how its presence shows: a config file whose name
/// starts with one of `prefixes`, or a dependency named in `packages`.
struct NodeTool {
    name: &'static str,
    prefixes: &'static [&'static str],
    packages: &'static [&'static str],
}

const NODE_TOOLS: [NodeTool; 11] = [
    NodeTool {
        name: "biome",
        prefixes: &["biome.json"],
        packages: &["@biomejs/biome"],
    },
    NodeTool {
        name: "cypress",
        prefixes: &["cypress.config."],
        packages: &["cypress"],
    },
    NodeTool {
        name: "eslint",
        prefixes: &["eslint.config.", ".eslintrc"],
        packages: &["eslint"],
    },
    NodeTool {
        name: "jest",
        prefixes: &["jest.config."],
        packages: &["jest"],
    },
    NodeTool {
        name: "knip",
        prefixes: &["knip.json", "knip.config.", ".knip.json"],
        packages: &["knip"],
    },
    NodeTool {
        name: "next",
        prefixes: &["next.config."],
        packages: &["next"],
    },
    NodeTool {
        name: "oxlint",
        prefixes: &[".oxlintrc.json", "oxlint.config.", "oxlintrc.json"],
        packages: &["oxlint"],
    },
    NodeTool {
        name: "playwright",
        prefixes: &["playwright.config."],
        packages: &["@playwright/test"],
    },
    NodeTool {
        name: "prettier",
        prefixes: &[".prettierrc", "prettier.config."],
        packages: &["prettier"],
    },
    NodeTool {
        name: "vite",
        prefixes: &["vite.config."],
        packages: &["vite"],
    },
    NodeTool {
        name: "vitest",
        prefixes: &["vitest.config.", "vitest.workspace."],
        packages: &["vitest"],
    },
];

fn has_node_dependency(json: Option<&Value>, package: &str) -> bool {
    json.is_some_and(|json| {
        ["dependencies", "devDependencies"]
            .iter()
            .any(|field| json.get(field).and_then(|deps| deps.get(package)).is_some())
    })
}

/// The commands a `package.json` gives through its scripts.
fn script_commands(json: Option<&Value>, package_manager: &str) -> Commands {
    let script = |name: &str| {
        json.and_then(|json| json.get("scripts"))
            .and_then(|scripts| scripts.get(name))
            .is_some()
            .then(|| format!("{package_manager} run {name}"))
    };
    Commands {
        build: script("build"),
        test: script("test"),
        lint: script("lint"),
        typecheck: script("typecheck"),
    }
}

/// Commands worked out from tool config files, for a project with no script
/// for the step. Only tools that check without changing files, and no
/// end-to-end runners: those need browsers and running services.
fn node_tool_commands(root: &Path, dir: &str, tools: &[String], exec: &str) -> Commands {
    let has = |tool: &str| tools.iter().any(|name| name == tool);
    let lint = if has("oxlint") {
        Some(format!("{exec} oxlint"))
    } else if has("eslint") {
        Some(format!("{exec} eslint ."))
    } else if has("biome") {
        Some(format!("{exec} biome check ."))
    } else {
        None
    };
    let test = if has("vitest") {
        Some(format!("{exec} vitest run"))
    } else if has("jest") {
        Some(format!("{exec} jest"))
    } else {
        None
    };
    // `tsc --noEmit` checks nothing in a tsconfig that only lists project
    // references, and `tsc -b` could write files, so such a project gets no
    // typecheck command from here.
    let typecheck = read_text(root, &in_dir(dir, "tsconfig.json"))
        .filter(|text| !text.contains("\"references\""))
        .map(|_| format!("{exec} tsc --noEmit"));
    Commands {
        build: None,
        test,
        lint,
        typecheck,
    }
}

fn or_else(first: Commands, second: Commands) -> Commands {
    Commands {
        build: first.build.or(second.build),
        test: first.test.or(second.test),
        lint: first.lint.or(second.lint),
        typecheck: first.typecheck.or(second.typecheck),
    }
}

/// A folder with a `package.json`. `inherited` is the package manager of the
/// workspace the project belongs to, used when the folder names none itself.
/// Returns the detail and, separately, the commands that came from tools.
fn node_detail(
    root: &Path,
    dir: &str,
    json: Option<&Value>,
    inherited: Option<&str>,
) -> (Detail, Commands) {
    let names = file_names(root, dir);
    let lockfiles = lockfiles_in(&names);
    let package_manager = declared_node_manager(json)
        .or_else(|| node_manager_from_locks(&lockfiles))
        .or_else(|| inherited.map(str::to_owned));
    let mut languages = vec!["javascript".to_owned()];
    if names.iter().any(|name| name == "tsconfig.json") || has_node_dependency(json, "typescript") {
        languages.push("typescript".to_owned());
    }
    let mut tools: Vec<String> = NODE_TOOLS
        .iter()
        .filter(|tool| {
            names
                .iter()
                .any(|name| tool.prefixes.iter().any(|prefix| name.starts_with(prefix)))
                || tool
                    .packages
                    .iter()
                    .any(|package| has_node_dependency(json, package))
        })
        .map(|tool| tool.name.to_owned())
        .collect();
    if names.iter().any(|name| name == "Dockerfile") {
        tools.push("docker".to_owned());
    }
    tools.sort();
    let scripts = script_commands(json, package_manager.as_deref().unwrap_or("npm"));
    let from_tools = node_tool_commands(root, dir, &tools, node_exec(package_manager.as_deref()));
    let detail = Detail {
        languages,
        package_manager,
        lockfiles: lockfiles
            .into_iter()
            .filter(|lock| !matches!(lock.as_str(), "Cargo.lock" | "go.sum"))
            .collect(),
        tools,
        commands: or_else(scripts.clone(), from_tools.clone()),
    };
    let only_tools = Commands {
        build: None,
        test: scripts.test.is_none().then_some(from_tools.test).flatten(),
        lint: scripts.lint.is_none().then_some(from_tools.lint).flatten(),
        typecheck: scripts
            .typecheck
            .is_none()
            .then_some(from_tools.typecheck)
            .flatten(),
    };
    (detail, only_tools)
}

/// A folder with a `Cargo.toml`.
fn rust_detail(root: &Path, dir: &str, cargo: &[TomlEntry]) -> Detail {
    let names = file_names(root, dir);
    let workspace = cargo.iter().any(|entry| entry.section == "workspace");
    let all = if workspace { " --workspace" } else { "" };
    let mut tools = Vec::new();
    if workspace {
        tools.push("cargo-workspace".to_owned());
    }
    if names.iter().any(|name| name == "Dockerfile") {
        tools.push("docker".to_owned());
    }
    Detail {
        languages: vec!["rust".to_owned()],
        package_manager: Some("cargo".to_owned()),
        lockfiles: names
            .iter()
            .filter(|name| *name == "Cargo.lock")
            .cloned()
            .collect(),
        tools,
        commands: Commands {
            build: Some(format!("cargo build{all}")),
            test: Some(format!("cargo test{all}")),
            lint: Some(format!("cargo clippy{all} --all-targets -- -D warnings")),
            typecheck: None,
        },
    }
}

/// A folder with a `go.mod`.
fn go_detail(root: &Path, dir: &str) -> Detail {
    let names = file_names(root, dir);
    Detail {
        languages: vec!["go".to_owned()],
        package_manager: Some("go".to_owned()),
        lockfiles: names
            .iter()
            .filter(|name| *name == "go.sum")
            .cloned()
            .collect(),
        tools: names
            .iter()
            .filter(|name| *name == "Dockerfile")
            .map(|_| "docker".to_owned())
            .collect(),
        commands: Commands {
            build: Some("go build ./...".to_owned()),
            test: Some("go test ./...".to_owned()),
            lint: Some("go vet ./...".to_owned()),
            typecheck: None,
        },
    }
}

/// Lower-case names of the packages listed in the `requirements*.txt` files
/// of a folder.
fn python_requirements(root: &Path, dir: &str, names: &[String]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for file in names.iter().filter(|name| {
        let lower = name.to_lowercase();
        lower.starts_with("requirements") && lower.ends_with(".txt")
    }) {
        let Some(text) = read_text(root, &in_dir(dir, file)) else {
            continue;
        };
        for line in text.lines() {
            let name: String = strip_comment(line)
                .trim()
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
                .collect();
            if !name.is_empty() {
                out.insert(name.to_lowercase().replace('_', "-"));
            }
        }
    }
    out
}

/// True when `dir` or a folder above it, up to the repo root, has one of
/// `files`, or a `pyproject.toml` with a `[tool.<table>` section. Python tools
/// look for their config in the folders above too.
fn python_config_above(root: &Path, dir: &str, files: &[&str], table: &str) -> bool {
    let mut current = dir.to_owned();
    loop {
        if files
            .iter()
            .any(|file| exists(root, &in_dir(&current, file)))
        {
            return true;
        }
        if read_text(root, &in_dir(&current, "pyproject.toml"))
            .is_some_and(|text| text.contains(&format!("[tool.{table}")))
        {
            return true;
        }
        if current == "." {
            return false;
        }
        current = match current.rsplit_once('/') {
            Some((parent, _)) => parent.to_owned(),
            None => ".".to_owned(),
        };
    }
}

/// Files that, next to a `requirements.txt`, make a folder a Python project.
const PYTHON_SIGNS: [&str; 9] = [
    "pytest.ini",
    "tox.ini",
    "setup.cfg",
    "conftest.py",
    "ruff.toml",
    ".ruff.toml",
    "mypy.ini",
    "manage.py",
    "alembic.ini",
];

/// True when the folder is a Python project: it has a `pyproject.toml`,
/// `setup.py` or `Pipfile`, or a `requirements.txt` together with a Python
/// config file or a `.py` file. A lone `requirements.txt` is often only the
/// input of a docs build or a container image.
fn is_python_project(names: &[String]) -> bool {
    let has = |file: &str| names.iter().any(|name| name == file);
    has("pyproject.toml")
        || has("setup.py")
        || has("Pipfile")
        || (has("requirements.txt")
            && (PYTHON_SIGNS.iter().any(|sign| has(sign))
                || names.iter().any(|name| name.ends_with(".py"))))
}

/// A Python project's folder.
fn python_detail(root: &Path, dir: &str) -> Detail {
    let names = file_names(root, dir);
    let has = |file: &str| names.iter().any(|name| name == file);
    let pyproject = read_text(root, &in_dir(dir, "pyproject.toml")).unwrap_or_default();
    let contains = |file: &str, needle: &str| {
        read_text(root, &in_dir(dir, file)).is_some_and(|text| text.contains(needle))
    };
    let requirements = python_requirements(root, dir, &names);

    let package_manager = if has("uv.lock") || pyproject.contains("[tool.uv") {
        "uv"
    } else if has("poetry.lock") || pyproject.contains("[tool.poetry") {
        "poetry"
    } else if has("Pipfile") {
        "pipenv"
    } else {
        "pip"
    };
    let run = match package_manager {
        "uv" => "uv run ",
        "poetry" => "poetry run ",
        "pipenv" => "pipenv run ",
        // The module form works in any environment that has the tool installed.
        _ => "python -m ",
    };

    let pytest = has("pytest.ini")
        || has("conftest.py")
        || pyproject.contains("[tool.pytest")
        || contains("tox.ini", "[pytest]")
        || contains("setup.cfg", "[tool:pytest]")
        || requirements.contains("pytest");
    let ruff = requirements.contains("ruff")
        || python_config_above(root, dir, &["ruff.toml", ".ruff.toml"], "ruff");
    let mypy = requirements.contains("mypy")
        || contains("setup.cfg", "[mypy")
        || python_config_above(root, dir, &["mypy.ini", ".mypy.ini"], "mypy");

    let mut tools = Vec::new();
    if has("alembic.ini") {
        tools.push("alembic".to_owned());
    }
    if has("Dockerfile") {
        tools.push("docker".to_owned());
    }
    for (found, tool) in [(mypy, "mypy"), (pytest, "pytest"), (ruff, "ruff")] {
        if found {
            tools.push(tool.to_owned());
        }
    }
    Detail {
        languages: vec!["python".to_owned()],
        package_manager: Some(package_manager.to_owned()),
        lockfiles: lockfiles_in(&names)
            .into_iter()
            .filter(|lock| matches!(lock.as_str(), "Pipfile.lock" | "poetry.lock" | "uv.lock"))
            .collect(),
        tools,
        commands: Commands {
            build: None,
            test: pytest.then(|| format!("{run}pytest")),
            lint: ruff.then(|| format!("{run}ruff check .")),
            typecheck: mypy.then(|| format!("{run}mypy .")),
        },
    }
}

/// The last segment of a folder path, or the unnamed-root marker.
fn folder_name(dir: &str) -> String {
    if dir == "." {
        UNNAMED_ROOT.to_owned()
    } else {
        dir.rsplit('/').next().unwrap_or(dir).to_owned()
    }
}

fn python_name(root: &Path, dir: &str) -> String {
    read_text(root, &in_dir(dir, "pyproject.toml"))
        .map(|text| parse_toml(&text))
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| {
                    matches!(entry.section.as_str(), "project" | "tool.poetry")
                        && entry.key == "name"
                })
                .map(|entry| unquote(&entry.value))
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| folder_name(dir))
}

fn go_name(root: &Path, dir: &str) -> String {
    read_text(root, &in_dir(dir, "go.mod"))
        .and_then(|text| {
            text.lines()
                .find_map(|line| line.trim().strip_prefix("module "))
                .map(|module| {
                    let module = module.trim().trim_matches('"');
                    module.rsplit('/').next().unwrap_or(module).to_owned()
                })
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| folder_name(dir))
}

// ------------------------------------------------------------ workspaces

struct Raw {
    name: String,
    root: String,
    kind: &'static str,
    source: &'static str,
    deps: Vec<String>,
    detail: Detail,
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

fn node_projects(
    root: &Path,
    root_json: Option<&Value>,
    root_detail: &Detail,
    warnings: &mut Vec<String>,
) -> (Vec<Raw>, bool) {
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
                source: source::ROOT,
                deps: node_deps(json),
                detail: root_detail.clone(),
            });
        }
    }
    for dir in expand_patterns(root, &patterns, "package.json", origin, warnings) {
        let file = format!("{dir}/package.json");
        let Some(json) = read_json(root, &file, warnings) else {
            continue;
        };
        if let Some(name) = json.get("name").and_then(Value::as_str) {
            let (detail, _) = node_detail(
                root,
                &dir,
                Some(&json),
                root_detail.package_manager.as_deref(),
            );
            out.push(Raw {
                name: name.to_owned(),
                root: dir,
                kind: "node",
                source: source::WORKSPACE,
                deps: node_deps(&json),
                detail,
            });
        }
    }
    (out, !patterns.is_empty())
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
            source: source::ROOT,
            deps: cargo_deps(cargo),
            detail: rust_detail(root, ".", cargo),
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
            let detail = rust_detail(root, &dir, &entries);
            out.push(Raw {
                name,
                root: dir,
                kind: "rust",
                source: source::WORKSPACE,
                deps: cargo_deps(&entries),
                detail,
            });
        }
    }
    out
}

// ------------------------------------------------------------- discovery

/// Repo-relative folders below the root, down to `DISCOVERY_DEPTH` levels,
/// that `.gitignore` does not ignore, with the names of their files. Sorted,
/// parents before children. Dot folders and `DISCOVERY_SKIPPED_DIRS` are left
/// out. Only `.gitignore` files inside the repo count, so the answer is the
/// same on every machine.
fn searchable_dirs(root: &Path) -> Vec<String> {
    let mut dirs = Vec::new();
    let walk = WalkBuilder::new(root)
        .max_depth(Some(DISCOVERY_DEPTH))
        .hidden(true)
        .follow_links(false)
        .parents(false)
        .ignore(false)
        .git_global(false)
        .git_exclude(false)
        .git_ignore(true)
        .require_git(false)
        .sort_by_file_name(|a, b| a.cmp(b))
        .filter_entry(|entry| {
            entry
                .file_name()
                .to_str()
                .is_none_or(|name| !DISCOVERY_SKIPPED_DIRS.contains(&name))
        })
        .build();
    for entry in walk.filter_map(|entry| entry.ok()) {
        if entry.depth() == 0 || !entry.file_type().is_some_and(|kind| kind.is_dir()) {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(root) else {
            continue;
        };
        let parts: Vec<String> = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect();
        dirs.push(parts.join("/"));
    }
    dirs.sort();
    dirs
}

fn is_inside(dir: &str, parent: &str) -> bool {
    dir.strip_prefix(parent)
        .is_some_and(|rest| rest.starts_with('/'))
}

/// Projects in subfolders that no workspace file lists. A folder is a project
/// of each kind it has a marker file for. A project's own folder is not
/// searched further: a marker file inside a project belongs to that project.
///
/// A workspace file at the root is the authority for its ecosystem: with a
/// Node workspace (`node_workspace`) no other Node projects are looked for,
/// and the same goes for a Cargo workspace (`cargo_workspace`).
fn discovered_projects(
    root: &Path,
    known: &[Raw],
    node_workspace: bool,
    cargo_workspace: bool,
    warnings: &mut Vec<String>,
) -> Vec<Raw> {
    let mut out: Vec<Raw> = Vec::new();
    let mut claimed: Vec<String> = known
        .iter()
        .filter(|project| project.root != ".")
        .map(|project| project.root.clone())
        .collect();
    for dir in searchable_dirs(root) {
        if claimed.iter().any(|parent| is_inside(&dir, parent)) {
            continue;
        }
        let names = file_names(root, &dir);
        let has = |file: &str| names.iter().any(|name| name == file);
        let is_known = |kind: &str| {
            known
                .iter()
                .any(|project| project.root == dir && project.kind == kind)
        };
        let mut found = Vec::new();
        if has("package.json") && !node_workspace && !is_known("node") {
            if let Some(json) = read_json(root, &in_dir(&dir, "package.json"), warnings) {
                let (detail, _) = node_detail(root, &dir, Some(&json), None);
                found.push(Raw {
                    name: json
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|name| !name.is_empty())
                        .map_or_else(|| folder_name(&dir), str::to_owned),
                    root: dir.clone(),
                    kind: "node",
                    source: source::DISCOVERED,
                    deps: node_deps(&json),
                    detail,
                });
            }
        }
        if has("Cargo.toml") && !cargo_workspace && !is_known("rust") {
            let entries = read_text(root, &in_dir(&dir, "Cargo.toml"))
                .map(|text| parse_toml(&text))
                .unwrap_or_default();
            found.push(Raw {
                name: package_name(&entries).unwrap_or_else(|| folder_name(&dir)),
                root: dir.clone(),
                kind: "rust",
                source: source::DISCOVERED,
                deps: cargo_deps(&entries),
                detail: rust_detail(root, &dir, &entries),
            });
        }
        if is_python_project(&names) {
            found.push(Raw {
                name: python_name(root, &dir),
                root: dir.clone(),
                kind: "python",
                source: source::DISCOVERED,
                deps: Vec::new(),
                detail: python_detail(root, &dir),
            });
        }
        if has("go.mod") {
            found.push(Raw {
                name: go_name(root, &dir),
                root: dir.clone(),
                kind: "go",
                source: source::DISCOVERED,
                deps: Vec::new(),
                detail: go_detail(root, &dir),
            });
        }
        if !found.is_empty() || known.iter().any(|project| project.root == dir) {
            claimed.push(dir);
        }
        out.extend(found);
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
                source: project.source.to_owned(),
                languages: project.detail.languages.clone(),
                package_manager: project.detail.package_manager.clone(),
                lockfiles: project.detail.lockfiles.clone(),
                tools: project.detail.tools.clone(),
                commands: project.detail.commands.clone(),
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
    let root_names = file_names(root, ".");

    let lockfiles = lockfiles_in(&root_names);
    let (root_node, root_tool_commands) = if has_package_json {
        node_detail(root, ".", package_json.as_ref(), None)
    } else {
        (Detail::default(), Commands::default())
    };
    let package_manager = root_node.package_manager.clone();

    let cargo_text = read_text(root, "Cargo.toml");
    let cargo = cargo_text.as_deref().map(parse_toml).unwrap_or_default();

    let mut languages: BTreeSet<String> = root_node.languages.iter().cloned().collect();
    if cargo_text.is_some() {
        languages.insert("rust".to_owned());
    }
    if exists(root, "pyproject.toml") || exists(root, "requirements.txt") {
        languages.insert("python".to_owned());
    }
    if exists(root, "go.mod") {
        languages.insert("go".to_owned());
    }

    let mut tools = BTreeSet::new();
    for (file, tool) in [
        ("nx.json", "nx"),
        ("turbo.json", "turbo"),
        ("pnpm-workspace.yaml", "pnpm-workspace"),
        ("docker-compose.yml", "docker-compose"),
        ("docker-compose.yaml", "docker-compose"),
        ("compose.yml", "docker-compose"),
        ("compose.yaml", "docker-compose"),
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

    // Scripts only: rules compare these with what the repo documents.
    let commands = script_commands(
        package_json.as_ref(),
        package_manager.as_deref().unwrap_or("npm"),
    );

    let (mut raw, node_workspace) =
        node_projects(root, package_json.as_ref(), &root_node, &mut warnings);
    let cargo_workspace = cargo
        .iter()
        .any(|entry| entry.section.split('.').next() == Some("workspace"));
    if cargo_text.is_some() {
        raw.extend(rust_projects(root, &cargo, &mut warnings));
    }
    if is_python_project(&root_names) {
        raw.push(Raw {
            name: python_name(root, "."),
            root: ".".to_owned(),
            kind: "python",
            source: source::ROOT,
            deps: Vec::new(),
            detail: python_detail(root, "."),
        });
    }
    if exists(root, "go.mod") {
        raw.push(Raw {
            name: go_name(root, "."),
            root: ".".to_owned(),
            kind: "go",
            source: source::ROOT,
            deps: Vec::new(),
            detail: go_detail(root, "."),
        });
    }
    let discovered =
        discovered_projects(root, &raw, node_workspace, cargo_workspace, &mut warnings);
    raw.extend(discovered);
    let projects = finish_projects(raw);

    let mut package_managers: BTreeSet<String> = package_manager.iter().cloned().collect();
    for project in &projects {
        languages.extend(project.languages.iter().cloned());
        package_managers.extend(project.package_manager.iter().cloned());
    }

    Ok(RepoFacts {
        schema_version: FACTS_SCHEMA_VERSION,
        package_manager,
        package_managers: package_managers.into_iter().collect(),
        lockfiles,
        languages: languages.into_iter().collect(),
        tools: tools.into_iter().map(str::to_owned).collect(),
        commands,
        projects,
        warnings,
        root_tool_commands,
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

fn command_lines(commands: &Commands, indent: &str) -> Vec<String> {
    [
        ("build", &commands.build),
        ("test", &commands.test),
        ("lint", &commands.lint),
        ("typecheck", &commands.typecheck),
    ]
    .into_iter()
    .filter_map(|(name, command)| {
        command
            .as_ref()
            .map(|command| format!("{indent}{name}: {command}\n"))
    })
    .collect()
}

/// The facts for people.
#[must_use]
pub fn format_facts_human(facts: &RepoFacts) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Package managers: {}\n",
        list_or_none(&facts.package_managers)
    ));
    out.push_str(&format!("Lockfiles: {}\n", list_or_none(&facts.lockfiles)));
    out.push_str(&format!("Languages: {}\n", list_or_none(&facts.languages)));
    out.push_str(&format!("Tools: {}\n", list_or_none(&facts.tools)));
    out.push_str("Commands at the repo root:\n");
    let lines = command_lines(&facts.commands, "  ");
    if lines.is_empty() {
        out.push_str("  none\n");
    }
    out.extend(lines);
    out.push_str(&format!("Projects ({}):\n", facts.projects.len()));
    for project in &facts.projects {
        out.push_str(&format!(
            "  {} [{}] {}\n",
            project.name, project.kind, project.root
        ));
        let mut stack = project.languages.clone();
        stack.extend(project.package_manager.iter().cloned());
        stack.extend(project.tools.iter().cloned());
        if !stack.is_empty() {
            out.push_str(&format!("    stack: {}\n", stack.join(", ")));
        }
        if !project.lockfiles.is_empty() {
            out.push_str(&format!(
                "    lockfiles: {}\n",
                project.lockfiles.join(", ")
            ));
        }
        out.extend(command_lines(&project.commands, "    "));
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
        assert_eq!(facts.schema_version, 2);
        assert_eq!(facts.package_manager, None);
        assert!(facts.lockfiles.is_empty() && facts.languages.is_empty());
        assert!(facts.tools.is_empty() && facts.projects.is_empty());
        assert_eq!(facts.commands, Commands::default());
        let json = format_facts_json(&facts);
        assert!(json.starts_with("{\n  \"schemaVersion\": 2,\n  \"packageManager\": null,"));
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
        // The root is a project of each kind it has a manifest for. The
        // Python one has no name of its own.
        let kinds: Vec<_> = facts
            .projects
            .iter()
            .map(|p| (p.name.as_str(), p.kind.as_str(), p.root.as_str()))
            .collect();
        assert_eq!(
            kinds,
            [
                (".", "python", "."),
                ("app", "node", "."),
                ("app", "rust", ".")
            ]
        );
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
        assert!(
            text.contains(
                "Projects (2):\n  a [node] p/a\n    stack: javascript\n  b [node] p/b\n    stack: javascript\n    depends on: a\n"
            ),
            "{text}"
        );
    }

    #[test]
    fn toml_reader_handles_multiline_arrays_and_comments() {
        let entries = parse_toml("[a]\nx = [\n \"1\", # c\n \"2\"\n]\ny = \"v # not\"\n");
        assert_eq!(quoted_strings(&entries[0].value), ["1", "2"]);
        assert_eq!(unquote(&entries[1].value), "v # not");
    }

    // ------------------------------------------- projects in subfolders

    fn by_root<'a>(facts: &'a RepoFacts, root: &str) -> &'a Project {
        facts
            .projects
            .iter()
            .find(|project| project.root == root)
            .unwrap_or_else(|| panic!("no project at {root}: {:?}", facts.projects))
    }

    #[test]
    fn a_course_project_reports_its_services_and_its_frontend_as_projects() {
        let fixture = crate::testing::course_project().unwrap();
        let facts = detect_facts(fixture.root()).unwrap();

        // The repo-level summary lists what every project uses.
        assert_eq!(facts.package_manager, None);
        assert_eq!(facts.package_managers, ["npm", "pip"]);
        assert_eq!(facts.languages, ["javascript", "python", "typescript"]);
        assert_eq!(facts.tools, ["docker-compose", "github-actions"]);
        assert_eq!(facts.commands, Commands::default());
        assert!(facts.warnings.is_empty(), "{:?}", facts.warnings);

        let roots: Vec<_> = facts.projects.iter().map(|p| p.root.as_str()).collect();
        assert_eq!(
            roots,
            [
                "backend/audit_service",
                "backend/auth_service",
                "frontend",
                "backend/ledger_service",
                "backend/market_service",
                "backend/realtime_service",
            ]
        );
        assert!(facts.projects.iter().all(|p| p.source == "discovered"));

        for service in crate::testing::COURSE_SERVICES {
            let project = by_root(&facts, &format!("backend/{service}"));
            assert_eq!(project.name, service);
            assert_eq!(project.kind, "python");
            assert_eq!(project.languages, ["python"]);
            assert_eq!(project.package_manager.as_deref(), Some("pip"));
            assert!(project.lockfiles.is_empty());
            assert_eq!(project.commands.test.as_deref(), Some("python -m pytest"));
            assert_eq!(project.commands.build, None);
            assert_eq!(project.commands.typecheck, None);
            // Ruff only where it is configured.
            if service == "auth_service" {
                assert_eq!(project.tools, ["alembic", "docker", "pytest", "ruff"]);
                assert_eq!(
                    project.commands.lint.as_deref(),
                    Some("python -m ruff check .")
                );
            } else {
                assert_eq!(project.tools, ["alembic", "docker", "pytest"]);
                assert_eq!(project.commands.lint, None);
            }
        }

        let frontend = by_root(&facts, "frontend");
        assert_eq!(
            (frontend.name.as_str(), frontend.kind.as_str()),
            ("frontend", "node")
        );
        assert_eq!(frontend.languages, ["javascript", "typescript"]);
        assert_eq!(frontend.package_manager.as_deref(), Some("npm"));
        assert_eq!(frontend.lockfiles, ["package-lock.json"]);
        assert_eq!(frontend.tools, ["knip", "oxlint", "playwright", "vite"]);
        assert_eq!(frontend.commands.build.as_deref(), Some("npm run build"));
        // No lint or typecheck script: the linter's config file and the
        // tsconfig give the commands. Playwright alone is not a test command.
        assert_eq!(frontend.commands.lint.as_deref(), Some("npx --no oxlint"));
        assert_eq!(
            frontend.commands.typecheck.as_deref(),
            Some("npx --no tsc --noEmit")
        );
        assert_eq!(frontend.commands.test, None);

        let json = format_facts_json(&facts);
        assert!(!json.contains('\\'), "{json}");
        let text = format_facts_human(&facts);
        assert!(text.starts_with("Package managers: npm, pip\n"), "{text}");
        assert!(
            text.contains(
                "  auth_service [python] backend/auth_service\n    stack: python, pip, alembic, docker, pytest, ruff\n    test: python -m pytest\n    lint: python -m ruff check .\n"
            ),
            "{text}"
        );
    }

    #[test]
    fn a_folder_without_a_marker_is_not_a_project() {
        let fixture = crate::testing::course_project().unwrap();
        let facts = detect_facts(fixture.root()).unwrap();
        // `backend/` has only a .coveragerc, and `backend/shared` only code.
        assert!(facts
            .projects
            .iter()
            .all(|p| p.root != "backend" && p.root != "backend/shared"));
    }

    #[test]
    fn a_lone_requirements_file_is_not_a_project() {
        let facts = facts(&[
            ("docs/requirements.txt", "sphinx\n"),
            ("api/requirements.txt", "flask\n"),
            ("api/app.py", ""),
            ("jobs/requirements.txt", "celery\n"),
            ("jobs/pytest.ini", "[pytest]\n"),
        ]);
        let roots: Vec<_> = facts.projects.iter().map(|p| p.root.as_str()).collect();
        assert_eq!(roots, ["api", "jobs"]);
        // A project with no test runner in sight has no test command.
        assert_eq!(by_root(&facts, "api").commands.test, None);
        assert_eq!(
            by_root(&facts, "jobs").commands.test.as_deref(),
            Some("python -m pytest")
        );
    }

    #[test]
    fn projects_are_found_down_to_the_depth_limit_and_no_deeper() {
        let facts = facts(&[
            ("a/b/c/d/go.mod", "module example.com/x/deep\n"),
            ("a/b/c/d/e/go.mod", "module example.com/x/too-deep\n"),
            (
                "one/two/three/four/five/package.json",
                r#"{"name":"buried"}"#,
            ),
        ]);
        assert_eq!(DISCOVERY_DEPTH, 4);
        let found: Vec<_> = facts
            .projects
            .iter()
            .map(|p| (p.name.as_str(), p.kind.as_str(), p.root.as_str()))
            .collect();
        assert_eq!(found, [("deep", "go", "a/b/c/d")]);
        let deep = &facts.projects[0];
        assert_eq!(deep.commands.test.as_deref(), Some("go test ./..."));
        assert_eq!(deep.package_manager.as_deref(), Some("go"));
        assert_eq!(facts.languages, ["go"]);
    }

    #[test]
    fn a_marker_inside_a_project_belongs_to_that_project() {
        let facts = facts(&[
            ("app/package.json", r#"{"name":"app"}"#),
            ("app/examples/demo/package.json", r#"{"name":"demo"}"#),
            (
                "app/tools/gen/pyproject.toml",
                "[project]\nname = \"gen\"\n",
            ),
            ("svc/Cargo.toml", "[package]\nname = \"svc\"\n"),
            ("svc/fuzz/Cargo.toml", "[package]\nname = \"svc-fuzz\"\n"),
        ]);
        let found: Vec<_> = facts
            .projects
            .iter()
            .map(|p| (p.name.as_str(), p.root.as_str()))
            .collect();
        assert_eq!(found, [("app", "app"), ("svc", "svc")]);
    }

    #[test]
    fn one_folder_can_hold_projects_of_two_kinds() {
        let facts = facts(&[
            ("tool/package.json", r#"{"name":"tool-js"}"#),
            (
                "tool/pyproject.toml",
                "[project]\nname = \"tool-py\"\n[tool.uv]\n",
            ),
            ("tool/uv.lock", ""),
        ]);
        let found: Vec<_> = facts
            .projects
            .iter()
            .map(|p| (p.name.as_str(), p.kind.as_str(), p.root.as_str()))
            .collect();
        assert_eq!(
            found,
            [("tool-js", "node", "tool"), ("tool-py", "python", "tool")]
        );
        let python = &facts.projects[1];
        assert_eq!(python.package_manager.as_deref(), Some("uv"));
        assert_eq!(python.lockfiles, ["uv.lock"]);
        assert_eq!(facts.package_managers, ["uv"]);
    }

    #[test]
    fn a_root_workspace_file_decides_the_projects_of_its_ecosystem() {
        // Node: only the listed member. Python is still searched for.
        let facts = facts(&[
            (
                "package.json",
                r#"{"name":"root","workspaces":["packages/a"]}"#,
            ),
            ("packages/a/package.json", r#"{"name":"a"}"#),
            ("packages/b/package.json", r#"{"name":"b"}"#),
            ("services/api/pyproject.toml", "[project]\nname = \"api\"\n"),
        ]);
        let found: Vec<_> = facts
            .projects
            .iter()
            .map(|p| (p.name.as_str(), p.source.as_str()))
            .collect();
        assert_eq!(
            found,
            [("a", "workspace"), ("api", "discovered"), ("root", "root")]
        );
        // A workspace member takes the root's package manager.
        let with_lock = self::facts(&[
            (
                "package.json",
                r#"{"name":"root","workspaces":["packages/a"]}"#,
            ),
            ("yarn.lock", ""),
            (
                "packages/a/package.json",
                r#"{"name":"a","scripts":{"test":"x"}}"#,
            ),
        ]);
        let a = by_root(&with_lock, "packages/a");
        assert_eq!(a.package_manager.as_deref(), Some("yarn"));
        assert_eq!(a.commands.test.as_deref(), Some("yarn run test"));
    }

    #[test]
    fn gitignored_folders_are_not_searched() {
        let facts = facts(&[
            (".gitignore", "/scratch/\nlocal-*/\n"),
            ("scratch/x/package.json", r#"{"name":"x"}"#),
            ("local-copy/package.json", r#"{"name":"copy"}"#),
            ("kept/package.json", r#"{"name":"kept"}"#),
        ]);
        let found: Vec<_> = facts.projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(found, ["kept"]);
    }

    #[test]
    fn linters_and_test_runners_are_detected_from_config_files() {
        let facts = facts(&[
            ("web/package.json", r#"{"scripts":{"lint":"custom-lint"}}"#),
            ("web/pnpm-lock.yaml", ""),
            ("web/eslint.config.js", ""),
            ("web/vitest.config.ts", ""),
            (
                "web/tsconfig.json",
                r#"{"references":[{"path":"./tsconfig.app.json"}]}"#,
            ),
        ]);
        let web = by_root(&facts, "web");
        // No name in package.json: the folder's name.
        assert_eq!(web.name, "web");
        assert_eq!(web.tools, ["eslint", "vitest"]);
        // A script wins over the tool's config file.
        assert_eq!(web.commands.lint.as_deref(), Some("pnpm run lint"));
        assert_eq!(web.commands.test.as_deref(), Some("pnpm exec vitest run"));
        // A tsconfig of project references gets no guessed typecheck command.
        assert_eq!(web.commands.typecheck, None);
    }

    #[test]
    fn ruff_configured_in_a_folder_above_counts() {
        let facts = facts(&[
            ("pyproject.toml", "[tool.ruff]\nline-length = 100\n"),
            ("services/a/requirements.txt", "pytest\nmypy\n"),
            ("services/a/main.py", ""),
        ]);
        let a = by_root(&facts, "services/a");
        assert_eq!(a.tools, ["mypy", "pytest", "ruff"]);
        assert_eq!(a.commands.lint.as_deref(), Some("python -m ruff check ."));
        assert_eq!(a.commands.typecheck.as_deref(), Some("python -m mypy ."));
    }
}
