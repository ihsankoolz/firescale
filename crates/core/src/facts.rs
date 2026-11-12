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
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use serde::Serialize;
use serde_json::Value;

use crate::error::{Error, Result};

/// Version of the `fscale facts --json` format.
pub const FACTS_SCHEMA_VERSION: u32 = 2;

/// How many folder levels below the repo root are searched for projects.
/// `backend/auth_service` is level 2.
pub const DISCOVERY_DEPTH: usize = 4;

/// Folders never searched for projects: dependencies, caches, build output,
/// and folders of examples and test fixtures.
/// Folders whose name starts with a dot are skipped too, and so is everything
/// `.gitignore` ignores.
pub const DISCOVERY_SKIPPED_DIRS: [&str; 26] = [
    // Sample and test input: a manifest in there is not a project to verify.
    "fixtures",
    "__fixtures__",
    "test-fixtures",
    "testdata",
    "examples",
    "example",
    "samples",
    "templates",
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
    /// For a Python project: the interpreter its commands use (ADR 0022).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interpreter: Option<Interpreter>,
}

/// The Python interpreter a project's commands run with, and why that one
/// (ADR 0022). The first match wins:
///
/// 1. `venv`: a `.venv` or `venv` folder in the project's folder with an
///    interpreter in it (`Scripts\python.exe` on Windows, `bin/python`
///    elsewhere).
/// 2. `uv`, `poetry`, `pipenv`: the project's own manager picks the
///    environment (`uv run`, `poetry run`, `pipenv run`).
/// 3. `active`: the virtual environment named by `VIRTUAL_ENV`.
/// 4. `path`: no virtual environment was found; `python` from the PATH.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Interpreter {
    /// `venv`, `uv`, `poetry`, `pipenv`, `active` or `path`.
    pub source: String,
    /// What each command starts with, for example `.venv/bin/python -m`,
    /// `uv run` or `python -m`.
    pub command: String,
    /// One sentence for people: where the interpreter came from.
    pub detail: String,
}

/// Commands to run, by purpose. Absent when the repo has no such script.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Commands {
    /// For a project: installs its dependencies. Verify names it when a step
    /// fails because the environment is not set up. Never set at the repo
    /// level, which lists scripts only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install: Option<String>,
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
    /// Things worth knowing that need no action, such as a watch-mode script
    /// replaced by its run-once sibling. Sorted.
    pub notes: Vec<String>,
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
pub(crate) fn strip_comment(line: &str) -> &str {
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

pub(crate) fn unquote(value: &str) -> String {
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
pub(crate) struct TomlEntry {
    pub(crate) section: String,
    pub(crate) key: String,
    pub(crate) value: String,
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
pub(crate) fn parse_toml(text: &str) -> Vec<TomlEntry> {
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
    interpreter: Option<Interpreter>,
    /// Node only: the steps whose package script watches for changes.
    watching: Vec<WatchScript>,
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

/// The run-once siblings of a step's script, tried in this order when the
/// step's own script watches for changes: `test:run`, `test:ci`, `test:once`.
pub const RUN_ONCE_SUFFIXES: [&str; 3] = ["run", "ci", "once"];

/// The commands of `vitest`. Any other word is a test filter.
const VITEST_COMMANDS: [&str; 8] = [
    "run",
    "watch",
    "dev",
    "related",
    "bench",
    "list",
    "init",
    "typecheck",
];

/// True for a `NAME=value` word in front of a command.
fn is_env_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// The tool of a package script that watches for changes and so may never
/// exit, or `None`. Each command of the script, split at `&&`, `||`, `;` and
/// `|`, is read: its words after any `NAME=value` and after `npx`,
/// `pnpm exec`, `yarn`, `yarn exec` or `bunx`. The list is in docs/facts.md
/// ("Watch-mode scripts").
fn watch_tool(script: &str) -> Option<&'static str> {
    script.split(['&', '|', ';']).find_map(command_watch_tool)
}

/// `watch_tool` for one command.
fn command_watch_tool(command: &str) -> Option<&'static str> {
    let all: Vec<&str> = command.split_whitespace().collect();
    let start = all
        .iter()
        .take_while(|word| is_env_assignment(word))
        .count();
    let mut words = &all[start..];
    match words {
        ["npx", rest @ ..] => {
            // `npx --no vitest`, `npx -y vitest`
            let flags = rest.iter().take_while(|word| word.starts_with('-')).count();
            words = &rest[flags..];
        }
        ["pnpm" | "yarn", "exec", rest @ ..] | ["yarn" | "bunx", rest @ ..] => words = rest,
        _ => {}
    }
    let (tool, args) = words.split_first()?;
    let any = |flags: &[&str]| args.iter().any(|arg| flags.contains(arg));
    let first_word = args.iter().find(|arg| !arg.starts_with('-')).copied();
    let (name, watches) = match *tool {
        "vitest" => {
            let command = args
                .iter()
                .find(|arg| VITEST_COMMANDS.contains(arg))
                .copied();
            (
                "vitest",
                matches!(command, None | Some("watch" | "dev"))
                    && !any(&["--run", "--watch=false", "--no-watch"]),
            )
        }
        "jest" => (
            "jest",
            any(&["--watch", "--watchAll", "--watch=true", "--watchAll=true"]),
        ),
        "react-scripts" => (
            "react-scripts",
            first_word == Some("test") && !any(&["--watchAll=false", "--watch=false"]),
        ),
        "ng" => (
            "ng",
            matches!(first_word, Some("test" | "t")) && !any(&["--watch=false", "--no-watch"]),
        ),
        "mocha" => ("mocha", any(&["--watch", "-w"])),
        "ava" => ("ava", any(&["--watch", "-w"])),
        "node" => ("node", any(&["--test"]) && any(&["--watch"])),
        "tsc" => ("tsc", any(&["--watch", "-w"])),
        "vue-cli-service" => (
            "vue-cli-service",
            first_word == Some("test:unit") && any(&["--watch"]),
        ),
        _ => return None,
    };
    watches.then_some(name)
}

/// A step whose package script watches for changes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WatchScript {
    /// `test`, `lint` or `typecheck`.
    step: &'static str,
    /// The text of the step's script.
    script: String,
    /// The tool that watches, such as `vitest`.
    tool: &'static str,
    /// The run-once sibling the step uses instead: its name and its text.
    sibling: Option<(String, String)>,
}

/// The commands a `package.json` gives through its scripts. A test, lint or
/// typecheck script that watches for changes is replaced by its first
/// run-once sibling (`<step>:run`, `<step>:ci`, `<step>:once`) that does not
/// watch; such steps are returned too, for the notes and warnings.
fn script_commands(json: Option<&Value>, package_manager: &str) -> (Commands, Vec<WatchScript>) {
    let scripts = json.and_then(|json| json.get("scripts"));
    let text = |name: &str| {
        scripts
            .and_then(|scripts| scripts.get(name))
            .and_then(Value::as_str)
    };
    let has = |name: &str| scripts.and_then(|scripts| scripts.get(name)).is_some();
    let run = |name: &str| has(name).then(|| format!("{package_manager} run {name}"));
    let mut watching = Vec::new();
    let mut step = |name: &'static str| -> Option<String> {
        let script = text(name)?;
        let Some(tool) = watch_tool(script) else {
            return run(name);
        };
        let sibling = RUN_ONCE_SUFFIXES.iter().find_map(|suffix| {
            let sibling = format!("{name}:{suffix}");
            text(&sibling)
                .filter(|command| watch_tool(command).is_none())
                .map(|command| (sibling, command.to_owned()))
        });
        let command = run(sibling
            .as_ref()
            .map_or(name, |(sibling, _)| sibling.as_str()));
        watching.push(WatchScript {
            step: name,
            script: script.to_owned(),
            tool,
            sibling,
        });
        command
    };
    let commands = Commands {
        install: None,
        build: run("build"),
        test: step("test").or_else(|| run("test")),
        lint: step("lint").or_else(|| run("lint")),
        typecheck: step("typecheck").or_else(|| run("typecheck")),
    };
    (commands, watching)
}

/// Adds what `fscale facts` says about the watch-mode scripts of the Node
/// project in `folder`: a note when a run-once sibling is used, else a
/// warning.
fn watch_messages(
    folder: &str,
    watching: &[WatchScript],
    notes: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    let place = if folder == "." { "repo root" } else { folder };
    for watch in watching {
        let WatchScript {
            step,
            script,
            tool,
            sibling,
        } = watch;
        let runs = format!("{place}: the {step} script runs `{script}`, which watches for changes");
        match sibling {
            Some((name, command)) => {
                notes.push(format!("{runs}; verify uses {name} (`{command}`) instead."));
            }
            // These tools turn watch mode off when CI is set.
            None if matches!(*tool, "vitest" | "react-scripts") => warnings.push(format!(
                "{runs} and may never exit. verify runs it with CI=1, which turns watch mode off for most tools (vitest, react-scripts); add a {step}:run script that runs once to be sure."
            )),
            None => warnings.push(format!(
                "{runs} and may never exit: CI=1, which verify sets, does not turn off watch mode for {tool}. Add a {step}:run script that runs once."
            )),
        }
    }
}

