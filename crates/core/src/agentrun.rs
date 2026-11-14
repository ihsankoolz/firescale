//! The guarded agent runner's pieces (M27, ADR 0047): the policy an agent
//! runs under, the adapter interface, the post-run diff guard, the worktree
//! a run works in, and the ledger. `fscale fix --agent` puts them together
//! (`crates/cli/src/agentfix.rs`).
//!
//! An adapter is a program that speaks a small JSON protocol: the runner
//! starts `<adapter> run <request.json>` and reads an [`AgentResult`], one
//! JSON line, from its standard output. M27 ships one adapter, `fake`, a stand-in
//! binary from the testkit, so tests and CI never call a real model; M28
//! adds the Claude Code and Codex adapters behind the same interface.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::git::run_git;
use crate::process::{run_tree, Ending, Watch, STOP};
use crate::{Error, Result};

/// Paths no agent may read or write: secrets (SPEC §7, guardrail 4). The
/// `.env` files are named one by one, so `.env.example` (a template the
/// env-example rule edits) stays editable; [`is_secret`] judges any other
/// `.env.*` a secret in the pre-flight and the diff guard.
pub const SECRET_PATHS: [&str; 16] = [
    ".env",
    "**/.env",
    ".env.local",
    "**/.env.local",
    ".env.*.local",
    "**/.env.*.local",
    ".env.development",
    ".env.production",
    ".env.staging",
    ".env.test",
    "*.pem",
    "**/*.pem",
    "*.key",
    "**/*.key",
    "**/secrets/**",
    "**/id_rsa*",
];

/// True for a path that holds secrets: any `.env`, `.env.*` or `.envrc`
/// file that is not a template, the [`SECRET_PATHS`], and anything in a
/// `secrets` folder. Letter case and `\\` separators do not matter (macOS
/// and Windows file systems ignore case).
#[must_use]
pub fn is_secret(path: &str) -> bool {
    let path = normalise(path);
    let name = path.rsplit('/').next().unwrap_or(&path);
    let env_file = name == ".env" || name.starts_with(".env.") || name == ".envrc";
    let named: Vec<String> = SECRET_PATHS.iter().map(|p| (*p).to_owned()).collect();
    let in_secrets = path.split('/').rev().skip(1).any(|part| part == "secrets");
    !is_example(&path) && (env_file || in_secrets || matches_any(&path, &named))
}

/// A path as the guards compare it: `/` separators, lower case.
#[must_use]
pub fn normalise(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

/// Paths a fix may never change (guardrail 7): the secrets, git itself,
/// firescale's config, and the agents' own settings and hooks.
pub const PROTECTED_PATHS: [&str; 10] = [
    "**/.git/**",
    "**/.gitmodules",
    "firescale.config.json",
    "firescale.config.ts",
    "**/.claude/**",
    "**/.codex/**",
    "**/.cursor/**",
    "**/.husky/**",
    "**/.firescale/**",
    "**/.mcp.json",
];

/// Commands an agent may never run, whatever the repo allows: the runner
/// never pushes, tags or publishes.
pub const DENIED_COMMANDS: [&str; 7] = [
    "git push",
    "git tag",
    "git remote",
    "npm publish",
    "pnpm publish",
    "yarn publish",
    "cargo publish",
];

/// What an agent may do in one run (guardrails 4, 5 and 6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Policy {
    /// Tools the agent may use without asking: reading, searching and
    /// editing files. No shell unless the repo allows it and the runner's
    /// own list does too.
    pub allowed_tools: Vec<String>,
    /// Commands it may never run.
    pub denied_commands: Vec<String>,
    /// Paths it may neither read nor write.
    pub denied_paths: Vec<String>,
    /// Most agent turns.
    pub max_turns: u32,
    /// Most spend in US dollars, where the agent reports it.
    pub max_cost_usd: f64,
    /// Wall-clock limit, after which the runner stops the agent.
    pub timeout_secs: u64,
    /// Always false: the repo's own agent hooks, settings and MCP servers
    /// are not loaded during a fix run (guardrail 6).
    pub load_project_config: bool,
}

/// The tools of the runner's own strict list.
pub const RUNNER_TOOLS: [&str; 5] = ["Read", "Glob", "Grep", "Edit", "Write"];

