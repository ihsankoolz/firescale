//! Agent hooks: installing them, and deciding whether an agent may stop.
//!
//! `fscale hooks install` writes each agent's hook config so that the agent
//! calls `fscale hooks run --agent <name>` when it is about to stop. That
//! command runs verify and, if it fails, tells the agent to keep working. A
//! counter caps how many times in a row one session can be sent back.
//!
//! Formats (checked on 2026-11-11 and 2026-11-12, see ADR 0014 and 0027):
//! - Claude Code: `.claude/settings.json`, a `Stop` hook. Confirmed from docs.
//! - Codex CLI: `.codex/hooks.json`, same shape. Confirmed from its source.
//! - Cursor: `.cursor/hooks.json`, a `stop` hook. Confirmed from Cursor's
//!   own example repositories (`cursor/cookbook`, `cursor/plugins`); its
//!   docs page could not be reached.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::error::{Error, Result};

/// How many times in a row a session is sent back before it is allowed to stop.
pub const DEFAULT_MAX_BLOCKS: u32 = 3;
/// Overrides `DEFAULT_MAX_BLOCKS`.
pub const MAX_BLOCKS_ENV: &str = "FSCALE_HOOK_MAX_BLOCKS";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
    Cursor,
}

pub const AGENTS: [Agent; 3] = [Agent::Claude, Agent::Codex, Agent::Cursor];

impl Agent {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Cursor => "cursor",
        }
    }

    pub fn parse(name: &str) -> Result<Self> {
        AGENTS
            .into_iter()
            .find(|agent| agent.name() == name)
            .ok_or_else(|| {
                Error::usage(format!(
                    "unknown agent \"{name}\". Supported agents: claude, codex, cursor."
                ))
            })
    }

    /// The hook config file, relative to the repo root, with forward slashes.
    #[must_use]
    pub fn config_file(self) -> &'static str {
        match self {
            Self::Claude => ".claude/settings.json",
            Self::Codex => ".codex/hooks.json",
            Self::Cursor => ".cursor/hooks.json",
        }
    }
}

/// True for the stop hook files of the agents at the repo root
/// (`.claude/settings.json`, `.codex/hooks.json`, `.cursor/hooks.json`).
/// Agents and firescale's check read them. Each is a file only of the units
/// whose folders hold it: the repo root's own steps, or a project that
/// reads it by its path. What it holds apart from its hook entries is in
/// every key by value ([`outside_hooks`], ADR 0064).
#[must_use]
pub fn is_agent_hook_file(path: &str) -> bool {
    AGENTS.iter().any(|agent| agent.config_file() == path)
}

/// The command an agent's hook runs.
#[must_use]
pub fn hook_command(bin: &str, agent: Agent) -> String {
    format!("{bin} hooks run --agent {}", agent.name())
}

/// The command that starts this version of firescale through npx, pinned:
/// `npx --yes firescale@<version>`, the same pin as the starter CI's tamper
/// step. A release changes it in the next `fscale` only.
#[must_use]
pub fn pinned_npx() -> String {
    format!("npx --yes firescale@{}", crate::version())
}

/// The version of a pin firescale wrote, [`pinned_npx`] of this or of any
/// other release (`npx --yes firescale@0.5.0`), else `None`. A version is
/// letters, digits, `.`, `-` and `+` only, so nothing else can follow it.
#[must_use]
pub fn pinned_version(prefix: &str) -> Option<&str> {
    let version = prefix.strip_prefix("npx --yes firescale@")?;
    let fits = version.starts_with(|c: char| c.is_ascii_digit())
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'));
    fits.then_some(version)
}

/// True when version `a` comes before version `b`: `major.minor.patch`
/// compared as numbers, and a pre-release (`0.5.2-rc.1`) before its
/// release. A version that does not read as one is never older.
fn older(a: &str, b: &str) -> bool {
    fn parts(version: &str) -> Option<([u64; 3], bool)> {
        let version = version.split('+').next()?;
        let (core, pre) = match version.split_once('-') {
            Some((core, _)) => (core, true),
            None => (version, false),
        };
        let mut numbers = core.split('.').map(|part| part.parse::<u64>().ok());
        let triple = [numbers.next()??, numbers.next()??, numbers.next()??];
        numbers.next().is_none().then_some((triple, pre))
    }
    match (parts(a), parts(b)) {
        (Some((a, a_pre)), Some((b, b_pre))) => a < b || (a == b && a_pre && !b_pre),
        _ => false,
    }
}

/// A pin firescale wrote that `fscale sync` moves: the old version and
/// what it becomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovedPin {
    pub from: String,
    pub to: String,
}

/// The agent's hook file `text` with the version of every stop hook
/// command firescale wrote with an older pin moved to this version, and
/// the pins moved. A command is moved only when it is exactly
/// `npx --yes firescale@<older> hooks run --agent <name>`: anything else in
/// the file, another command included, stays byte for byte. A newer pin is
/// left as it is (a teammate's older fscale never moves it back), and so is
/// a command whose text also stands somewhere else in the file than in
/// the stop hooks. `None` when nothing moves (owner's decision on ADR 0064).
#[must_use]
pub fn move_pins(text: &str, agent: Agent) -> Option<(String, Vec<MovedPin>)> {
    let value = serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')).ok()?;
    let suffix = format!(" hooks run --agent {}", agent.name());
    let this = crate::version();
    let mut out = text.to_owned();
    let mut moved: Vec<MovedPin> = Vec::new();
    let commands = stop_commands(&value, agent);
    let mut done: Vec<&str> = Vec::new();
    for command in commands.iter().copied() {
        if done.contains(&command) {
            continue;
        }
        done.push(command);
        let Some(version) = command
            .strip_suffix(suffix.as_str())
            .and_then(pinned_version)
        else {
            continue;
        };
        if !older(version, this) {
            continue;
        }
        let quoted = serde_json::to_string(command).ok()?;
        let in_stop = commands.iter().filter(|other| **other == command).count();
        if out.matches(quoted.as_str()).count() != in_stop {
            continue;
        }
        let new_command = hook_command(&pinned_npx(), agent);
        let new = serde_json::to_string(&new_command).ok()?;
        let candidate = out.replace(quoted.as_str(), &new);
        // The text must have changed this command where the hooks hold it,
        // and nothing else: written with an escape, the same text elsewhere
        // could have been changed instead (review of the final diff).
        let mut expected =
            serde_json::from_str::<Value>(out.trim_start_matches('\u{feff}')).ok()?;
        rename_stop_command(&mut expected, agent, command, &new_command);
        let got = serde_json::from_str::<Value>(candidate.trim_start_matches('\u{feff}')).ok()?;
        if got != expected {
            return None;
        }
        out = candidate;
        moved.push(MovedPin {
            from: version.to_owned(),
            to: this.to_owned(),
        });
    }
    (!moved.is_empty()).then_some((out, moved))
}

/// Sets every stop hook command of `agent` that is `from` to `to`.
fn rename_stop_command(value: &mut Value, agent: Agent, from: &str, to: &str) {
    let key = match agent {
        Agent::Claude | Agent::Codex => "Stop",
        Agent::Cursor => "stop",
    };
    let Some(list) = value
        .get_mut("hooks")
        .and_then(|hooks| hooks.get_mut(key))
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    let entries: Vec<&mut Value> = match agent {
        Agent::Claude | Agent::Codex => list
            .iter_mut()
            .filter_map(|group| group.get_mut("hooks").and_then(Value::as_array_mut))
            .flatten()
            .collect(),
        Agent::Cursor => list.iter_mut().collect(),
    };
    for entry in entries {
        if let Some(command) = entry.get_mut("command") {
            if command.as_str() == Some(from) {
                *command = Value::String(to.to_owned());
            }
        }
    }
}

/// What `fscale doctor` starts for a pin: npx with `--no`, which runs
/// firescale only when it is installed or already in npx's cache, and
/// never downloads it (second review of ADR 0064).
#[must_use]
pub fn no_download(version: &str) -> String {
    format!("npx --no -- firescale@{version}")
}

/// True when npx said, under `--no`, that it would have had to download.
#[must_use]
pub fn not_downloaded(check: &StartCheck) -> bool {
    matches!(check, StartCheck::Failed { output, .. } if output.contains("npx canceled due to missing packages"))
}

/// A pin's answer: firescale 0.5.1 and older answer `--version --json`
/// with the version alone. From the pinned package, that version is
/// firescale's answer.
#[must_use]
pub fn pinned_answer(version: &str, check: StartCheck) -> StartCheck {
    match check {
        StartCheck::NotFirescale(answer) if answer.trim() == version => {
            StartCheck::Answered(version.to_owned())
        }
        other => other,
    }
}