/// Commands worked out from tool config files, for a project with no script
/// for the step. Only tools that check without changing files, and no
/// end-to-end runners: those need browsers and running services.
fn node_tool_commands(
    root: &Path,
    dir: &str,
    tools: &[String],
    configured: &[String],
    exec: &str,
) -> Commands {
    let has = |tool: &str| tools.iter().any(|name| name == tool);
    // A linter is only run when its config file is there: `eslint .` fails
    // without one, and a dependency alone does not say the project uses it.
    let has_config = |tool: &str| configured.iter().any(|name| name == tool);
    let lint = if has_config("oxlint") {
        Some(format!("{exec} oxlint"))
    } else if has_config("eslint") {
        Some(format!("{exec} eslint ."))
    } else if has_config("biome") {
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
        install: None,
        build: None,
        test,
        lint,
        typecheck,
    }
}

fn or_else(first: Commands, second: Commands) -> Commands {
    Commands {
        install: first.install.or(second.install),
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
    let configured: Vec<String> = NODE_TOOLS
        .iter()
        .filter(|tool| {
            names
                .iter()
                .any(|name| tool.prefixes.iter().any(|prefix| name.starts_with(prefix)))
        })
        .map(|tool| tool.name.to_owned())
        .collect();
    let mut tools: Vec<String> = NODE_TOOLS
        .iter()
        .filter(|tool| {
            configured.iter().any(|name| name == tool.name)
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
    let (scripts, watching) = script_commands(json, package_manager.as_deref().unwrap_or("npm"));
    let from_tools = node_tool_commands(
        root,
        dir,
        &tools,
        &configured,
        node_exec(package_manager.as_deref()),
    );
    // A manifest that cannot be read gives no commands at all.
    let install = Commands {
        install: json.map(|_| format!("{} install", package_manager.as_deref().unwrap_or("npm"))),
        ..Commands::default()
    };
    let detail = Detail {
        languages,
        package_manager,
        lockfiles: lockfiles
            .into_iter()
            .filter(|lock| !matches!(lock.as_str(), "Cargo.lock" | "go.sum"))
            .collect(),
        tools,
        commands: or_else(or_else(scripts.clone(), from_tools.clone()), install),
        interpreter: None,
        watching,
    };
    let only_tools = Commands {
        install: None,
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
            install: None,
        },
        interpreter: None,
        watching: Vec::new(),
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
            install: None,
        },
        interpreter: None,
        watching: Vec::new(),
    }
}

/// How many levels of `-r` includes between requirements files are followed.
const MAX_REQUIREMENT_INCLUDES: usize = 5;

/// The package name at the start of a requirement such as `Ruff[x]>=1`,
/// lower-cased with `_` as `-`. `None` for a URL, a VCS link or a path.
pub(crate) fn requirement_name(spec: &str) -> Option<String> {
    let spec = spec.trim();
    let name: String = spec
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .collect();
    let starts_well = name.starts_with(|c: char| c.is_ascii_alphanumeric());
    // `git+https://…`, `https://…`, `file:…` and `C:\…` are not names.
    let looks_like_link = spec[name.len()..].starts_with([':', '+', '/', '\\']);
    (starts_well && !looks_like_link).then(|| name.to_lowercase().replace('_', "-"))
}

/// The quoted strings of an array value, leaving out inline tables such as
/// `{include-group = "test"}`.
pub(crate) fn array_strings(value: &str) -> Vec<String> {
    let mut kept = String::new();
    let mut quote: Option<char> = None;
    let mut depth = 0usize;
    for c in value.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '{') => {
                depth += 1;
                continue;
            }
            (None, '}') => {
                depth = depth.saturating_sub(1);
                continue;
            }
            _ => {}
        }
        if depth == 0 {
            kept.push(c);
        }
    }
    quoted_strings(&kept)
}

/// The file named by a `-r <file>`, `--requirement <file>` or
/// `--requirement=<file>` line.
pub(crate) fn requirement_include(line: &str) -> Option<String> {
    let rest = if let Some(rest) = line.strip_prefix("--requirement") {
        if !rest.starts_with(|c: char| c == '=' || c.is_whitespace()) {
            return None;
        }
        rest
    } else {
        line.strip_prefix("-r")?
    };
    let target = unquote(rest.trim_start().trim_start_matches('=').trim());
    (!target.is_empty()).then_some(target)
}

