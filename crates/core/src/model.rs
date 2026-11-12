//! The diagnostics data model. Field names and order match the documented
//! `fscale check --json` format, so serialising these types is the format.

use serde::{Deserialize, Serialize};

/// How bad a violation is. `Blocker` is the most severe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Blocker,
    High,
    Medium,
    Low,
}

/// What kind of problem a rule guards against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Security,
    Reliability,
    Consistency,
    Maintainability,
}

/// Who can fix a violation: the rule's own fix, a coding agent following the
/// rule's SKILL.md, or a human.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fixable {
    Deterministic,
    Agent,
    Manual,
}

/// `Off`: not run. `Evaluated`: reported only. `Enforced`: a violation fails the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Off,
    Evaluated,
    Enforced,
}

macro_rules! as_str {
    ($type:ty { $($variant:ident => $text:literal),+ $(,)? }) => {
        impl $type {
            #[must_use]
            pub fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $text),+ }
            }
        }
    };
}

as_str!(Severity { Blocker => "blocker", High => "high", Medium => "medium", Low => "low" });
as_str!(Category {
    Security => "security",
    Reliability => "reliability",
    Consistency => "consistency",
    Maintainability => "maintainability",
});
as_str!(Fixable { Deterministic => "deterministic", Agent => "agent", Manual => "manual" });
as_str!(Status { Off => "off", Evaluated => "evaluated", Enforced => "enforced" });

impl Status {
    /// Parses a status as written in a config file.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "off" => Some(Self::Off),
            "evaluated" => Some(Self::Evaluated),
            "enforced" => Some(Self::Enforced),
            _ => None,
        }
    }
}

/// An inclusive, 1-based line range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineRange {
    pub start_line: u32,
    pub end_line: u32,
}

/// What a rule's check returns for each violation. The engine adds the rule id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub message: String,
    /// Path relative to the repo root, with forward slashes.
    pub file: String,
    pub range: Option<LineRange>,
    pub project: Option<String>,
    pub fixable: Fixable,
    /// Set only when this finding is less or more severe than the rule's own
    /// severity. `None` means the rule's severity.
    pub severity: Option<Severity>,
}

/// One violation of one rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    /// Full rule id, `<pack>/<rule>`.
    pub rule_id: String,
    pub message: String,
    pub file: String,
    pub fixable: Fixable,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<LineRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// This violation's own severity, when it differs from the rule's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<Severity>,
}

/// What a rule's fix returns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FixResult {
    /// Repo-relative paths the fix changed, including files it removed from
    /// the git index. Empty when there was nothing left to fix.
    pub changed_files: Vec<String>,
    /// Things the fix could not do, for a human or agent to follow up on.
    pub notes: Vec<String>,
}

/// A rule's metadata. The same shape describes built-in and custom rules.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleMeta {
    /// `<pack>/<rule>`, lowercase letters, digits and hyphens.
    pub id: String,
    pub severity: Severity,
    pub category: Category,
    /// One sentence stating what must be true.
    pub description: String,
    /// Globs of the files the check depends on. Used for cache hashing from M3.
    #[serde(default)]
    pub inputs: Vec<String>,
    /// Whether the rule has a deterministic fix.
    #[serde(default)]
    pub has_fix: bool,
    /// Ids of rules whose fixes must run before this rule's fix. Only orders
    /// fixes: it does not change when the rule is checked.
    #[serde(default)]
    pub after: Vec<String>,
}