/// True when the repo's package.json has an `fscale` script: a command
/// such as `yarn --silent fscale` then runs the repo's own text.
#[must_use]
pub fn has_fscale_script(root: &Path) -> bool {
    fs::read_to_string(root.join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')).ok())
        .and_then(|json| json.get("scripts")?.get("fscale").cloned())
        .is_some_and(|script| script.is_string())
}

/// How this repo starts fscale: through its own `fscale` package script
/// when it has one, else through the firescale it lists among its
/// dependencies, else this version of firescale through npx, pinned
/// ([`pinned_npx`]). The answer depends on the repo only, never on this
/// machine's PATH, so every teammate's agent runs the same command, and a
/// release of firescale does not change it (ADR 0064).
#[must_use]
pub fn fscale_command(facts: &crate::facts::RepoFacts, package_json: Option<&Value>) -> String {
    let has_script = package_json
        .and_then(|json| json.get("scripts"))
        .and_then(|scripts| scripts.get("fscale"))
        .is_some_and(Value::is_string);
    let manager = facts.package_manager.as_deref();
    if has_script {
        return match manager {
            Some("pnpm") => "pnpm --silent fscale".to_owned(),
            Some("yarn") => "yarn --silent fscale".to_owned(),
            Some("bun") => "bun run --silent fscale".to_owned(),
            _ => "npm run --silent fscale --".to_owned(),
        };
    }
    let installed = ["devDependencies", "dependencies"].iter().any(|list| {
        package_json
            .and_then(|json| json.get(list))
            .and_then(|deps| deps.get("firescale"))
            .is_some()
    });
    if installed {
        // The `fscale` that `node_modules/.bin` holds, never a download:
        // `npx --no` fails instead of fetching it. `--` ends npx's own
        // options: npm 10 takes the `--version` of `npx --no fscale
        // --version` for itself (second review of ADR 0064).
        return match manager {
            Some("pnpm") => "pnpm exec fscale".to_owned(),
            Some("yarn") => "yarn --silent fscale".to_owned(),
            Some("bun") => "bun run --silent fscale".to_owned(),
            _ => "npx --no -- fscale".to_owned(),
        };
    }
    pinned_npx()
}

/// [`fscale_command`] for the repo at `root`: the default of
/// `fscale hooks install --bin`.
#[must_use]
pub fn default_bin(root: &Path) -> String {
    let package_json: Option<Value> = fs::read_to_string(root.join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str(text.trim_start_matches('\u{feff}')).ok());
    match crate::facts::detect_facts_with(root, None) {
        Ok(facts) => fscale_command(&facts, package_json.as_ref()),
        // Facts that cannot be read leave no package manager to name.
        Err(_) => pinned_npx(),
    }
}

/// How the commands of our stop hook in an agent's file start fscale: the
/// text before `hooks run --agent <name>` (`npx firescale`), once each.
/// This is text from the repo: it is only started when it is a command
/// fscale would write itself ([`may_start`]).
#[must_use]
pub fn installed_prefixes(root: &Path, agent: Agent) -> Vec<String> {
    let needle = format!("hooks run --agent {}", agent.name());
    let Some(value) = fs::read_to_string(resolve(root, agent.config_file()))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')).ok())
    else {
        return Vec::new();
    };
    let mut prefixes = Vec::new();
    for command in stop_commands(&value, agent) {
        let Some(at) = command.find(&needle) else {
            continue;
        };
        let prefix = command[..at].trim().to_owned();
        if !prefix.is_empty() && !prefixes.contains(&prefix) {
            prefixes.push(prefix);
        }
    }
    prefixes
}

/// True when fscale may start `prefix` to check it: it is exactly one of
/// the commands fscale writes itself (`allowed`: the repo's default, and a
/// `--bin` given in this run), or a pin any release of firescale wrote
/// ([`pinned_version`]). Any other text in a hook file came from the
/// repo, and starting it would run the repo's text on this machine
/// (review of ADR 0064).
#[must_use]
pub fn may_start(prefix: &str, allowed: &[&str]) -> bool {
    allowed.contains(&prefix) || pinned_version(prefix).is_some()
}

/// What an agent hook file holds apart from what firescale writes in it
/// (its stop hook entry, `... hooks run --agent <name>`, and Cursor's
/// `"version": 1`), written the same way whatever the order of its keys
/// and its spacing. Claude Code gives the `env` of `.claude/settings.json`
/// to every command it starts, and any other hook (a `SessionStart` hook
/// can set that env) can change what verify sees, so this part is in every
/// step's key by value (ADR 0064, and its second review). Empty hook lists
/// count as absent. A file that is absent counts as `{}`; one that is not
/// a JSON object counts as its bytes.
#[must_use]
pub fn outside_hooks(agent: Agent, bytes: Option<&[u8]>) -> String {
    let Some(bytes) = bytes else {
        return "{}".to_owned();
    };
    let text = String::from_utf8_lossy(bytes);
    match serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')) {
        Ok(Value::Object(mut map)) => {
            if let Some(hooks) = map.get_mut("hooks") {
                set_aside_ours(hooks, agent);
                if hooks.as_object().is_some_and(Map::is_empty) {
                    map.remove("hooks");
                }
            }
            if agent == Agent::Cursor && map.get("version") == Some(&json!(1)) {
                map.remove("version");
            }
            canonical(&Value::Object(map))
        }
        _ => format!("bytes:{}", blake3::hash(bytes).to_hex()),
    }
}

/// The commands a release of firescale writes before
/// `hooks run --agent <name>`: the defaults of [`fscale_command`] now and
/// before 0.5.2 (`fscale`, `npx firescale`, `npx --no fscale`), and any pin
/// ([`pinned_version`]).
const WRITTEN: [&str; 9] = [
    "fscale",
    "npx firescale",
    "npx --no fscale",
    "npx --no -- fscale",
    "pnpm exec fscale",
    "pnpm --silent fscale",
    "yarn --silent fscale",
    "bun run --silent fscale",
    "npm run --silent fscale --",
];

/// True when `command` is exactly a stop hook command firescale writes:
/// one of its forms, then ` hooks run --agent <name>`, and nothing more.
/// A prefix of env settings, a `cd`, or more text after it is not (review
/// of the final diff): those change what verify sees.
fn written_by_firescale(command: &str, agent: Agent) -> bool {
    command
        .strip_suffix(&format!(" hooks run --agent {}", agent.name()))
        .is_some_and(|prefix| WRITTEN.contains(&prefix) || pinned_version(prefix).is_some())
}

/// Removes firescale's own stop hook entries from a `hooks` object, then
/// the groups and events left empty.
fn set_aside_ours(hooks: &mut Value, agent: Agent) {
    let ours = |entry: &Value| {
        entry
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(|command| written_by_firescale(command, agent))
    };
    let stop = match agent {
        Agent::Claude | Agent::Codex => "Stop",
        Agent::Cursor => "stop",
    };
    let Some(events) = hooks.as_object_mut() else {
        return;
    };
    if let Some(list) = events.get_mut(stop).and_then(Value::as_array_mut) {
        match agent {
            Agent::Claude | Agent::Codex => {
                for group in list.iter_mut() {
                    if let Some(inner) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                        inner.retain(|entry| !ours(entry));
                    }
                }
            }
            Agent::Cursor => list.retain(|entry| !ours(entry)),
        }
    }
    for list in events.values_mut() {
        if let Some(list) = list.as_array_mut() {
            list.retain(|group| {
                !group
                    .get("hooks")
                    .and_then(Value::as_array)
                    .is_some_and(Vec::is_empty)
            });
        }
    }
    events.retain(|_, list| !list.as_array().is_some_and(Vec::is_empty));
}

/// JSON with the keys of every object sorted, and no spacing.
fn canonical(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let fields: Vec<String> = keys
                .into_iter()
                .map(|key| format!("{}:{}", Value::from(key.as_str()), canonical(&map[key])))
                .collect();
            format!("{{{}}}", fields.join(","))
        }
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(canonical).collect();
            format!("[{}]", items.join(","))
        }
        other => other.to_string(),
    }
}

/// How long the check waits for `<command> --version --json`: long enough
/// for npx to download firescale once.
pub const START_CHECK_TIMEOUT: Duration = Duration::from_secs(120);

/// What the check asks a hook's command: `--version --json`, which only
/// firescale (0.5.2 and later) answers with `{"name":"firescale",...}`.
/// One start answers both whether it is firescale and which version.
#[must_use]
pub fn probe_line(prefix: &str) -> String {
    format!("{prefix} --version --json")
}

/// What starting [`probe_line`] gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartCheck {
    /// It exited 0 with firescale's answer: this version.
    Answered(String),
    /// It exited 0 with something else: the last lines it printed.
    NotFirescale(String),
    /// It exited with this code (`None`: stopped by a signal) and printed
    /// this.
    Failed { code: Option<i32>, output: String },
    /// It did not finish within [`START_CHECK_TIMEOUT`].
    TimedOut,
    /// The shell could not be started.
    NoShell(String),
}

/// A path as plain text to compare: canonical when it exists, with `/`,
/// lower case on Windows, no trailing `/`.
fn comparable(path: &Path) -> String {
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = path.to_string_lossy().replace('\\', "/");
    let text = text
        .strip_prefix("//?/")
        .unwrap_or(&text)
        .trim_end_matches('/')
        .to_owned();
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text
    }
}

/// True for a PATH folder that a package manager puts first for the run
/// of a script or of a downloaded package, which an agent started from a
/// terminal does not have: `node_modules/.bin` of the repo root or of a
/// folder above it (`npm run`, `pnpm`, `yarn`, `bun run`), a folder of
/// npx's, pnpm's, yarn's or bun's downloads (`_npx`, `dlx`, `dlx-*`,
/// `bunx-*`), and `node-gyp-bin`. Any other folder inside a
/// `node_modules` stays: pnpm/action-setup puts pnpm itself in
/// `~/setup-pnpm/node_modules/.bin` (CI of the second review).
fn launcher_folder(dir: &Path, root: &str) -> bool {
    let parts: Vec<String> = dir
        .components()
        .map(|part| part.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    if !parts.iter().any(|part| part == "node_modules") {
        return false;
    }
    if parts.iter().any(|part| {
        part == "_npx"
            || part == "dlx"
            || part.starts_with("dlx-")
            || part.starts_with("bunx-")
            || part == "node-gyp-bin"
    }) {
        return true;
    }
    if dir.file_name().is_none_or(|name| name != ".bin") {
        return false;
    }
    let Some(holder) = dir.parent().filter(|parent| {
        parent
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("node_modules"))
    }) else {
        return false;
    };
    let Some(base) = holder.parent() else {
        return false;
    };
    let base = comparable(base);
    // The repo root's, a folder's above it, or a workspace package's
    // inside it (`pnpm --filter web ...`; review of the final diff).
    root == base || root.starts_with(&format!("{base}/")) || base.starts_with(&format!("{root}/"))
}

/// The PATH the check gives a hook's command: this process's PATH without
/// the folders a package manager put there for its own run
/// ([`launcher_folder`]), and those folders (review of ADR 0064: run
/// through npx, `--bin fscale` found `node_modules/.bin/fscale`, which an
/// agent would not).
#[must_use]
pub fn check_path(root: &Path) -> (Option<std::ffi::OsString>, Vec<String>) {
    let Some(path) = std::env::var_os("PATH") else {
        return (None, Vec::new());
    };
    let root = comparable(root);
    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    for dir in std::env::split_paths(&path) {
        let shown = dir.to_string_lossy().into_owned();
        if launcher_folder(&dir, &root) {
            dropped.push(shown);
        } else {
            kept.push(dir);
        }
    }
    (std::env::join_paths(kept).ok(), dropped)
}

/// What the check says before it starts `prefix`: that it may download
/// firescale, or that it runs the repo's own fscale. Empty for anything
/// else.
#[must_use]
pub fn launch_note(prefix: &str) -> String {
    let words: Vec<&str> = prefix.split_whitespace().collect();
    let first = words.first().copied().unwrap_or_default();
    let next = words
        .iter()
        .skip(1)
        .copied()
        .find(|word| !word.starts_with('-'));
    let local = words.contains(&"--no");
    if first == "npx" && local && next.is_some_and(|word| word.starts_with("firescale@")) {
        return " It downloads nothing: npx runs firescale only when it is installed or already in npx's cache.".to_owned();
    }
    let tool = match (first, next) {
        ("npx" | "bunx", _) if !local => Some(first.to_owned()),
        ("npm", Some(sub @ ("exec" | "x"))) if !local => Some(format!("npm {sub}")),
        ("pnpm" | "yarn", Some("dlx")) | ("bun", Some("x")) => {
            Some(format!("{first} {}", next.unwrap_or_default()))
        }
        _ => None,
    };
    if let Some(tool) = tool {
        return format!(" {tool} may download firescale from the npm registry first.");
    }
    if (first == "npx" && local) || (first == "pnpm" && next == Some("exec")) {
        return " It runs the fscale installed in this repo's node_modules.".to_owned();
    }
    // `npm run fscale`, `pnpm --silent fscale`, `bun run --silent fscale`.
    let after_run = || {
        words
            .iter()
            .skip(2)
            .copied()
            .find(|word| !word.starts_with('-'))
    };
    let script = match (first, next) {
        ("npm" | "bun" | "pnpm" | "yarn", Some("run" | "run-script")) => after_run(),
        ("pnpm" | "yarn", next) => next,
        _ => None,
    };
    if script == Some("fscale") {
        return " It runs this repo's own `fscale` package script.".to_owned();
    }
    String::new()
}

