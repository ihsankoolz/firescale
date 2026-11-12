//! Agent hooks: installing them, and deciding whether an agent may stop.
//!
//! `fscale hooks install` writes each agent's hook config so that the agent
//! calls `fscale hooks run --agent <name>` when it is about to stop. That
//! command runs verify and, if it fails, tells the agent to keep working. A
//! counter caps how many times in a row one session can be sent back.
//!
//! Formats (checked on 2026-11-11, see ADR 0014):
//! - Claude Code: `.claude/settings.json`, a `Stop` hook. Confirmed from docs.
//! - Codex CLI: `.codex/hooks.json`, same shape. Confirmed from its source.
//! - Cursor: `.cursor/hooks.json`, a `stop` hook. NOT confirmed: its docs
//!   could not be reached when this was written.

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

/// The command an agent's hook runs.
#[must_use]
pub fn hook_command(bin: &str, agent: Agent) -> String {
    format!("{bin} hooks run --agent {}", agent.name())
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

/// Adds the stop hook to an agent's config, keeping everything else in it.
/// Returns the new file content, or `None` when the hook is already there.
pub fn merge_hook(existing: Option<&str>, agent: Agent, bin: &str) -> Result<Option<String>> {
    let file = agent.config_file();
    let mut root: Value = match existing.map(|text| text.trim_start_matches('\u{feff}')) {
        None => json!({}),
        Some(text) if text.trim().is_empty() => json!({}),
        Some(text) => serde_json::from_str(text).map_err(|error| {
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
    match agent {
        Agent::Claude | Agent::Codex => {
            let hooks = object_mut(child(top, "hooks", json!({})), file, "\"hooks\"")?;
            let stop = child(hooks, "Stop", json!([]));
            array_mut(stop, file, "\"hooks.Stop\"")?
                .push(json!({ "hooks": [{ "type": "command", "command": command }] }));
        }
        Agent::Cursor => {
            top.entry("version").or_insert_with(|| json!(1));
            let hooks = object_mut(child(top, "hooks", json!({})), file, "\"hooks\"")?;
            let stop = child(hooks, "stop", json!([]));
            array_mut(stop, file, "\"hooks.stop\"")?.push(json!({ "command": command }));
        }
    }
    let text = serde_json::to_string_pretty(&root).unwrap_or_default();
    Ok(Some(format!("{text}\n")))
}

fn resolve(root: &Path, file: &str) -> PathBuf {
    file.split('/')
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

/// Installs the hook for one agent. Returns true when the file was written,
/// false when the hook was already installed.
pub fn install_hook(root: &Path, agent: Agent, bin: &str) -> Result<bool> {
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
    let Some(content) = merge_hook(existing.as_deref(), agent, bin)? else {
        return Ok(false);
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
    Ok(true)
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
    fn existing_settings_and_hooks_are_kept() {
        let existing = r#"{"permissions":{"allow":["Bash(ls)"]},"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo bye"}]}],"PreToolUse":[]}}"#;
        let merged = merge_hook(Some(existing), Agent::Claude, "npx fscale")
            .unwrap()
            .unwrap();
        let value: Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(value["permissions"]["allow"][0], "Bash(ls)");
        assert_eq!(value["hooks"]["Stop"][0]["hooks"][0]["command"], "echo bye");
        assert_eq!(
            value["hooks"]["Stop"][1]["hooks"][0]["command"],
            "npx fscale hooks run --agent claude"
        );
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
        let elsewhere = r#"{"permissions":{"allow":["Bash(fscale hooks run --agent claude)"]},"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"fscale hooks run --agent claude"}]}]}}"#;
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
    fn a_null_hooks_key_is_treated_as_missing() {
        let merged = merge_hook(
            Some(r#"{"hooks":null,"model":"x"}"#),
            Agent::Codex,
            "fscale",
        )
        .unwrap()
        .unwrap();
        let value: Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(value["model"], "x");
        assert_eq!(
            value["hooks"]["Stop"][0]["hooks"][0]["command"],
            "fscale hooks run --agent codex"
        );
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
}
