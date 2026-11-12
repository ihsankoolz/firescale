//! `fscale doctor`: is this machine ready to work on the repo? (SPEC §17,
//! ADR 0026)
//!
//! It checks the machine, not the repo's code:
//!
//! - **Tools**: the versions the repo declares (`engines`, `.nvmrc`,
//!   `.node-version`, `packageManager`, `rust-toolchain.toml`,
//!   `.python-version`, `requires-python`, `go.mod`) against what is
//!   installed.
//! - **Environment variables**: every name in each `.env.example` is set in
//!   the environment or present in the local env file beside it. Only names
//!   are read from a local env file; values are never kept or printed.
//! - **Dependencies**: per project, installed and matching the lockfile or
//!   the requirements; for Python, the virtual environment too.
//! - **Services**: the local services that Compose files and `.env.example`
//!   URLs name are reachable. Only host, port and service name are shown.
//!
//! Every problem comes with the command that fixes it. Nothing is installed,
//! started or changed: version commands (`node --version`, ...) are the only
//! programs it runs, and `rustup` is asked which toolchains are installed
//! rather than run in the repo, where it could download one.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use serde::Serialize;
use serde_json::Value;

use crate::facts::{
    parse_toml, read_requirements, requirement_name, venv_python, RepoFacts, VENV_DIRS,
};
use crate::process::{run_tree, Ending, Watch};
use crate::services::{folders_up, listening, services_in, ENV_EXAMPLE, PROBE_TIMEOUT};

/// How long a version command may take.
pub const VERSION_TIMEOUT: Duration = Duration::from_secs(10);

/// Version of the `fscale doctor --json` format.
pub const DOCTOR_SCHEMA_VERSION: u32 = 1;

/// Local env files a `.env.example` is filled into, beside it.
pub const LOCAL_ENV_FILES: [&str; 2] = [".env", ".env.local"];

/// How many names a line lists before "and N more".
const MAX_NAMES: usize = 8;

/// How a check came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Ok,
    /// Something is wrong; `fscale doctor` exits 1.
    Problem,
    /// Worth a look, but not wrong for certain.
    Warning,
}

/// One thing doctor looked at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    /// `tools`, `env`, `dependencies` or `services`.
    pub section: &'static str,
    /// The project's folder, `.` for the root; absent for the whole repo.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// What was checked: `node`, `pnpm`, `backend/.env.example`, ...
    pub subject: String,
    pub status: Status,
    /// One sentence on what was found.
    pub detail: String,
    /// The command (or the step) that fixes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

/// Everything `fscale doctor` reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorReport {
    pub schema_version: u32,
    /// True when there is no problem (warnings are allowed).
    pub ok: bool,
    pub problems: usize,
    pub warnings: usize,
    pub checks: Vec<Finding>,
}

/// What doctor asks the machine. Tests replace it.
pub trait Machine {
    /// What `<program> <args>` prints when run in `dir` (stdout, then
    /// stderr), or `None` when it cannot be run or fails.
    fn run(&self, dir: &Path, program: &str, args: &[&str]) -> Option<String>;
    /// The first line that `run` gives.
    fn version(&self, dir: &Path, program: &str, args: &[&str]) -> Option<String> {
        self.run(dir, program, args)?
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_owned)
    }
    /// True when the variable is set to something in the environment.
    fn env_set(&self, name: &str) -> bool;
    /// True when something listens on `host:port`.
    fn listening(&self, host: &str, port: u16) -> bool;
}

/// The real machine.
pub struct ThisMachine;

impl Machine for ThisMachine {
    fn run(&self, dir: &Path, program: &str, args: &[&str]) -> Option<String> {
        let mut command = if Path::new(program).is_absolute() {
            // An interpreter path is run as it is, with no shell to read
            // `$`, `&` or quotes in it.
            let mut direct = Command::new(program);
            direct.args(args);
            direct
        } else {
            // A tool name goes through the shell, so `pnpm.cmd` and the
            // like are found on Windows.
            let line = std::iter::once(program.to_owned())
                .chain(args.iter().map(|arg| (*arg).to_owned()))
                .collect::<Vec<_>>()
                .join(" ");
            shell(&line)
        };
        command
            .current_dir(dir)
            // Asking for a version must never download anything: not a
            // package manager through corepack, not a Go toolchain that
            // go.mod names.
            .env("COREPACK_ENABLE_NETWORK", "0")
            .env("COREPACK_ENABLE_DOWNLOAD_PROMPT", "0")
            .env("GOTOOLCHAIN", "local");
        let never = AtomicBool::new(false);
        let watch = Watch {
            timeout: Some(VERSION_TIMEOUT),
            stop: &never,
            heartbeat: Duration::ZERO,
            on_heartbeat: &|_, _| {},
            command: "",
            on_output: &|_| false,
            on_spawn: &|_| {},
        };
        // The command runs as a tree with a time limit: a shim that hangs
        // (a version manager waiting on the network) cannot hang doctor.
        let finished = run_tree(command, &watch).ok()?;
        if finished.ending != Ending::Exited(Some(0)) {
            return None;
        }
        Some(format!(
            "{}\n{}",
            String::from_utf8_lossy(&finished.stdout),
            String::from_utf8_lossy(&finished.stderr)
        ))
    }

    fn env_set(&self, name: &str) -> bool {
        std::env::var_os(name).is_some_and(|value| !value.is_empty())
    }

    fn listening(&self, host: &str, port: u16) -> bool {
        listening(host, port, PROBE_TIMEOUT)
    }
}

#[cfg(windows)]
fn shell(command: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut shell = Command::new("cmd");
    shell
        .args(["/D", "/S", "/C"])
        .raw_arg(format!("\"{command}\""));
    shell
}

#[cfg(not(windows))]
fn shell(command: &str) -> Command {
    let mut shell = Command::new("sh");
    shell.args(["-c", command]);
    shell
}

// ------------------------------------------------------------- versions

/// The numbers of a version: `v22.11.0` gives `[22, 11, 0]`, `3.12.1rc1`
/// gives `[3, 12, 1]`. `None` when it does not start with a number.
#[must_use]
pub fn version_numbers(text: &str) -> Option<Vec<u64>> {
    let text = text.trim().trim_start_matches(['v', 'V', '=']);
    let mut numbers = Vec::new();
    for part in text.split('.') {
        let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            break;
        }
        numbers.push(digits.parse().ok()?);
        if digits.len() != part.len() {
            break;
        }
    }
    (!numbers.is_empty()).then_some(numbers)
}

/// The first thing in `line` that looks like a version: `go version
/// go1.22.3 linux/amd64` gives `1.22.3`.
fn find_version(line: &str) -> Option<String> {
    line.split(|c: char| c.is_whitespace() || c == '(' || c == ',')
        .map(|word| word.trim_start_matches(|c: char| c.is_ascii_alphabetic()))
        .find(|word| {
            word.starts_with(|c: char| c.is_ascii_digit())
                && word.contains('.')
                && version_numbers(word).is_some()
        })
        .map(|word| {
            word.trim_end_matches(|c: char| !c.is_ascii_alphanumeric())
                .to_owned()
        })
}