/// What the repo's own agent settings take away (the runner never runs
/// wider than they allow).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoLimits {
    /// Tool rules the repo denies (`.claude/settings.json`
    /// `permissions.deny`), kept as written.
    pub denied: Vec<String>,
    /// Tools the repo denies outright (a rule with no scope, `"Edit"`).
    pub denied_tools: BTreeSet<String>,
    /// The repo's settings let no agent edit files (Codex `sandbox_mode =
    /// "read-only"`, Claude Code `defaultMode: "plan"`).
    pub read_only: Option<String>,
    /// Paths the repo's settings deny editing (`Edit(src/**)`,
    /// `Write(./docs/**)`): the diff guard protects them.
    pub denied_paths: Vec<String>,
}

/// Reads what the repo's agent settings deny: `.claude/settings.json` (and
/// `settings.local.json`) `permissions.deny` and `defaultMode`, and
/// `.codex/config.toml` `sandbox_mode`.
#[must_use]
pub fn repo_limits(root: &Path) -> RepoLimits {
    let mut limits = RepoLimits::default();
    for file in [".claude/settings.json", ".claude/settings.local.json"] {
        let Ok(text) = fs::read_to_string(root.join(file)) else {
            continue;
        };
        let Ok(value) =
            serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}'))
        else {
            continue;
        };
        // `ask` needs a person, and a run has none: it counts as `deny`.
        for list in ["/permissions/deny", "/permissions/ask"] {
            let Some(rules) = value.pointer(list).and_then(serde_json::Value::as_array) else {
                continue;
            };
            for rule in rules.iter().filter_map(serde_json::Value::as_str) {
                limits.denied.push(rule.to_owned());
                match rule.split_once('(') {
                    None => {
                        limits.denied_tools.insert(rule.to_owned());
                        // In Claude Code, Edit stands for every tool that
                        // edits files.
                        if rule == "Edit" {
                            limits.denied_tools.insert("Write".to_owned());
                        }
                    }
                    Some(("Edit" | "Write" | "Read", rest)) => {
                        let glob = rest
                            .trim_end_matches(')')
                            .trim_start_matches("./")
                            .trim_start_matches('/');
                        if !glob.is_empty() {
                            limits.denied_paths.push(glob.to_owned());
                        }
                    }
                    Some(_) => {}
                }
            }
        }
        if value
            .pointer("/permissions/defaultMode")
            .and_then(serde_json::Value::as_str)
            == Some("plan")
        {
            limits.read_only = Some(format!("{file} sets permissions.defaultMode to \"plan\""));
        }
    }
    if let Ok(text) = fs::read_to_string(root.join(".codex").join("config.toml")) {
        let read_only = text.lines().any(|line| {
            let line = line.trim();
            line.starts_with("sandbox_mode") && line.contains("read-only")
        });
        if read_only {
            limits.read_only =
                Some(".codex/config.toml sets sandbox_mode to \"read-only\"".to_owned());
        }
    }
    limits
}

impl Policy {
    /// The runner's strict policy, narrowed by the repo's own limits: a tool
    /// the repo denies is not allowed, and every rule the repo denies is
    /// denied too.
    #[must_use]
    pub fn strict(
        limits: &RepoLimits,
        protected: &[String],
        max_turns: u32,
        max_cost_usd: f64,
        timeout_secs: u64,
    ) -> Self {
        let mut denied_paths: Vec<String> = SECRET_PATHS.iter().map(|p| (*p).to_owned()).collect();
        denied_paths.extend(protected.iter().cloned());
        denied_paths.extend(limits.denied_paths.iter().cloned());
        denied_paths.extend(PROTECTED_PATHS.iter().map(|p| (*p).to_owned()));
        let mut denied_commands: Vec<String> =
            DENIED_COMMANDS.iter().map(|c| (*c).to_owned()).collect();
        denied_commands.extend(limits.denied.iter().cloned());
        Self {
            allowed_tools: RUNNER_TOOLS
                .iter()
                .filter(|tool| !limits.denied_tools.contains(**tool))
                .map(|tool| (*tool).to_owned())
                .collect(),
            denied_commands,
            denied_paths,
            max_turns,
            max_cost_usd,
            timeout_secs,
            load_project_config: false,
        }
    }
}

