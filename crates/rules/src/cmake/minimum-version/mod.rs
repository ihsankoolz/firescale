//! `cmake/minimum-version`: no `CMakeLists.txt` asks for the behaviour of a
//! CMake older than 3.10. CMake 4.0 removed compatibility with versions
//! before 3.5 (configuring stops with an error), and CMake 3.31 deprecated
//! versions before 3.10 (a warning on every configure). Both run here.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{all_commands, applies, finding, syntax};

#[cfg(test)]
mod tests;

pub const ID: &str = "cmake/minimum-version";

pub struct MinimumVersion;

/// What CMake 4 does with a policy version: `Some(true)` when it stops,
/// `Some(false)` when it warns, `None` when it accepts it.
#[must_use]
pub fn outcome(version: (u32, u32)) -> Option<bool> {
    if version < (3, 5) {
        Some(true)
    } else if version < (3, 10) {
        Some(false)
    } else {
        None
    }
}

impl BuiltinRule for MinimumVersion {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::High,
            category: Category::Consistency,
            description:
                "No CMakeLists.txt asks for a CMake older than 3.10, which CMake 4 refuses or warns about."
                    .to_owned(),
            inputs: super::inputs(&[]),
            has_fix: false,
            after: Vec::new(),
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        if !applies(ctx)? {
            return Ok(Vec::new());
        }
        let mut found = Vec::new();
        for (path, commands) in all_commands(ctx)? {
            if !path.ends_with("CMakeLists.txt") {
                continue;
            }
            let Some((written, version)) = syntax::required_version(&commands) else {
                continue;
            };
            let Some(stops) = outcome(version) else {
                continue;
            };
            let minimum = written.split("...").next().unwrap_or(&written);
            let mut finding = if stops {
                finding(
                    &path,
                    format!(
                        "{path} asks for CMake {written}, and CMake 4 stops there (\"Compatibility with CMake < 3.5 has been removed from CMake\"). Raise it to the oldest CMake the project supports, 3.10 or later, and build with that CMake to see what its policies change; `-DCMAKE_POLICY_VERSION_MINIMUM=3.5` only works around it."
                    ),
                    Fixable::Manual,
                )
            } else {
                finding(
                    &path,
                    format!(
                        "{path} asks for CMake {written}, and CMake 3.31 and later warn on every configure that compatibility with CMake < 3.10 will be removed. Raise it to 3.10 or later (`cmake_minimum_required(VERSION {minimum}...3.10)` keeps the minimum and takes 3.10's policies) and build to see what changes."
                    ),
                    Fixable::Manual,
                )
            };
            if !stops {
                finding.severity = Some(Severity::Medium);
            }
            found.push(finding);
        }
        Ok(found)
    }

    fn fix(&self, _ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        Ok(FixResult::default())
    }
}