fn compare(a: &[u64], b: &[u64]) -> std::cmp::Ordering {
    let len = a.len().max(b.len());
    for index in 0..len {
        let (x, y) = (
            a.get(index).copied().unwrap_or(0),
            b.get(index).copied().unwrap_or(0),
        );
        if x != y {
            return x.cmp(&y);
        }
    }
    std::cmp::Ordering::Equal
}

/// True when `have` matches a version written with only some parts:
/// `22` matches `22.11.0`, `3.12` matches `3.12.1`.
fn same_prefix(want: &[u64], have: &[u64]) -> bool {
    want.iter().zip(have).all(|(a, b)| a == b) && want.len() <= have.len().max(want.len())
}

/// One comparison of a range: `>=22.18.0`, `^3.1`, `~1.2`, `22.x`, `<4`.
/// Which rules a version range follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    /// npm's semver ranges (`engines`): `<=22` means below 23.
    Npm,
    /// Python's PEP 440 specifiers (`requires-python`): `==3.12` means
    /// exactly 3.12.0, and `.*` is a prefix.
    Pep440,
}

/// The version just above every version that starts with `want`:
/// `22` gives `23`, `3.12` gives `3.13`.
fn next_after(want: &[u64]) -> Vec<u64> {
    let mut next = want.to_vec();
    if let Some(last) = next.last_mut() {
        *last += 1;
    }
    next
}

fn satisfies_one(term: &str, have: &[u64], flavor: Flavor) -> Option<bool> {
    let term = term.trim();
    if term.is_empty() || term == "*" || term == "x" {
        return Some(true);
    }
    let (op, rest) = ["<=", ">=", "==", "!=", "~=", "<", ">", "=", "^", "~"]
        .iter()
        .find_map(|op| term.strip_prefix(op).map(|rest| (*op, rest.trim())))
        .unwrap_or(("", term));
    // `22.x`, `3.*`: a prefix.
    let wildcard = rest.contains(".x") || rest.contains(".*") || rest.ends_with(".X");
    let want = version_numbers(rest)?;
    let ordering = compare(have, &want);
    // npm reads a partial version as a range: `<=22` is `<23`, `>22` is `>=23`.
    let partial = flavor == Flavor::Npm && want.len() < 3 && !wildcard;
    use std::cmp::Ordering::{Equal, Greater, Less};
    Some(match op {
        ">=" => ordering != Less,
        ">" if partial => compare(have, &next_after(&want)) != Less,
        ">" => ordering == Greater,
        "<=" if partial => compare(have, &next_after(&want)) == Less,
        "<=" => ordering != Greater,
        "<" => ordering == Less,
        "!=" if wildcard => !same_prefix(&want, have),
        "!=" => ordering != Equal,
        "==" if flavor == Flavor::Pep440 && !wildcard => ordering == Equal,
        "^" => {
            // The left-most non-zero part stays the same.
            let fixed = want
                .iter()
                .position(|part| *part != 0)
                .unwrap_or(want.len() - 1);
            ordering != Less && have.get(..=fixed) == want.get(..=fixed)
        }
        "~" => {
            ordering != Less
                && have.first() == want.first()
                && (want.len() < 2 || have.get(1) == want.get(1))
        }
        // PEP 440 `~=3.10`: at least 3.10, below 4.
        "~=" => {
            let keep = want.len().saturating_sub(1).max(1);
            ordering != Less && have.get(..keep) == want.get(..keep)
        }
        _ if wildcard => same_prefix(&want, have),
        _ => {
            // `22` or `=22.1`: the parts given must match.
            same_prefix(&want, have)
        }
    })
}

/// True when `have` is in the range. npm ranges (`>=18 <21 || ^22`) and
/// Python specifiers (`>=3.10,<4`) are read; `None` when the range cannot be
/// read.
#[must_use]
pub fn satisfies(range: &str, have: &str) -> Option<bool> {
    satisfies_as(range, have, Flavor::Npm)
}

/// [`satisfies`], with the rules of `flavor`.
#[must_use]
pub fn satisfies_as(range: &str, have: &str, flavor: Flavor) -> Option<bool> {
    let have = version_numbers(have)?;
    let mut any = false;
    for alternative in range.split("||") {
        let alternative = alternative.trim();
        // `1.2 - 2.3`: a hyphen range.
        if let Some((low, high)) = alternative.split_once(" - ") {
            let low = version_numbers(low)?;
            let high = version_numbers(high)?;
            if compare(&have, &low) != std::cmp::Ordering::Less
                && (compare(&have, &high) != std::cmp::Ordering::Greater
                    || same_prefix(&high, &have))
            {
                any = true;
            }
            continue;
        }
        let mut all = true;
        for term in alternative
            .split([',', ' '])
            .filter(|term| !term.is_empty())
        {
            // `>= 22`: the operator and the number may be apart.
            all &= satisfies_one(term, &have, flavor)?;
        }
        any |= all;
    }
    Some(any)
}

/// Joins `>= 22` into `>=22`, so a range can be split on spaces.
fn tighten(range: &str) -> String {
    let mut out = String::new();
    let mut chars = range.chars().peekable();
    while let Some(c) = chars.next() {
        out.push(c);
        if matches!(c, '<' | '>' | '=' | '^' | '~') {
            while chars.peek() == Some(&' ') {
                chars.next();
            }
        }
    }
    out
}

// ---------------------------------------------------------------- files

fn read(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|text| text.trim_start_matches('\u{feff}').to_owned())
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&read(path)?).ok()
}

/// A repo-relative path for people: `backend/.env.example`, `.nvmrc`.
fn shown(folder: &str, file: &str) -> String {
    if folder == "." || folder.is_empty() {
        file.to_owned()
    } else {
        format!("{folder}/{file}")
    }
}

fn dir_of(root: &Path, folder: &str) -> PathBuf {
    folder
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|meta| meta.modified()).ok()
}

/// `a, b, c and 4 more`.
fn list(names: &[String]) -> String {
    if names.len() <= MAX_NAMES {
        return names.join(", ");
    }
    format!(
        "{} and {} more",
        names[..MAX_NAMES].join(", "),
        names.len() - MAX_NAMES
    )
}

/// The names in an env file. Values are dropped as each line is read.
fn env_names(text: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, _)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            names.insert(key.to_owned());
        }
    }
    names
}

// ---------------------------------------------------------------- checks

struct Doctor<'a> {
    root: &'a Path,
    facts: &'a RepoFacts,
    machine: &'a dyn Machine,
    findings: Vec<Finding>,
}