/// What the runner hands an adapter (guardrail 3: one rule's skill and its
/// diagnostics, nothing else).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    /// The worktree the agent works in: never the person's checkout.
    pub cwd: String,
    pub rule: String,
    /// The rule's SKILL.md.
    pub skill: String,
    /// The rule's diagnostics, as `fscale check --json` gives them.
    pub diagnostics: serde_json::Value,
    /// The instruction, built from the two above.
    pub prompt: String,
    pub policy: Policy,
}

/// What an adapter reports back.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentResult {
    /// The agent finished without an error of its own.
    pub ok: bool,
    /// Spend in US dollars, when the agent reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turns: Option<u32>,
    /// A short reason when `ok` is false, shown to the person and never
    /// written to the ledger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Set by the runner, never read from the agent: the time limit stopped
    /// it.
    #[serde(skip)]
    pub timed_out: bool,
}

/// An agent CLI behind the JSON protocol.
pub trait Adapter {
    /// The name in the ledger and in commit trailers.
    fn name(&self) -> &str;
    /// Guardrail 1: the agent's CLI is installed and authenticated. `Err`
    /// says what is missing and how to fix it.
    ///
    /// # Errors
    ///
    /// When it is not installed or not signed in.
    fn preflight(&self) -> std::result::Result<String, String>;
    /// Runs the agent on `request`. The runner stops it after the policy's
    /// time limit.
    ///
    /// # Errors
    ///
    /// When the agent cannot be started or its answer cannot be read.
    fn run(&self, request: &Request) -> Result<AgentResult>;
}

/// An adapter that is a program speaking the protocol: the request on its
/// standard input, the result on its standard output.
pub struct ProgramAdapter {
    pub name: String,
    pub program: PathBuf,
    pub args: Vec<String>,
}

/// The environment every agent process gets on top of the person's: git
/// pushes go nowhere (`url.<nowhere>.pushInsteadOf` set empty matches every
/// URL), git never asks for a password, and firescale's own hooks stay out.
#[must_use]
pub fn agent_env() -> Vec<(&'static str, String)> {
    vec![
        ("GIT_CONFIG_COUNT", "2".to_owned()),
        (
            "GIT_CONFIG_KEY_0",
            "url.file:///firescale-runner-never-pushes/.pushInsteadOf".to_owned(),
        ),
        ("GIT_CONFIG_VALUE_0", String::new()),
        // A remote with its own `pushurl` ignores `pushInsteadOf`: no
        // transport at all is allowed, so no push (and no fetch) can go out.
        ("GIT_CONFIG_KEY_1", "protocol.allow".to_owned()),
        ("GIT_CONFIG_VALUE_1", "never".to_owned()),
        ("GIT_SSH_COMMAND", "false".to_owned()),
        ("GIT_TERMINAL_PROMPT", "0".to_owned()),
        ("FSCALE_AGENT_RUN", "1".to_owned()),
    ]
}

impl Adapter for ProgramAdapter {
    fn name(&self) -> &str {
        &self.name
    }

    fn preflight(&self) -> std::result::Result<String, String> {
        let installed = Command::new(&self.program)
            .args(&self.args)
            .arg("--version")
            .stdin(Stdio::null())
            .output()
            .map_err(|_| format!("the {} agent is not installed ({} was not found). Install it, or pick another with --agent.", self.name, self.program.display()))?;
        if !installed.status.success() {
            return Err(format!(
                "the {} agent did not answer --version. Check its install.",
                self.name
            ));
        }
        let version = String::from_utf8_lossy(&installed.stdout).trim().to_owned();
        let auth = Command::new(&self.program)
            .args(&self.args)
            .args(["auth", "status"])
            .stdin(Stdio::null())
            .output()
            .map_err(|error| {
                format!(
                    "the {} agent's sign-in could not be checked ({error}).",
                    self.name
                )
            })?;
        if !auth.status.success() {
            return Err(format!(
                "the {} agent is not signed in. Sign in with its own CLI first.",
                self.name
            ));
        }
        Ok(version)
    }

