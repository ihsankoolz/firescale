//! `verify.tamper`: the guard's policy (M24, ADR 0044).
//!
//! Either a mode alone (`"enforced"`) or an object:
//!
//! ```json
//! "tamper": {
//!   "policy": "enforced",
//!   "testPaths": ["checks/**/*.py"],
//!   "allow": ["tests/legacy/**", { "path": "src/generated/**", "kinds": ["suppression-added"] }]
//! }
//! ```
//!
//! - `policy`: `off`, `evaluated` (the default) or `enforced`.
//! - `testPaths`: globs of more files to read as test files, beside the
//!   names and folders the guard knows: they are protected like the others.
//! - `allow`: globs of files left out of the counts, for every kind of
//!   finding or only the `kinds` listed.
//!
//! The guard reads it from the base commit only (`base_policy` in the
//! parent module), so a change cannot widen `allow`, drop a `testPaths`
//! entry or turn the guard off for itself.

use globset::{GlobBuilder, GlobMatcher};
use serde_json::Value;

use super::{Kind, Policy};

/// One `allow` entry.
#[derive(Debug, Clone)]
pub struct Allowance {
    /// The glob, as written.
    pub path: String,
    matcher: GlobMatcher,
    /// The kinds of finding it allows; empty for every kind.
    pub kinds: Vec<Kind>,
}

/// `verify.tamper`, read.
#[derive(Debug, Clone)]
pub struct TamperPolicy {
    pub mode: Policy,
    /// The `testPaths` globs, as written.
    pub test_paths: Vec<String>,
    test_matchers: Vec<GlobMatcher>,
    pub allow: Vec<Allowance>,
}

impl Default for TamperPolicy {
    fn default() -> Self {
        Self::of_mode(Policy::Evaluated)
    }
}

impl PartialEq for TamperPolicy {
    fn eq(&self, other: &Self) -> bool {
        self.mode == other.mode
            && self.test_paths == other.test_paths
            && self.allow.len() == other.allow.len()
            && self
                .allow
                .iter()
                .zip(&other.allow)
                .all(|(a, b)| a.path == b.path && a.kinds == b.kinds)
    }
}

impl Eq for TamperPolicy {}

/// The kinds `allow` may name.
const KINDS: [(&str, Kind); 9] = [
    ("test-deleted", Kind::TestDeleted),
    ("test-skipped", Kind::TestSkipped),
    ("assertion-removed", Kind::AssertionRemoved),
    ("suppression-added", Kind::SuppressionAdded),
    ("coverage-lowered", Kind::CoverageLowered),
    ("ci-weakened", Kind::CiWeakened),
    ("no-verify", Kind::NoVerify),
    ("rule-disabled", Kind::RuleDisabled),
    ("not-checked", Kind::NotChecked),
];

fn glob(pattern: &str) -> Result<GlobMatcher, String> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map(|glob| glob.compile_matcher())
        .map_err(|error| format!("\"{pattern}\" is not a valid glob ({error})"))
}

impl TamperPolicy {
    /// A mode with no test paths and no exceptions.
    #[must_use]
    pub fn of_mode(mode: Policy) -> Self {
        Self {
            mode,
            test_paths: Vec::new(),
            test_matchers: Vec::new(),
            allow: Vec::new(),
        }
    }

    /// Reads a `verify.tamper` value. The error says what is wrong, without
    /// the config file's name.
    pub fn parse(value: &Value) -> Result<Self, String> {
        const MODES: &str = "\"off\", \"evaluated\" (report only, the default) or \"enforced\" (findings fail verify)";
        let mode = |value: &Value| {
            value
                .as_str()
                .and_then(Policy::parse)
                .ok_or_else(|| format!("verify.tamper.policy must be {MODES}"))
        };
        let object = match value {
            Value::String(_) => {
                return value
                    .as_str()
                    .and_then(Policy::parse)
                    .map(Self::of_mode)
                    .ok_or_else(|| format!("verify.tamper must be {MODES}, or an object with \"policy\", \"testPaths\" and \"allow\""));
            }
            Value::Object(object) => object,
            _ => {
                return Err(format!(
                    "verify.tamper must be {MODES}, or an object with \"policy\", \"testPaths\" and \"allow\""
                ))
            }
        };
        let mut policy = Self::default();
        for (key, value) in object {
            match key.as_str() {
                "policy" => policy.mode = mode(value)?,
                "testPaths" => {
                    let Some(list) = value.as_array() else {
                        return Err("verify.tamper.testPaths must be a list of globs of more test files, for example [\"checks/**/*.py\"]".to_owned());
                    };
                    for entry in list {
                        let Some(pattern) = entry.as_str() else {
                            return Err("verify.tamper.testPaths must hold only globs (strings)".to_owned());
                        };
                        policy.test_matchers.push(glob(pattern).map_err(|why| format!("verify.tamper.testPaths: {why}"))?);
                        policy.test_paths.push(pattern.to_owned());
                    }
                }
                "allow" => {
                    let Some(list) = value.as_array() else {
                        return Err("verify.tamper.allow must be a list of globs, or of { \"path\": glob, \"kinds\": [...] }".to_owned());
                    };
                    for entry in list {
                        policy.allow.push(allowance(entry)?);
                    }
                }
                other => {
                    return Err(format!(
                        "unknown key \"{other}\" in verify.tamper. Allowed keys: policy, testPaths, allow"
                    ))
                }
            }
        }
        Ok(policy)
    }

