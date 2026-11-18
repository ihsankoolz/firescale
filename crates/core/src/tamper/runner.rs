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
                format!("runner script {name}: {check}"),
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
    let mut hook: Option<usize> = None;
    let mut hook_counted = false;
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        let top = !line.starts_with([' ', '\t']);
        let code = line.split('#').next().unwrap_or("").trim_end();
        if top && !code.is_empty() {
            hook = None;
            if code.starts_with("def pytest_sessionfinish(") {
                hook = Some(number);
                hook_counted = false;
                continue;
            }
            let compact: String = code.chars().filter(|c| !c.is_whitespace()).collect();
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

/// `session.exitstatus = 0`, `exitstatus = …`, `setattr(session, "exitstatus", …)`:
/// an assignment, not a comparison.
fn sets_exitstatus(code: &str) -> bool {
    if code.contains("setattr(") && code.contains("exitstatus") {
        return true;
    }
    let mut rest = code;
    while let Some(at) = rest.find("exitstatus") {
        let after = rest[at + "exitstatus".len()..].trim_start();
        if after.starts_with('=') && !after.starts_with("==") {
            return true;
        }
        rest = &rest[at + "exitstatus".len()..];
    }
    false
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
                "runner script test: vitest @4: test: vitest run || true",
                "runner script lint: eslint @5: lint: eslint . ; exit 0",
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
}
