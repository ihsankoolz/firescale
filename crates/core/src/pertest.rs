//! Per-test results (M20, ADR 0038): which tests failed in a step, read
//! from two runners without changing the user's command.
//!
//! - **pytest**: a JUnit XML report, asked for through `PYTEST_ADDOPTS`
//!   (`--junitxml=<file in the temp folder>`), which pytest reads from the
//!   environment. The command line is unchanged.
//! - **Vitest and Jest**: their default reporters' failure lines. Their
//!   JSON reporters need flags on the command line, which verify does not
//!   add (ADR 0038 says why).
//!
//! A test's identity is its file (repo-relative, forward slashes) and its
//! name with its suites: `tests/test_auth.py::TestLogin::test_ok`,
//! `src/a.test.ts > math > adds`.

use std::collections::{BTreeMap, BTreeSet};

/// The runners with per-test results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestRunner {
    Pytest,
    Vitest,
    Jest,
}

impl TestRunner {
    /// The runner a step's command starts, when it is one of the three.
    /// A package script (`npm test`) is matched by the script's text
    /// (`script`), when known.
    #[must_use]
    pub fn of(command: &str, script: Option<&str>) -> Option<Self> {
        let text = format!("{command} {}", script.unwrap_or("")).to_ascii_lowercase();
        // Each word, without its folder and its extension: `vitest`,
        // `node_modules/.bin/vitest`, `pytest.exe`, `python -m pytest`.
        let stems: Vec<&str> = text
            .split(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '&' || c == ';')
            .map(|word| word.rsplit(['/', '\\']).next().unwrap_or(word))
            .map(|word| word.split('.').next().unwrap_or(word))
            .collect();
        if stems.iter().any(|stem| *stem == "pytest" || *stem == "py") && text.contains("pytest")
            || text.contains("py.test")
        {
            Some(Self::Pytest)
        } else if stems.contains(&"vitest") {
            Some(Self::Vitest)
        } else if stems.contains(&"jest") {
            Some(Self::Jest)
        } else {
            None
        }
    }

    /// Reads this runner's results from a finished step: the JUnit report
    /// for pytest, the output for Vitest and Jest.
    #[must_use]
    pub fn read(self, output: &str, junit: Option<&str>) -> Option<TestResults> {
        match self {
            Self::Pytest => junit.and_then(parse_junit),
            Self::Vitest => parse_vitest(output),
            Self::Jest => parse_jest(output),
        }
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Pytest => "pytest",
            Self::Vitest => "Vitest",
            Self::Jest => "Jest",
        }
    }
}

/// What a step's tests did, test by test.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TestResults {
    /// Tests that failed or errored, by identity.
    pub failed: BTreeSet<String>,
    /// How many tests passed.
    pub passed: u64,
}

// ------------------------------------------------------------------ JUnit