/// `target`, as written in the repo-relative `file`, as a repo-relative
/// path. `None` when it is absolute or leaves the repo.
pub(crate) fn resolve_include(file: &str, target: &str) -> Option<String> {
    let absolute = target.starts_with(['/', '\\'])
        || target.as_bytes().get(1) == Some(&b':')
        || target.contains("://");
    if absolute {
        return None;
    }
    let mut parts: Vec<&str> = file.split('/').collect();
    parts.pop();
    for part in target.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(part),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// Adds the packages of a requirements file, and of the files it includes,
/// to `out`. Constraint files (`-c`), editable installs (`-e`), other
/// options and links are left out.
pub(crate) fn read_requirements(
    root: &Path,
    file: &str,
    depth: usize,
    seen: &mut BTreeSet<String>,
    out: &mut BTreeSet<String>,
) {
    if !seen.insert(file.to_owned()) {
        return;
    }
    let Some(text) = read_text(root, file) else {
        return;
    };
    for line in text.trim_start_matches('\u{feff}').lines() {
        let line = strip_comment(line).trim();
        if line.starts_with('-') {
            if let Some(target) = requirement_include(line) {
                if depth < MAX_REQUIREMENT_INCLUDES {
                    if let Some(included) = resolve_include(file, &target) {
                        read_requirements(root, &included, depth + 1, seen, out);
                    }
                }
            }
            continue;
        }
        out.extend(requirement_name(line));
    }
}

/// Packages from the dependency lists of a `pyproject.toml`: PEP 621
/// dependencies and extras, PEP 735 dependency groups, uv, PDM and Poetry.
fn pyproject_dependencies(text: &str, out: &mut BTreeSet<String>) {
    for entry in parse_toml(text) {
        let section = entry.section.as_str();
        let array = (section == "project" && entry.key == "dependencies")
            || (section == "tool.uv" && entry.key == "dev-dependencies")
            || matches!(
                section,
                "project.optional-dependencies" | "dependency-groups" | "tool.pdm.dev-dependencies"
            );
        let poetry_table = matches!(
            section,
            "tool.poetry.dependencies" | "tool.poetry.dev-dependencies"
        ) || section
            .strip_prefix("tool.poetry.group.")
            .is_some_and(|rest| rest.ends_with(".dependencies"));
        if array {
            out.extend(
                array_strings(&entry.value)
                    .iter()
                    .filter_map(|spec| requirement_name(spec)),
            );
        } else if poetry_table {
            out.extend(requirement_name(&entry.key));
        }
    }
}

/// Lower-case names of the packages a Python project declares: in its
/// requirements files (`.txt` files with `requirements` in the name, the
/// `.txt` files of a `requirements/` folder, and the files they include with
/// `-r`), its `pyproject.toml` and its `Pipfile`. Only the project's own
/// folder is searched.
fn python_dependencies(root: &Path, dir: &str, names: &[String]) -> BTreeSet<String> {
    let is_txt = |name: &String| name.to_lowercase().ends_with(".txt");
    let mut files: Vec<String> = names
        .iter()
        .filter(|name| is_txt(name) && name.to_lowercase().contains("requirements"))
        .map(|name| in_dir(dir, name))
        .collect();
    let folder = in_dir(dir, "requirements");
    files.extend(
        file_names(root, &folder)
            .iter()
            .filter(|name| is_txt(name))
            .map(|name| in_dir(&folder, name)),
    );

    let mut out = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for file in &files {
        read_requirements(root, file, 0, &mut seen, &mut out);
    }
    if let Some(text) = read_text(root, &in_dir(dir, "pyproject.toml")) {
        pyproject_dependencies(&text, &mut out);
    }
    if let Some(text) = read_text(root, &in_dir(dir, "Pipfile")) {
        for entry in parse_toml(&text) {
            if matches!(entry.section.as_str(), "packages" | "dev-packages") {
                out.extend(requirement_name(&entry.key));
            }
        }
    }
    out
}

/// The packages a Python project in `dir` (repo-relative, `.` for the
/// root) declares in its requirements files, `pyproject.toml` or `Pipfile`.
#[must_use]
pub fn python_declared(root: &Path, dir: &str) -> BTreeSet<String> {
    let names = file_names(root, dir);
    python_dependencies(root, dir, &names)
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
const PYTHON_SIGNS: [&str; 10] = [
    "pyproject.toml",
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

/// True when the folder is a Python project: it has a `setup.py`, a
/// `Pipfile` or a `pyproject.toml` that describes a package, or a
/// `requirements.txt` together with a Python config file or a `.py` file.
///
/// A lone `requirements.txt` is often only the input of a docs build or a
/// container image. A `pyproject.toml` with only `[tool.*]` sections is
/// shared tool config, often placed above the real projects, and must not
/// stop the search there.
fn is_python_project(root: &Path, dir: &str, names: &[String]) -> bool {
    let has = |file: &str| names.iter().any(|name| name == file);
    let package_pyproject = has("pyproject.toml")
        && read_text(root, &in_dir(dir, "pyproject.toml")).is_some_and(|text| {
            parse_toml(&text).iter().any(|entry| {
                matches!(
                    entry.section.as_str(),
                    "project" | "tool.poetry" | "build-system"
                )
            })
        });
    package_pyproject
        || has("setup.py")
        || has("Pipfile")
        || (has("requirements.txt")
            && (PYTHON_SIGNS.iter().any(|sign| has(sign))
                || names.iter().any(|name| name.ends_with(".py"))))
}

/// The folders that hold a project's own virtual environment, in order.
pub const VENV_DIRS: [&str; 2] = [".venv", "venv"];

/// The interpreter inside a virtual environment folder, relative to it, as
/// the platform's shell runs it.
#[must_use]
pub fn venv_python(venv: &str) -> String {
    if cfg!(windows) {
        format!("{venv}\\Scripts\\python.exe")
    } else {
        format!("{venv}/bin/python")
    }
}

/// The parts of `venv_python` as path segments, for checking the disk.
fn venv_python_parts() -> &'static [&'static str] {
    if cfg!(windows) {
        &["Scripts", "python.exe"]
    } else {
        &["bin", "python"]
    }
}

/// Quotes a path for the platform's shell when it holds anything other than
/// plain path characters. Both `sh` and `cmd` take double quotes; `sh` gets
/// single quotes when the path has a character double quotes would expand.
#[must_use]
pub fn shell_quote(path: &str) -> String {
    // A backslash is a path separator only for cmd; sh reads it as an escape.
    let allowed = if cfg!(windows) {
        "/\\._-:+@"
    } else {
        "/._-:+@"
    };
    let plain = !path.is_empty()
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || allowed.contains(c));
    if plain {
        path.to_owned()
    } else if cfg!(windows) || !path.contains(['$', '`', '"', '\\', '!']) {
        format!("\"{path}\"")
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    }
}

/// The dependency file to install a Python project from: the first of these
/// in its folder. Development lists come first, because they also hold the
/// test runner and the linters that verify runs.
const PYTHON_INSTALL_FILES: [&str; 7] = [
    "requirements-dev.txt",
    "requirements_dev.txt",
    "dev-requirements.txt",
    "requirements-test.txt",
    "requirements_test.txt",
    "test-requirements.txt",
    "requirements.txt",
];

/// Picks the interpreter for a Python project's commands (see `Interpreter`
/// for the order) and the command that installs its dependencies.
/// `manager` is `uv`, `poetry`, `pipenv` or `pip`. `active` is the value of
/// `VIRTUAL_ENV`, if any.
fn python_interpreter(
    root: &Path,
    dir: &str,
    names: &[String],
    manager: &str,
    active: Option<&Path>,
) -> (Interpreter, Option<String>) {
    let folder = in_dir(dir, ".");
    let base = folder
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .fold(root.to_path_buf(), |path, part| path.join(part));
    let has = |file: &str| names.iter().any(|name| name == file);
    // A development list that does not include requirements.txt itself
    // (`-r requirements.txt`) is installed together with it, so the app's
    // own dependencies are there too.
    let includes_base = |file: &str| {
        read_text(root, &in_dir(dir, file)).is_some_and(|text| {
            text.lines().any(|line| {
                let line = strip_comment(line).trim();
                let target = line
                    .strip_prefix("--requirement=")
                    .or_else(|| line.strip_prefix("--requirement"))
                    .or_else(|| line.strip_prefix("-r"))
                    .map(str::trim);
                target == Some("requirements.txt")
            })
        })
    };
    let install_from = |python: &str| {
        PYTHON_INSTALL_FILES
            .iter()
            .find(|file| has(file))
            .map(|file| {
                if *file != "requirements.txt" && has("requirements.txt") && !includes_base(file) {
                    format!("{python} -m pip install -r requirements.txt -r {file}")
                } else {
                    format!("{python} -m pip install -r {file}")
                }
            })
            .or_else(|| {
                (has("pyproject.toml") || has("setup.py"))
                    .then(|| format!("{python} -m pip install -e ."))
            })
    };

    let managed = match manager {
        "uv" => Some((
            "uv run",
            "uv sync",
            "a uv project: uv picks the environment",
        )),
        "poetry" => Some((
            "poetry run",
            "poetry install",
            "a poetry project: poetry picks the environment",
        )),
        "pipenv" => Some((
            "pipenv run",
            "pipenv install --dev",
            "a Pipfile project: pipenv picks the environment",
        )),
        _ => None,
    };
    let mut broken = Vec::new();
    for venv in VENV_DIRS {
        let folder = base.join(venv);
        if !folder.is_dir() {
            continue;
        }
        let interpreter = venv_python_parts()
            .iter()
            .fold(folder.clone(), |path, part| path.join(part));
        if interpreter.is_file() {
            let python = venv_python(venv);
            // A uv, poetry or pipenv project installs with its manager, also
            // when its .venv runs the commands: uv's venvs have no pip.
            let install = match managed {
                Some((_, install, _)) => Some(install.to_owned()),
                None => install_from(&python),
            };
            return (
                Interpreter {
                    source: "venv".to_owned(),
                    command: format!("{python} -m"),
                    detail: format!("the virtual environment in the project's {venv} folder"),
                },
                install,
            );
        }
        broken.push(venv);
    }
    let not_usable = if broken.is_empty() {
        String::new()
    } else {
        format!(
            "; {} has no {}",
            broken.join(" and "),
            venv_python_parts().join("/")
        )
    };

    if let Some((command, install, detail)) = managed {
        return (
            Interpreter {
                source: manager.to_owned(),
                command: command.to_owned(),
                detail: format!("{detail}{not_usable}"),
            },
            Some(install.to_owned()),
        );
    }

    // cmd expands `%NAME%` even inside quotes, so on Windows a path with `%`
    // cannot be written into a command safely; such an environment is skipped.
    let quotable = |path: &Path| !(cfg!(windows) && path.to_string_lossy().contains(['%', '"']));
    if let Some(active) = active.filter(|path| path.is_absolute() && quotable(path)) {
        let interpreter: PathBuf = venv_python_parts()
            .iter()
            .fold(active.to_path_buf(), |path, part| path.join(part));
        if interpreter.is_file() {
            let python = shell_quote(&interpreter.to_string_lossy());
            let install = install_from(&python);
            return (
                Interpreter {
                    source: "active".to_owned(),
                    command: format!("{python} -m"),
                    detail: format!("the active virtual environment (VIRTUAL_ENV){not_usable}"),
                },
                install,
            );
        }
    }

    // Today's behaviour: install into the same `python` the steps run with.
    // Creating `.venv` first (`python -m venv .venv`) makes the next run use it.
    let install = install_from("python");
    (
        Interpreter {
            source: "path".to_owned(),
            command: "python -m".to_owned(),
            detail: format!(
                "no virtual environment found, so python from the PATH: no .venv or venv folder in the project{not_usable}, no active VIRTUAL_ENV, and not a uv or poetry project"
            ),
        },
        install,
    )
}

/// A Python project's folder. `active` is the value of `VIRTUAL_ENV`.
/// True when a folder above `dir`, up to the repo root, holds a uv
/// workspace (`[tool.uv.workspace]` in its `pyproject.toml`): its members
/// share the workspace's `uv.lock` and environment, so uv runs them.
fn in_uv_workspace(root: &Path, dir: &str) -> bool {
    let parts: Vec<&str> = dir
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    (0..parts.len()).any(|depth| {
        let folder = parts[..depth].join("/");
        let folder = if folder.is_empty() {
            ".".to_owned()
        } else {
            folder
        };
        read_text(root, &in_dir(&folder, "pyproject.toml"))
            .is_some_and(|text| text.contains("[tool.uv.workspace]"))
    })
}

fn python_detail(root: &Path, dir: &str, active: Option<&Path>) -> Detail {
    let names = file_names(root, dir);
    let has = |file: &str| names.iter().any(|name| name == file);
    let pyproject = read_text(root, &in_dir(dir, "pyproject.toml")).unwrap_or_default();
    let contains = |file: &str, needle: &str| {
        read_text(root, &in_dir(dir, file)).is_some_and(|text| text.contains(needle))
    };
    let requirements = python_dependencies(root, dir, &names);

    let package_manager =
        if has("uv.lock") || pyproject.contains("[tool.uv") || in_uv_workspace(root, dir) {
            "uv"
        } else if has("poetry.lock") || pyproject.contains("[tool.poetry") {
            "poetry"
        } else if has("Pipfile") {
            "pipenv"
        } else {
            "pip"
        };
    // The module form (`python -m pytest`) works in any environment that
    // has the tool installed.
    let (interpreter, install) = python_interpreter(root, dir, &names, package_manager, active);
    let run = format!("{} ", interpreter.command);

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
            install,
            build: None,
            test: pytest.then(|| format!("{run}pytest")),
            lint: ruff.then(|| format!("{run}ruff check .")),
            typecheck: mypy.then(|| format!("{run}mypy .")),
        },
        interpreter: Some(interpreter),
        watching: Vec::new(),
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
        // A private app's package.json often has no name (found on
        // vercel/commerce): it is still the root's Node project. Its name
        // is never the folder's, so a dry run in a scratch copy sees the
        // same facts. A nameless workspace root is only a container.
        if let Some(name) = json
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| patterns.is_empty().then(|| UNNAMED_ROOT.to_owned()))
        {
            out.push(Raw {
                name,
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
    active: Option<&Path>,
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
            // A manifest that cannot be read still marks a project. It gets
            // no commands, so verify lists its steps as skipped instead of
            // leaving the project out.
            let json = read_json(root, &in_dir(&dir, "package.json"), warnings);
            let (detail, _) = node_detail(root, &dir, json.as_ref(), None);
            found.push(Raw {
                name: json
                    .as_ref()
                    .and_then(|json| json.get("name"))
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty())
                    .map_or_else(|| folder_name(&dir), str::to_owned),
                root: dir.clone(),
                kind: "node",
                source: source::DISCOVERED,
                deps: json.as_ref().map(node_deps).unwrap_or_default(),
                detail,
            });
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
        if is_python_project(root, &dir, &names) {
            found.push(Raw {
                name: python_name(root, &dir),
                root: dir.clone(),
                kind: "python",
                source: source::DISCOVERED,
                deps: Vec::new(),
                detail: python_detail(root, &dir, active),
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
            let mut commands = project.detail.commands.clone();
            // The root installs a workspace's members.
            if project.source == source::WORKSPACE {
                commands.install = None;
            }
            Project {
                name: project.name.clone(),
                root: project.root.clone(),
                kind: project.kind.to_owned(),
                source: project.source.to_owned(),
                languages: project.detail.languages.clone(),
                package_manager: project.detail.package_manager.clone(),
                lockfiles: project.detail.lockfiles.clone(),
                tools: project.detail.tools.clone(),
                commands,
                depends_on,
                interpreter: project.detail.interpreter.clone(),
            }
        })
        .collect();
    projects.sort_by(|a, b| (&a.name, &a.kind, &a.root).cmp(&(&b.name, &b.kind, &b.root)));
    projects.dedup();
    projects
}

// ---------------------------------------------------------------- facts

/// Detects the facts of the repo at `root`. A Python project with no
/// environment of its own uses the one named by `VIRTUAL_ENV` (ADR 0022).
pub fn detect_facts(root: &Path) -> Result<RepoFacts> {
    let active = std::env::var_os("VIRTUAL_ENV")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    detect_facts_with(root, active.as_deref())
}

/// `detect_facts` with the active virtual environment given, not read from
/// the process environment.
pub fn detect_facts_with(root: &Path, active: Option<&Path>) -> Result<RepoFacts> {
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
    // No guessed typecheck at the root: a root tsconfig is often only a base
    // for the packages below it, and `tsc --noEmit` fails on one.
    let root_tool_commands = Commands {
        typecheck: None,
        ..root_tool_commands
    };

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

    // Scripts only: rules compare these with what the repo documents. A
    // script that watches for changes gives way to its run-once sibling, as
    // in a project, so verify never runs a command that does not exit.
    let (commands, _) = script_commands(
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
    if is_python_project(root, ".", &root_names) {
        raw.push(Raw {
            name: python_name(root, "."),
            root: ".".to_owned(),
            kind: "python",
            source: source::ROOT,
            deps: Vec::new(),
            detail: python_detail(root, ".", active),
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
    let discovered = discovered_projects(
        root,
        &raw,
        node_workspace,
        cargo_workspace,
        active,
        &mut warnings,
    );
    raw.extend(discovered);
    // The root's scripts once, then every other Node project's. The root's
    // own project, if any, has the same scripts.
    let mut notes = Vec::new();
    watch_messages(".", &root_node.watching, &mut notes, &mut warnings);
    for project in raw
        .iter()
        .filter(|project| project.kind == "node" && project.root != ".")
    {
        watch_messages(
            &project.root,
            &project.detail.watching,
            &mut notes,
            &mut warnings,
        );
    }
    notes.sort();
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
        notes,
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
        ("install", &commands.install),
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
        stack.extend(
            project
                .package_manager
                .iter()
                .filter(|manager| !project.languages.contains(manager))
                .cloned(),
        );
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
        if let Some(interpreter) = &project.interpreter {
            out.push_str(&format!(
                "    interpreter: {} ({})\n",
                interpreter.command.trim_end_matches(" -m"),
                interpreter.detail
            ));
        }
        if !project.depends_on.is_empty() {
            out.push_str(&format!(
                "    depends on: {}\n",
                project.depends_on.join(", ")
            ));
        }
    }
    if !facts.notes.is_empty() {
        out.push_str("Notes:\n");
        for note in &facts.notes {
            out.push_str(&format!("  {note}\n"));
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
        detect_facts_with(fixture.root(), None).unwrap()
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
    fn a_nameless_app_at_the_root_is_still_a_project() {
        // Found on vercel/commerce: a private Next.js app with no "name".
        let facts = facts(&[(
            "package.json",
            r#"{"private":true,"scripts":{"build":"next build"},"dependencies":{"next":"15"}}"#,
        )]);
        assert_eq!(facts.projects.len(), 1);
        assert_eq!(facts.projects[0].root, ".");
        assert_eq!(facts.projects[0].kind, "node");
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
    fn a_member_of_a_uv_workspace_runs_with_uv() {
        // Found on fastapi/full-stack-fastapi-template: uv.lock and the
        // workspace are at the root, the backend is a member.
        let fixture = Fixture::new(
            &[
                (
                    "pyproject.toml",
                    "[tool.uv.workspace]\nmembers = [\"backend\"]\n",
                ),
                ("uv.lock", ""),
                (
                    "backend/pyproject.toml",
                    "[project]\nname = \"app\"\ndependencies = [\"pytest\"]\n",
                ),
                ("backend/app/main.py", ""),
            ],
            Track::NoGit,
        )
        .unwrap();
        let facts = detect_facts_with(fixture.root(), None).unwrap();
        let backend = facts
            .projects
            .iter()
            .find(|project| project.root == "backend")
            .expect("backend");
        assert_eq!(backend.package_manager.as_deref(), Some("uv"));
        assert_eq!(
            backend.interpreter.as_ref().map(|i| i.source.as_str()),
            Some("uv")
        );
    }

    #[test]
    fn a_course_project_reports_its_services_and_its_frontend_as_projects() {
        let fixture = crate::testing::course_project().unwrap();
        let facts = detect_facts_with(fixture.root(), None).unwrap();

        // The repo-level summary lists what every project uses.
        assert_eq!(facts.package_manager, None);
        assert_eq!(facts.package_managers, ["npm", "pip"]);
        assert_eq!(facts.languages, ["javascript", "python", "typescript"]);
        assert_eq!(facts.tools, ["docker-compose", "github-actions"]);
        assert_eq!(facts.commands, Commands::default());
        assert!(facts.warnings.is_empty(), "{:?}", facts.warnings);
        // The frontend's test script watches; its test:run sibling runs once.
        assert_eq!(
            facts.notes,
            ["frontend: the test script runs `vitest`, which watches for changes; verify uses test:run (`vitest run`) instead."]
        );

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
        // tsconfig give the commands.
        assert_eq!(frontend.commands.lint.as_deref(), Some("npx --no oxlint"));
        assert_eq!(
            frontend.commands.typecheck.as_deref(),
            Some("npx --no tsc --noEmit")
        );
        // `"test": "vitest"` watches, so its run-once sibling is used.
        assert_eq!(frontend.commands.test.as_deref(), Some("npm run test:run"));

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
        assert!(text.contains("    test: npm run test:run\n"), "{text}");
        assert!(
            text.ends_with(&format!("Notes:\n  {}\n", facts.notes[0])),
            "{text}"
        );
    }

    #[test]
    fn a_folder_without_a_marker_is_not_a_project() {
        let fixture = crate::testing::course_project().unwrap();
        let facts = detect_facts_with(fixture.root(), None).unwrap();
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
            // Tool config only: not a project, and the search goes on below it.
            ("pyproject.toml", "[tool.ruff]\nline-length = 100\n"),
            ("services/pyproject.toml", "[tool.mypy]\nstrict = true\n"),
            ("services/a/requirements.txt", "pytest\n"),
            ("services/a/main.py", ""),
        ]);
        let roots: Vec<_> = facts.projects.iter().map(|p| p.root.as_str()).collect();
        assert_eq!(roots, ["services/a"]);
        let a = by_root(&facts, "services/a");
        assert_eq!(a.tools, ["mypy", "pytest", "ruff"]);
        assert_eq!(a.commands.lint.as_deref(), Some("python -m ruff check ."));
        assert_eq!(a.commands.typecheck.as_deref(), Some("python -m mypy ."));
    }

    #[test]
    fn folders_of_examples_and_fixtures_are_not_searched() {
        let facts = facts(&[
            ("package.json", r#"{"name":"lib","scripts":{"test":"x"}}"#),
            ("examples/basic/package.json", r#"{"name":"basic"}"#),
            ("test/fixtures/broken/package.json", "{ nope"),
            (
                "src/__fixtures__/x/pyproject.toml",
                "[project]\nname = \"x\"\n",
            ),
            ("templates/app/go.mod", "module example.com/app\n"),
            ("site/package.json", r#"{"name":"site"}"#),
        ]);
        let found: Vec<_> = facts.projects.iter().map(|p| p.root.as_str()).collect();
        assert_eq!(found, [".", "site"]);
        assert!(facts.warnings.is_empty(), "{:?}", facts.warnings);
    }

    #[test]
    fn a_project_whose_manifest_cannot_be_read_is_still_listed() {
        let facts = facts(&[
            ("a/package.json", r#"{"name":"a","scripts":{"test":"x"}}"#),
            (
                "b/package.json",
                r#"{"name":"b","scripts":{"test":"exit 1"},}"#,
            ),
        ]);
        let found: Vec<_> = facts
            .projects
            .iter()
            .map(|p| (p.name.as_str(), p.root.as_str()))
            .collect();
        assert_eq!(found, [("a", "a"), ("b", "b")]);
        // No commands can be read from it, so verify will skip and name its steps.
        assert_eq!(by_root(&facts, "b").commands, Commands::default());
        assert_eq!(facts.warnings.len(), 1);
        assert!(facts.warnings[0].starts_with("b/package.json is not valid JSON"));
    }

    #[test]
    fn a_linter_named_only_as_a_dependency_gives_no_command() {
        let facts = facts(&[
            (
                "web/package.json",
                r#"{"devDependencies":{"eslint":"9","vitest":"3"}}"#,
            ),
            ("web/tsconfig.json", "{}"),
            ("package.json", r#"{"name":"root"}"#),
            ("tsconfig.json", "{}"),
            ("eslint.config.js", ""),
        ]);
        let web = by_root(&facts, "web");
        assert_eq!(web.tools, ["eslint", "vitest"]);
        assert_eq!(web.commands.lint, None);
        // A test runner works without a config file.
        assert_eq!(web.commands.test.as_deref(), Some("npx --no vitest run"));
        // At the root, a linter's config file gives a command, but a
        // tsconfig does not: it is often only a base for the packages below.
        assert_eq!(
            facts.root_tool_commands.lint.as_deref(),
            Some("npx --no eslint .")
        );
        assert_eq!(facts.root_tool_commands.typecheck, None);
    }

    // ------------------------------------------- watch-mode scripts

    #[test]
    fn watch_commands_are_detected() {
        for (script, tool) in [
            ("vitest", "vitest"),
            ("vitest watch", "vitest"),
            ("vitest dev", "vitest"),
            ("vitest --coverage", "vitest"),
            ("vitest src/login", "vitest"),
            ("vitest --config vitest.config.ts", "vitest"),
            ("NODE_ENV=test vitest", "vitest"),
            ("npx vitest", "vitest"),
            ("npx --no vitest", "vitest"),
            ("pnpm exec vitest", "vitest"),
            ("yarn vitest", "vitest"),
            ("yarn exec vitest", "vitest"),
            ("bunx vitest", "vitest"),
            ("jest --watch", "jest"),
            ("jest --watchAll", "jest"),
            ("npx jest --watchAll --coverage", "jest"),
            ("react-scripts test", "react-scripts"),
            ("react-scripts test --env=jsdom", "react-scripts"),
            ("ng test", "ng"),
            ("ng t --code-coverage", "ng"),
            ("mocha --watch", "mocha"),
            ("mocha -w test/*.js", "mocha"),
            ("ava --watch", "ava"),
            ("ava -w", "ava"),
            ("node --test --watch", "node"),
            ("node --watch --test test/", "node"),
            ("tsc --watch", "tsc"),
            ("tsc -w --noEmit", "tsc"),
            ("tsc --noEmit --watch", "tsc"),
            ("vue-cli-service test:unit --watch", "vue-cli-service"),
            ("  vitest  ", "vitest"),
            ("tsc --noEmit && vitest", "vitest"),
            ("prisma generate; jest --watch", "jest"),
            ("tsc -b || mocha -w", "mocha"),
        ] {
            assert_eq!(watch_tool(script), Some(tool), "{script}");
        }
    }

    #[test]
    fn run_once_commands_are_not_watch_commands() {
        for script in [
            "vitest run",
            "vitest --run",
            "vitest run --coverage",
            "vitest --config vitest.config.ts run",
            "vitest --watch=false",
            "vitest --no-watch",
            "vitest related src/a.ts --run",
            "vitest bench",
            "vitest list",
            "npx vitest run",
            "pnpm exec vitest run",
            "jest",
            "jest --ci",
            "jest --coverage",
            "jest --watchAll=false",
            "react-scripts test --watchAll=false",
            "react-scripts build",
            "ng test --watch=false",
            "ng test --no-watch",
            "ng build",
            "mocha",
            "ava",
            "node --test",
            "node --watch server.js",
            "tsc",
            "tsc --noEmit",
            "tsc -b",
            "vue-cli-service test:unit",
            "vite",
            "eslint .",
            "playwright test",
            "echo vitest",
            "tsc --noEmit && vitest run",
            "vitest run | tee out.txt",
            "eslint . && tsc --noEmit",
            "",
            "npx",
        ] {
            assert_eq!(watch_tool(script), None, "{script}");
        }
    }

    fn web_project(scripts: &str) -> (RepoFacts, Project) {
        let manifest = format!(r#"{{"name":"web","scripts":{scripts}}}"#);
        let facts = facts(&[("web/package.json", manifest.as_str())]);
        let web = by_root(&facts, "web").clone();
        (facts, web)
    }

    #[test]
    fn a_watch_script_gives_way_to_its_run_once_sibling() {
        let (facts, web) = web_project(r#"{"test":"vitest","test:run":"vitest run"}"#);
        assert_eq!(web.commands.test.as_deref(), Some("npm run test:run"));
        assert_eq!(
            facts.notes,
            ["web: the test script runs `vitest`, which watches for changes; verify uses test:run (`vitest run`) instead."]
        );
        assert!(facts.warnings.is_empty(), "{:?}", facts.warnings);
    }

    #[test]
    fn run_once_siblings_are_tried_in_order() {
        let (_, web) = web_project(
            r#"{"test":"vitest","test:once":"vitest run","test:ci":"vitest run --reporter=junit","test:run":"vitest run --coverage"}"#,
        );
        assert_eq!(web.commands.test.as_deref(), Some("npm run test:run"));
        let (_, web) =
            web_project(r#"{"test":"vitest","test:once":"vitest run","test:ci":"vitest --run"}"#);
        assert_eq!(web.commands.test.as_deref(), Some("npm run test:ci"));
        let (_, web) = web_project(r#"{"test":"vitest","test:once":"vitest run"}"#);
        assert_eq!(web.commands.test.as_deref(), Some("npm run test:once"));
    }

    #[test]
    fn a_sibling_that_watches_too_is_not_used() {
        let (facts, web) =
            web_project(r#"{"test":"vitest","test:run":"vitest watch","test:ci":"vitest run"}"#);
        assert_eq!(web.commands.test.as_deref(), Some("npm run test:ci"));
        assert_eq!(facts.notes.len(), 1);
        assert!(facts.notes[0].contains("verify uses test:ci (`vitest run`)"));
        // Only watching siblings: the step keeps its own script.
        let (facts, web) = web_project(r#"{"test":"vitest","test:run":"vitest dev"}"#);
        assert_eq!(web.commands.test.as_deref(), Some("npm run test"));
        assert!(facts.notes.is_empty());
        assert_eq!(facts.warnings.len(), 1);
    }

    #[test]
    fn a_watch_script_with_no_sibling_is_kept_with_a_warning() {
        let (facts, web) = web_project(r#"{"test":"vitest"}"#);
        assert_eq!(web.commands.test.as_deref(), Some("npm run test"));
        assert!(facts.notes.is_empty());
        assert_eq!(
            facts.warnings,
            ["web: the test script runs `vitest`, which watches for changes and may never exit. verify runs it with CI=1, which turns watch mode off for most tools (vitest, react-scripts); add a test:run script that runs once to be sure."]
        );
        // A tool that keeps watching with CI set is named as such.
        let (facts, web) = web_project(r#"{"typecheck":"tsc --noEmit --watch"}"#);
        assert_eq!(web.commands.typecheck.as_deref(), Some("npm run typecheck"));
        assert_eq!(
            facts.warnings,
            ["web: the typecheck script runs `tsc --noEmit --watch`, which watches for changes and may never exit: CI=1, which verify sets, does not turn off watch mode for tsc. Add a typecheck:run script that runs once."]
        );
    }

    #[test]
    fn lint_and_typecheck_scripts_get_the_same_treatment() {
        let (facts, web) = web_project(
            r#"{"lint":"npx eslint . && tsc -w","typecheck":"tsc --watch","typecheck:ci":"tsc --noEmit","build":"tsc --watch","build:run":"tsc"}"#,
        );
        // Every command of a script is read.
        assert_eq!(web.commands.lint.as_deref(), Some("npm run lint"));
        assert_eq!(facts.warnings.len(), 1);
        assert!(facts.warnings[0].starts_with(
            "web: the lint script runs `npx eslint . && tsc -w`, which watches for changes and may never exit: CI=1, which verify sets, does not turn off watch mode for tsc."
        ));
        assert_eq!(
            web.commands.typecheck.as_deref(),
            Some("npm run typecheck:ci")
        );
        // build is not a step verify runs.
        assert_eq!(web.commands.build.as_deref(), Some("npm run build"));
        assert_eq!(
            facts.notes,
            ["web: the typecheck script runs `tsc --watch`, which watches for changes; verify uses typecheck:ci (`tsc --noEmit`) instead."]
        );
    }

    #[test]
    fn the_sibling_runs_with_the_projects_package_manager() {
        for (lockfile, manager) in [
            ("pnpm-lock.yaml", "pnpm"),
            ("yarn.lock", "yarn"),
            ("bun.lock", "bun"),
            ("package-lock.json", "npm"),
        ] {
            let lock = format!("web/{lockfile}");
            let facts = facts(&[
                (
                    "web/package.json",
                    r#"{"scripts":{"test":"pnpm exec vitest","test:run":"pnpm exec vitest run"}}"#,
                ),
                (lock.as_str(), ""),
            ]);
            let web = by_root(&facts, "web");
            assert_eq!(
                web.commands.test,
                Some(format!("{manager} run test:run")),
                "{lockfile}"
            );
        }
    }

    #[test]
    fn the_root_commands_use_the_sibling_too_and_are_named_once() {
        let facts = facts(&[
            (
                "package.json",
                r#"{"name":"app","scripts":{"test":"jest --watch","test:ci":"jest --ci"}}"#,
            ),
            ("pnpm-lock.yaml", ""),
        ]);
        // verify's root runner reads `commands`, so it must not run the
        // watching script either.
        assert_eq!(facts.commands.test.as_deref(), Some("pnpm run test:ci"));
        assert_eq!(
            by_root(&facts, ".").commands.test.as_deref(),
            Some("pnpm run test:ci")
        );
        assert_eq!(
            facts.notes,
            ["repo root: the test script runs `jest --watch`, which watches for changes; verify uses test:ci (`jest --ci`) instead."]
        );
        let text = format_facts_human(&facts);
        assert!(
            text.contains("Commands at the repo root:\n  test: pnpm run test:ci\n"),
            "{text}"
        );
        assert!(
            text.ends_with(&format!("Notes:\n  {}\n", facts.notes[0])),
            "{text}"
        );
        let json = format_facts_json(&facts);
        assert!(
            json.contains("\n  \"notes\": [\n    \"repo root: "),
            "{json}"
        );
    }

    #[test]
    fn workspace_members_with_watch_scripts_are_named_by_folder() {
        let facts = facts(&[
            (
                "package.json",
                r#"{"name":"root","workspaces":["packages/*"]}"#,
            ),
            (
                "packages/b/package.json",
                r#"{"name":"b","scripts":{"test":"vitest"}}"#,
            ),
            (
                "packages/a/package.json",
                r#"{"name":"a","scripts":{"test":"vitest","test:run":"vitest run"}}"#,
            ),
        ]);
        assert_eq!(
            by_root(&facts, "packages/a").commands.test.as_deref(),
            Some("npm run test:run")
        );
        assert_eq!(facts.notes.len(), 1);
        assert!(facts.notes[0].starts_with("packages/a: the test script"));
        assert_eq!(facts.warnings.len(), 1);
        assert!(facts.warnings[0].starts_with("packages/b: the test script"));
        // No watching scripts: empty lists, still in the JSON.
        let plain = self::facts(&[("package.json", r#"{"scripts":{"test":"vitest run"}}"#)]);
        assert!(plain.notes.is_empty() && plain.warnings.is_empty());
        assert!(format_facts_json(&plain).contains("\"notes\": []"));
        assert!(!format_facts_human(&plain).contains("Notes:"));
    }

    // ------------------------------------------- python dependency lists

    fn ends_with(command: Option<&String>, suffix: &str) -> bool {
        command.is_some_and(|command| command.ends_with(suffix))
    }

    #[test]
    fn requirement_names_are_cut_at_the_version_extras_or_marker() {
        for (spec, name) in [
            ("ruff>=0.4", Some("ruff")),
            ("ruff[extra]", Some("ruff")),
            ("ruff ; python_version>\"3.8\"", Some("ruff")),
            ("Ruff==1", Some("ruff")),
            ("ruff @ https://example.com/ruff.whl", Some("ruff")),
            ("Ruff_LSP", Some("ruff-lsp")),
            ("git+https://github.com/astral-sh/ruff#egg=ruff", None),
            ("https://example.com/ruff.whl", None),
            ("file:ruff", None),
            ("./ruff", None),
            ("", None),
        ] {
            assert_eq!(requirement_name(spec).as_deref(), name, "{spec}");
        }
    }

    #[test]
    fn requirement_includes_are_resolved_inside_the_repo_only() {
        assert_eq!(
            requirement_include("-r dev.txt").as_deref(),
            Some("dev.txt")
        );
        assert_eq!(requirement_include("-rdev.txt").as_deref(), Some("dev.txt"));
        assert_eq!(
            requirement_include("--requirement dev.txt").as_deref(),
            Some("dev.txt")
        );
        assert_eq!(
            requirement_include("--requirement=dev.txt").as_deref(),
            Some("dev.txt")
        );
        for line in ["-c constraints.txt", "-e .", "--index-url https://x", "-r"] {
            assert_eq!(requirement_include(line), None, "{line}");
        }
        assert_eq!(
            resolve_include("app/requirements.txt", "../shared/dev.txt").as_deref(),
            Some("shared/dev.txt")
        );
        assert_eq!(
            resolve_include("requirements.txt", "./requirements\\dev.txt").as_deref(),
            Some("requirements/dev.txt")
        );
        for target in ["../dev.txt", "/etc/dev.txt", "\\dev.txt", "C:\\dev.txt"] {
            assert_eq!(
                resolve_include("requirements.txt", target),
                None,
                "{target}"
            );
        }
    }

    #[test]
    fn ruff_and_mypy_in_dev_requirements_files_count() {
        let facts = facts(&[
            ("app/requirements.txt", "flask\n"),
            ("app/dev-requirements.txt", "Ruff==0.4.4\n"),
            ("app/test_requirements.txt", "mypy>=1.10\n"),
            ("app/main.py", ""),
        ]);
        let app = by_root(&facts, "app");
        assert_eq!(app.tools, ["mypy", "ruff"]);
        assert!(ends_with(app.commands.lint.as_ref(), "ruff check ."));
        assert!(ends_with(app.commands.typecheck.as_ref(), "mypy ."));
    }

    #[test]
    fn ruff_in_a_requirements_folder_counts() {
        let facts = facts(&[
            ("app/requirements.txt", "flask\n"),
            (
                "app/requirements/dev.txt",
                "ruff>=0.4 ; python_version > \"3.8\"\n",
            ),
            ("app/main.py", ""),
        ]);
        let app = by_root(&facts, "app");
        assert_eq!(app.tools, ["ruff"]);
        assert!(ends_with(app.commands.lint.as_ref(), "ruff check ."));
        assert_eq!(app.commands.typecheck, None);
    }

    #[test]
    fn a_requirements_include_inside_the_repo_is_followed() {
        let facts = facts(&[
            (
                "services/api/requirements.txt",
                "flask\n-r ../shared/requirements-dev.txt\n",
            ),
            ("services/api/main.py", ""),
            (
                "services/shared/requirements-dev.txt",
                "--requirement=lint.txt\n",
            ),
            ("services/shared/lint.txt", "ruff[toml]\r\n"),
        ]);
        let roots: Vec<_> = facts.projects.iter().map(|p| p.root.as_str()).collect();
        assert_eq!(roots, ["services/api"]);
        let api = by_root(&facts, "services/api");
        assert_eq!(api.tools, ["ruff"]);
        assert!(ends_with(api.commands.lint.as_ref(), "ruff check ."));
    }

    #[test]
    fn ruff_and_mypy_in_pyproject_optional_dependencies_count() {
        let facts = facts(&[(
            "app/pyproject.toml",
            "[project]\nname = \"app\"\ndependencies = [\"fastapi\"]\n\n\
             [project.optional-dependencies]\ndev = [\n  \"ruff>=0.4\",\n  \"mypy\",  # types\n]\n",
        )]);
        let app = by_root(&facts, "app");
        assert_eq!(app.tools, ["mypy", "ruff"]);
        assert!(ends_with(app.commands.lint.as_ref(), "ruff check ."));
        assert!(ends_with(app.commands.typecheck.as_ref(), "mypy ."));
    }

    #[test]
    fn ruff_in_pyproject_dependency_groups_counts() {
        let facts = facts(&[(
            "app/pyproject.toml",
            "[project]\nname = \"app\"\n\n[dependency-groups]\n\
             lint = [\"ruff\"]\nmypy = [\"types-requests\"]\n\
             dev = [{ include-group = \"lint\" }, { include-group = \"mypy\" }, \"pytest\"]\n",
        )]);
        let app = by_root(&facts, "app");
        // The group named `mypy` is not the package.
        assert_eq!(app.tools, ["pytest", "ruff"]);
        assert!(ends_with(app.commands.lint.as_ref(), "ruff check ."));
        assert_eq!(app.commands.typecheck, None);
    }

    #[test]
    fn mypy_in_the_dev_dependencies_of_uv_and_pdm_counts() {
        for section in [
            "[tool.uv]\ndev-dependencies = [\"mypy>=1\"]\n",
            "[tool.pdm.dev-dependencies]\ntyping = [\"mypy>=1\"]\n",
        ] {
            let pyproject = format!("[project]\nname = \"app\"\n\n{section}");
            let facts = facts(&[("app/pyproject.toml", pyproject.as_str())]);
            let app = by_root(&facts, "app");
            assert_eq!(app.tools, ["mypy"], "{section}");
            assert!(ends_with(app.commands.typecheck.as_ref(), "mypy ."));
            assert_eq!(app.commands.lint, None);
        }
    }

    #[test]
    fn ruff_and_mypy_in_poetry_dependency_groups_count() {
        let facts = facts(&[(
            "app/pyproject.toml",
            "[tool.poetry]\nname = \"app\"\n\n[tool.poetry.dependencies]\npython = \"^3.11\"\n\n\
             [tool.poetry.group.dev.dependencies]\nruff = \"^0.4\"\n\
             MyPy = { version = \"^1.10\", optional = true }\n",
        )]);
        let app = by_root(&facts, "app");
        assert_eq!(app.tools, ["mypy", "ruff"]);
        assert!(ends_with(app.commands.lint.as_ref(), "ruff check ."));
        assert!(ends_with(app.commands.typecheck.as_ref(), "mypy ."));
    }

    #[test]
    fn ruff_in_the_dev_packages_of_a_pipfile_counts() {
        let facts = facts(&[(
            "app/Pipfile",
            "[packages]\nflask = \"*\"\n\n[dev-packages]\nruff = \"*\"\n\"pytest\" = \"*\"\n",
        )]);
        let app = by_root(&facts, "app");
        assert_eq!(app.tools, ["pytest", "ruff"]);
        assert!(ends_with(app.commands.lint.as_ref(), "ruff check ."));
        assert_eq!(app.commands.typecheck, None);
    }

    #[test]
    fn names_that_only_look_like_ruff_do_not_count() {
        let facts = facts(&[
            (
                "app/requirements.txt",
                "ruff-lsp\nruffus==2\nmypy-extensions\n# ruff\nflask  # ruff is set up elsewhere\n\
                 git+https://github.com/astral-sh/ruff#egg=ruff\n-e ./ruff\n\
                 -c ruff-constraints.txt\n--index-url https://example.com/simple\n",
            ),
            ("app/ruff-constraints.txt", "ruff==0.4\n"),
            (
                "app/pyproject.toml",
                "[project]\nname = \"app\"\ndependencies = [\n  \"flask\",\n  # \"ruff\",\n]\n",
            ),
        ]);
        let app = by_root(&facts, "app");
        assert!(app.tools.is_empty(), "{:?}", app.tools);
        assert_eq!(app.commands.lint, None);
        assert_eq!(app.commands.typecheck, None);
    }

    #[test]
    fn a_requirements_file_above_the_project_does_not_count() {
        let facts = facts(&[
            ("requirements.txt", "ruff\n"),
            ("requirements-dev.txt", "ruff\nmypy\n"),
            ("app/requirements.txt", "flask\n"),
            ("app/main.py", ""),
        ]);
        let roots: Vec<_> = facts.projects.iter().map(|p| p.root.as_str()).collect();
        assert_eq!(roots, ["app"]);
        let app = by_root(&facts, "app");
        assert!(app.tools.is_empty(), "{:?}", app.tools);
        assert_eq!(app.commands.lint, None);
        assert_eq!(app.commands.typecheck, None);
    }

    #[test]
    fn a_requirements_include_that_leaves_the_repo_is_ignored() {
        let fixture = Fixture::new(
            &[
                ("app/main.py", ""),
                ("shared/requirements-lint.txt", "ruff\n"),
            ],
            Track::NoGit,
        )
        .unwrap();
        // Absolute, even when it points into the repo: not followed.
        let absolute = fixture
            .root()
            .join("shared")
            .join("requirements-lint.txt")
            .display()
            .to_string();
        fixture
            .write(
                "app/requirements.txt",
                &format!("flask\n-r ../../../outside/requirements.txt\n-r {absolute}\n"),
            )
            .unwrap();
        let facts = detect_facts_with(fixture.root(), None).unwrap();
        assert!(facts.warnings.is_empty(), "{:?}", facts.warnings);
        let app = by_root(&facts, "app");
        assert!(app.tools.is_empty(), "{:?}", app.tools);
        assert_eq!(app.commands.lint, None);
    }

    #[test]
    fn a_requirements_include_cycle_ends() {
        let facts = facts(&[
            ("app/requirements.txt", "-r deps/base.txt\nflask\n"),
            (
                "app/deps/base.txt",
                "-r base.txt\n-r ../requirements.txt\nmypy\n",
            ),
            ("app/main.py", ""),
        ]);
        let app = by_root(&facts, "app");
        assert_eq!(app.tools, ["mypy"]);
        assert!(ends_with(app.commands.typecheck.as_ref(), "mypy ."));
        assert_eq!(app.commands.lint, None);
    }

    #[test]
    fn requirement_includes_stop_at_the_depth_limit() {
        let facts = facts(&[
            ("app/requirements.txt", "-r deps/1.txt\n"),
            ("app/deps/1.txt", "-r 2.txt\n"),
            ("app/deps/2.txt", "-r 3.txt\n"),
            ("app/deps/3.txt", "-r 4.txt\n"),
            ("app/deps/4.txt", "-r 5.txt\n"),
            ("app/deps/5.txt", "mypy\n-r 6.txt\n"),
            ("app/deps/6.txt", "ruff\n"),
            ("app/main.py", ""),
        ]);
        let app = by_root(&facts, "app");
        assert_eq!(app.tools, ["mypy"]);
    }

    #[test]
    fn a_python_project_without_dependency_lists_gets_no_lint_command() {
        let facts = facts(&[("app/setup.py", ""), ("app/main.py", "")]);
        let app = by_root(&facts, "app");
        assert!(app.tools.is_empty(), "{:?}", app.tools);
        assert_eq!(app.commands.lint, None);
        assert_eq!(app.commands.typecheck, None);
    }

    // ------------------------------------------- Python interpreters (ADR 0022)

    /// The interpreter a venv folder holds on this platform.
    fn venv_file(venv: &str) -> String {
        if cfg!(windows) {
            format!("{venv}/Scripts/python.exe")
        } else {
            format!("{venv}/bin/python")
        }
    }

    /// A venv interpreter file for the other platform: never used here.
    fn foreign_venv_file(venv: &str) -> String {
        if cfg!(windows) {
            format!("{venv}/bin/python")
        } else {
            format!("{venv}/Scripts/python.exe")
        }
    }

    fn service(extra: &[(&str, &str)]) -> Vec<(String, String)> {
        let mut files = vec![
            (
                "svc/requirements-dev.txt".to_owned(),
                "pytest\nruff\n".to_owned(),
            ),
            ("svc/requirements.txt".to_owned(), "pyjwt\n".to_owned()),
            ("svc/main.py".to_owned(), String::new()),
        ];
        files.extend(
            extra
                .iter()
                .map(|(file, content)| ((*file).to_owned(), (*content).to_owned())),
        );
        files
    }

    fn svc_with(extra: &[(&str, &str)], active: Option<&Path>) -> (Fixture, Project) {
        let files = service(extra);
        let borrowed: Vec<(&str, &str)> = files
            .iter()
            .map(|(file, content)| (file.as_str(), content.as_str()))
            .collect();
        let fixture = Fixture::new(&borrowed, Track::NoGit).unwrap();
        let facts = detect_facts_with(fixture.root(), active).unwrap();
        let project = by_root(&facts, "svc").clone();
        (fixture, project)
    }

    #[test]
    fn a_venv_in_the_project_folder_runs_its_steps() {
        let file = format!("svc/{}", venv_file(".venv"));
        let (_f, svc) = svc_with(&[(&file, "")], None);
        let python = venv_python(".venv");
        let interpreter = svc.interpreter.unwrap();
        assert_eq!(interpreter.source, "venv");
        assert_eq!(interpreter.command, format!("{python} -m"));
        assert_eq!(svc.commands.test, Some(format!("{python} -m pytest")));
        assert_eq!(svc.commands.lint, Some(format!("{python} -m ruff check .")));
        // requirements-dev.txt does not include requirements.txt, so both
        // are installed: the app's own dependencies (pyjwt) are needed too.
        assert_eq!(
            svc.commands.install,
            Some(format!(
                "{python} -m pip install -r requirements.txt -r requirements-dev.txt"
            ))
        );
    }

    #[cfg(windows)]
    #[test]
    fn on_windows_the_venv_interpreter_is_scripts_python_exe() {
        let (_f, svc) = svc_with(&[("svc/.venv/Scripts/python.exe", "")], None);
        assert_eq!(
            svc.commands.test.as_deref(),
            Some(".venv\\Scripts\\python.exe -m pytest")
        );
        // A venv made on Linux or macOS has no Scripts\python.exe.
        let (_f, svc) = svc_with(&[("svc/.venv/bin/python", "")], None);
        assert_eq!(svc.commands.test.as_deref(), Some("python -m pytest"));
        assert!(svc
            .interpreter
            .unwrap()
            .detail
            .contains(".venv has no Scripts/python.exe"));
    }

    #[cfg(not(windows))]
    #[test]
    fn elsewhere_the_venv_interpreter_is_bin_python() {
        let (_f, svc) = svc_with(&[("svc/.venv/bin/python", "")], None);
        assert_eq!(
            svc.commands.test.as_deref(),
            Some(".venv/bin/python -m pytest")
        );
        // A venv made on Windows has no bin/python.
        let (_f, svc) = svc_with(&[("svc/.venv/Scripts/python.exe", "")], None);
        assert_eq!(svc.commands.test.as_deref(), Some("python -m pytest"));
        assert!(svc
            .interpreter
            .unwrap()
            .detail
            .contains(".venv has no bin/python"));
    }

    #[test]
    fn a_venv_folder_named_venv_counts_and_dot_venv_comes_first() {
        let venv = format!("svc/{}", venv_file("venv"));
        let (_f, svc) = svc_with(&[(&venv, "")], None);
        assert_eq!(
            svc.commands.test,
            Some(format!("{} -m pytest", venv_python("venv")))
        );
        let dot = format!("svc/{}", venv_file(".venv"));
        let (_f, svc) = svc_with(&[(&venv, ""), (&dot, "")], None);
        assert_eq!(
            svc.commands.test,
            Some(format!("{} -m pytest", venv_python(".venv")))
        );
    }

    #[test]
    fn a_venv_without_an_interpreter_for_this_platform_is_not_used() {
        let foreign = format!("svc/{}", foreign_venv_file(".venv"));
        let (_f, svc) = svc_with(&[(&foreign, "")], None);
        let interpreter = svc.interpreter.unwrap();
        assert_eq!(interpreter.source, "path");
        assert_eq!(svc.commands.test.as_deref(), Some("python -m pytest"));
    }

    #[test]
    fn uv_and_poetry_projects_use_their_manager_unless_the_folder_has_a_venv() {
        let (_f, svc) = svc_with(&[("svc/uv.lock", "")], None);
        assert_eq!(svc.interpreter.as_ref().unwrap().source, "uv");
        assert_eq!(svc.commands.test.as_deref(), Some("uv run pytest"));
        assert_eq!(svc.commands.install.as_deref(), Some("uv sync"));
        let (_f, svc) = svc_with(&[("svc/poetry.lock", "")], None);
        assert_eq!(svc.interpreter.as_ref().unwrap().source, "poetry");
        assert_eq!(svc.commands.test.as_deref(), Some("poetry run pytest"));
        assert_eq!(svc.commands.install.as_deref(), Some("poetry install"));
        // The project's own .venv wins: it is what uv made, and running it
        // directly never installs anything.
        let dot = format!("svc/{}", venv_file(".venv"));
        let (_f, svc) = svc_with(&[("svc/uv.lock", ""), (&dot, "")], None);
        assert_eq!(svc.interpreter.as_ref().unwrap().source, "venv");
        // It still installs with its manager: uv's venvs have no pip.
        assert_eq!(svc.commands.install.as_deref(), Some("uv sync"));
        let (_f, svc) = svc_with(&[("svc/poetry.lock", ""), (&dot, "")], None);
        assert_eq!(svc.commands.install.as_deref(), Some("poetry install"));
    }

    #[test]
    fn an_active_virtual_env_is_used_when_the_project_has_none_of_its_own() {
        let outside = Fixture::new(&[(&venv_file("env"), "")], Track::NoGit).unwrap();
        let active = outside.root().join("env");
        let (_f, svc) = svc_with(&[], Some(&active));
        let interpreter = svc.interpreter.unwrap();
        assert_eq!(interpreter.source, "active");
        let expected = shell_quote(
            &venv_python_parts()
                .iter()
                .fold(active.clone(), |path, part| path.join(part))
                .to_string_lossy(),
        );
        assert_eq!(svc.commands.test, Some(format!("{expected} -m pytest")));
        // The project's own venv comes first, and so does a uv project.
        let dot = format!("svc/{}", venv_file(".venv"));
        let (_f, svc) = svc_with(&[(&dot, "")], Some(&active));
        assert_eq!(svc.interpreter.unwrap().source, "venv");
        let (_f, svc) = svc_with(&[("svc/uv.lock", "")], Some(&active));
        assert_eq!(svc.interpreter.unwrap().source, "uv");
        // A VIRTUAL_ENV with no interpreter in it is ignored.
        let empty = outside.root().join("nothing-here");
        let (_f, svc) = svc_with(&[], Some(&empty));
        assert_eq!(svc.interpreter.unwrap().source, "path");
    }

    #[cfg(windows)]
    #[test]
    fn on_windows_an_active_env_whose_path_has_a_percent_sign_is_skipped() {
        // cmd would expand %...% inside the quotes.
        let outside = Fixture::new(&[("100%done/Scripts/python.exe", "")], Track::NoGit).unwrap();
        let active = outside.root().join("100%done");
        let (_f, svc) = svc_with(&[], Some(&active));
        assert_eq!(svc.interpreter.unwrap().source, "path");
    }

    #[test]
    fn with_no_virtual_environment_python_from_the_path_is_used_and_said() {
        let (_f, svc) = svc_with(&[], None);
        let interpreter = svc.interpreter.clone().unwrap();
        assert_eq!(interpreter.source, "path");
        assert_eq!(interpreter.command, "python -m");
        assert!(interpreter
            .detail
            .starts_with("no virtual environment found"));
        // Today's commands, unchanged.
        assert_eq!(svc.commands.test.as_deref(), Some("python -m pytest"));
        assert_eq!(
            svc.commands.install.as_deref(),
            Some("python -m pip install -r requirements.txt -r requirements-dev.txt")
        );
        // A development list that includes requirements.txt is enough alone.
        let (_f, svc) = svc_with(
            &[("svc/requirements-dev.txt", "-r requirements.txt\npytest\n")],
            None,
        );
        assert_eq!(
            svc.commands.install.as_deref(),
            Some("python -m pip install -r requirements-dev.txt")
        );
        let facts = RepoFacts {
            projects: vec![svc],
            ..RepoFacts::default()
        };
        let text = format_facts_human(&facts);
        assert!(
            text.contains("    interpreter: python (no virtual environment found"),
            "{text}"
        );
        let json = format_facts_json(&facts);
        assert!(json.contains("\"source\": \"path\""), "{json}");
    }

    #[test]
    fn shell_quote_leaves_plain_paths_alone() {
        assert_eq!(
            shell_quote("/home/me/env/bin/python"),
            "/home/me/env/bin/python"
        );
        assert_eq!(
            shell_quote("/my envs/a/bin/python"),
            "\"/my envs/a/bin/python\""
        );
        if !cfg!(windows) {
            assert_eq!(shell_quote("/a/$b/python"), "'/a/$b/python'");
            assert_eq!(shell_quote("/a/it's/python"), "\"/a/it's/python\"");
        }
    }
}