/// Starts [`probe_line`] in the repo root through the platform shell
/// (`sh -c`, `cmd /D /S /C`), with stdin closed, as an agent starts its
/// hook command from the project root (ADR 0064): with `path` as the PATH
/// ([`check_path`]) and, for Claude Code's file, `CLAUDE_PROJECT_DIR` set
/// to the root. It runs what the hook would run: a package script of the
/// repo, or npx, which may download firescale. The caller says so before it
/// calls this, and starts only a command fscale writes ([`may_start`]).
#[must_use]
pub fn check_start(
    root: &Path,
    prefix: &str,
    path: Option<&std::ffi::OsStr>,
    claude: bool,
    downloads: bool,
) -> StartCheck {
    use std::sync::atomic::AtomicBool;

    use crate::process::{run_tree, Ending, Watch};

    let mut command = crate::doctor::shell(&probe_line(prefix));
    // On Windows `cmd` looks for a program in the current folder before
    // the PATH: a repo's own `npx.cmd` would run (review of the final diff).
    command
        .current_dir(root)
        .env("NoDefaultCurrentDirectoryInExePath", "1");
    if !downloads {
        // Nor may a package manager be downloaded through corepack.
        command
            .env("COREPACK_ENABLE_NETWORK", "0")
            .env("COREPACK_ENABLE_DOWNLOAD_PROMPT", "0");
    }
    if let Some(path) = path {
        command.env("PATH", path);
    }
    if claude {
        command.env("CLAUDE_PROJECT_DIR", root);
    }
    let never = AtomicBool::new(false);
    let watch = Watch {
        timeout: Some(START_CHECK_TIMEOUT),
        idle: None,
        stop: &never,
        heartbeat: Duration::ZERO,
        on_heartbeat: &|_, _, _| {},
        command: "",
        on_output: &|_| false,
        on_spawn: &|_| {},
    };
    let finished = match run_tree(command, &watch) {
        Ok(finished) => finished,
        Err(error) => return StartCheck::NoShell(error.to_string()),
    };
    let stdout = String::from_utf8_lossy(&finished.stdout).into_owned();
    match finished.ending {
        Ending::Exited(Some(0)) => {
            let answer = stdout.lines().map(str::trim).find_map(|line| {
                let value: Value = serde_json::from_str(line).ok()?;
                (value.get("name")?.as_str()? == "firescale")
                    .then(|| value.get("version")?.as_str().map(str::to_owned))?
            });
            match answer {
                Some(version) => StartCheck::Answered(version),
                None => {
                    let shown: Vec<&str> = stdout
                        .lines()
                        .map(str::trim)
                        .filter(|line| !line.is_empty())
                        .collect();
                    StartCheck::NotFirescale(shown[shown.len().saturating_sub(3)..].join(" "))
                }
            }
        }
        Ending::TimedOut => StartCheck::TimedOut,
        Ending::Exited(code) => StartCheck::Failed {
            code,
            output: format!("{stdout}\n{}", String::from_utf8_lossy(&finished.stderr))
                .trim()
                .to_owned(),
        },
        Ending::Stopped | Ending::GaveUp => StartCheck::Failed {
            code: None,
            output: String::new(),
        },
    }
}

/// The check's result in words: `Ok` with the version firescale answered,
/// or `Err` with why the command cannot start, starting with the command.
///
/// # Errors
///
/// Why the command cannot start.
pub fn start_verdict(prefix: &str, check: &StartCheck) -> std::result::Result<String, String> {
    let line = probe_line(prefix);
    Err(match check {
        StartCheck::Answered(version) => return Ok(version.clone()),
        StartCheck::NotFirescale(answer) if answer.is_empty() => {
            format!("`{line}` printed nothing, where firescale prints its name and version.")
        }
        StartCheck::NotFirescale(answer) => format!(
            "`{line}` answered \"{answer}\", which is not firescale's answer (a line {{\"name\":\"firescale\",\"version\":...}}): another program, or a firescale older than 0.5.2."
        ),
        StartCheck::Failed { code, output } => {
            let code =
                code.map_or_else(|| "no exit code".to_owned(), |code| format!("code {code}"));
            let shown: Vec<&str> = output
                .lines()
                .filter(|line| !line.trim().is_empty())
                .collect();
            let tail = &shown[shown.len().saturating_sub(10)..];
            if tail.is_empty() {
                format!("`{line}` exited with {code} and printed nothing.")
            } else {
                format!(
                    "`{line}` exited with {code} and printed:\n{}",
                    tail.iter()
                        .map(|line| format!("  {}", line.trim_end()))
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            }
        }
        StartCheck::TimedOut => format!(
            "`{line}` did not answer within {} seconds.",
            START_CHECK_TIMEOUT.as_secs()
        ),
        StartCheck::NoShell(error) => {
            format!("the shell that starts `{line}` could not be started ({error}).")
        }
    })
}

/// How the agent hook files among the changed files reach
/// `verify --affected` (ADR 0064).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookFileChanges {
    /// Changed only inside their hook entries, and in no folder of the
    /// graph: they reach only the units whose folders hold the repo root.
    pub settled: Vec<&'static str>,
    /// Changed outside their hook entries (`env`, `permissions`, ...), or
    /// there is no base to compare with: they reach every project.
    pub outside: Vec<&'static str>,
}

/// Sorts the changed agent hook files ([`HookFileChanges`]). A hook file
/// that a node of the graph holds (a project that reads it by its path)
/// and that changed only inside its hook entries is in neither list: it is
/// a file of that node, as any other.
#[must_use]
pub fn hook_file_changes(
    root: &Path,
    base: Option<&str>,
    changed: &std::collections::BTreeSet<String>,
    graph: &crate::graph::Graph,
) -> HookFileChanges {
    let agents: Vec<Agent> = AGENTS
        .into_iter()
        .filter(|agent| changed.contains(agent.config_file()))
        .collect();
    let files: Vec<&'static str> = agents.iter().map(|agent| agent.config_file()).collect();
    let mut sorted = HookFileChanges::default();
    if files.is_empty() {
        return sorted;
    }
    let Some(commit) = crate::configkey::comparison_commit(root, base) else {
        sorted.outside = files;
        return sorted;
    };
    let at_base = crate::git::files_at(root, &commit, &files);
    for agent in agents {
        let file = agent.config_file();
        let head = fs::read(resolve(root, file)).ok();
        let was = at_base.get(file).map(Vec::as_slice);
        if outside_hooks(agent, head.as_deref()) != outside_hooks(agent, was) {
            sorted.outside.push(file);
            continue;
        }
        let held = graph.nodes.iter().any(|node| {
            node.path != "." && (file == node.path || file.starts_with(&format!("{}/", node.path)))
        });
        if !held {
            sorted.settled.push(file);
        }
    }
    sorted
}

/// Why fscale did not start a command it found in `file`, and what to do.
#[must_use]
pub fn not_started(agent: Agent, prefix: &str, writes: &str) -> String {
    if matches!(prefix, "fscale" | "npx firescale") {
        // The default of firescale 0.5.1 and older, which nothing rewrites:
        // `sync` moves only a pin (the owner's decision on ADR 0064).
        return format!(
            "it runs `{}`, the command firescale 0.5.1 and older wrote. It starts only where `{prefix}` is on the PATH, and fscale does not start it to check. To get `{}`, remove that stop hook entry from {} and run `fscale hooks install --agent {}`.",
            hook_command(prefix, agent),
            hook_command(writes, agent),
            agent.config_file(),
            agent.name()
        );
    }
    format!(
        "it runs `{}`, which is not the command fscale writes here (`{writes}`), and fscale starts no other. Read it in {}; if it is yours, check that it starts: run `{}` in the repo root.",
        hook_command(prefix, agent),
        agent.config_file(),
        probe_line(prefix)
    )
}

fn object_mut<'a>(
    value: &'a mut Value,
    file: &str,
    what: &str,
) -> Result<&'a mut Map<String, Value>> {
    value.as_object_mut().ok_or_else(|| {
        Error::user(format!(
            "{file}: {what} is not a JSON object, so the hook cannot be added. Fix the file by hand, then run this again."
        ))
    })
}

fn array_mut<'a>(value: &'a mut Value, file: &str, what: &str) -> Result<&'a mut Vec<Value>> {
    value.as_array_mut().ok_or_else(|| {
        Error::user(format!(
            "{file}: {what} is not a JSON list, so the hook cannot be added. Fix the file by hand, then run this again."
        ))
    })
}

/// The commands of the agent's stop hooks in a parsed config file. Only the
/// stop event counts: the same text under another event, or in a permission
/// list, is not a stop hook.
fn stop_commands(value: &Value, agent: Agent) -> Vec<&str> {
    let entries = |key: &str| {
        value
            .get("hooks")
            .and_then(|hooks| hooks.get(key))
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
    };
    fn command(entry: &Value) -> Option<&str> {
        entry.get("command").and_then(Value::as_str)
    }
    match agent {
        Agent::Claude | Agent::Codex => entries("Stop")
            .iter()
            .filter_map(|group| group.get("hooks").and_then(Value::as_array))
            .flatten()
            .filter_map(command)
            .collect(),
        Agent::Cursor => entries("stop").iter().filter_map(command).collect(),
    }
}

/// True when one of the agent's stop hooks is our hook command, whatever
/// binary prefix it was installed with.
fn mentions_hook(value: &Value, agent: Agent) -> bool {
    let needle = format!("hooks run --agent {}", agent.name());
    stop_commands(value, agent)
        .iter()
        .any(|command| command.contains(&needle))
}

/// Returns the object at `key`, creating it when it is missing or null.
fn child<'a>(map: &'a mut Map<String, Value>, key: &str, empty: Value) -> &'a mut Value {
    let slot = map.entry(key).or_insert(Value::Null);
    if slot.is_null() {
        *slot = empty;
    }
    slot
}

/// Claude Code's time limit for a command hook whose entry sets no
/// `timeout`, in seconds ("Defaults: 600 for `command`", code.claude.com,
/// hooks reference, read on 2026-11-18; ADR 0082).
pub const CLAUDE_DEFAULT_TIMEOUT: u64 = 600;