    fn run(&self, request: &Request) -> Result<AgentResult> {
        // The request goes in a file outside the repo, removed afterwards;
        // the agent runs as a process tree that a time limit or Ctrl+C
        // stops as a whole.
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let file = std::env::temp_dir().join(format!(
            "firescale-request-{}-{unique}.json",
            std::process::id()
        ));
        let body =
            serde_json::to_vec(request).map_err(|error| Error::internal(error.to_string()))?;
        fs::write(&file, body).map_err(|error| {
            Error::user(format!("could not write the agent's request ({error})."))
        })?;
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .arg("run")
            .arg(&file)
            .current_dir(&request.cwd)
            .envs(agent_env());
        let quiet = |_: Duration, _: Option<&str>, _: Option<u8>| {};
        let watch = Watch {
            timeout: Some(Duration::from_secs(request.policy.timeout_secs.max(1))),
            idle: None,
            stop: &STOP,
            heartbeat: Duration::from_secs(3600),
            on_heartbeat: &quiet,
            command: "",
            on_output: &|_| false,
            on_spawn: &|_| {},
        };
        let finished = run_tree(command, &watch);
        let _ = fs::remove_file(&file);
        let finished = finished.map_err(|error| {
            Error::user(format!(
                "the {} agent could not be started ({error}).",
                self.name
            ))
        })?;
        let mut result = match finished.ending {
            Ending::TimedOut => {
                return Ok(AgentResult {
                    ok: false,
                    error: Some(format!(
                        "stopped after the time limit of {} s",
                        request.policy.timeout_secs
                    )),
                    timed_out: true,
                    ..AgentResult::default()
                })
            }
            Ending::Stopped | Ending::GaveUp => {
                return Ok(AgentResult {
                    ok: false,
                    error: Some("stopped".to_owned()),
                    ..AgentResult::default()
                })
            }
            Ending::Exited(code) => {
                let text = String::from_utf8_lossy(&finished.stdout);
                let last = text
                    .lines()
                    .rev()
                    .find(|line| line.trim_start().starts_with('{'))
                    .unwrap_or("");
                let mut result: AgentResult = serde_json::from_str(last).unwrap_or(AgentResult {
                    ok: false,
                    error: Some("the agent's answer could not be read".to_owned()),
                    ..AgentResult::default()
                });
                if code != Some(0) {
                    result.ok = false;
                    result.error.get_or_insert_with(|| {
                        format!(
                            "the agent exited with {}",
                            code.map_or("a signal".to_owned(), |c| c.to_string())
                        )
                    });
                }
                result
            }
        };
        // Keep only a short reason: never the agent's own words at length.
        if let Some(error) = &mut result.error {
            *error = error.chars().take(160).collect();
        }
        Ok(result)
    }
}

/// True for a repo-relative path that one of `globs` names (`*` stays in
/// a folder, `**` crosses them).
#[must_use]
pub fn matches_any(path: &str, globs: &[String]) -> bool {
    let path = normalise(path);
    let globs: Vec<String> = globs.iter().map(|glob| normalise(glob)).collect();
    let set = globs.iter().filter_map(|glob| {
        globset::GlobBuilder::new(glob)
            .literal_separator(true)
            .build()
            .ok()
    });
    let mut builder = globset::GlobSetBuilder::new();
    for glob in set {
        builder.add(glob);
    }
    builder.build().is_ok_and(|set| set.is_match(&path))
}

/// Why a run's diff is rejected (guardrail 7), or `None`.
#[must_use]
pub fn diff_guard(
    changed: &[Change],
    allowed: &[String],
    extra_protected: &[String],
) -> Option<String> {
    let mut protected: Vec<String> = PROTECTED_PATHS.iter().map(|p| (*p).to_owned()).collect();
    protected.extend(extra_protected.iter().cloned());
    for change in changed {
        if is_secret(&change.path) || matches_any(&change.path, &protected) {
            return Some(format!("it changed the protected path {}", change.path));
        }
        // A link or a submodule could point anywhere, inside the repo or out.
        if change.mode == "120000" || change.mode == "160000" {
            return Some(format!("it added a link or a submodule at {}", change.path));
        }
    }
    changed
        .iter()
        .find(|change| !matches_any(&change.path, allowed))
        .map(|change| {
            format!(
                "it changed {}, outside what the rule may change",
                change.path
            )
        })
}

