//! The `agent-ready` rule pack: what a repo needs so that a coding agent can
//! work in it and prove its work. Opt-in: add "agent-ready" to `packs`.

use std::fs;
use std::io::ErrorKind;

use firescale_core::rule::{BuiltinPack, RuleContext};

#[path = "agents-md/mod.rs"]
pub mod agents_md;
#[path = "commands-documented/mod.rs"]
pub mod commands_documented;
#[path = "single-test-command/mod.rs"]
pub mod single_test_command;
#[path = "verify-hook/mod.rs"]
pub mod verify_hook;

/// The instructions file agents read, at the repo root.
pub(crate) const AGENTS_MD: &str = "AGENTS.md";

/// What is at `AGENTS.md` in the repo root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgentsMd {
    Missing,
    /// Something is there, but it cannot be read as a file (a folder, say).
    Unreadable,
    /// The content, without a byte order mark. Bytes that are not UTF-8 are
    /// replaced, so a file in another encoding never aborts the run.
    Text(String),
}

pub(crate) fn read_agents_md(ctx: &RuleContext) -> AgentsMd {
    match fs::read(ctx.root().join(AGENTS_MD)) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            AgentsMd::Text(text.trim_start_matches('\u{feff}').to_owned())
        }
        Err(error) if error.kind() == ErrorKind::NotFound => AgentsMd::Missing,
        Err(_) => AgentsMd::Unreadable,
    }
}

#[must_use]
pub fn pack() -> BuiltinPack {
    BuiltinPack {
        name: "agent-ready",
        rules: vec![
            Box::new(agents_md::AgentsMdExists),
            Box::new(commands_documented::CommandsDocumented),
            Box::new(single_test_command::SingleTestCommand),
            Box::new(verify_hook::VerifyHook),
        ],
    }
}
