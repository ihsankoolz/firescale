//! `cmake/tests`: a CMake project with code registers tests, and turns
//! testing on in its root `CMakeLists.txt` before it adds them, so `ctest
//! --test-dir build` runs them. Without either, ctest prints "No tests were
//! found!!!" and exits 0 (CMake 3.28 and 4.4, run here), so `fscale verify`
//! passes having tested nothing.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{applies, code, finding, line_ending, own_commands, syntax, syntax::Command};

#[cfg(test)]
mod tests;

pub const ID: &str = "cmake/tests";

pub struct Tests;

/// True when a command registers a test with CTest: `add_test`, or a
/// helper or framework that does: GoogleTest's `gtest_discover_tests` and
/// `gtest_add_tests`, Catch2's `catch_discover_tests` and
/// `ParseAndAddCatchTests`, doctest's `doctest_discover_tests`, ROS 2's
/// `ament_add_gtest`, `ament_add_gmock` and `ament_add_pytest_test`, KDE's
/// `ecm_add_test(s)`, and project helpers named `*add_test*` (found in
/// review). Not `*add_pytests`: curl's builds a target, not a test.
#[must_use]
pub fn registers_tests(command: &Command) -> bool {
    let name = command.name.as_str();
    name.contains("add_test")
        || name.contains("add_gtest")
        || name.contains("add_gmock")
        || (name.starts_with("ament_add_") && (name.contains("test") || name.contains("gmock")))
        || name.ends_with("_discover_tests")
        || name == "parseandaddcatchtests"
}

/// True when a command registers a test with CTest's own `add_test` or a
/// test framework's module, which leave turning testing on to the project.
/// A helper of a larger system (ROS 2's ament, KDE's ECM) may turn it on
/// itself, through modules the reader does not see: then the rule trusts
/// it (found in review).
fn registers_directly(command: &Command) -> bool {
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

/// True when the root `CMakeLists.txt` turns testing on in time: before it
/// registers a test or adds a subfolder that registers one (testing turned
/// on after them is not on for them; found in review), or through a module
/// it includes before them. A call in an `if()` counts: projects guard it
/// with `BUILD_TESTING` or "only when built on its own". `tested` holds the
/// folders whose CMake files register tests.
fn root_turns_testing_on(root: &[Command], modules_enable: bool, tested: &[String]) -> bool {
    for command in root {
        if enables_testing(command) || (command.name == "include" && modules_enable) {
            return true;
        }
        if registers_tests(command) || adds_tested_folder(command, tested) {
            return false;
        }
    }
    false
}

/// True when the command is an `add_subdirectory()` of a folder that holds,
/// or is above, one of `tested`.
fn adds_tested_folder(command: &Command, tested: &[String]) -> bool {
    if command.name != "add_subdirectory" {
        return false;
    }
    let Some(folder) = command.args.first() else {
        return false;
    };
    let folder = folder.trim_start_matches("./").trim_end_matches('/');
    tested.iter().any(|path| {
        path == folder
            || path
                .strip_prefix(folder)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// What the root `CMakeLists.txt` becomes with `enable_testing()` right
/// after its top-level `project()` call, or `None` when it has none (a
/// `project()` in an `if()` branch may not run: found in review) or cannot
/// be read. The line goes after the call's own line comment, or straight
/// after the call when a bracket comment or another command follows on its
/// line, and the result is read again to be sure the call is there.
#[must_use]
pub fn with_testing(text: &str) -> Option<String> {
    let commands = syntax::commands(text)?;
    let project = commands
        .iter()
        .find(|command| command.name == "project" && command.top_level())?;
    let newline = line_ending(text);
    let rest = &text[project.end..];
    let spaces = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    let after = &rest[spaces..];
    let line_comment = after.starts_with('#') && !after.starts_with("#[");
    let insert_at = if after.is_empty() || after.starts_with(['\r', '\n']) || line_comment {
        // The end of the call's line.
        project.end + rest.find('\n').map_or(rest.len(), |offset| offset + 1)
    } else {
        project.end
    };
    let indent: String = text[..project.start]
        .rsplit('\n')
        .next()
        .unwrap_or_default()
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let mut out = String::with_capacity(text.len() + 20);
    out.push_str(&text[..insert_at]);
    if !out.ends_with('\n') {
        out.push_str(newline);
    }
    out.push_str(&indent);
    out.push_str("enable_testing()");
    out.push_str(newline);
    out.push_str(&text[insert_at..]);
    let again = syntax::commands(&out)?;
    let on = again
        .iter()
        .take_while(|command| command.name != "add_subdirectory" && !registers_tests(command))
        .any(enables_testing);
    on.then_some(out)
}

/// What the rule finds: `Some(true)` when tests are registered but testing
/// is not on in time at the root, `Some(false)` when no test is
/// registered, `None` when all is well or the rule does not apply.
fn state(ctx: &RuleContext) -> Result<Option<bool>> {
    if !applies(ctx)? || !ctx.root().join("CMakeLists.txt").is_file() || code(ctx)?.0.is_empty() {
        return Ok(None);
    }
    let own = own_commands(ctx)?;
    if !own
        .iter()
        .any(|(_, commands)| commands.iter().any(registers_tests))
    {
        return Ok(Some(false));
    }
    if !own
        .iter()
        .any(|(_, commands)| commands.iter().any(registers_directly))
    {
        return Ok(None);
    }
    let Some((_, root)) = own.iter().find(|(path, _)| path == "CMakeLists.txt") else {
        // A root file the reader cannot follow: `cmake/lists` names it.
        return Ok(None);
    };
    // A module (`*.cmake`) of the project's that turns testing on counts
    // when the root includes modules before it adds anything.
    let modules_enable = own
        .iter()
        .filter(|(path, _)| !path.ends_with("CMakeLists.txt"))
        .any(|(_, commands)| commands.iter().any(enables_testing));
    let tested: Vec<String> = own
        .iter()
        .filter(|(_, commands)| commands.iter().any(registers_tests))
        .filter_map(|(path, _)| path.rsplit_once('/').map(|(folder, _)| folder.to_owned()))
        .collect();
    Ok((!root_turns_testing_on(root, modules_enable, &tested)).then_some(true))
}

impl BuiltinRule for Tests {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Consistency,
            description: "A CMake project with code registers tests and turns testing on in its root CMakeLists.txt before adding them."
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
                let how = if fixable == Fixable::Deterministic {
                    "`fscale sync` adds enable_testing() after project()."
                } else {
                    "Add enable_testing() to the root CMakeLists.txt after project() and before add_subdirectory() and the tests (its project() is in a block, so `fscale sync` does not)."
                };
                Ok(vec![finding(
                    "CMakeLists.txt",
                    format!("The project registers tests, but its root CMakeLists.txt does not call enable_testing() or include(CTest) before adding them, so the build has no test list at its top and `ctest --test-dir build` prints \"No tests were found!!!\" and passes. {how}"),
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