/// One path a run changed, against the commit it started from: deleted,
/// added or changed, with the file mode it has now (`000000` when gone).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    pub mode: String,
}

/// What `git diff --cached --raw --no-renames -z <base>` lists: every path,
/// so a rename shows both its sides.
#[must_use]
pub fn staged_changes(worktree: &Path, base: &str) -> Vec<Change> {
    let Ok(out) = run_git(
        worktree,
        &[
            "diff",
            "--cached",
            "--raw",
            "--no-renames",
            "-z",
            base,
            "--",
        ],
    ) else {
        return Vec::new();
    };
    let parts: Vec<&str> = out.stdout.split('\0').collect();
    let mut changes = Vec::new();
    let mut index = 0;
    while index + 1 < parts.len() {
        let meta = parts[index];
        let path = parts[index + 1];
        // `:100644 100644 <a> <b> M`: the second mode is the new one.
        let mode = meta.split_whitespace().nth(1).unwrap_or("").to_owned();
        if meta.starts_with(':') && !path.is_empty() {
            changes.push(Change {
                path: path.replace('\\', "/"),
                mode,
            });
        }
        index += 2;
    }
    changes
}

/// A template of a secret file, not a secret: `.env.example`,
/// `.env.sample`, `.env.template`, `config.dist`.
#[must_use]
pub fn is_example(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    [".example", ".sample", ".template", ".dist"]
        .iter()
        .any(|end| name.ends_with(end))
}

/// One line of `.firescale/ledger.jsonl` (guardrail 10). Never the prompt,
/// a file's contents or anything the agent wrote: only what is listed here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerEntry {
    pub schema: u32,
    /// RFC 3339, UTC, seconds.
    pub time: String,
    pub rule: String,
    pub agent: String,
    /// `pass` (committed) or `fail` (rolled back).
    pub result: String,
    /// Why a run failed, as one of a fixed set of codes (a refusal before
    /// the run writes no line): `agent-error`, `over-budget`, `timeout`, `diff-guard`, `no-change`,
    /// `still-failing`, `tamper`, `verify`, `outside-worktree`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub files_changed: usize,
    pub lines_changed: usize,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turns: Option<u32>,
}

pub const LEDGER: &str = ".firescale/ledger.jsonl";

/// Appends one entry to the repo's ledger.
///
/// # Errors
///
/// When the file cannot be written.
pub fn append_ledger(root: &Path, entry: &LedgerEntry) -> Result<()> {
    let path = root.join(".firescale").join("ledger.jsonl");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            Error::user(format!("could not create {} ({error}).", parent.display()))
        })?;
    }
    let mut line =
        serde_json::to_string(entry).map_err(|error| Error::internal(error.to_string()))?;
    line.push('\n');
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| Error::user(format!("could not write {} ({error}).", path.display())))?;
    file.write_all(line.as_bytes())
        .map_err(|error| Error::user(format!("could not write {} ({error}).", path.display())))
}

/// The current time, RFC 3339 in UTC to the second, without a date crate.
#[must_use]
pub fn now_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = secs / 86_400;
    let rest = secs % 86_400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let day_of_year = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
}

/// A worktree on a new branch, for one run (guardrail 2). Dropping it rolls
/// the run back (the worktree and its branch go), on every path out of a
/// run, unless [`RunWorktree::keep_branch`] was called.
pub struct RunWorktree {
    pub root: PathBuf,
    pub path: PathBuf,
    pub branch: String,
    kept: std::cell::Cell<bool>,
}

