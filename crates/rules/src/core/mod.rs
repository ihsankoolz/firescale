//! The `core` rule pack (SPEC §6).

use firescale_core::rule::BuiltinPack;

#[path = "no-committed-env/mod.rs"]
pub mod no_committed_env;

#[must_use]
pub fn pack() -> BuiltinPack {
    BuiltinPack {
        name: "core",
        rules: vec![Box::new(no_committed_env::NoCommittedEnv)],
    }
}
