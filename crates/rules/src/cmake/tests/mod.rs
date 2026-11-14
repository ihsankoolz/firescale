//! `cmake/tests`: a CMake project with code registers tests, and turns
//! testing on in its root `CMakeLists.txt`, so `ctest --test-dir build`
//! runs them. Without either, ctest prints "No tests were found!!!" and
//! exits 0 (CMake 3.28 and 4.4, run here), so `fscale verify` passes having
//! tested nothing.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{all_commands, applies, code, finding, line_ending, syntax, syntax::Command};

#[cfg(test)]
mod tests;

pub const ID: &str = "cmake/tests";

pub struct Tests;

/// True when a command registers a test with CTest: `add_test`, or a test
/// framework's discovery (`gtest_discover_tests`, `gtest_add_tests`,
/// Catch2's `catch_discover_tests`, doctest's `doctest_discover_tests`, …).
#[must_use]
pub fn registers_tests(command: &Command) -> bool {
    command.name == "add_test"
        || command.name == "gtest_add_tests"
        || command.name.ends_with("_discover_tests")
        || command.name == "parseandaddcatchtests"
}

/// True when a command turns testing on: `enable_testing()`, or
/// `include(CTest)`, which calls it.
#[must_use]
pub fn enables_testing(command: &Command) -> bool {
    command.name == "enable_testing"
        || (command.name == "include"
            && command
                .args
                .first()
                .is_some_and(|arg| arg.eq_ignore_ascii_case("CTest")))
}

/// What the root `CMakeLists.txt` becomes with `enable_testing()` on the
/// line after its `project()` call, or `None` when it has no `project()`
/// or cannot be read.
#[must_use]
pub fn with_testing(text: &str) -> Option<String> {
    let commands = syntax::commands(text)?;
    let project = commands.iter().find(|command| command.name == "project")?;
    let newline = line_ending(text);
    let line_end = text[project.end..]
        .find('\n')
        .map_or(text.len(), |offset| project.end + offset + 1);
    let indent: String = text[..project.start]
        .rsplit('\n')
        .next()
        .unwrap_or_default()
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let mut out = String::with_capacity(text.len() + 20);
    out.push_str(&text[..line_end]);
    if line_end == text.len() && !text.ends_with('\n') {
        out.push_str(newline);
    }
    out.push_str(&indent);
    out.push_str("enable_testing()");
    out.push_str(newline);
    out.push_str(&text[line_end..]);
    Some(out)
}

/// What the rule finds: `Some(true)` when tests are registered but testing
/// is not on at the root (a fix adds it), `Some(false)` when no test is
/// registered, `None` when all is well or the rule does not apply.
fn state(ctx: &RuleContext) -> Result<Option<bool>> {
    if !applies(ctx)? || !ctx.root().join("CMakeLists.txt").is_file() || code(ctx)?.0.is_empty() {
        return Ok(None);
    }
    let all = all_commands(ctx)?;
    let registered = all
        .iter()
        .any(|(_, commands)| commands.iter().any(registers_tests));
    if !registered {
        return Ok(Some(false));
    }
    // Turned on at the top: the root CMakeLists.txt, or a module it may
    // include (`*.cmake`). A subfolder's enable_testing() makes no test
    // file at the top of the build, where ctest looks.
    let on = all
        .iter()
        .filter(|(path, _)| path == "CMakeLists.txt" || !path.ends_with("CMakeLists.txt"))
        .any(|(_, commands)| commands.iter().any(enables_testing));
    Ok((!on).then_some(true))
}

impl BuiltinRule for Tests {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Consistency,
            description: "A CMake project with code registers tests and turns testing on in its root CMakeLists.txt."
                .to_owned(),
            inputs: super::inputs(&[]),
            has_fix: true,
            after: vec!["cmake/lists".to_owned()],
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        match state(ctx)? {
            None => Ok(Vec::new()),
            Some(false) => Ok(vec![finding(
                "CMakeLists.txt",
                "The project has C or C++ code and its CMake files register no test (add_test, gtest_discover_tests, catch_discover_tests, …), so `ctest` prints \"No tests were found!!!\" and passes. Add a test executable and register it with add_test, or with the discovery command of the test framework the project uses.".to_owned(),
                Fixable::Agent,
            )]),
            Some(true) => {
                let fixable = ctx
                    .read("CMakeLists.txt")?
                    .and_then(|text| with_testing(&text))
                    .map_or(Fixable::Manual, |_| Fixable::Deterministic);
                Ok(vec![finding(
                    "CMakeLists.txt",
                    "The project registers tests, but its root CMakeLists.txt does not call enable_testing() or include(CTest), so the build has no test list at its top and `ctest --test-dir build` prints \"No tests were found!!!\" and passes. `fscale sync` adds enable_testing() after project().".to_owned(),
                    fixable,
                )])
            }
        }
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        if state(ctx)? != Some(true) {
            return Ok(result);
        }
        let Some(text) = ctx.read("CMakeLists.txt")? else {
            return Ok(result);
        };
        if let Some(updated) = with_testing(&text) {
            ctx.write("CMakeLists.txt", &updated)?;
            result.changed_files.push("CMakeLists.txt".to_owned());
        }
        Ok(result)
    }
}