impl RunWorktree {
    /// Adds a worktree of `HEAD` on the new branch `branch`, in the
    /// system's temporary folder.
    ///
    /// # Errors
    ///
    /// When git cannot add it.
    pub fn add(root: &Path, branch: &str) -> Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "firescale-run-{}-{}",
            std::process::id(),
            branch.replace(['/', '\\'], "-")
        ));
        let target = path.to_string_lossy().into_owned();
        let out = run_git(
            root,
            &["worktree", "add", "--quiet", "-b", branch, &target, "HEAD"],
        )?;
        if out.code != 0 {
            return Err(Error::user(format!(
                "could not create a worktree on the branch {branch}: {}",
                out.stderr.trim()
            )));
        }
        Ok(Self {
            root: root.to_path_buf(),
            path,
            branch: branch.to_owned(),
            kept: std::cell::Cell::new(false),
        })
    }

    /// Removes the worktree, even one the agent locked, and what is left of
    /// its folder.
    fn remove_worktree(&self) {
        let target = self.path.to_string_lossy().into_owned();
        let _ = run_git(&self.root, &["worktree", "unlock", &target]);
        let _ = run_git(
            &self.root,
            &["worktree", "remove", "--force", "--force", &target],
        );
        let _ = fs::remove_dir_all(&self.path);
        let _ = run_git(&self.root, &["worktree", "prune"]);
    }

    /// Removes the worktree and keeps its branch with the run's commit.
    pub fn keep_branch(&self) {
        self.kept.set(true);
        self.remove_worktree();
    }
}

impl Drop for RunWorktree {
    fn drop(&mut self) {
        if !self.kept.get() {
            self.remove_worktree();
            let _ = run_git(&self.root, &["branch", "-D", &self.branch]);
        }
    }
}

/// What a run may not change in the person's repo, outside its own
/// worktree: refs (branches, tags, remotes), the shared git config, the git
/// hooks, and the working tree and `HEAD` of the checkout. Taken before a
/// run and compared after it: the worktree shares the repo's refs, config
/// and hooks, so an agent could otherwise reach them from inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoState {
    refs: std::collections::BTreeMap<String, String>,
    config: Vec<u8>,
    hooks: std::collections::BTreeMap<String, Vec<u8>>,
    status: Vec<String>,
    head: String,
    git_dir: PathBuf,
}

