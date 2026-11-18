//! A test runner made unable to fail (ADR 0086): report only.
//!
//! - A script of a `package.json` that checks the code (`test`, `lint`,
//!   `typecheck` and their kin, as CI's detector names them) in which a
//!   command that checks (as CI's detector reads it, [`super::ci::check_of`])
//!   is followed by `|| true`, `|| :`, `|| exit 0`, `|| echo …` (CI's
//!   detector's list) or `; exit 0`. npm, pnpm, Yarn and Bun run a script
//!   with `sh -c`, without `-e`, so after `; exit 0` the script passes
//!   whatever came before.
//! - A `conftest.py` with `sys.exit(…)`, `os._exit(…)` or
//!   `raise SystemExit` at the top level, which pytest runs when it imports
//!   the file, or a `pytest_sessionfinish` hook that sets `exitstatus`,
//!   which rewrites the run's result after the tests.
//!
//! Each such place counts against the file's key once; the counts are
//! compared over the whole change, like CI's, so one already there on the
//! base is not new. A finding of this kind never fails verify, even when
//! the guard is enforced: it is shown for a reviewer.

use super::config::Measure;
use super::formats::{self, Format};
use super::Kind;

/// True for a `package.json` or a `conftest.py`, in any folder.
pub(crate) fn reads(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name == "package.json" || name == "conftest.py"
}

pub(crate) fn measures(path: &str, text: &str) -> Result<Vec<Measure>, usize> {
    if path.rsplit('/').next() == Some("conftest.py") {
        return Ok(conftest(text));
    }
    let entries = formats::read(Format::Json, text)?;
    let mut measures = Vec::new();
    for entry in &entries {
        let [scripts, name] = entry.path.as_slice() else {
            continue;
        };
        if scripts != "scripts" || !super::ci::is_check_script(name) {
            continue;
        }
        let script = entry.value.as_text();
        for (command, rest) in super::ci::commands(&script) {
            let Some(check) = super::ci::check_of(&command) else {
                continue;
            };
            if super::ci::swallowed(&rest).is_none() && !exits_zero(&rest) {
                continue;
            }
            measures.push(Measure::count(
                Kind::RunnerWeakened,
                // Not the script's name: a script renamed with what it
                // swallowed is the same place (found in review).
                format!("runner script: {check}"),
                -1,
                entry.line,
                format!("{name}: {script}"),
            ));
        }
    }
    Ok(measures)
}

/// True when what follows a command is `; exit 0`.
fn exits_zero(rest: &str) -> bool {
    rest.trim_start()
        .strip_prefix(';')
        .is_some_and(|tail| tail.split_whitespace().take(2).eq(["exit", "0"]))
}

/// The places in a `conftest.py` that end or rewrite the run.
fn conftest(text: &str) -> Vec<Measure> {
    let mut measures = Vec::new();
    // The indent of the `def pytest_sessionfinish(` whose body this is, at
    // the top level or in a plugin class.
    let mut hook: Option<usize> = None;
    let mut hook_counted = false;
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
        let top = indent == 0;
        let code = line.split('#').next().unwrap_or("").trim_end();
        let statement = code.trim_start();
        if statement.starts_with("def pytest_sessionfinish(") {
            hook = Some(indent);
            hook_counted = false;
            continue;
        }
        // A line at the hook's indent or less ends its body, unless it
        // closes the signature (`) -> None:`, as Black writes it).
        if hook
            .is_some_and(|at| !statement.is_empty() && indent <= at && !statement.starts_with(')'))
        {
            hook = None;
        }
        if top && !code.is_empty() {
            let compact: String = code.chars().filter(|c| !c.is_whitespace()).collect();
            if hook.is_some() {
                continue;
            }
            if compact.starts_with("sys.exit(")
                || compact.starts_with("os._exit(")
                || compact == "raiseSystemExit"
                || compact.starts_with("raiseSystemExit(")
            {
                measures.push(Measure::count(
                    Kind::RunnerWeakened,
                    "runner conftest.py: exits at the top level",
                    -1,
                    number,
                    code.trim().to_owned(),
                ));
            }
            continue;
        }
        if hook.is_some() && !hook_counted && sets_exitstatus(code) {
            hook_counted = true;
            measures.push(Measure::count(
                Kind::RunnerWeakened,
                "runner conftest.py: pytest_sessionfinish sets exitstatus",
                -1,
                number,
                format!("pytest_sessionfinish: {}", code.trim()),
            ));
        }
    }
    measures
}