impl Doctor<'_> {
    fn add(
        &mut self,
        section: &'static str,
        project: Option<&str>,
        subject: impl Into<String>,
        status: Status,
        detail: impl Into<String>,
        fix: Option<String>,
    ) {
        self.findings.push(Finding {
            section,
            project: project.map(str::to_owned),
            subject: subject.into(),
            status,
            detail: detail.into(),
            fix,
        });
    }

    /// The folders of every project, root first, without repeats.
    fn folders(&self) -> Vec<String> {
        let mut folders = vec![".".to_owned()];
        for project in &self.facts.projects {
            if !folders.contains(&project.root) {
                folders.push(project.root.clone());
            }
        }
        folders
    }

    fn has_kind(&self, kind: &str) -> bool {
        self.facts
            .projects
            .iter()
            .any(|project| project.kind == kind)
    }

    // --- tools

    fn node(&mut self) {
        if !self.has_kind("node") {
            return;
        }
        // What the repo asks for: engines.node in package.json files, .nvmrc
        // and .node-version, at the root and in each Node project.
        let mut wants: Vec<(String, String)> = Vec::new();
        for project in self.facts.projects.iter().filter(|p| p.kind == "node") {
            let dir = dir_of(self.root, &project.root);
            if let Some(range) = read_json(&dir.join("package.json"))
                .as_ref()
                .and_then(|json| json.pointer("/engines/node"))
                .and_then(Value::as_str)
            {
                wants.push((
                    range.to_owned(),
                    shown(&project.root, "package.json (engines.node)"),
                ));
            }
            for file in [".nvmrc", ".node-version"] {
                if let Some(text) = read(&dir.join(file)) {
                    let first = text.lines().next().unwrap_or("").trim().to_owned();
                    if version_numbers(&first).is_some() {
                        wants.push((first, shown(&project.root, file)));
                    }
                }
            }
        }
        // One line per range, with every file that asks for it.
        let mut grouped: Vec<(String, String)> = Vec::new();
        for (range, source) in wants {
            match grouped.iter_mut().find(|(known, _)| *known == range) {
                Some((_, sources)) => {
                    sources.push_str(", ");
                    sources.push_str(&source);
                }
                None => grouped.push((range, source)),
            }
        }
        let wants = grouped;
        let installed = self
            .machine
            .version(self.root, "node", &["--version"])
            .and_then(|line| {
                find_version(&line).or_else(|| {
                    version_numbers(&line).map(|_| line.trim_start_matches('v').to_owned())
                })
            });
        let Some(have) = installed else {
            let want = wants
                .first()
                .map_or_else(String::new, |(range, _)| format!(" {range}"));
            self.add(
                "tools",
                None,
                "node",
                Status::Problem,
                "Node.js is not installed, or not on the PATH.",
                Some(format!("install Node.js{want} from https://nodejs.org (or with a version manager such as nvm or fnm)")),
            );
            return;
        };
        if wants.is_empty() {
            self.add(
                "tools",
                None,
                "node",
                Status::Ok,
                format!("node {have} (the repo declares no version)"),
                None,
            );
            return;
        }
        for (range, source) in wants {
            let exact = !range.contains(['<', '>', '^', '~', '|', '*', 'x', ' ']);
            match satisfies(&tighten(&range), &have) {
                Some(true) => self.add("tools", None, "node", Status::Ok, format!("node {have} matches {range} ({source})"), None),
                Some(false) => self.add(
                    "tools",
                    None,
                    "node",
                    Status::Problem,
                    format!("node {have} is installed, but {source} asks for {range}."),
                    Some(if exact {
                        format!("install Node.js {range}, for example with `nvm install {range}` or `fnm install {range}`")
                    } else {
                        format!("install a Node.js version that matches {range}, for example with nvm or fnm")
                    }),
                ),
                None => self.add(
                    "tools",
                    None,
                    "node",
                    Status::Warning,
                    format!("node {have} is installed; {source} asks for \"{range}\", which doctor cannot read."),
                    None,
                ),
            }
        }
    }

    fn package_managers(&mut self) {
        // `packageManager` in a package.json: `pnpm@12.18.0+sha512...`.
        let mut declared: BTreeMap<String, (String, String)> = BTreeMap::new();
        let mut used: BTreeSet<String> = BTreeSet::new();
        for project in self.facts.projects.iter().filter(|p| p.kind == "node") {
            if let Some(manager) = &project.package_manager {
                used.insert(manager.clone());
            }
            let dir = dir_of(self.root, &project.root);
            let Some(field) = read_json(&dir.join("package.json")).and_then(|json| {
                json.get("packageManager")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }) else {
                continue;
            };
            if let Some((name, version)) = field.split_once('@') {
                let version = version.split('+').next().unwrap_or(version).to_owned();
                declared.entry(name.to_owned()).or_insert((
                    version,
                    shown(&project.root, "package.json (packageManager)"),
                ));
                used.insert(name.to_owned());
            }
        }
        for manager in used {
            if !matches!(manager.as_str(), "npm" | "pnpm" | "yarn" | "bun") {
                continue;
            }
            let want = declared.get(&manager).cloned();
            let have = self
                .machine
                .version(self.root, &manager, &["--version"])
                .and_then(|line| {
                    find_version(&line).or_else(|| version_numbers(&line).map(|_| line))
                });
            let install = |version: Option<&str>| match (manager.as_str(), version) {
                ("bun", _) => "install Bun from https://bun.sh".to_owned(),
                ("npm", Some(version)) => format!("npm install -g npm@{version}"),
                ("npm", None) => "npm comes with Node.js: install Node.js".to_owned(),
                (name, Some(version)) => format!("npm install -g {name}@{version}"),
                (name, None) => format!("npm install -g {name}"),
            };
            match (have, want) {
                (None, want) => {
                    let asks = want.as_ref().map_or_else(String::new, |(version, source)| {
                        format!("; {source} asks for {manager}@{version}")
                    });
                    self.add(
                        "tools",
                        None,
                        manager.clone(),
                        Status::Problem,
                        format!("{manager} is not installed, or not on the PATH{asks}."),
                        Some(install(want.as_ref().map(|(version, _)| version.as_str()))),
                    );
                }
                (Some(have), None) => self.add(
                    "tools",
                    None,
                    manager.clone(),
                    Status::Ok,
                    format!("{manager} {have}"),
                    None,
                ),
                (Some(have), Some((version, source))) => {
                    let (have_numbers, want_numbers) =
                        (version_numbers(&have), version_numbers(&version));
                    let same_major = have_numbers.as_ref().and_then(|n| n.first())
                        == want_numbers.as_ref().and_then(|n| n.first());
                    if have_numbers == want_numbers {
                        self.add(
                            "tools",
                            None,
                            manager.clone(),
                            Status::Ok,
                            format!("{manager} {have} matches {source}"),
                            None,
                        );
                    } else if same_major {
                        self.add(
                            "tools",
                            None,
                            manager.clone(),
                            Status::Warning,
                            format!("{manager} {have} is installed; {source} asks for {version}. The same major version usually works."),
                            Some(install(Some(&version))),
                        );
                    } else {
                        self.add(
                            "tools",
                            None,
                            manager.clone(),
                            Status::Problem,
                            format!(
                                "{manager} {have} is installed, but {source} asks for {version}."
                            ),
                            Some(install(Some(&version))),
                        );
                    }
                }
            }
        }
    }

    fn rust(&mut self) {
        if !self.has_kind("rust") {
            return;
        }
        let channel = ["rust-toolchain.toml", "rust-toolchain"]
            .iter()
            .find_map(|file| {
                let text = read(&self.root.join(file))?;
                if *file == "rust-toolchain" && !text.contains('[') {
                    return Some((text.trim().to_owned(), (*file).to_owned()));
                }
                parse_toml(&text)
                    .into_iter()
                    .find(|entry| entry.section == "toolchain" && entry.key == "channel")
                    .map(|entry| (crate::facts::unquote(&entry.value), (*file).to_owned()))
            });
        // rustup is asked from the repo's parent folder, so it never reads
        // the toolchain file and never downloads anything.
        let neutral = self.root.parent().unwrap_or(self.root);
        let rustup = self.machine.run(neutral, "rustup", &["toolchain", "list"]);
        let rustc = self.machine.version(neutral, "rustc", &["--version"]);
        if rustup.is_none() && rustc.is_none() {
            self.add(
                "tools",
                None,
                "rust",
                Status::Problem,
                "Rust is not installed, or not on the PATH.",
                Some("install Rust with rustup from https://rustup.rs".to_owned()),
            );
            return;
        }
        let Some((channel, file)) = channel else {
            let have = rustc
                .as_deref()
                .and_then(find_version)
                .unwrap_or_else(|| "installed".to_owned());
            self.add(
                "tools",
                None,
                "rust",
                Status::Ok,
                format!("rustc {have} (the repo pins no toolchain)"),
                None,
            );
            return;
        };
        if let Some(all) = rustup {
            let found = all.lines().any(|line| {
                let name = line.split_whitespace().next().unwrap_or("");
                name == channel || name.starts_with(&format!("{channel}-"))
            });
            if found {
                self.add(
                    "tools",
                    None,
                    "rust",
                    Status::Ok,
                    format!("toolchain {channel} is installed ({file})"),
                    None,
                );
            } else {
                self.add(
                    "tools",
                    None,
                    "rust",
                    Status::Problem,
                    format!(
                        "{file} asks for the Rust toolchain {channel}, which is not installed."
                    ),
                    Some(format!("rustup toolchain install {channel}")),
                );
            }
            return;
        }
        let have = rustc.as_deref().and_then(find_version).unwrap_or_default();
        let matches = version_numbers(&channel).is_none()
            || version_numbers(&have).is_some_and(|have| {
                same_prefix(&version_numbers(&channel).unwrap_or_default(), &have)
            });
        if matches {
            self.add(
                "tools",
                None,
                "rust",
                Status::Ok,
                format!("rustc {have} ({file} asks for {channel})"),
                None,
            );
        } else {
            self.add(
                "tools",
                None,
                "rust",
                Status::Problem,
                format!("rustc {have} is installed, but {file} asks for {channel}."),
                Some(format!("install rustup from https://rustup.rs, then run `rustup toolchain install {channel}`")),
            );
        }
    }

    fn go(&mut self) {
        for project in self
            .facts
            .projects
            .iter()
            .filter(|p| p.kind == "go")
            .map(|p| p.root.clone())
            .collect::<Vec<_>>()
        {
            let dir = dir_of(self.root, &project);
            let want = read(&dir.join("go.mod")).and_then(|text| {
                text.lines()
                    .find_map(|line| line.trim().strip_prefix("go ").map(|v| v.trim().to_owned()))
            });
            let have = self
                .machine
                .version(&dir, "go", &["version"])
                .and_then(|line| find_version(&line));
            match (have, want) {
                (None, _) => self.add(
                    "tools",
                    Some(&project),
                    subject_in(&project, "go"),
                    Status::Problem,
                    "Go is not installed, or not on the PATH.",
                    Some("install Go from https://go.dev/dl".to_owned()),
                ),
                (Some(have), Some(want))
                    if satisfies(&format!(">={want}"), &have) == Some(false) =>
                {
                    self.add(
                        "tools",
                        Some(&project),
                        subject_in(&project, "go"),
                        Status::Problem,
                        format!(
                            "go {have} is installed, but {} asks for go {want} or later.",
                            shown(&project, "go.mod")
                        ),
                        Some(format!("install Go {want} or later from https://go.dev/dl")),
                    )
                }
                (Some(have), _) => self.add(
                    "tools",
                    Some(&project),
                    subject_in(&project, "go"),
                    Status::Ok,
                    format!("go {have}"),
                    None,
                ),
            }
        }
    }

    // --- Python: interpreter, virtual environment, dependencies

    fn python(&mut self) {
        let projects: Vec<_> = self
            .facts
            .projects
            .iter()
            .filter(|p| p.kind == "python")
            .cloned()
            .collect();
        for project in projects {
            let dir = dir_of(self.root, &project.root);
            let folder = project.root.clone();
            let install = project.commands.install.clone();
            let source = project
                .interpreter
                .as_ref()
                .map_or("path", |i| i.source.as_str())
                .to_owned();
            let mut venv_home = dir.clone();
            let mut venv = VENV_DIRS
                .iter()
                .find(|venv| dir.join(venv).is_dir())
                .copied();
            // uv keeps a workspace's environment at the workspace root.
            if venv.is_none() && source == "uv" {
                let mut current = dir.parent();
                while let Some(at) = current.filter(|at| at.starts_with(self.root)) {
                    if at.join(".venv").is_dir() {
                        venv = Some(".venv");
                        venv_home = at.to_path_buf();
                        break;
                    }
                    current = at.parent();
                }
            }
            // An active virtual environment outside the project, which
            // verify uses too (ADR 0022): check that one.
            if venv.is_none() && source == "active" {
                if let Some(active) = std::env::var_os("VIRTUAL_ENV")
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
                {
                    let python = if cfg!(windows) {
                        active.join("Scripts").join("python.exe")
                    } else {
                        active.join("bin").join("python")
                    };
                    let have = self
                        .machine
                        .version(&dir, &python.to_string_lossy(), &["--version"])
                        .and_then(|line| find_version(&line));
                    let name = active
                        .file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                    match &have {
                        Some(version) => self.add(
                            "dependencies",
                            Some(&folder),
                            format!("{} (active)", subject_in(&folder, "virtual environment")),
                            Status::Ok,
                            format!("the active virtual environment {name} (VIRTUAL_ENV), with Python {version}"),
                            None,
                        ),
                        None => self.add(
                            "dependencies",
                            Some(&folder),
                            format!("{} (active)", subject_in(&folder, "virtual environment")),
                            Status::Problem,
                            format!("VIRTUAL_ENV names {name}, which has no working Python interpreter."),
                            Some("deactivate it, or make it again".to_owned()),
                        ),
                    }
                    self.python_version(&project.root, &dir, have.as_deref());
                    if have.is_some() {
                        self.python_packages(&folder, &dir, &active, install.as_deref());
                    }
                    continue;
                }
            }
            // The interpreter: the venv's, else python from the PATH.
            let program = match venv {
                Some(venv) => {
                    let path = venv_home.join(
                        venv_python(venv)
                            .replace('\\', "/")
                            .split('/')
                            .collect::<PathBuf>(),
                    );
                    path.to_string_lossy().into_owned()
                }
                None if source == "path" || source == "active" => {
                    if cfg!(windows) {
                        "python".to_owned()
                    } else {
                        "python3".to_owned()
                    }
                }
                None => String::new(),
            };
            let have = (!program.is_empty())
                .then(|| self.machine.version(&dir, &program, &["--version"]))
                .flatten()
                .and_then(|line| find_version(&line));
            // The virtual environment.
            match (venv, source.as_str()) {
                (Some(venv), _) if have.is_none() => self.add(
                    "dependencies",
                    Some(&folder),
                    shown(&folder, venv),
                    Status::Problem,
                    format!("{} has no working Python interpreter; it was probably made on another machine or with a Python that is gone.", shown(&folder, venv)),
                    Some(format!("delete {venv}, then in {folder}: python -m venv {venv}{}", install.as_ref().map_or_else(String::new, |i| format!(" && {i}")))),
                ),
                (Some(venv), _) => self.add("dependencies", Some(&folder), shown(&folder, venv), Status::Ok, format!("virtual environment with Python {}", have.clone().unwrap_or_default()), None),
                (None, "uv" | "poetry" | "pipenv") => {
                    let manager = source.clone();
                    let found = self.machine.version(&dir, &manager, &["--version"]).is_some();
                    if !found {
                        self.add(
                            "tools",
                            Some(&folder),
                            manager.clone(),
                            Status::Problem,
                            format!("{folder} uses {manager}, which is not installed or not on the PATH."),
                            Some(match manager.as_str() {
                                "uv" => "install uv: https://docs.astral.sh/uv/getting-started/installation/".to_owned(),
                                "poetry" => "install Poetry: https://python-poetry.org/docs/#installation".to_owned(),
                                _ => "python -m pip install --user pipenv".to_owned(),
                            }),
                        );
                    } else if manager == "uv" {
                        self.add(
                            "dependencies",
                            Some(&folder),
                            shown(&folder, ".venv"),
                            Status::Problem,
                            format!("{folder} has no virtual environment yet."),
                            Some(format!("in {folder}: {}", install.clone().unwrap_or_else(|| "uv sync".to_owned()))),
                        );
                    } else {
                        self.add(
                            "dependencies",
                            Some(&folder),
                            format!("{manager} environment"),
                            Status::Warning,
                            format!("{manager} may keep the virtual environment outside {folder}; doctor cannot check its packages."),
                            install.clone().map(|i| format!("in {folder}: {i}")),
                        );
                    }
                }
                (None, _) => {
                    // The install command, run with the new environment's Python.
                    let then = install.as_deref().map_or_else(
                        || "install the dependencies into it".to_owned(),
                        |install| match install.strip_prefix("python -m ") {
                            Some(rest) => format!("{} -m {rest}", venv_python(".venv")),
                            None => install.to_owned(),
                        },
                    );
                    self.add(
                        "dependencies",
                        Some(&folder),
                        shown(&folder, ".venv"),
                        Status::Problem,
                        format!("{folder} has no virtual environment (.venv or venv), so its packages would go into the Python on the PATH."),
                        Some(format!("in {folder}: python -m venv .venv, then {then}")),
                    );
                }
            }
            // uv, poetry and pipenv fetch the Python they need themselves.
            if have.is_some() || matches!(source.as_str(), "path" | "active" | "venv") {
                self.python_version(&project.root, &dir, have.as_deref());
            }
            if let Some(venv) = venv {
                self.python_packages(&folder, &dir, &venv_home.join(venv), install.as_deref());
            }
        }
    }

    fn python_version(&mut self, folder: &str, dir: &Path, have: Option<&str>) {
        let mut wants = Vec::new();
        // `.python-version` here or in a folder above, as pyenv and uv read it.
        let mut current = Some(dir);
        while let Some(at) = current {
            if let Some(text) = read(&at.join(".python-version")) {
                let first = text.lines().next().unwrap_or("").trim().to_owned();
                if version_numbers(&first).is_some() {
                    let rel = at
                        .strip_prefix(self.root)
                        .map(|p| p.to_string_lossy().replace('\\', "/"))
                        .unwrap_or_default();
                    wants.push((
                        first,
                        shown(if rel.is_empty() { "." } else { &rel }, ".python-version"),
                    ));
                }
                break;
            }
            if at == self.root {
                break;
            }
            current = at.parent().filter(|p| p.starts_with(self.root));
        }
        if let Some(text) = read(&dir.join("pyproject.toml")) {
            if let Some(entry) = parse_toml(&text)
                .into_iter()
                .find(|e| e.section == "project" && e.key == "requires-python")
            {
                wants.push((
                    crate::facts::unquote(&entry.value),
                    shown(folder, "pyproject.toml (requires-python)"),
                ));
            }
        }
        let Some(have) = have else {
            if let Some((want, source)) = wants.first() {
                self.add(
                    "tools",
                    Some(folder),
                    subject_in(folder, "python"),
                    Status::Problem,
                    format!("No Python interpreter was found for {folder}; {source} asks for {want}."),
                    Some(format!("install Python {want} from https://www.python.org/downloads (or with uv or pyenv)")),
                );
            }
            return;
        };
        for (want, source) in wants {
            match satisfies_as(&tighten(&want), have, Flavor::Pep440) {
                Some(true) => self.add(
                    "tools",
                    Some(folder),
                    subject_in(folder, "python"),
                    Status::Ok,
                    format!("python {have} matches {want} ({source})"),
                    None,
                ),
                Some(false) => self.add(
                    "tools",
                    Some(folder),
                    subject_in(folder, "python"),
                    Status::Problem,
                    format!("python {have} is used for {folder}, but {source} asks for {want}."),
                    Some(format!(
                        "install Python {want}, then make the virtual environment again with it"
                    )),
                ),
                None => {}
            }
        }
    }

    fn python_packages(&mut self, folder: &str, dir: &Path, venv: &Path, install: Option<&str>) {
        let installed = installed_python_packages(venv);
        let fix = install.map(|install| format!("in {folder}: {install}"));
        // What should be installed: the requirements files the install
        // command names (they include the rest with -r), else all
        // requirements*.txt, else [project] dependencies.
        let rel = |file: &str| shown(folder, file);
        let mut files: Vec<String> = install
            .map(|install| {
                install
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .windows(2)
                    .filter(|pair| pair[0] == "-r")
                    .map(|pair| rel(pair[1]))
                    .collect()
            })
            .unwrap_or_default();
        if files.is_empty() {
            if let Ok(entries) = fs::read_dir(dir) {
                let mut names: Vec<String> = entries
                    .filter_map(Result::ok)
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| {
                        n.to_lowercase().contains("requirements")
                            && n.to_lowercase().ends_with(".txt")
                    })
                    .collect();
                names.sort();
                files = names.iter().map(|n| rel(n)).collect();
            }
        }
        let mut wanted: BTreeMap<String, Option<String>> = BTreeMap::new();
        let mut source = String::new();
        if files.is_empty() {
            if let Some(text) = read(&dir.join("pyproject.toml")) {
                for entry in parse_toml(&text) {
                    if entry.section == "project" && entry.key == "dependencies" {
                        for spec in crate::facts::array_strings(&entry.value) {
                            if let Some(name) = requirement_name(&spec) {
                                wanted.insert(normalize_package(&name), pinned(&spec));
                            }
                        }
                    }
                }
                source = rel("pyproject.toml");
            }
        } else {
            let mut names = BTreeSet::new();
            let mut seen = BTreeSet::new();
            for file in &files {
                read_requirements(self.root, file, 0, &mut seen, &mut names);
            }
            // The pins, from the same files.
            let mut pins: BTreeMap<String, String> = BTreeMap::new();
            for file in &seen {
                let Some(text) = read(&dir_of(self.root, file)) else {
                    continue;
                };
                for line in text.lines() {
                    let line = crate::facts::strip_comment(line).trim();
                    if line.starts_with('-') || line.contains(';') {
                        continue;
                    }
                    if let (Some(name), Some(pin)) = (requirement_name(line), pinned(line)) {
                        pins.insert(normalize_package(&name), pin);
                    }
                }
            }
            // A requirement with an environment marker may not apply here.
            let marked: BTreeSet<String> = seen
                .iter()
                .filter_map(|file| read(&dir_of(self.root, file)))
                .flat_map(|text| {
                    text.lines()
                        .filter(|line| line.contains(';') && !line.trim_start().starts_with('#'))
                        .filter_map(requirement_name)
                        .map(|name| normalize_package(&name))
                        .collect::<Vec<_>>()
                })
                .collect();
            for name in names {
                let name = normalize_package(&name);
                if marked.contains(&name) {
                    continue;
                }
                let pin = pins.get(&name).cloned();
                wanted.insert(name, pin);
            }
            source = files.join(", ");
        }
        if wanted.is_empty() {
            return;
        }
        let mut missing = Vec::new();
        let mut wrong = Vec::new();
        for (name, pin) in &wanted {
            match (installed.get(name), pin) {
                (None, _) => missing.push(name.clone()),
                (Some(have), Some(want)) if !same_python_version(have, want) => {
                    wrong.push(format!("{name} {have} (wants {want})"));
                }
                _ => {}
            }
        }
        let subject = subject_in(folder, "packages");
        if missing.is_empty() && wrong.is_empty() {
            self.add(
                "dependencies",
                Some(folder),
                subject,
                Status::Ok,
                format!("{} in {source} installed", packages_are(wanted.len())),
                None,
            );
            return;
        }
        let mut parts = Vec::new();
        if !missing.is_empty() {
            parts.push(format!(
                "{} of {} packages in {source} are not installed: {}",
                missing.len(),
                wanted.len(),
                list(&missing)
            ));
        }
        if !wrong.is_empty() {
            let verb = if wrong.len() == 1 {
                "does not match its pinned version"
            } else {
                "do not match their pinned versions"
            };
            parts.push(format!("{} {verb}: {}", wrong.len(), list(&wrong)));
        }
        self.add(
            "dependencies",
            Some(folder),
            subject,
            Status::Problem,
            format!("{}.", parts.join("; ")),
            fix,
        );
    }

    // --- Node dependencies

    fn node_dependencies(&mut self) {
        let projects: Vec<_> = self
            .facts
            .projects
            .iter()
            .filter(|p| p.kind == "node")
            .cloned()
            .collect();
        let root_install = self
            .facts
            .projects
            .iter()
            .find(|p| p.root == ".")
            .and_then(|p| p.commands.install.clone());
        for project in projects {
            let dir = dir_of(self.root, &project.root);
            let Some(json) = read_json(&dir.join("package.json")) else {
                continue;
            };
            let install = project
                .commands
                .install
                .clone()
                .or_else(|| root_install.clone());
            let fix = install.as_ref().map(|install| {
                if project.commands.install.is_some() && project.root != "." {
                    format!("in {}: {install}", project.root)
                } else {
                    install.clone()
                }
            });
            let mut names: Vec<String> = Vec::new();
            for field in ["dependencies", "devDependencies"] {
                if let Some(map) = json.get(field).and_then(Value::as_object) {
                    for (name, spec) in map {
                        // Workspace links are not downloaded.
                        let spec = spec.as_str().unwrap_or("");
                        if spec.starts_with("workspace:")
                            || spec.starts_with("file:")
                            || spec.starts_with("link:")
                        {
                            continue;
                        }
                        names.push(name.clone());
                    }
                }
            }
            if names.is_empty() {
                continue;
            }
            // Node looks in node_modules here and in every folder above.
            let missing: Vec<String> = names
                .iter()
                .filter(|name| !self.node_package_found(&dir, name))
                .cloned()
                .collect();
            let subject = subject_in(&project.root, "node_modules");
            if missing.len() == names.len() {
                self.add(
                    "dependencies",
                    Some(&project.root),
                    subject,
                    Status::Problem,
                    format!(
                        "the dependencies of {} are not installed.",
                        shown(&project.root, "package.json")
                    ),
                    fix,
                );
                continue;
            }
            if !missing.is_empty() {
                self.add(
                    "dependencies",
                    Some(&project.root),
                    subject,
                    Status::Problem,
                    format!(
                        "{} of {} packages in {} are not installed: {}.",
                        missing.len(),
                        names.len(),
                        shown(&project.root, "package.json"),
                        list(&missing)
                    ),
                    fix,
                );
                continue;
            }
            // Installed: is the lockfile newer than the last install?
            if let Some((lockfile, marker)) = self.lock_and_marker(&dir) {
                if let (Some(lock_time), Some(marker_time)) =
                    (modified(&lockfile), modified(&marker))
                {
                    if lock_time > marker_time {
                        let name = lockfile
                            .strip_prefix(self.root)
                            .map(|p| p.to_string_lossy().replace('\\', "/"))
                            .unwrap_or_default();
                        self.add(
                            "dependencies",
                            Some(&project.root),
                            subject,
                            Status::Problem,
                            format!("{name} changed after the last install, so the installed packages may not match it."),
                            fix,
                        );
                        continue;
                    }
                }
            }
            self.add(
                "dependencies",
                Some(&project.root),
                subject,
                Status::Ok,
                format!(
                    "all {} packages in {} are installed",
                    names.len(),
                    shown(&project.root, "package.json")
                ),
                None,
            );
        }
    }

    fn node_package_found(&self, dir: &Path, name: &str) -> bool {
        let parts: PathBuf = name.split('/').collect();
        let mut current = Some(dir);
        while let Some(at) = current {
            if at
                .join("node_modules")
                .join(&parts)
                .join("package.json")
                .is_file()
            {
                return true;
            }
            if at == self.root {
                return false;
            }
            current = at.parent().filter(|p| p.starts_with(self.root));
        }
        false
    }

    /// The nearest lockfile, from `dir` up to the root, and the file the
    /// package manager writes into `node_modules` beside it on each install.
    fn lock_and_marker(&self, dir: &Path) -> Option<(PathBuf, PathBuf)> {
        let pairs = [
            ("package-lock.json", ".package-lock.json"),
            ("pnpm-lock.yaml", ".modules.yaml"),
            ("yarn.lock", ".yarn-integrity"),
            ("yarn.lock", ".yarn-state.yml"),
            ("bun.lock", ".bun-tag"),
        ];
        let mut current = Some(dir);
        while let Some(at) = current {
            for (lock, marker) in pairs {
                let lockfile = at.join(lock);
                let marker = at.join("node_modules").join(marker);
                if lockfile.is_file() && marker.is_file() {
                    return Some((lockfile, marker));
                }
            }
            if pairs.iter().any(|(lock, _)| at.join(lock).is_file()) || at == self.root {
                return None;
            }
            current = at.parent().filter(|p| p.starts_with(self.root));
        }
        None
    }

    // --- env variables

    fn env(&mut self) {
        for folder in self.folders() {
            let dir = dir_of(self.root, &folder);
            let Some(text) = read(&dir.join(ENV_EXAMPLE)) else {
                continue;
            };
            let wanted = env_names(&text);
            if wanted.is_empty() {
                continue;
            }
            let mut local = BTreeSet::new();
            let mut local_files = Vec::new();
            for file in LOCAL_ENV_FILES {
                if let Some(text) = read(&dir.join(file)) {
                    local.extend(env_names(&text));
                    local_files.push(shown(&folder, file));
                }
            }
            let missing: Vec<String> = wanted
                .iter()
                .filter(|name| !self.machine.env_set(name) && !local.contains(*name))
                .cloned()
                .collect();
            let example = shown(&folder, ENV_EXAMPLE);
            let project = (folder != ".").then_some(folder.as_str());
            if missing.is_empty() {
                let place = if local_files.is_empty() {
                    "the environment".to_owned()
                } else {
                    format!("the environment or {}", local_files.join(", "))
                };
                self.add(
                    "env",
                    project,
                    example,
                    Status::Ok,
                    format!(
                        "{} set in {place}",
                        if wanted.len() == 1 {
                            "the 1 variable is".to_owned()
                        } else {
                            format!("all {} variables are", wanted.len())
                        }
                    ),
                    None,
                );
                continue;
            }
            let target = shown(&folder, ".env");
            let fix = if local_files.is_empty() {
                format!("copy {example} to {target} and fill in the values (or set the variables in your shell)")
            } else {
                format!(
                    "add {} to {} (see {example} for what each one is)",
                    list(&missing),
                    local_files[0]
                )
            };
            let where_looked = if local_files.is_empty() {
                format!("the environment, and there is no {target}")
            } else {
                format!("the environment or {}", local_files.join(", "))
            };
            self.add(
                "env",
                project,
                example,
                Status::Problem,
                format!(
                    "{} of {} variables are not set in {where_looked}: {}.",
                    missing.len(),
                    wanted.len(),
                    list(&missing)
                ),
                Some(fix),
            );
        }
    }

    // --- services

    fn services(&mut self) {
        // Every folder a project's verify step reads services from: the
        // project's own and those above it, up to the root.
        let mut folders: Vec<PathBuf> = Vec::new();
        for folder in self.folders() {
            for up in folders_up(self.root, &dir_of(self.root, &folder))
                .into_iter()
                .rev()
            {
                if !folders.contains(&up) {
                    folders.push(up);
                }
            }
        }
        for service in services_in(self.root, &folders) {
            if self.machine.listening(&service.host, service.port) {
                self.add(
                    "services",
                    None,
                    service.describe(),
                    Status::Ok,
                    format!("reachable ({})", service.source),
                    None,
                );
            } else {
                self.add(
                    "services",
                    None,
                    service.describe(),
                    Status::Problem,
                    format!(
                        "not reachable: nothing listens on port {} ({}).",
                        service.port, service.source
                    ),
                    Some(service.next_step()),
                );
            }
        }
    }
}