fn common_dir(root: &Path) -> Option<PathBuf> {
    let out = run_git(
        root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .ok()?;
    (out.code == 0).then(|| PathBuf::from(out.stdout.trim()))
}

impl RepoState {
    /// The state of the repo at `root` now.
    #[must_use]
    pub fn capture(root: &Path) -> Self {
        let git_dir = common_dir(root).unwrap_or_else(|| root.join(".git"));
        let lines = |args: &[&str]| -> Vec<String> {
            run_git(root, args)
                .ok()
                .map(|out| {
                    out.stdout
                        .lines()
                        .map(str::to_owned)
                        .filter(|l| !l.is_empty())
                        .collect()
                })
                .unwrap_or_default()
        };
        let refs = lines(&["for-each-ref", "--format=%(refname) %(objectname)"])
            .into_iter()
            .filter_map(|line| {
                line.split_once(' ')
                    .map(|(r, o)| (r.to_owned(), o.to_owned()))
            })
            .collect();
        let hooks = fs::read_dir(git_dir.join("hooks"))
            .map(|entries| {
                entries
                    .filter_map(std::result::Result::ok)
                    .filter_map(|entry| {
                        let name = entry.file_name().to_string_lossy().into_owned();
                        fs::read(entry.path()).ok().map(|bytes| (name, bytes))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            refs,
            config: fs::read(git_dir.join("config")).unwrap_or_default(),
            hooks,
            status: lines(&[
                "status",
                "--porcelain",
                "--untracked-files=all",
                "--",
                ".",
                ":(exclude).firescale",
            ]),
            head: lines(&["rev-parse", "HEAD"]).join(""),
            git_dir,
        }
    }

    /// What changed since `self`, leaving out the run's own branch: one
    /// line per change.
    #[must_use]
    pub fn changes(&self, after: &Self, own_branch: &str) -> Vec<String> {
        let own = format!("refs/heads/{own_branch}");
        let mut out = Vec::new();
        let names: BTreeSet<&String> = self.refs.keys().chain(after.refs.keys()).collect();
        for name in names.into_iter().filter(|name| **name != own) {
            match (self.refs.get(name), after.refs.get(name)) {
                (None, Some(_)) => out.push(format!("created {name}")),
                (Some(_), None) => out.push(format!("deleted {name}")),
                (Some(a), Some(b)) if a != b => out.push(format!("moved {name}")),
                _ => {}
            }
        }
        if self.config != after.config {
            out.push("changed the repo's git config".to_owned());
        }
        if self.hooks != after.hooks {
            out.push("changed the repo's git hooks".to_owned());
        }
        if self.head != after.head {
            out.push("moved your HEAD".to_owned());
        }
        if self.status != after.status {
            out.push("changed files in your checkout".to_owned());
        }
        out
    }

    /// Puts back the refs, the git config and the hooks as they were (the
    /// checkout's files are left for the person: they started clean, and
    /// `git status` shows what changed).
    pub fn restore(&self, root: &Path, after: &Self, own_branch: &str) {
        let own = format!("refs/heads/{own_branch}");
        for (name, object) in &after.refs {
            if *name != own && !self.refs.contains_key(name) {
                let _ = run_git(root, &["update-ref", "-d", name]);
            } else if let Some(before) = self.refs.get(name).filter(|before| *before != object) {
                let _ = run_git(root, &["update-ref", name, before]);
            }
        }
        for (name, object) in &self.refs {
            if !after.refs.contains_key(name) {
                let _ = run_git(root, &["update-ref", name, object]);
            }
        }
        if self.config != after.config {
            let _ = fs::write(self.git_dir.join("config"), &self.config);
        }
        if self.hooks != after.hooks {
            let dir = self.git_dir.join("hooks");
            for name in after
                .hooks
                .keys()
                .filter(|name| !self.hooks.contains_key(*name))
            {
                let _ = fs::remove_file(dir.join(name));
            }
            for (name, bytes) in &self.hooks {
                if after.hooks.get(name) != Some(bytes) {
                    let _ = fs::write(dir.join(name), bytes);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_policy_never_allows_more_than_the_repo() {
        let limits = RepoLimits {
            denied: vec!["Bash(curl *)".to_owned(), "Write".to_owned()],
            denied_tools: ["Write".to_owned()].into_iter().collect(),
            read_only: None,
            denied_paths: vec!["src/**".to_owned()],
        };
        let policy = Policy::strict(&limits, &[], 10, 1.0, 60);
        assert!(policy.denied_paths.contains(&"src/**".to_owned()));
        assert!(!policy.allowed_tools.contains(&"Write".to_owned()));
        assert!(policy.allowed_tools.contains(&"Edit".to_owned()));
        assert!(policy.denied_commands.contains(&"Bash(curl *)".to_owned()));
        assert!(policy.denied_commands.contains(&"git push".to_owned()));
        assert!(policy.denied_paths.contains(&".env".to_owned()));
        assert!(
            is_secret(".env.production")
                && is_secret("api/.env.staging.local")
                && is_secret("certs/a.pem")
        );
        assert!(!is_secret(".env.example") && !is_secret("src/env.ts"));
        assert!(!policy.load_project_config);
    }

    #[test]
    fn the_diff_guard_rejects_protected_and_unrelated_paths() {
        let change = |path: &str| {
            vec![Change {
                path: path.to_owned(),
                mode: "100644".to_owned(),
            }]
        };
        let allowed = vec!["**/.env.example".to_owned(), "src/**".to_owned()];
        let all = vec!["**".to_owned()];
        assert_eq!(diff_guard(&change(".env.example"), &allowed, &[]), None);
        for path in [
            ".env",
            "api/.env.production",
            ".ENV",
            ".envrc",
            "SECRETS/x",
            "certs/a.PEM",
            ".CLAUDE/settings.json",
            "packages/a/.claude/x.json",
            "packages/a/.mcp.json",
            "sub/.git/config",
            "firescale.config.json",
        ] {
            assert!(
                diff_guard(&change(path), &all, &[]).is_some_and(|why| why.contains("protected")),
                "{path}"
            );
        }
        assert!(
            diff_guard(&change("infra/main.tf"), &all, &["infra\\**".to_owned()])
                .is_some_and(|why| why.contains("protected"))
        );
        assert!(diff_guard(&change("README.md"), &allowed, &[])
            .is_some_and(|why| why.contains("outside")));
        let link = vec![Change {
            path: "src/hook.js".to_owned(),
            mode: "120000".to_owned(),
        }];
        assert!(diff_guard(&link, &all, &[]).is_some_and(|why| why.contains("link")));
    }

    #[test]
    fn dates_are_utc() {
        let now = now_utc();
        assert_eq!(now.len(), 20);
        assert!(now.ends_with('Z'));
        assert!(now.starts_with("20"));
    }
}
