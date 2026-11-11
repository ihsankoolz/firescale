//! Built-in rule packs. Every rule here is compiled into the `fscale` binary.

use firescale_core::rule::BuiltinPack;

#[path = "agent-ready/mod.rs"]
pub mod agent_ready;
pub mod core;

/// Every built-in pack, in the order they are listed to users.
#[must_use]
pub fn builtin_packs() -> Vec<BuiltinPack> {
    vec![core::pack(), agent_ready::pack()]
}
