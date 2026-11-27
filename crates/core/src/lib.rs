//! Engine for firescale.
//!
//! This crate holds everything the `fscale` binary needs that is not a rule:
//! the diagnostics model, config loading, git and file access, the rule
//! trait, report formatting and the bridge to Node for TypeScript rules.

pub mod activity;
pub mod affected;
#[cfg(test)]
mod affected_covers_key;
pub mod agentrun;
pub mod blame;
pub mod cache;
pub mod check;
pub mod cmake;
pub mod config;
pub mod configkey;
pub mod diff;
pub mod doctor;
pub mod ecosystems;
pub mod error;
pub mod explain;
pub mod facts;
pub mod fixloop;
pub mod git;
pub mod graph;
pub mod hooks;
pub mod imports;
pub mod jsonedit;
pub mod ledger;
pub mod lintsource;
pub mod model;
pub mod node;
pub mod oldtests;
pub mod ownfolder;
pub mod pertest;
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
pub mod sharedcache;
pub mod stepcache;
pub mod steptimes;
pub mod tamper;
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

    /// A release version: `MAJOR.MINOR.PATCH` with an optional `-<pre>`
    /// part of ASCII letters, digits, `.` and `-`: the rule of
    /// `scripts/release-registry.mjs` (`VERSION`). A pre-release such as
    /// `0.6.0-rc.1` is a release.
    fn is_release_version(text: &str) -> bool {
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let parts: Vec<&str> = core.split('.').collect();
        let core_ok = parts.len() == 3
            && parts
                .iter()
                .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
        let pre_ok = pre.is_none_or(|pre| {
            !pre.is_empty()
                && pre
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        });
        core_ok && pre_ok
    }

    #[test]
    fn version_is_semver_like() {
        assert!(
            is_release_version(version()),
            "expected MAJOR.MINOR.PATCH with an optional -<pre> part, got {}",
            version()
        );
    }

    #[test]
    fn a_release_version_is_x_y_z_with_an_optional_pre_part_and_nothing_else() {
        for good in [
            "0.5.3",
            "0.6.0",
            "0.6.0-rc.1",
            "10.20.30",
            "1.0.0-alpha.1-b",
        ] {
            assert!(is_release_version(good), "{good:?} should be a version");
        }
        for bad in [
            "0.6",
            "v0.6.0",
            "0.6.0-",
            "",
            "0.6.0.1",
            "0.6.0+build",
            "0.6.0-rc 1",
            " 0.6.0",
            "0..0",
        ] {
            assert!(!is_release_version(bad), "{bad:?} should not be a version");
        }
    }
}