/// `all 12 packages are`, `the 1 package is`.
fn packages_are(count: usize) -> String {
    if count == 1 {
        "the 1 package is".to_owned()
    } else {
        format!("all {count} packages are")
    }
}

/// `backend/api packages`, or `packages` for the root.
fn subject_in(folder: &str, what: &str) -> String {
    if folder == "." {
        what.to_owned()
    } else {
        format!("{folder} {what}")
    }
}

/// `requests==2.31.0` gives `2.31.0`; `Django==4.2.*` gives `4.2.*`.
fn pinned(spec: &str) -> Option<String> {
    let index = spec.find("==")?;
    if spec[index..].starts_with("===") {
        return None;
    }
    let rest = spec[index + 2..].trim();
    let version: String = rest
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != ',' && *c != ';' && *c != '"' && *c != '\'')
        .collect();
    (!version.is_empty()).then_some(version)
}

/// A package name as pip compares it: lower case, runs of `-`, `_` and `.`
/// as one `-`.
fn normalize_package(name: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in name.chars() {
        if matches!(c, '-' | '_' | '.') {
            dash = true;
        } else {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.push(c.to_ascii_lowercase());
        }
    }
    out
}

/// `2.31` and `2.31.0` are the same version; `4.2.*` matches `4.2.7`.
fn same_python_version(have: &str, want: &str) -> bool {
    if let Some(prefix) = want.strip_suffix(".*") {
        return have == prefix || have.starts_with(&format!("{prefix}."));
    }
    let trim = |version: &str| {
        let mut parts: Vec<&str> = version.split('.').collect();
        while parts.len() > 1 && parts.last() == Some(&"0") {
            parts.pop();
        }
        parts.join(".")
    };
    trim(have) == trim(want)
}

