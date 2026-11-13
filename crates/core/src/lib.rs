//! Engine for firescale.
//!
//! This crate holds everything the `fscale` binary needs that is not a rule:
//! the diagnostics model, config loading, git and file access, the rule
//! trait, report formatting and the bridge to Node for TypeScript rules.

pub mod activity;
pub mod affected;
pub mod cache;
pub mod check;
pub mod config;
pub mod diff;
pub mod doctor;
pub mod error;
pub mod explain;
pub mod facts;
pub mod fixloop;
pub mod git;
pub mod graph;
pub mod hooks;
pub mod imports;
pub mod jsonedit;
pub mod model;
pub mod node;
pub mod process;
pub mod progress;
pub mod references;
pub mod report;
pub mod rule;
pub mod runs;
pub mod sarif;
pub mod scratch;
pub mod select;
pub mod services;
pub mod setup;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod verify;
pub mod walk;

pub use error::{Error, Result};

/// The firescale version, taken from the Cargo workspace version.
#[must_use]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::version;

    #[test]
    fn version_is_semver_like() {
        let parts: Vec<&str> = version().split('.').collect();
        assert_eq!(
            parts.len(),
            3,
            "expected MAJOR.MINOR.PATCH, got {}",
            version()
        );
        for part in parts {
            assert!(
                part.chars().all(|c| c.is_ascii_digit()),
                "non-numeric part in {}",
                version()
            );
        }
    }
}