    /// Reads the base commit's value: like [`TamperPolicy::parse`], but a
    /// value it cannot read never fails the run, and is never read looser
    /// than it is written: it keeps the mode when that is readable (else
    /// `evaluated`) and every `testPaths` glob that is valid, and drops
    /// `allow`.
    #[must_use]
    pub fn lenient(value: &Value) -> Self {
        Self::parse(value).unwrap_or_else(|_| {
            let mode = value
                .as_str()
                .or_else(|| value.get("policy").and_then(Value::as_str))
                .and_then(Policy::parse)
                .unwrap_or(Policy::Evaluated);
            let mut policy = Self::of_mode(mode);
            for pattern in value
                .get("testPaths")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if let Ok(matcher) = glob(pattern) {
                    policy.test_matchers.push(matcher);
                    policy.test_paths.push(pattern.to_owned());
                }
            }
            policy
        })
    }

    /// True when `testPaths` names this file.
    #[must_use]
    pub fn names_test(&self, path: &str) -> bool {
        self.test_matchers
            .iter()
            .any(|matcher| matcher.is_match(path))
    }

    /// True when `allow` leaves this kind of finding in this file out.
    #[must_use]
    pub fn allows(&self, path: &str, kind: Kind) -> bool {
        self.allow.iter().any(|allowance| {
            allowance.matcher.is_match(path)
                && (allowance.kinds.is_empty() || allowance.kinds.contains(&kind))
        })
    }
}

fn allowance(entry: &Value) -> Result<Allowance, String> {
    let shape = "verify.tamper.allow entries are globs, or { \"path\": glob, \"kinds\": [\"test-deleted\", \"test-skipped\", \"assertion-removed\", \"suppression-added\", \"coverage-lowered\", \"ci-weakened\", \"no-verify\", \"rule-disabled\", \"not-checked\"] }";
    let (path, kinds) = match entry {
        Value::String(path) => (path.as_str(), Vec::new()),
        Value::Object(object) => {
            if let Some(other) = object
                .keys()
                .find(|key| !matches!(key.as_str(), "path" | "kinds"))
            {
                return Err(format!(
                    "unknown key \"{other}\" in a verify.tamper.allow entry. {shape}"
                ));
            }
            let path = object
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| shape.to_owned())?;
            let mut kinds = Vec::new();
            if let Some(list) = object.get("kinds") {
                for kind in list.as_array().ok_or_else(|| shape.to_owned())? {
                    let name = kind.as_str().unwrap_or_default();
                    let Some((_, kind)) = KINDS.iter().find(|(known, _)| *known == name) else {
                        return Err(format!("verify.tamper.allow: unknown kind {kind}. {shape}"));
                    };
                    kinds.push(*kind);
                }
            }
            (path, kinds)
        }
        _ => return Err(shape.to_owned()),
    };
    Ok(Allowance {
        path: path.to_owned(),
        matcher: glob(path).map_err(|why| format!("verify.tamper.allow: {why}"))?,
        kinds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_mode_alone_or_an_object() {
        assert_eq!(
            TamperPolicy::parse(&json!("enforced")).map(|p| p.mode),
            Ok(Policy::Enforced)
        );
        let policy = TamperPolicy::parse(&json!({
            "policy": "enforced",
            "testPaths": ["checks/**/*.py"],
            "allow": ["tests/legacy/**", { "path": "src/gen/**", "kinds": ["suppression-added"] }]
        }))
        .unwrap();
        assert_eq!(policy.mode, Policy::Enforced);
        assert!(policy.names_test("checks/smoke/run.py"));
        assert!(!policy.names_test("src/run.py"));
        assert!(policy.allows("tests/legacy/test_a.py", Kind::TestDeleted));
        assert!(policy.allows("src/gen/api.ts", Kind::SuppressionAdded));
        assert!(!policy.allows("src/gen/api.ts", Kind::TestDeleted));
        assert!(!policy.allows("tests/test_a.py", Kind::TestDeleted));
        // `*` does not cross folders.
        let star = TamperPolicy::parse(&json!({ "allow": ["tests/*.py"] })).unwrap();
        assert!(!star.allows("tests/deep/test_a.py", Kind::TestDeleted));
        assert_eq!(star.mode, Policy::Evaluated);
    }

    #[test]
    fn mistakes_say_what_is_allowed() {
        for (value, says) in [
            (json!("strict"), "verify.tamper must be \"off\""),
            (json!({ "mode": "enforced" }), "unknown key \"mode\""),
            (json!({ "policy": "on" }), "verify.tamper.policy must be"),
            (json!({ "testPaths": "tests" }), "testPaths must be a list"),
            (json!({ "testPaths": ["a/[b"] }), "not a valid glob"),
            (
                json!({ "allow": [{ "path": "a/**", "kinds": ["test-moved"] }] }),
                "unknown kind",
            ),
            (
                json!({ "allow": [{ "glob": "a/**" }] }),
                "unknown key \"glob\"",
            ),
            (json!(true), "verify.tamper must be"),
        ] {
            let error = TamperPolicy::parse(&value).expect_err(says);
            assert!(error.contains(says), "{error}");
        }
    }

    #[test]
    fn the_base_value_never_fails_the_run() {
        assert_eq!(
            TamperPolicy::lenient(&json!({ "policy": "enforced", "allow": [3] })),
            TamperPolicy::of_mode(Policy::Enforced)
        );
        assert_eq!(
            TamperPolicy::lenient(&json!("strict")),
            TamperPolicy::of_mode(Policy::Evaluated)
        );
        // A bad `allow` entry drops `allow`, never the test paths.
        let kept = TamperPolicy::lenient(&json!({
            "testPaths": ["checks/**", "a/[b"],
            "allow": ["tests/**", 3]
        }));
        assert_eq!(kept.test_paths, ["checks/**"]);
        assert!(kept.names_test("checks/smoke.py"));
        assert!(kept.allow.is_empty());
    }
}