/// The packages installed in a virtual environment, by normalised name,
/// with their versions, read from the `.dist-info` folders.
fn installed_python_packages(venv: &Path) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    let mut site_dirs = vec![venv.join("Lib").join("site-packages")];
    for lib in ["lib", "lib64"] {
        if let Ok(entries) = fs::read_dir(venv.join(lib)) {
            for entry in entries.filter_map(Result::ok) {
                site_dirs.push(entry.path().join("site-packages"));
            }
        }
    }
    for site in site_dirs {
        let Ok(entries) = fs::read_dir(&site) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(stem) = name
                .strip_suffix(".dist-info")
                .or_else(|| name.strip_suffix(".egg-info"))
            else {
                continue;
            };
            let mut parts = stem.splitn(3, '-');
            let (Some(package), Some(version)) = (parts.next(), parts.next()) else {
                continue;
            };
            found.insert(normalize_package(package), version.to_owned());
        }
    }
    found
}

/// Runs every check.
#[must_use]
pub fn run_doctor(root: &Path, facts: &RepoFacts, machine: &dyn Machine) -> DoctorReport {
    let mut doctor = Doctor {
        root,
        facts,
        machine,
        findings: Vec::new(),
    };
    doctor.node();
    doctor.package_managers();
    doctor.rust();
    doctor.go();
    doctor.python();
    doctor.node_dependencies();
    doctor.env();
    doctor.services();
    // Sections in a fixed order; within one, the order checks were made.
    let order = |section: &str| {
        ["tools", "env", "dependencies", "services"]
            .iter()
            .position(|s| *s == section)
            .unwrap_or(9)
    };
    doctor
        .findings
        .sort_by_key(|finding| order(finding.section));
    let problems = doctor
        .findings
        .iter()
        .filter(|f| f.status == Status::Problem)
        .count();
    let warnings = doctor
        .findings
        .iter()
        .filter(|f| f.status == Status::Warning)
        .count();
    DoctorReport {
        schema_version: DOCTOR_SCHEMA_VERSION,
        ok: problems == 0,
        problems,
        warnings,
        checks: doctor.findings,
    }
}