/// The `timeout` to write on Claude Code's stop hook when the planned
/// steps took `steps` in all when they last ran (ADR 0082): twice that, for
/// the rerun of a new failure and a step whose pass is not cached, plus
/// `base` for the base run of blame-aware verify, rounded up to whole
/// minutes, and never under Claude Code's own default.
#[must_use]
pub fn hook_timeout_for(steps: Duration, base: Duration) -> u64 {
    let steps = u64::try_from(steps.as_millis().div_ceil(1000)).unwrap_or(u64::MAX);
    let wanted = steps.saturating_mul(2).saturating_add(base.as_secs());
    wanted
        .div_ceil(60)
        .saturating_mul(60)
        .max(CLAUDE_DEFAULT_TIMEOUT)
}

/// What installing a hook did to its file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installed {
    /// The stop hook was added.
    Added,
    /// The stop hook was there; its `timeout` was added as a new line.
    TimeoutAdded,
    /// The stop hook was there, and the file is as it was.
    AlreadyThere,
}

/// What [`merge_hook_with`] found about the Claude Code entry's `timeout`
/// when it left the entry as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeptTimeout {
    /// Nothing to say: no limit was asked for, or it was written.
    #[default]
    None,
    /// The entry has a `timeout` of its own (seconds), which is kept: no
    /// value in the file is ever changed.
    Kept(u64),
    /// The entry has none, and it cannot go in as a new line (the entry is
    /// written on one line).
    CannotAdd,
}

/// Claude Code's stop hook entry that runs firescale's hook command, if
/// the parsed file has one: the first of them.
fn claude_entry(value: &Value) -> Option<&Value> {
    let needle = format!("hooks run --agent {}", Agent::Claude.name());
    value
        .get("hooks")?
        .get("Stop")?
        .as_array()?
        .iter()
        .filter_map(|group| group.get("hooks").and_then(Value::as_array))
        .flatten()
        .find(|entry| {
            entry
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(|command| command.contains(&needle))
        })
}

fn claude_entry_mut(value: &mut Value) -> Option<&mut Map<String, Value>> {
    let needle = format!("hooks run --agent {}", Agent::Claude.name());
    value
        .get_mut("hooks")?
        .get_mut("Stop")?
        .as_array_mut()?
        .iter_mut()
        .filter_map(|group| group.get_mut("hooks").and_then(Value::as_array_mut))
        .flatten()
        .find(|entry| {
            entry
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(|command| command.contains(&needle))
        })?
        .as_object_mut()
}

/// The time limit of the repo's Claude Code stop hook that runs firescale:
/// `None` when there is no such hook, `Some(None)` when its entry sets no
/// `timeout` (Claude Code's default applies), else the seconds it sets.
#[must_use]
pub fn claude_hook_timeout(root: &Path) -> Option<Option<u64>> {
    let text = fs::read_to_string(resolve(root, Agent::Claude.config_file())).ok()?;
    let value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    let entry = claude_entry(&value)?;
    Some(entry.get("timeout").and_then(Value::as_u64))
}

/// Adds the stop hook to an agent's config. A file that has something in it
/// is never re-serialised: the hook goes in as new lines and every line a
/// person wrote stays as it was (ADR 0027). When that cannot be done the
/// file is refused, with an error that says what to add by hand.
/// Returns the new file content, or `None` when the hook is already there.
pub fn merge_hook(existing: Option<&str>, agent: Agent, bin: &str) -> Result<Option<String>> {
    merge_hook_with(existing, agent, bin, None).map(|(text, _, _)| text)
}

/// [`merge_hook`], and for Claude Code a `timeout` of `timeout` seconds on
/// the entry (ADR 0082): on a new entry, and as a new line on an entry
/// firescale's hook already has without one. A `timeout` in the file is
/// never changed. Other agents take no `timeout`.
pub fn merge_hook_with(
    existing: Option<&str>,
    agent: Agent,
    bin: &str,
    timeout: Option<u64>,
) -> Result<(Option<String>, Installed, KeptTimeout)> {
    let timeout = timeout.filter(|_| agent == Agent::Claude);
    let file = agent.config_file();
    let parsed = existing
        .filter(|text| !text.trim_start_matches('\u{feff}').trim().is_empty())
        .and_then(|text| serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')).ok());
    if let (Some(text), Some(mut root), Some(seconds)) = (existing, parsed, timeout) {
        if mentions_hook(&root, agent) {
            let Some(entry) = claude_entry_mut(&mut root) else {
                return Ok((None, Installed::AlreadyThere, KeptTimeout::None));
            };
            if let Some(have) = entry.get("timeout") {
                let kept = have.as_u64().map_or(KeptTimeout::None, KeptTimeout::Kept);
                return Ok((None, Installed::AlreadyThere, kept));
            }
            entry.insert("timeout".to_owned(), json!(seconds));
            return Ok(match crate::jsonedit::insert_merge(text, &root) {
                Some(merged) => (Some(merged), Installed::TimeoutAdded, KeptTimeout::None),
                None => (None, Installed::AlreadyThere, KeptTimeout::CannotAdd),
            });
        }
    }
    Ok(match merge_new(existing, agent, bin, timeout, file)? {
        Some(text) => (Some(text), Installed::Added, KeptTimeout::None),
        None => (None, Installed::AlreadyThere, KeptTimeout::None),
    })
}

fn merge_new(
    existing: Option<&str>,
    agent: Agent,
    bin: &str,
    timeout: Option<u64>,
    file: &str,
) -> Result<Option<String>> {
    let existing = existing.filter(|text| !text.trim_start_matches('\u{feff}').trim().is_empty());
    let mut root: Value = match existing {
        None => json!({}),
        Some(text) => serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|error| {
            Error::user(format!(
                "{file} is not valid JSON ({error}), so the hook cannot be added. Fix the file, then run this again."
            ))
        })?,
    };
    if mentions_hook(&root, agent) {
        return Ok(None);
    }
    let command = hook_command(bin, agent);
    let top = object_mut(&mut root, file, "the top level")?;
    let mut also = "";
    let (list, entry) = match agent {
        Agent::Claude | Agent::Codex => {
            let mut hook = json!({ "type": "command", "command": command });
            if let (Some(seconds), Some(map)) = (timeout, hook.as_object_mut()) {
                map.insert("timeout".to_owned(), json!(seconds));
            }
            let entry = json!({ "hooks": [hook] });
            let hooks = object_mut(child(top, "hooks", json!({})), file, "\"hooks\"")?;
            let stop = child(hooks, "Stop", json!([]));
            array_mut(stop, file, "\"hooks.Stop\"")?.push(entry.clone());
            ("hooks.Stop", entry)
        }
        Agent::Cursor => {
            if !top.contains_key("version") {
                top.insert("version".to_owned(), json!(1));
                also = "\"version\": 1 at the top level and ";
            }
            let entry = json!({ "command": command });
            let hooks = object_mut(child(top, "hooks", json!({})), file, "\"hooks\"")?;
            let stop = child(hooks, "stop", json!([]));
            array_mut(stop, file, "\"hooks.stop\"")?.push(entry.clone());
            ("hooks.stop", entry)
        }
    };
    let Some(text) = existing else {
        let text = serde_json::to_string_pretty(&root).unwrap_or_default();
        return Ok(Some(format!("{text}\n")));
    };
    crate::jsonedit::insert_merge(text, &root).map(Some).ok_or_else(|| {
        Error::user(format!(
            "{file} was left as it is: the stop hook cannot go in as new lines, and firescale does not rewrite lines you wrote. The file, or the object or list the hook goes into, is written on one line or is null. Put its brackets on their own lines and run this again, or add it by hand. Add {also}this item to the list \"{list}\" by hand:\n\n{}\n\nThen run this again to check it.",
            serde_json::to_string_pretty(&entry).unwrap_or_default()
        ))
    })
}

fn resolve(root: &Path, file: &str) -> PathBuf {
    file.split('/')
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

/// Installs the hook for one agent. Returns true when the file was written,
/// false when the hook was already installed.
pub fn install_hook(root: &Path, agent: Agent, bin: &str) -> Result<bool> {
    install_hook_with(root, agent, bin, None).map(|(state, _)| state != Installed::AlreadyThere)
}

/// [`install_hook`] with a `timeout` for Claude Code's entry
/// ([`merge_hook_with`]).
pub fn install_hook_with(
    root: &Path,
    agent: Agent,
    bin: &str,
    timeout: Option<u64>,
) -> Result<(Installed, KeptTimeout)> {
    let file = agent.config_file();
    let path = resolve(root, file);
    let existing = match fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(Error::user(format!(
                "Could not read {file} ({error}). Make sure it is a readable file."
            )));
        }
    };
    let (content, state, kept) = merge_hook_with(existing.as_deref(), agent, bin, timeout)?;
    let Some(content) = content else {
        return Ok((state, kept));
    };
    let failed = |error: std::io::Error| {
        Error::user(format!(
            "Could not write {file} ({error}). Check the folder's permissions."
        ))
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(failed)?;
    }
    fs::write(&path, content).map_err(failed)?;
    Ok((state, kept))
}

/// True when the repo has our stop hook installed for this agent.
#[must_use]
pub fn hook_installed(root: &Path, agent: Agent) -> bool {
    fs::read_to_string(resolve(root, agent.config_file()))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')).ok())
        .is_some_and(|value| mentions_hook(&value, agent))
}

/// What the agent sent the hook on stdin, as far as the loop guard needs it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookInput {
    /// Identifies the agent session, so sessions do not share a counter.
    pub session: String,
    /// Whether the agent says it is already continuing because of a stop hook.
    /// `None` when the agent did not say.
    pub continuing: Option<bool>,
    /// Cursor's own count of how many times the stop hook has sent it back.
    pub loop_count: Option<u32>,
}

/// Reads the hook's stdin. Anything unreadable gives the defaults: a hook
/// must not fail just because an agent changed its payload.
#[must_use]
pub fn parse_hook_input(stdin: &str) -> HookInput {
    let value: Value = serde_json::from_str(stdin.trim()).unwrap_or(Value::Null);
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    HookInput {
        session: text("session_id")
            .or_else(|| text("conversation_id"))
            .unwrap_or_else(|| "default".to_owned()),
        continuing: value.get("stop_hook_active").and_then(Value::as_bool),
        loop_count: value
            .get("loop_count")
            .and_then(Value::as_u64)
            .and_then(|count| u32::try_from(count).ok()),
    }
}

/// What the hook tells the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Verify passed: the agent may stop.
    Allow,
    /// Verify failed: send the agent back, with this many blocks so far.
    Block { blocks: u32 },
    /// Verify still fails, but the cap is reached: let the agent stop.
    GiveUp { blocks: u32 },
}

fn state_file(root: &Path, agent: Agent, session: &str) -> PathBuf {
    let safe: String = session
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    root.join(".firescale")
        .join("hooks")
        .join(format!("{}-{safe}.json", agent.name()))
}

