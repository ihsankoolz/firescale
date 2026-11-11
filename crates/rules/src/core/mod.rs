//! The `core` rule pack (SPEC §6).

use firescale_core::rule::BuiltinPack;

#[path = "ci-exists/mod.rs"]
pub mod ci_exists;
#[path = "env-example/mod.rs"]
pub mod env_example;
#[path = "gitignore-basics/mod.rs"]
pub mod gitignore_basics;
#[path = "no-committed-env/mod.rs"]
pub mod no_committed_env;
#[path = "no-secrets-in-source/mod.rs"]
pub mod no_secrets_in_source;
#[path = "readme-setup/mod.rs"]
pub mod readme_setup;
#[path = "scripts-present/mod.rs"]
pub mod scripts_present;
#[path = "single-lockfile/mod.rs"]
pub mod single_lockfile;

#[must_use]
pub fn pack() -> BuiltinPack {
    BuiltinPack {
        name: "core",
        rules: vec![
            Box::new(gitignore_basics::GitignoreBasics),
            Box::new(single_lockfile::SingleLockfile),
            Box::new(ci_exists::CiExists),
            Box::new(readme_setup::ReadmeSetup),
            Box::new(env_example::EnvExample),
            Box::new(scripts_present::ScriptsPresent),
            Box::new(no_secrets_in_source::NoSecretsInSource),
            Box::new(no_committed_env::NoCommittedEnv),
        ],
    }
}