/// `session.exitstatus = 0`, `exitstatus = …`, `x.exitstatus |= …`,
/// `setattr(session, "exitstatus", …)`: a statement that assigns it, not a
/// comparison, a keyword argument or text (found in review).
fn sets_exitstatus(code: &str) -> bool {
    let statement = code.trim();
    if statement.starts_with("setattr(") && statement.contains("exitstatus") {
        return true;
    }
    // The target before the operator: names and dots only.
    let target_end = statement
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))
        .unwrap_or(statement.len());
    let (target, rest) = statement.split_at(target_end);
    if !(target == "exitstatus" || target.ends_with(".exitstatus")) {
        return false;
    }
    let operator = rest.trim_start();
    let assigns = operator.starts_with('=') && !operator.starts_with("==");
    let augmented = ["+=", "-=", "|=", "&=", "*="]
        .iter()
        .any(|op| operator.starts_with(op));
    assigns || augmented
}

#[cfg(test)]
mod tests {
    use super::*;

    fn whats(path: &str, text: &str) -> Vec<String> {
        measures(path, text)
            .unwrap()
            .into_iter()
            .map(|m| format!("{} @{}: {}", m.key, m.line, m.what))
            .collect()
    }

    #[test]
    fn a_check_script_that_cannot_fail_counts_and_others_do_not() {
        let package = r#"{
  "scripts": {
    "build": "tsc -p . || true",
    "test": "vitest run || true",
    "lint": "eslint . ; exit 0",
    "typecheck": "rm -rf out || true && tsc --noEmit",
    "test:e2e": "playwright test; exit 1"
  }
}"#;
        assert_eq!(
            whats("packages/a/package.json", package),
            [
                "runner script: vitest @4: test: vitest run || true",
                "runner script: eslint @5: lint: eslint . ; exit 0",
            ]
        );
    }

    #[test]
    fn conftest_exits_and_a_session_hook_that_sets_the_status_count() {
        let text = "import sys\n\ndef helper():\n    sys.exit(2)\n\n\ndef pytest_sessionfinish(session, exitstatus):\n    if exitstatus == 0:\n        print('ok')\n    session.exitstatus = 0\n    session.exitstatus = 0\n\n\nsys.exit(0)\n";
        assert_eq!(
            whats("tests/conftest.py", text),
            [
                "runner conftest.py: pytest_sessionfinish sets exitstatus @10: pytest_sessionfinish: session.exitstatus = 0",
                "runner conftest.py: exits at the top level @14: sys.exit(0)",
            ]
        );
        assert!(whats(
            "conftest.py",
            "def pytest_sessionfinish(session, exitstatus):\n    print(exitstatus == 0)\n"
        )
        .is_empty());
    }

    #[test]
    fn a_hook_with_its_signature_on_several_lines_or_in_a_class_is_read() {
        let black = "import pytest\n\n\ndef pytest_sessionfinish(\n    session: pytest.Session, exitstatus: int\n) -> None:\n    session.exitstatus = 0\n";
        assert_eq!(
            whats("conftest.py", black),
            ["runner conftest.py: pytest_sessionfinish sets exitstatus @7: pytest_sessionfinish: session.exitstatus = 0"]
        );
        let plugin = "import sys\n\n\nclass Plugin:\n    def pytest_sessionfinish(self, session):\n        session.exitstatus = 0\n\n\nsys.exit(0)\n";
        assert_eq!(
            whats("conftest.py", plugin),
            [
                "runner conftest.py: pytest_sessionfinish sets exitstatus @6: pytest_sessionfinish: session.exitstatus = 0",
                "runner conftest.py: exits at the top level @9: sys.exit(0)",
            ]
        );
    }

    #[test]
    fn a_hook_that_only_passes_or_prints_the_status_does_not_count() {
        let text = "def pytest_sessionfinish(session, exitstatus):\n    notify(session, exitstatus=exitstatus)\n    print(f\"exitstatus={exitstatus}\")\n    log('exitstatus = %s', exitstatus)\n";
        assert!(whats("conftest.py", text).is_empty());
    }

    #[test]
    fn a_script_renamed_with_what_it_swallowed_is_the_same_place() {
        let key = |text: &str| -> Vec<String> {
            measures("package.json", text)
                .unwrap()
                .into_iter()
                .map(|m| m.key)
                .collect()
        };
        assert_eq!(
            key(r#"{"scripts": {"test": "vitest run || true"}}"#),
            key(r#"{"scripts": {"test:unit": "vitest run || true"}}"#)
        );
    }
}