/// A count older than this is from an earlier piece of work and is ignored.
const STALE_AFTER: Duration = Duration::from_secs(60 * 60);

/// The stored count. Zero when there is none, when it cannot be read, or
/// when it is stale.
fn read_blocks(path: &Path) -> u32 {
    let stale = fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age > STALE_AFTER);
    if stale {
        return 0;
    }
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.get("blocks").and_then(Value::as_u64))
        .and_then(|blocks| u32::try_from(blocks).ok())
        .unwrap_or(0)
}

/// True when the path exists and is a symlink. The counter is never written
/// through one, so a repo cannot point it at a file outside `.firescale`.
fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

/// Stores the count. False when it could not be stored safely.
fn write_blocks(path: &Path, blocks: u32) -> bool {
    let Some(hooks_dir) = path.parent() else {
        return false;
    };
    let Some(dot_firescale) = hooks_dir.parent() else {
        return false;
    };
    if is_symlink(dot_firescale) || is_symlink(hooks_dir) || is_symlink(path) {
        return false;
    }
    if fs::create_dir_all(hooks_dir).is_err() {
        return false;
    }
    // The same self-ignoring folder the cache uses.
    let ignore = dot_firescale.join(".gitignore");
    if !is_symlink(&ignore) && !ignore.exists() {
        let _ = fs::write(
            ignore,
            "# Created by firescale. Nothing in this folder should be committed.\n*\n",
        );
    }
    fs::write(path, format!("{}\n", json!({ "blocks": blocks }))).is_ok()
}

/// The loop guard. Decides what to tell the agent and updates the counter.
///
/// - Verify passed: reset the counter, allow.
/// - The agent says this is a fresh stop attempt (`stop_hook_active: false`,
///   or a `loop_count` of 0): start a new count.
/// - Otherwise, including when the agent says nothing either way, each
///   failure adds one to the stored count. At `max_blocks` the agent is let
///   go and the counter is reset, so the next turn starts fresh.
/// - If the count cannot be stored, the agent is let go: a guard that cannot
///   count must not block forever.
pub fn decide(
    root: &Path,
    agent: Agent,
    input: &HookInput,
    verify_ok: bool,
    max_blocks: u32,
) -> Decision {
    let path = state_file(root, agent, &input.session);
    if verify_ok {
        let _ = fs::remove_file(&path);
        return Decision::Allow;
    }
    let fresh = input.continuing == Some(false) || input.loop_count == Some(0);
    let stored = if fresh { 0 } else { read_blocks(&path) };
    // Trust whichever count is higher: ours, or the agent's own.
    let previous = stored.max(input.loop_count.unwrap_or(0));
    if previous >= max_blocks {
        let _ = fs::remove_file(&path);
        return Decision::GiveUp { blocks: previous };
    }
    let blocks = previous + 1;
    if !write_blocks(&path, blocks) {
        return Decision::GiveUp { blocks: previous };
    }
    Decision::Block { blocks }
}

/// The cap from the environment, or the default.
#[must_use]
pub fn max_blocks_from_env() -> u32 {
    std::env::var(MAX_BLOCKS_ENV)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(DEFAULT_MAX_BLOCKS)
}

/// How a hook process answers its agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookReply {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: u8,
}

/// What the stop hook sends an agent with a block: the verify summary,
/// opened by a note when a step failed because the environment is not set
/// up. That note tells the agent not to change code for it (ADR 0022).
#[must_use]
pub fn block_summary(report: &crate::verify::VerifyReport) -> String {
    let summary = crate::verify::format_verify_human(report);
    let setup: Vec<&crate::verify::StepReport> = report
        .steps
        .iter()
        .filter(|step| step.status == crate::verify::StepStatus::SetupNeeded)
        .collect();
    if setup.is_empty() {
        return summary;
    }
    let fixes: Vec<String> = setup
        .iter()
        .filter_map(|step| {
            step.fix.as_ref().map(|fix| match &step.project {
                Some(project) if project != "." => format!("`{fix}` in {project}"),
                _ => format!("`{fix}`"),
            })
        })
        .collect();
    let next = if fixes.is_empty() {
        "Set up what the summary names, or tell the user it is needed.".to_owned()
    } else {
        format!(
            "Run {} to set it up, or tell the user it is needed.",
            fixes.join(", ")
        )
    };
    let failed = report
        .steps
        .iter()
        .filter(|step| step.status == crate::verify::StepStatus::Failed)
        .count();
    let also = if failed == 0 {
        String::new()
    } else {
        format!(
            " Separately, {} FAILED in the code: {} need{} a code fix as usual.",
            if failed == 1 { "1 step" } else { "other steps" },
            if failed == 1 { "it" } else { "they" },
            if failed == 1 { "s" } else { "" }
        )
    };
    format!(
        "SETUP NEEDED: {} could not run because the environment is not ready (a dependency or program is missing). That is not a problem in the code. Do not edit code, tests or config to get past it. {next}{also}\n\n{summary}",
        if setup.len() == 1 { "1 step" } else { "some steps" }
    )
}