fn section_title(section: &str) -> &'static str {
    match section {
        "tools" => "Tools",
        "env" => "Environment variables",
        "dependencies" => "Dependencies",
        _ => "Services",
    }
}

/// The report for people.
#[must_use]
pub fn format_doctor_human(report: &DoctorReport) -> String {
    let mut out = String::new();
    let mut section = "";
    for finding in &report.checks {
        if finding.section != section {
            section = finding.section;
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(section_title(section));
            out.push('\n');
        }
        let label = match finding.status {
            Status::Ok => "ok     ",
            Status::Problem => "PROBLEM",
            Status::Warning => "warning",
        };
        out.push_str(&format!(
            "  {label}  {}: {}\n",
            finding.subject, finding.detail
        ));
        if let Some(fix) = &finding.fix {
            out.push_str(&format!("           next: {fix}\n"));
        }
    }
    if report.checks.is_empty() {
        out.push_str("Nothing to check: no projects, env examples or services were found.\n");
    }
    out.push('\n');
    let plural = |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
    if report.ok {
        if report.warnings == 0 {
            out.push_str("doctor: OK - this machine is ready for this repo.\n");
        } else {
            out.push_str(&format!(
                "doctor: OK with {} - read them above.\n",
                plural(report.warnings, "warning")
            ));
        }
    } else {
        out.push_str(&format!(
            "doctor: {}{} - run the `next:` commands above, then `fscale doctor` again.\n",
            plural(report.problems, "problem"),
            if report.warnings > 0 {
                format!(", {}", plural(report.warnings, "warning"))
            } else {
                String::new()
            }
        ));
    }
    out
}