fn attribute(tag: &str, name: &str) -> Option<String> {
    let needle = format!(" {name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let end = tag[start..].find('"')? + start;
    Some(unescape(&tag[start..end]))
}

fn unescape(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// The identity of a pytest test case from its JUnit attributes:
/// `classname="tests.test_auth.TestLogin"`, `name="test_ok"` and, when
/// present, `file="tests/test_auth.py"`. The module part of the class name
/// becomes the file, so ids look like pytest's own node ids.
fn pytest_id(classname: &str, name: &str, file: Option<&str>) -> String {
    let parts: Vec<&str> = classname.split('.').collect();
    // The class names start with a capital letter; the module path does not.
    let module_len = parts
        .iter()
        .position(|part| part.chars().next().is_some_and(char::is_uppercase))
        .unwrap_or(parts.len());
    let file = file.map_or_else(
        || {
            if module_len == 0 {
                String::new()
            } else {
                format!("{}.py", parts[..module_len].join("/"))
            }
        },
        |file| file.replace('\\', "/"),
    );
    let mut id = file;
    for class in &parts[module_len..] {
        id.push_str("::");
        id.push_str(class);
    }
    id.push_str("::");
    id.push_str(name);
    id
}

/// Reads a JUnit XML report as pytest writes it.
#[must_use]
pub fn parse_junit(xml: &str) -> Option<TestResults> {
    if !xml.contains("<testsuite") {
        return None;
    }
    let mut results = TestResults::default();
    let mut rest = xml;
    while let Some(start) = rest.find("<testcase") {
        let after = &rest[start..];
        let Some(tag_end) = after.find('>') else {
            break;
        };
        let tag = &after[..=tag_end];
        let self_closing = tag.ends_with("/>");
        let body = if self_closing {
            ""
        } else {
            let close = after.find("</testcase>").unwrap_or(after.len());
            &after[tag_end + 1..close]
        };
        let classname = attribute(tag, "classname").unwrap_or_default();
        let name = attribute(tag, "name").unwrap_or_default();
        let file = attribute(tag, "file");
        let id = pytest_id(&classname, &name, file.as_deref());
        if body.contains("<failure") || body.contains("<error") {
            results.failed.insert(id);
        } else if !body.contains("<skipped") {
            results.passed += 1;
        }
        rest = &after[tag_end + 1..];
    }
    Some(results)
}

// ----------------------------------------------------------- Vitest, Jest

fn clean(line: &str) -> String {
    crate::verify::strip_ansi(line).trim().to_owned()
}

/// Reads Vitest's default reporter: ` FAIL  src/a.test.ts > math > adds`
/// lines in its failure summary, and its `Tests  1 failed | 4 passed` line.
#[must_use]
pub fn parse_vitest(output: &str) -> Option<TestResults> {
    let mut results = TestResults::default();
    let mut seen_summary = false;
    for raw in output.lines() {
        let line = clean(raw);
        if let Some(rest) = line.strip_prefix("FAIL ") {
            let rest = rest.trim();
            // `FAIL  file > suite > test`; a file-level failure has no `>`.
            let id = rest
                .split(" [ ")
                .next()
                .unwrap_or(rest)
                .trim()
                .replace('\\', "/");
            if !id.is_empty() {
                results.failed.insert(id);
            }
        }
        if let Some(rest) = line.strip_prefix("Tests ") {
            seen_summary = true;
            for part in rest.split('|') {
                let mut words = part.split_whitespace();
                if let (Some(count), Some(kind)) = (words.next(), words.next()) {
                    if kind.starts_with("passed") {
                        results.passed = count.parse().unwrap_or(0);
                    }
                }
            }
        }
    }
    (seen_summary || !results.failed.is_empty()).then_some(results)
}

/// Reads Jest's default reporter: a `FAIL path` line per failing file and a
/// `● suite › test` line per failing test, and its `Tests:` summary.
#[must_use]
pub fn parse_jest(output: &str) -> Option<TestResults> {
    let mut results = TestResults::default();
    let mut seen_summary = false;
    let mut file: Option<String> = None;
    for raw in output.lines() {
        let line = clean(raw);
        if let Some(rest) = line.strip_prefix("FAIL ") {
            file = Some(
                rest.split_whitespace()
                    .next()
                    .unwrap_or("")
                    .replace('\\', "/"),
            );
            continue;
        }
        if line.starts_with("PASS ") {
            file = None;
            continue;
        }
        if let Some(name) = line.strip_prefix("● ") {
            if name.starts_with("Test suite failed to run") {
                if let Some(file) = &file {
                    results.failed.insert(file.clone());
                }
                continue;
            }
            let id = match &file {
                Some(file) => format!("{file} › {}", name.trim()),
                None => name.trim().to_owned(),
            };
            results.failed.insert(id);
        }
        if let Some(rest) = line.strip_prefix("Tests:") {
            seen_summary = true;
            for part in rest.split(',') {
                let mut words = part.split_whitespace();
                if let (Some(count), Some(kind)) = (words.next(), words.next()) {
                    if kind.starts_with("passed") {
                        results.passed = count.parse().unwrap_or(0);
                    }
                }
            }
        }
    }
    (seen_summary || !results.failed.is_empty()).then_some(results)
}

// ------------------------------------------------------------- comparing

/// A test file renamed between the base and the head, as git reports it.
pub type Renames = BTreeMap<String, String>;

/// The base's test ids with renamed files moved to their head paths, so a
/// test that failed in `tests/old.py` and fails in `tests/new.py` after a
/// rename is the same test (ADR 0038).
#[must_use]
pub fn follow_renames(
    base: &BTreeSet<String>,
    renames: &Renames,
    prefix: &str,
) -> BTreeSet<String> {
    base.iter()
        .map(|id| {
            for (old, new) in renames {
                let old = old.strip_prefix(prefix).unwrap_or(old);
                let new = new.strip_prefix(prefix).unwrap_or(new);
                if let Some(rest) = id.strip_prefix(old) {
                    if rest.is_empty() || rest.starts_with("::") || rest.starts_with(' ') {
                        return format!("{new}{rest}");
                    }
                }
            }
            id.clone()
        })
        .collect()
}

/// The text of the package script a command runs (`npm test`, `pnpm run
/// lint`, `yarn test`, `bun run test`), read from `package.json` in `dir`.
#[must_use]
pub fn script_text(dir: &std::path::Path, command: &str) -> Option<String> {
    let words: Vec<&str> = command.split_whitespace().collect();
    let manager = words.first()?.rsplit(['/', '\\']).next()?;
    let manager = manager.split('.').next()?;
    if !matches!(manager, "npm" | "pnpm" | "yarn" | "bun") {
        return None;
    }
    let name = match words.get(1).copied() {
        Some("run" | "run-script") => words.get(2).copied()?,
        Some(name) if !name.starts_with('-') => name,
        _ => return None,
    };
    let text = std::fs::read_to_string(dir.join("package.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.get("scripts")?.get(name)?.as_str().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runners_are_found_in_commands_and_scripts() {
        assert_eq!(
            TestRunner::of("python -m pytest", None),
            Some(TestRunner::Pytest)
        );
        assert_eq!(
            TestRunner::of(".venv/bin/pytest -q", None),
            Some(TestRunner::Pytest)
        );
        assert_eq!(
            TestRunner::of("node pytest.js", None),
            Some(TestRunner::Pytest)
        );
        assert_eq!(
            TestRunner::of("npx vitest run", None),
            Some(TestRunner::Vitest)
        );
        assert_eq!(
            TestRunner::of("npm test", Some("vitest run")),
            Some(TestRunner::Vitest)
        );
        assert_eq!(
            TestRunner::of("pnpm run test", Some("jest --ci")),
            Some(TestRunner::Jest)
        );
        assert_eq!(TestRunner::of("cargo test", None), None);
        assert_eq!(TestRunner::of("npm test", Some("mocha")), None);
    }

    #[test]
    fn a_junit_report_gives_pytest_ids() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?><testsuites><testsuite name="pytest" errors="0" failures="2" skipped="1" tests="5">
<testcase classname="tests.test_auth.TestLogin" name="test_ok" file="tests/test_auth.py" time="0.01" />
<testcase classname="tests.test_auth.TestLogin" name="test_bad" time="0.01"><failure message="assert 1 == 2">assert 1 == 2</failure></testcase>
<testcase classname="tests.test_math" name="test_add[1-2]" time="0.01"><error message="boom">E</error></testcase>
<testcase classname="tests.test_math" name="test_skip" time="0.0"><skipped message="no" /></testcase>
<testcase classname="tests.test_math" name="test_mul" time="0.01"></testcase>
</testsuite></testsuites>"#;
        let results = parse_junit(xml).unwrap();
        assert_eq!(results.passed, 2);
        assert_eq!(
            results.failed.into_iter().collect::<Vec<_>>(),
            vec![
                "tests/test_auth.py::TestLogin::test_bad".to_owned(),
                "tests/test_math.py::test_add[1-2]".to_owned(),
            ]
        );
        assert!(parse_junit("not xml").is_none());
    }

    #[test]
    fn vitest_and_jest_failure_lines_give_test_ids() {
        let vitest = "\
 RUN  v3.2.4 /repo

 ❯ src/math.test.ts (3 tests | 2 failed) 5ms
   × adds 2ms
⎯⎯⎯⎯⎯⎯⎯ Failed Tests 2 ⎯⎯⎯⎯⎯⎯⎯

 FAIL  src/math.test.ts > math > adds
AssertionError: expected 3 to be 4
 FAIL  src/math.test.ts > math > multiplies
AssertionError: expected 6 to be 9

 Test Files  1 failed (1)
      Tests  2 failed | 1 passed (3)
";
        let results = parse_vitest(vitest).unwrap();
        assert_eq!(results.passed, 1);
        assert_eq!(
            results.failed.into_iter().collect::<Vec<_>>(),
            vec![
                "src/math.test.ts > math > adds".to_owned(),
                "src/math.test.ts > math > multiplies".to_owned()
            ]
        );
        let jest = "\
FAIL src/math.test.js
  math
    ✓ subtracts (2 ms)
    ✕ adds (3 ms)

  ● math › adds

    expect(received).toBe(expected)

PASS src/other.test.js

Tests:       1 failed, 3 passed, 4 total
";
        let results = parse_jest(jest).unwrap();
        assert_eq!(results.passed, 3);
        assert_eq!(
            results.failed.into_iter().collect::<Vec<_>>(),
            vec!["src/math.test.js › math › adds".to_owned()]
        );
        assert!(parse_vitest("no tests here").is_none());
    }

    #[test]
    fn a_renamed_file_keeps_its_tests() {
        let base: BTreeSet<String> = [
            "tests/test_old.py::test_a".to_owned(),
            "tests/test_x.py::test_b".to_owned(),
        ]
        .into_iter()
        .collect();
        let renames: Renames = [(
            "api/tests/test_old.py".to_owned(),
            "api/tests/test_new.py".to_owned(),
        )]
        .into_iter()
        .collect();
        let moved = follow_renames(&base, &renames, "api/");
        assert!(moved.contains("tests/test_new.py::test_a"));
        assert!(moved.contains("tests/test_x.py::test_b"));
    }
}