/// Turns a decision into the output the agent expects.
///
/// Claude Code and Codex: exit code 2 with the message on stderr blocks the
/// stop. Cursor: a JSON object on stdout with `followup_message`.
#[must_use]
pub fn reply(agent: Agent, decision: &Decision, summary: &str, max_blocks: u32) -> HookReply {
    match decision {
        Decision::Allow => {
            HookReply { stdout: String::new(), stderr: String::new(), exit_code: 0 }
        }
        Decision::GiveUp { blocks } => HookReply {
            stdout: if agent == Agent::Cursor { "{}\n".to_owned() } else { String::new() },
            stderr: format!(
                "fscale: verify still fails after {blocks} attempts, so the agent is allowed to stop. Run `fscale verify` to see what is left.\n"
            ),
            exit_code: 0,
        },
        Decision::Block { blocks } => {
            let message = format!(
                "fscale verify failed, so the work is not done yet (attempt {blocks} of {max_blocks}).\n\n{summary}"
            );
            match agent {
                Agent::Claude | Agent::Codex => {
                    HookReply { stdout: String::new(), stderr: message, exit_code: 2 }
                }
                Agent::Cursor => HookReply {
                    stdout: format!("{}\n", json!({ "followup_message": message })),
                    stderr: String::new(),
                    exit_code: 0,
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Fixture, Track};

    fn input(session: &str, continuing: bool) -> HookInput {
        HookInput {
            session: session.to_owned(),
            continuing: Some(continuing),
            loop_count: None,
        }
    }

    #[test]
    fn a_new_file_gets_just_the_hook() {
        let claude = merge_hook(None, Agent::Claude, "fscale").unwrap().unwrap();
        assert_eq!(
            claude,
            "{\n  \"hooks\": {\n    \"Stop\": [\n      {\n        \"hooks\": [\n          {\n            \"type\": \"command\",\n            \"command\": \"fscale hooks run --agent claude\"\n          }\n        ]\n      }\n    ]\n  }\n}\n"
        );
        let cursor = merge_hook(Some("  "), Agent::Cursor, "fscale")
            .unwrap()
            .unwrap();
        assert!(cursor.contains("\"version\": 1"), "{cursor}");
        assert!(
            cursor.contains("\"command\": \"fscale hooks run --agent cursor\""),
            "{cursor}"
        );
    }

    #[test]
    fn the_hook_limit_is_twice_the_steps_and_the_base_run_in_whole_minutes() {
        let min = |m: u64| Duration::from_secs(m * 60);
        assert_eq!(hook_timeout_for(min(20), min(5)), 45 * 60);
        // Never under Claude Code's default.
        assert_eq!(
            hook_timeout_for(Duration::ZERO, min(5)),
            CLAUDE_DEFAULT_TIMEOUT
        );
        assert_eq!(hook_timeout_for(min(1), min(5)), CLAUDE_DEFAULT_TIMEOUT);
        // Rounded up: 2 x 10m 0.5s + 5m is 25m 1s, so 26m.
        assert_eq!(
            hook_timeout_for(min(10) + Duration::from_millis(500), min(5)),
            26 * 60
        );
        assert_eq!(hook_timeout_for(Duration::MAX, min(5)), u64::MAX);
    }

    #[test]
    fn only_claude_codes_entry_gets_a_timeout_and_a_timeout_in_the_file_is_kept() {
        let (claude, state, _) = merge_hook_with(None, Agent::Claude, "fscale", Some(900)).unwrap();
        assert_eq!(state, Installed::Added);
        let claude = claude.unwrap();
        assert!(claude.contains("\"timeout\": 900"), "{claude}");
        for agent in [Agent::Codex, Agent::Cursor] {
            let (text, _, _) = merge_hook_with(None, agent, "fscale", Some(900)).unwrap();
            assert!(!text.unwrap().contains("timeout"));
        }
        // Installed before without one: added as a new line, then left.
        let before = merge_hook(None, Agent::Claude, "fscale").unwrap().unwrap();
        let (after, state, kept) =
            merge_hook_with(Some(&before), Agent::Claude, "fscale", Some(900)).unwrap();
        assert_eq!((state, kept), (Installed::TimeoutAdded, KeptTimeout::None));
        let after = after.unwrap();
        assert!(every_line_is_kept(&before, &after), "{after}");
        assert_eq!(after.lines().count(), before.lines().count() + 1);
        let (again, state, kept) =
            merge_hook_with(Some(&after), Agent::Claude, "fscale", Some(1200)).unwrap();
        assert_eq!(
            (again, state, kept),
            (None, Installed::AlreadyThere, KeptTimeout::Kept(900))
        );
        // An entry on one line cannot take the new line.
        let one_line = "{\n  \"hooks\": {\n    \"Stop\": [\n      {\n        \"hooks\": [\n          { \"type\": \"command\", \"command\": \"fscale hooks run --agent claude\" }\n        ]\n      }\n    ]\n  }\n}\n";
        assert_eq!(
            merge_hook_with(Some(one_line), Agent::Claude, "fscale", Some(900)).unwrap(),
            (None, Installed::AlreadyThere, KeptTimeout::CannotAdd)
        );
        // With no limit asked for, an installed hook is left alone.
        assert_eq!(
            merge_hook_with(Some(&before), Agent::Claude, "fscale", None).unwrap(),
            (None, Installed::AlreadyThere, KeptTimeout::None)
        );
    }

    #[test]
    fn a_pin_firescale_wrote_is_its_own_command_whatever_the_version() {
        assert_eq!(pinned_version("npx --yes firescale@0.5.0"), Some("0.5.0"));
        assert_eq!(
            pinned_version("npx --yes firescale@0.6.0-rc.1"),
            Some("0.6.0-rc.1")
        );
        assert_eq!(pinned_version("npx --yes firescale@latest"), None);
        assert_eq!(pinned_version("npx --yes firescale@0.5.0 && echo"), None);
        assert_eq!(pinned_version("npx --yes firescale@0.5.0;echo"), None);
        assert_eq!(pinned_version("npx firescale@0.5.0"), None);
        assert!(may_start(
            "npx --yes firescale@0.5.0",
            &["pnpm --silent fscale"]
        ));
        assert!(!may_start("npx --yes firescale@0.5.0 &&", &[]));
    }

    #[test]
    fn only_the_entries_firescale_writes_are_set_aside() {
        let none = outside_hooks(Agent::Claude, None);
        let ours = |agent: Agent| {
            let command = format!(
                "npx --yes firescale@0.5.0 hooks run --agent {}",
                agent.name()
            );
            match agent {
                Agent::Cursor => {
                    json!({ "version": 1, "hooks": { "stop": [{ "command": command }] } })
                }
                _ => {
                    json!({ "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": command }] }] } })
                }
            }
        };
        for agent in AGENTS {
            let text = |value: &Value| serde_json::to_vec(value).unwrap();
            assert_eq!(
                outside_hooks(agent, Some(&text(&ours(agent)))),
                none,
                "{agent:?}"
            );
            // Another hook entry, of the stop event or of any other: what
            // it does reaches verify (a SessionStart hook can set its env).
            let mut other = ours(agent);
            let (stop, start) = match agent {
                Agent::Cursor => ("stop", "sessionStart"),
                _ => ("Stop", "SessionStart"),
            };
            let entry = match agent {
                Agent::Cursor => json!({ "command": "echo API_MODE=strict" }),
                _ => json!({ "hooks": [{ "type": "command", "command": "echo API_MODE=strict" }] }),
            };
            other["hooks"][start] = json!([entry.clone()]);
            assert_ne!(outside_hooks(agent, Some(&text(&other))), none, "{agent:?}");
            let mut beside = ours(agent);
            beside["hooks"][stop].as_array_mut().unwrap().push(entry);
            assert_ne!(
                outside_hooks(agent, Some(&text(&beside))),
                none,
                "{agent:?}"
            );
        }
    }

    #[test]
    fn the_command_is_the_one_the_repo_decides() {
        let facts = |manager: Option<&str>| crate::facts::RepoFacts {
            package_manager: manager.map(str::to_owned),
            ..crate::facts::RepoFacts::default()
        };
        let script = json!({ "scripts": { "fscale": "fscale" } });
        // No fscale of the repo's own: this version, pinned, so a release
        // does not change every teammate's hook (review of ADR 0064).
        let pinned = format!("npx --yes firescale@{}", crate::version());
        assert_eq!(fscale_command(&facts(None), None), pinned);
        assert_eq!(
            fscale_command(&facts(Some("pnpm")), Some(&json!({ "name": "app" }))),
            pinned
        );
        // firescale among the repo's dependencies: the one it installs.
        let installed = json!({ "devDependencies": { "firescale": "^0.5.0" } });
        for (manager, command) in [
            (Some("pnpm"), "pnpm exec fscale"),
            (Some("yarn"), "yarn --silent fscale"),
            (Some("bun"), "bun run --silent fscale"),
            // `--` ends npx's own options: with `npx --no fscale --version`,
            // npm 10 printed its own version (second review of ADR 0064).
            (None, "npx --no -- fscale"),
        ] {
            assert_eq!(fscale_command(&facts(manager), Some(&installed)), command);
        }
        assert_eq!(
            fscale_command(
                &facts(None),
                Some(&json!({ "dependencies": { "firescale": "0.5.1" } }))
            ),
            "npx --no -- fscale"
        );
        assert_eq!(
            fscale_command(&facts(Some("pnpm")), Some(&script)),
            "pnpm --silent fscale"
        );
        assert_eq!(
            fscale_command(&facts(None), Some(&script)),
            "npm run --silent fscale --"
        );
    }

    #[test]
    fn the_command_before_hooks_run_is_read_back_from_the_file() {
        let fixture = Fixture::new(
            &[(
                ".claude/settings.json",
                "{\n\"hooks\":{\"Stop\":[{\"hooks\":[{\"type\":\"command\",\"command\":\"echo bye\"},\n{\"type\":\"command\",\"command\":\"npx firescale  hooks run --agent claude\"},\n{\"type\":\"command\",\"command\":\"npx firescale hooks run --agent claude\"}]}]},\n\"permissions\":{\"allow\":[\"Bash(fscale hooks run --agent claude)\"]}}\n",
            )],
            Track::NoGit,
        )
        .unwrap();
        assert_eq!(
            installed_prefixes(fixture.root(), Agent::Claude),
            ["npx firescale"]
        );
        assert!(installed_prefixes(fixture.root(), Agent::Codex).is_empty());
        assert!(is_agent_hook_file(".codex/hooks.json"));
        assert!(!is_agent_hook_file("web/.codex/hooks.json"));
        assert!(!is_agent_hook_file(".claude/settings.local.json"));
    }

    #[test]
    fn existing_settings_and_hooks_are_kept() {
        let existing = r#"{
"permissions":{"allow":["Bash(ls)"]},"hooks":{"Stop":[
{"hooks":[{"type":"command","command":"echo bye"}]}],"PreToolUse":[]}}"#;
        let merged = merge_hook(Some(existing), Agent::Claude, "npx fscale")
            .unwrap()
            .unwrap();
        let value: Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(value["permissions"]["allow"][0], "Bash(ls)");
        // The new hook goes first in the list, where it needs no change to
        // an existing line (ADR 0027).
        assert_eq!(
            value["hooks"]["Stop"][0]["hooks"][0]["command"],
            "npx fscale hooks run --agent claude"
        );
        assert_eq!(value["hooks"]["Stop"][1]["hooks"][0]["command"], "echo bye");
        assert!(value["hooks"]["PreToolUse"].is_array());
    }

    #[test]
    fn installing_twice_changes_nothing_even_with_another_bin() {
        let first = merge_hook(None, Agent::Codex, "fscale").unwrap().unwrap();
        assert_eq!(
            merge_hook(Some(&first), Agent::Codex, "fscale").unwrap(),
            None
        );
        assert_eq!(
            merge_hook(Some(&first), Agent::Codex, "npx fscale").unwrap(),
            None
        );
        // Another agent's hook in the same shape does not count.
        assert!(merge_hook(Some(&first), Agent::Claude, "fscale")
            .unwrap()
            .is_some());
    }

    #[test]
    fn a_broken_or_odd_file_is_an_error_that_says_what_to_do() {
        let error = merge_hook(Some("{ nope"), Agent::Claude, "fscale").unwrap_err();
        assert!(error
            .message()
            .contains(".claude/settings.json is not valid JSON"));
        let error = merge_hook(Some(r#"{"hooks":[]}"#), Agent::Claude, "fscale").unwrap_err();
        assert!(
            error.message().contains("\"hooks\" is not a JSON object"),
            "{}",
            error.message()
        );
    }

    #[test]
    fn install_writes_the_file_and_reports_whether_it_changed() {
        let fixture = Fixture::new(&[], Track::NoGit).unwrap();
        for agent in AGENTS {
            assert!(!hook_installed(fixture.root(), agent));
            assert!(install_hook(fixture.root(), agent, "fscale").unwrap());
            assert!(!install_hook(fixture.root(), agent, "fscale").unwrap());
            assert!(hook_installed(fixture.root(), agent));
        }
        assert!(fixture
            .read(".codex/hooks.json")
            .unwrap()
            .contains("hooks run --agent codex"));
    }

    /// Every line of `before`, with its own line ending, is in `after`, in
    /// the same order.
    fn every_line_is_kept(before: &str, after: &str) -> bool {
        let mut lines = after.split_inclusive('\n');
        before
            .split_inclusive('\n')
            .all(|line| lines.any(|candidate| candidate == line))
    }

    /// Files people wrote, in layouts `serde_json` would not write.
    const A_PERSONS_FILES: [(&str, Agent, &str); 11] = [
        (
            "four spaces",
            Agent::Claude,
            "{\n    \"model\": \"opus\",\n    \"permissions\": {\n        \"deny\": [\n            \"Read(./.env)\"\n        ]\n    }\n}\n",
        ),
        (
            "tabs",
            Agent::Claude,
            "{\n\t\"model\": \"opus\",\n\t\"hooks\": {\n\t\t\"PreToolUse\": []\n\t}\n}\n",
        ),
        (
            "a number written 1e3, odd spacing and key order",
            Agent::Claude,
            "{\n  \"zeta\" :   1e3,\n   \"alpha\":[ 1,2 ],\n\n  \"hooks\" : {\n      \"PreToolUse\" : [ ]\n  }\n}\n",
        ),
        (
            "CRLF line endings",
            Agent::Claude,
            "{\r\n    \"model\": \"opus\",\r\n    \"hooks\": {\r\n        \"Stop\": [\r\n            { \"hooks\": [ { \"type\": \"command\", \"command\": \"echo bye\" } ] }\r\n        ]\r\n    }\r\n}\r\n",
        ),
        (
            "no newline at the end",
            Agent::Claude,
            "{\n    \"model\": \"opus\"\n}",
        ),
        (
            "a byte order mark",
            Agent::Claude,
            "\u{feff}{\n    \"model\": \"opus\"\n}\n",
        ),
        (
            "four spaces",
            Agent::Codex,
            "{\n    \"hooks\": {\n        \"SessionStart\": [\n            { \"hooks\": [ { \"type\": \"command\", \"command\": \"echo hi\" } ] }\n        ]\n    }\n}\n",
        ),
        (
            "tabs and CRLF, no newline at the end",
            Agent::Codex,
            "{\r\n\t\"hooks\": {\r\n\t\t\"Stop\": [\r\n\t\t\t{ \"hooks\": [ { \"type\": \"command\", \"command\": \"echo bye\" } ] }\r\n\t\t]\r\n\t}\r\n}",
        ),
        (
            "four spaces, a stop hook of their own",
            Agent::Cursor,
            "{\n    \"version\": 1,\n    \"hooks\": {\n        \"stop\": [\n            { \"command\": \"echo bye\" }\n        ]\n    }\n}\n",
        ),
        (
            "tabs, no version",
            Agent::Cursor,
            "{\n\t\"hooks\": {\n\t\t\"afterFileEdit\": [ { \"command\": \"fmt\" } ]\n\t}\n}\n",
        ),
        (
            "CRLF, a version written 1e0",
            Agent::Cursor,
            "{\r\n  \"version\" : 1e0,\r\n  \"hooks\" : {\r\n  }\r\n}\r\n",
        ),
    ];

    #[test]
    fn installing_into_a_persons_file_keeps_every_line_they_wrote() {
        for (layout, agent, before) in A_PERSONS_FILES {
            let file = agent.config_file();
            let fixture = Fixture::new(&[(file, before)], Track::NoGit).unwrap();
            assert!(
                install_hook(fixture.root(), agent, "npx firescale").unwrap(),
                "{file}, {layout}"
            );
            let after = fixture.read(file).unwrap();
            assert!(
                every_line_is_kept(before, &after),
                "{file}, {layout}: a line was changed.\nbefore: {before:?}\nafter:  {after:?}"
            );
            // The new lines use the file's own line ending.
            if before.contains("\r\n") {
                assert!(!after.replace("\r\n", "").contains('\n'), "{after:?}");
            } else {
                assert!(!after.contains('\r'), "{after:?}");
            }
            // The hook is in, and nothing a person had is gone.
            assert!(hook_installed(fixture.root(), agent), "{file}, {layout}");
            let parse = |text: &str| -> Value {
                serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap()
            };
            assert!(
                crate::jsonedit::holds(&parse(&after), &parse(before)),
                "{file}, {layout}: {after}"
            );
            // A second install changes nothing.
            assert!(!install_hook(fixture.root(), agent, "npx firescale").unwrap());
            assert_eq!(fixture.read(file).unwrap(), after, "{file}, {layout}");
        }
    }

    #[test]
    fn a_file_the_hook_cannot_go_into_as_new_lines_is_refused_and_left_alone() {
        let cases = [
            (
                "written on one line",
                Agent::Claude,
                "{\"permissions\":{\"deny\":[\"Read(./.env)\"]}}\n",
            ),
            (
                "an empty object on one line",
                Agent::Claude,
                "{\n  \"model\": \"opus\",\n  \"hooks\": {}\n}\n",
            ),
            (
                "a list on one line",
                Agent::Codex,
                "{\n  \"hooks\": {\n    \"Stop\": []\n  }\n}\n",
            ),
            (
                "written on one line",
                Agent::Cursor,
                "{\"version\":1,\"hooks\":{\"stop\":[{\"command\":\"echo bye\"}]}}",
            ),
        ];
        for (layout, agent, before) in cases {
            let file = agent.config_file();
            let fixture = Fixture::new(&[(file, before)], Track::NoGit).unwrap();
            let error = install_hook(fixture.root(), agent, "fscale").unwrap_err();
            assert_eq!(fixture.read(file).unwrap(), before, "{file}, {layout}");
            let message = error.message();
            // The problem, and the next step: what to add by hand.
            assert!(
                message.contains(&format!("{file} was left as it is")),
                "{message}"
            );
            assert!(message.contains("by hand"), "{message}");
            assert!(
                message.contains(&format!(
                    "\"command\": \"fscale hooks run --agent {}\"",
                    agent.name()
                )),
                "{message}"
            );
        }
    }

    #[test]
    fn a_value_of_another_type_or_a_comment_is_refused_and_left_alone() {
        let cases = [
            (
                "{\n  \"hooks\": {\n    \"Stop\": \"none\"\n  }\n}\n",
                "\"hooks.Stop\" is not a JSON list",
            ),
            (
                "{\n  \"hooks\": [\n  ]\n}\n",
                "\"hooks\" is not a JSON object",
            ),
            (
                "{\n  // my settings\n  \"model\": \"opus\"\n}\n",
                ".claude/settings.json is not valid JSON",
            ),
        ];
        for (before, problem) in cases {
            let fixture = Fixture::new(&[(".claude/settings.json", before)], Track::NoGit).unwrap();
            let error = install_hook(fixture.root(), Agent::Claude, "fscale").unwrap_err();
            assert!(error.message().contains(problem), "{}", error.message());
            assert_eq!(fixture.read(".claude/settings.json").unwrap(), before);
        }
    }

    #[test]
    fn hook_input_is_read_leniently() {
        let parsed = parse_hook_input(r#"{"session_id":"abc","stop_hook_active":true,"extra":1}"#);
        assert_eq!(
            parsed,
            HookInput {
                session: "abc".to_owned(),
                continuing: Some(true),
                loop_count: None
            }
        );
        let cursor = parse_hook_input(r#"{"conversation_id":"c1","loop_count":2}"#);
        assert_eq!(cursor.session, "c1");
        assert_eq!(cursor.loop_count, Some(2));
        assert_eq!(
            parse_hook_input("not json"),
            HookInput {
                session: "default".to_owned(),
                ..HookInput::default()
            }
        );
        assert_eq!(parse_hook_input("").session, "default");
    }

    #[test]
    fn the_loop_guard_blocks_up_to_the_cap_then_lets_the_agent_go() {
        let fixture = Fixture::new(&[], Track::NoGit).unwrap();
        let root = fixture.root();
        // First stop attempt of a turn.
        assert_eq!(
            decide(root, Agent::Claude, &input("s1", false), false, 3),
            Decision::Block { blocks: 1 }
        );
        // The agent tried again and verify still fails.
        assert_eq!(
            decide(root, Agent::Claude, &input("s1", true), false, 3),
            Decision::Block { blocks: 2 }
        );
        assert_eq!(
            decide(root, Agent::Claude, &input("s1", true), false, 3),
            Decision::Block { blocks: 3 }
        );
        // The cap is reached: stop blocking, and start fresh next time.
        assert_eq!(
            decide(root, Agent::Claude, &input("s1", true), false, 3),
            Decision::GiveUp { blocks: 3 }
        );
        assert_eq!(
            decide(root, Agent::Claude, &input("s1", true), false, 3),
            Decision::Block { blocks: 1 }
        );
    }

    #[test]
    fn passing_resets_the_count_and_sessions_are_separate() {
        let fixture = Fixture::new(&[], Track::NoGit).unwrap();
        let root = fixture.root();
        assert_eq!(
            decide(root, Agent::Codex, &input("a", false), false, 3),
            Decision::Block { blocks: 1 }
        );
        assert_eq!(
            decide(root, Agent::Codex, &input("a", true), false, 3),
            Decision::Block { blocks: 2 }
        );
        // Another session, and another agent, have their own counters.
        assert_eq!(
            decide(root, Agent::Codex, &input("b", true), false, 3),
            Decision::Block { blocks: 1 }
        );
        assert_eq!(
            decide(root, Agent::Claude, &input("a", true), false, 3),
            Decision::Block { blocks: 1 }
        );
        // Verify passes: allowed, and the count starts over.
        assert_eq!(
            decide(root, Agent::Codex, &input("a", true), true, 3),
            Decision::Allow
        );
        assert_eq!(
            decide(root, Agent::Codex, &input("a", true), false, 3),
            Decision::Block { blocks: 1 }
        );
        // A fresh stop attempt (not a continuation) also starts over.
        assert_eq!(
            decide(root, Agent::Codex, &input("a", false), false, 3),
            Decision::Block { blocks: 1 }
        );
        assert!(fixture
            .read(".firescale/.gitignore")
            .unwrap()
            .contains("\n*\n"));
    }

    #[test]
    fn the_agents_own_loop_count_is_honoured() {
        let fixture = Fixture::new(&[], Track::NoGit).unwrap();
        let from_cursor = HookInput {
            session: "c".to_owned(),
            continuing: None,
            loop_count: Some(3),
        };
        assert_eq!(
            decide(fixture.root(), Agent::Cursor, &from_cursor, false, 3),
            Decision::GiveUp { blocks: 3 }
        );
    }

    #[test]
    fn a_cap_of_zero_never_blocks_and_odd_session_ids_are_safe() {
        let fixture = Fixture::new(&[], Track::NoGit).unwrap();
        let odd = input("../../etc/passwd: *?", false);
        assert_eq!(
            decide(fixture.root(), Agent::Claude, &odd, false, 0),
            Decision::GiveUp { blocks: 0 }
        );
        assert_eq!(
            decide(fixture.root(), Agent::Claude, &odd, false, 3),
            Decision::Block { blocks: 1 }
        );
        let hooks_dir = fixture.root().join(".firescale").join("hooks");
        let names: Vec<String> = std::fs::read_dir(hooks_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["claude-______etc_passwd____.json"]);
    }

    #[test]
    fn the_cap_holds_when_the_agent_does_not_say_whether_it_is_continuing() {
        let fixture = Fixture::new(&[], Track::NoGit).unwrap();
        let silent = HookInput {
            session: "quiet".to_owned(),
            continuing: None,
            loop_count: None,
        };
        let decisions: Vec<Decision> = (0..5)
            .map(|_| decide(fixture.root(), Agent::Cursor, &silent, false, 3))
            .collect();
        assert_eq!(
            decisions,
            [
                Decision::Block { blocks: 1 },
                Decision::Block { blocks: 2 },
                Decision::Block { blocks: 3 },
                Decision::GiveUp { blocks: 3 },
                Decision::Block { blocks: 1 },
            ]
        );
    }

    #[test]
    fn a_count_that_cannot_be_stored_lets_the_agent_go() {
        // `.firescale` is a file, so the counter folder cannot be created.
        let fixture = Fixture::new(&[(".firescale", "in the way\n")], Track::NoGit).unwrap();
        assert_eq!(
            decide(fixture.root(), Agent::Claude, &input("s", true), false, 3),
            Decision::GiveUp { blocks: 0 }
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_counter_is_never_written_through_a_symlink() {
        let fixture = Fixture::new(&[("outside/keep.txt", "x\n")], Track::NoGit).unwrap();
        let root = fixture.root();
        std::fs::create_dir_all(root.join(".firescale")).unwrap();
        std::os::unix::fs::symlink(root.join("outside"), root.join(".firescale").join("hooks"))
            .unwrap();
        assert_eq!(
            decide(root, Agent::Claude, &input("s", true), false, 3),
            Decision::GiveUp { blocks: 0 }
        );
        let names: Vec<String> = std::fs::read_dir(root.join("outside"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["keep.txt"]);
    }

    #[test]
    fn only_a_stop_hook_counts_as_installed() {
        let elsewhere = r#"{
"permissions":{"allow":["Bash(fscale hooks run --agent claude)"]},"hooks":{
"PreToolUse":[{"hooks":[{"type":"command","command":"fscale hooks run --agent claude"}]}]}}"#;
        let merged = merge_hook(Some(elsewhere), Agent::Claude, "fscale")
            .unwrap()
            .expect("the stop hook is still missing, so it is added");
        let value: Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(
            value["hooks"]["Stop"][0]["hooks"][0]["command"],
            "fscale hooks run --agent claude"
        );
        assert!(value["hooks"]["PreToolUse"].is_array());
        // Cursor's event is lower-case `stop`; Claude's `Stop` shape is not Cursor's.
        assert!(merge_hook(Some(&merged), Agent::Cursor, "fscale")
            .unwrap()
            .is_some());
    }

    #[test]
    fn a_null_hooks_key_is_left_for_a_person() {
        // Putting the hook there would rewrite the line that says `null`.
        for existing in [
            r#"{"hooks":null,"model":"x"}"#,
            "{\n  \"hooks\": null,\n  \"model\": \"x\"\n}\n",
        ] {
            let error = merge_hook(Some(existing), Agent::Codex, "fscale").unwrap_err();
            assert!(
                error
                    .message()
                    .contains("Add this item to the list \"hooks.Stop\" by hand"),
                "{}",
                error.message()
            );
        }
    }

    #[test]
    fn replies_follow_each_agents_contract() {
        let block = Decision::Block { blocks: 1 };
        let claude = reply(Agent::Claude, &block, "verify: FAILED at test\n", 3);
        assert_eq!(claude.exit_code, 2);
        assert!(claude.stdout.is_empty());
        assert!(claude.stderr.contains("attempt 1 of 3"));
        assert!(claude.stderr.ends_with("verify: FAILED at test\n"));

        let cursor = reply(Agent::Cursor, &block, "summary", 3);
        assert_eq!(cursor.exit_code, 0);
        let value: Value = serde_json::from_str(&cursor.stdout).unwrap();
        assert!(value["followup_message"]
            .as_str()
            .unwrap()
            .ends_with("summary"));

        let allow = reply(Agent::Codex, &Decision::Allow, "", 3);
        assert_eq!(
            allow,
            HookReply {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0
            }
        );

        let give_up = reply(Agent::Cursor, &Decision::GiveUp { blocks: 3 }, "", 3);
        assert_eq!(give_up.exit_code, 0);
        assert_eq!(give_up.stdout, "{}\n");
        assert!(give_up.stderr.contains("allowed to stop"));
    }

    fn report_with(output: &str) -> crate::verify::VerifyReport {
        use crate::verify::{run_verify, CheckStep, Planned, ProjectPlan, VerifyPlan};
        let fixture = Fixture::new(&[("svc/main.py", "")], Track::NoGit).unwrap();
        let command = if cfg!(windows) {
            format!("echo {output} 1>&2 & exit /b 1")
        } else {
            format!("echo \"{output}\" >&2; exit 1")
        };
        let plan = VerifyPlan {
            runner: "projects".to_owned(),
            projects: vec![ProjectPlan {
                root: "svc".to_owned(),
                kind: Some("python".to_owned()),
                steps: vec![("test".to_owned(), Planned::Command(command))],
                install: Some("uv sync".to_owned()),
                python_from_path: false,
            }],
            ..VerifyPlan::default()
        };
        let check = CheckStep {
            ok: true,
            detail: "no rules".to_owned(),
            output: Vec::new(),
            duration_ms: 0,
            command: "fscale check".to_owned(),
            violations: Vec::new(),
        };
        run_verify(fixture.root(), "0.0.0", check, &plan).unwrap()
    }

    #[test]
    fn a_block_for_a_setup_problem_tells_the_agent_not_to_edit_code() {
        let report = report_with("ModuleNotFoundError: No module named 'jwt'");
        let summary = block_summary(&report);
        assert!(
            summary.starts_with(
                "SETUP NEEDED: 1 step could not run because the environment is not ready"
            ),
            "{summary}"
        );
        assert!(
            summary.contains("Do not edit code, tests or config to get past it."),
            "{summary}"
        );
        assert!(
            summary.contains("Run `uv sync` in svc to set it up"),
            "{summary}"
        );
        let block = reply(Agent::Claude, &Decision::Block { blocks: 1 }, &summary, 3);
        assert_eq!(block.exit_code, 2);
        assert!(block.stderr.contains("Do not edit code"));
    }

    #[test]
    fn a_block_with_setup_and_real_failures_says_the_failures_need_code_fixes() {
        use crate::verify::{run_verify, CheckStep, Planned, ProjectPlan, VerifyPlan};
        let fixture = Fixture::new(&[("a/x.txt", ""), ("b/x.txt", "")], Track::NoGit).unwrap();
        let say = |text: &str| {
            if cfg!(windows) {
                format!("echo {text} 1>&2 & exit /b 1")
            } else {
                format!("echo \"{text}\" >&2; exit 1")
            }
        };
        let unit = |root: &str, command: String| ProjectPlan {
            root: root.to_owned(),
            kind: Some("python".to_owned()),
            steps: vec![("test".to_owned(), Planned::Command(command))],
            install: Some("uv sync".to_owned()),
            python_from_path: false,
        };
        let plan = VerifyPlan {
            runner: "projects".to_owned(),
            projects: vec![
                unit("a", say("No module named jwt")),
                unit("b", say("assert 1 == 2")),
            ],
            ..VerifyPlan::default()
        };
        let check = CheckStep {
            ok: true,
            detail: "no rules".to_owned(),
            output: Vec::new(),
            duration_ms: 0,
            command: "fscale check".to_owned(),
            violations: Vec::new(),
        };
        let report = run_verify(fixture.root(), "0.0.0", check, &plan).unwrap();
        let summary = block_summary(&report);
        assert!(summary.starts_with("SETUP NEEDED: 1 step"), "{summary}");
        assert!(
            summary
                .contains("Separately, 1 step FAILED in the code: it needs a code fix as usual."),
            "{summary}"
        );
    }

    #[test]
    fn a_block_for_a_real_failure_is_the_plain_summary() {
        let report = report_with("assert 1 == 2");
        assert_eq!(
            block_summary(&report),
            crate::verify::format_verify_human(&report)
        );
        assert!(!block_summary(&report).contains("SETUP"));
    }

    #[test]
    fn older_compares_versions_as_numbers() {
        assert!(older("0.5.0", "0.5.2"));
        assert!(older("0.9.9", "0.10.0"));
        assert!(older("0.5.2-rc.1", "0.5.2"));
        assert!(!older("0.5.2", "0.5.2"));
        assert!(!older("99.0.0", "0.5.2"));
        assert!(!older("0.5", "0.5.2"));
        assert!(!older("latest", "0.5.2"));
    }

    #[test]
    fn sync_moves_only_an_older_pin_firescale_wrote() {
        let old = "npx --yes firescale@0.0.1 hooks run --agent claude";
        let text = format!(
            "{{\n  \"hooks\": {{\n    \"Stop\": [{{ \"hooks\": [\n      {{ \"type\": \"command\", \"command\": \"{old}\" }},\n      {{ \"type\": \"command\", \"command\": \"{old} --budget 60\" }},\n      {{ \"type\": \"command\", \"command\": \"npx firescale hooks run --agent claude\" }}\n    ] }}]\n  }}\n}}\n"
        );
        let (after, moved) = move_pins(&text, Agent::Claude).unwrap();
        let new = hook_command(&pinned_npx(), Agent::Claude);
        assert_eq!(
            after,
            text.replacen(&format!("\"{old}\""), &format!("\"{new}\""), 1)
        );
        assert_eq!(
            moved,
            [MovedPin {
                from: "0.0.1".to_owned(),
                to: crate::version().to_owned()
            }]
        );
        // Nothing more to move once it is this version.
        assert_eq!(move_pins(&after, Agent::Claude), None);
    }

    #[test]
    fn sync_leaves_a_newer_pin_another_agent_s_and_a_command_found_elsewhere() {
        let newer = r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"npx --yes firescale@99.0.0 hooks run --agent claude"}]}]}}"#;
        assert_eq!(move_pins(newer, Agent::Claude), None);
        let codex_in_claude = r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"npx --yes firescale@0.0.1 hooks run --agent codex"}]}]}}"#;
        assert_eq!(move_pins(codex_in_claude, Agent::Claude), None);
        // The same text in another hook: a person decides.
        let elsewhere = r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"npx --yes firescale@0.0.1 hooks run --agent claude"}]}],"Stop":[{"hooks":[{"type":"command","command":"npx --yes firescale@0.0.1 hooks run --agent claude"}]}]}}"#;
        assert_eq!(move_pins(elsewhere, Agent::Claude), None);
        let cursor = r#"{"version":1,"hooks":{"stop":[{"command":"npx --yes firescale@0.0.1 hooks run --agent cursor"}]}}"#;
        let (after, _) = move_pins(cursor, Agent::Cursor).unwrap();
        assert!(after.contains(&format!(
            "firescale@{} hooks run --agent cursor",
            crate::version()
        )));
        assert_eq!(move_pins("{ nope", Agent::Cursor), None);
    }

    // Review of the final diff, finding 2: any stop entry that held the
    // text `hooks run --agent <name>` was set aside, an env prefix included.
    #[test]
    fn only_firescale_s_own_stop_command_is_set_aside() {
        let file = |command: &str| {
            serde_json::to_vec(&json!({
                "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": command }] }] }
            }))
            .unwrap()
        };
        let outside = |command: &str| outside_hooks(Agent::Claude, Some(&file(command)));
        let pinned = hook_command(&pinned_npx(), Agent::Claude);
        assert_eq!(outside(&pinned), "{}");
        assert_eq!(outside("fscale hooks run --agent claude"), "{}");
        assert_eq!(outside("npx --no -- fscale hooks run --agent claude"), "{}");
        for (before, after) in [
            (
                format!("API_MODE=lax {pinned}"),
                format!("API_MODE=strict {pinned}"),
            ),
            (format!("cd web && {pinned}"), format!("cd api && {pinned}")),
            (
                "npx --yes firescale@0.5.0 hooks run --agent claude --budget 60".to_owned(),
                "npx --yes firescale@0.5.0 hooks run --agent claude --budget 90".to_owned(),
            ),
        ] {
            assert_ne!(outside(&before), outside(&after), "{before}");
        }
    }

    // Review of the final diff, finding 4: a workspace package's
    // `node_modules/.bin`, which `pnpm --filter web fscale ...` puts first.
    #[test]
    fn a_node_modules_bin_inside_the_repo_is_a_launcher_folder() {
        let fixture = Fixture::new(&[("web/package.json", "{}\n")], Track::NoGit).unwrap();
        let root = comparable(fixture.root());
        let bin = fixture.root().join("web").join("node_modules").join(".bin");
        assert!(launcher_folder(&bin, &root));
        assert!(launcher_folder(
            &fixture.root().join("node_modules").join(".bin"),
            &root
        ));
        // A tool's own folder elsewhere stays.
        assert!(!launcher_folder(
            Path::new("/opt/setup-pnpm/node_modules/.bin"),
            &root
        ));
    }

    // Review of the final diff, finding 5: the pin's text was matched as
    // re-written JSON, so the same text elsewhere could be edited instead.
    #[test]
    fn sync_moves_nothing_when_the_stop_command_is_written_another_way() {
        // In the stop hook, the @ of the pin is written as a JSON escape.
        let escaped = format!("firescale{}u00400.0.1", char::from(92u8));
        let text = format!(
            r#"{{"note":"npx --yes firescale@0.0.1 hooks run --agent claude","hooks":{{"Stop":[{{"hooks":[{{"type":"command","command":"npx --yes {escaped} hooks run --agent claude"}}]}}]}}}}"#
        );
        assert_eq!(move_pins(&text, Agent::Claude), None);
    }
}