/// The report as JSON, ending with a newline.
#[must_use]
pub fn format_doctor_json(report: &DoctorReport) -> String {
    let mut text = serde_json::to_string_pretty(report).unwrap_or_else(|_| "{}".to_owned());
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_read_and_compared() {
        assert_eq!(version_numbers("v22.11.0"), Some(vec![22, 11, 0]));
        assert_eq!(version_numbers("3.12.1rc1"), Some(vec![3, 12, 1]));
        assert_eq!(version_numbers("lts/*"), None);
        assert_eq!(
            find_version("go version go1.22.3 linux/amd64").as_deref(),
            Some("1.22.3")
        );
        assert_eq!(find_version("Python 3.12.1").as_deref(), Some("3.12.1"));
        assert_eq!(
            find_version("rustc 1.97.0 (2d8144b78 2026-07-07)").as_deref(),
            Some("1.97.0")
        );
        for (range, have, want) in [
            (">=22.18.0", "22.23.4", true),
            (">=22.18.0", "20.11.0", false),
            ("^18 || ^20", "20.1.0", true),
            ("^18 || ^20", "22.0.0", false),
            (">=18 <21", "20.9.0", true),
            (">=18 <21", "21.0.0", false),
            ("22", "22.4.1", true),
            ("22", "20.4.1", false),
            ("22.x", "22.4.1", true),
            ("~1.2.3", "1.2.9", true),
            ("~1.2.3", "1.3.0", false),
            (">=3.10,<4", "3.12.1", true),
            (">=3.10,<4", "3.9.2", false),
            ("~=3.10", "3.12.0", true),
            ("3.12", "3.12.7", true),
            ("3.12", "3.11.7", false),
            ("14 - 16", "16.3.0", true),
            ("14 - 16", "17.0.0", false),
        ] {
            assert_eq!(
                satisfies(&tighten(range), have),
                Some(want),
                "{range} vs {have}"
            );
        }
        assert_eq!(satisfies(&tighten(">= 22"), "22.1.0"), Some(true));
        // npm reads partial versions as ranges (review of M12).
        assert_eq!(satisfies(">=18 <=22", "22.11.0"), Some(true));
        assert_eq!(satisfies(">16", "16.5.0"), Some(false));
        assert_eq!(satisfies(">16", "17.0.0"), Some(true));
        // PEP 440: wildcards for != and exact ==.
        assert_eq!(
            satisfies_as(">=3.10,!=3.12.*", "3.12.4", Flavor::Pep440),
            Some(false)
        );
        assert_eq!(
            satisfies_as(">=3.10,!=3.12.*", "3.13.0", Flavor::Pep440),
            Some(true)
        );
        assert_eq!(
            satisfies_as("==3.12", "3.12.4", Flavor::Pep440),
            Some(false)
        );
        assert_eq!(satisfies_as("==3.12", "3.12.0", Flavor::Pep440), Some(true));
        assert_eq!(
            satisfies_as("==3.12.*", "3.12.4", Flavor::Pep440),
            Some(true)
        );
    }

    #[test]
    fn env_names_never_keep_values() {
        let names = env_names("# c\nexport A=1\nB = \"two words\"\nnot a line\nC=\n");
        assert_eq!(names.into_iter().collect::<Vec<_>>(), ["A", "B", "C"]);
    }

    #[test]
    fn python_names_and_versions_compare_like_pip() {
        assert_eq!(
            normalize_package("Django_REST.framework"),
            "django-rest-framework"
        );
        assert!(same_python_version("2.31.0", "2.31"));
        assert!(same_python_version("4.2.7", "4.2.*"));
        assert!(!same_python_version("4.3.0", "4.2.*"));
        assert_eq!(
            pinned("requests[socks]==2.31.0 ; python_version>'3'").as_deref(),
            Some("2.31.0")
        );
        assert_eq!(pinned("requests>=2"), None);
    }
}
