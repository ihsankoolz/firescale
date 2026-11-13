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

/// The identity of a pytest test case from its JUnit attributes
/// (`junit_family=xunit1`, which verify asks for, gives `file`):
/// `classname="tests.test_auth.TestLogin"`, `name="test_ok"`,
/// `file="tests/test_auth.py"` give `tests/test_auth.py::TestLogin::test_ok`,
/// pytest's own node id. A collection error has an empty class name and
/// the module in `name`.
fn pytest_id(classname: &str, name: &str, file: Option<&str>) -> String {
    if classname.is_empty() {
        return file.map_or_else(
            || format!("{}.py", name.replace('.', "/")),
            |file| file.replace('\\', "/"),
        );
    }
    let (file, classes): (String, Vec<&str>) = match file {
        Some(file) => {
            let file = file.replace('\\', "/");
            let module = file.strip_suffix(".py").unwrap_or(&file).replace('/', ".");
            let classes = classname
                .strip_prefix(&module)
                .map(|rest| rest.split('.').filter(|part| !part.is_empty()).collect())
                .unwrap_or_default();
            (file, classes)
        }
        None => {
            // No file: the class names are the parts that start with a
            // capital letter after the module path.
            let parts: Vec<&str> = classname.split('.').collect();
            let module_len = parts
                .iter()
                .position(|part| part.chars().next().is_some_and(char::is_uppercase))
                .unwrap_or(parts.len());
            (
                format!("{}.py", parts[..module_len].join("/")),
                parts[module_len..].to_vec(),
            )
        }
    };
    let mut id = file;
    for class in classes {
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

/// Reads Vitest's default reporter: the ` FAIL  file > suite > test` lines
/// of its `Failed Tests` and `Failed Suites` sections (not a `FAIL` a test
/// printed itself), and its `Tests  1 failed | 4 passed` line.
#[must_use]
pub fn parse_vitest(output: &str) -> Option<TestResults> {
    let mut results = TestResults::default();
    let mut seen_summary = false;
    let mut in_failures = false;
    for raw in output.lines() {
        let line = clean(raw);
        if line.contains("Failed Tests") || line.contains("Failed Suites") {
            in_failures = true;
            continue;
        }
        if line.starts_with("Test Files ") {
            in_failures = false;
        }
        if let Some(rest) = line.strip_prefix("FAIL ").filter(|_| in_failures) {
            let mut id = rest.trim().replace('\\', "/");
            // A trailing ` [ file ]` names the file of a suite-level failure.
            if id.ends_with(']') {
                if let Some(cut) = id.rfind(" [ ") {
                    id.truncate(cut);
                }
            }
            let id = id.trim().to_owned();
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
            // `FAIL src/a.test.js`, or with a project's display name
            // first: `FAIL web src/a.test.js`. The file is the last word
            // that looks like a path.
            file = rest
                .split_whitespace()
                .rev()
                .find(|word| word.contains('/') || word.contains('\\') || word.contains('.'))
                .map(|word| word.replace('\\', "/"));
            continue;
        }
        if line.starts_with("PASS ") {
            file = None;
            continue;
        }
        if let Some(name) = line.strip_prefix("● ") {
            // Only inside a failing file, and not Jest's own notices.
            let notice = [
                "Console",
                "Deprecation Warning",
                "Validation Warning",
                "Validation Error",
            ]
            .contains(&name.trim());
            if file.is_none() || notice {
                continue;
            }
            if name.starts_with("Test suite failed to run") {
                if let Some(file) = &file {
                    results.failed.insert(file.clone());
                }
                continue;
            }
            if let Some(file) = &file {
                results.failed.insert(format!("{file} › {}", name.trim()));
            }
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
                // An id is relative to the runner's root folder: the
                // project's (`tests/x.py`) or the repo's (`api/tests/x.py`).
                let pairs = [
                    (
                        old.strip_prefix(prefix).unwrap_or(old),
                        new.strip_prefix(prefix).unwrap_or(new),
                    ),
                    (old.as_str(), new.as_str()),
                ];
                for (old, new) in pairs {
                    if let Some(rest) = id.strip_prefix(old) {
                        if rest.is_empty() || rest.starts_with("::") || rest.starts_with(' ') {
                            return format!("{new}{rest}");
                        }
                    }
                }
            }
            id.clone()
        })
        .collect()
}

/// A path for the JUnit report of one pytest step: in the temp folder, with
/// a name of constant shape (digits are folded in signatures), unique per
/// process and step, so `--jobs` units never share one.
#[must_use]
pub fn junit_path() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let step = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    std::env::temp_dir().join(format!(
        "firescale-junit-{:010}-{step:06}.xml",
        std::process::id()
    ))
}

/// The `PYTEST_ADDOPTS` value that asks for a JUnit report at `path`, after
/// the user's own value. pytest splits it like a POSIX shell, so the path is
/// written with forward slashes and in single quotes: a space or a
/// backslash in it (a Windows temp folder) stays part of the path.
#[must_use]
pub fn addopts(own: &str, path: &std::path::Path) -> String {
    let path = path
        .to_string_lossy()
        .replace('\\', "/")
        .replace('\'', "'\"'\"'");
    format!("{own} -o junit_family=xunit1 '--junitxml={path}'")
        .trim()
        .to_owned()
}

/// True when the user asks pytest for a JUnit report already: on the
/// command line, in `PYTEST_ADDOPTS`, or in a pytest config file in the
/// step's folder or above (`addopts = --junitxml=...`).
#[must_use]
pub fn user_asks_for_junit(root: &std::path::Path, dir: &std::path::Path, command: &str) -> bool {
    if command.contains("--junitxml") || command.contains("--junit-xml") {
        return true;
    }
    if std::env::var("PYTEST_ADDOPTS").is_ok_and(|own| own.contains("--junit")) {
        return true;
    }
    let mut at = Some(dir);
    while let Some(folder) = at {
        for name in [
            "pytest.ini",
            "pyproject.toml",
            "setup.cfg",
            "tox.ini",
            ".pytest.ini",
        ] {
            if std::fs::read_to_string(folder.join(name)).is_ok_and(|text| text.contains("--junit"))
            {
                return true;
            }
        }
        if folder == root {
            break;
        }
        at = folder.parent().filter(|up| up.starts_with(root));
    }
    false
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

    /// Splits like Python's `shlex.split` in POSIX mode, enough for these
    /// tests: spaces, single and double quotes, backslash escapes.
    fn shlex(text: &str) -> Vec<String> {
        let mut words = Vec::new();
        let mut word = String::new();
        let mut started = false;
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            match c {
                '\'' => {
                    started = true;
                    for inner in chars.by_ref() {
                        if inner == '\'' {
                            break;
                        }
                        word.push(inner);
                    }
                }
                '"' => {
                    started = true;
                    while let Some(inner) = chars.next() {
                        match inner {
                            '"' => break,
                            '\\' => word.extend(chars.next()),
                            other => word.push(other),
                        }
                    }
                }
                '\\' => {
                    started = true;
                    word.extend(chars.next());
                }
                c if c.is_whitespace() => {
                    if started {
                        words.push(std::mem::take(&mut word));
                        started = false;
                    }
                }
                other => {
                    started = true;
                    word.push(other);
                }
            }
        }
        if started {
            words.push(word);
        }
        words
    }

    #[test]
    fn the_report_option_survives_pytest_splitting_it() {
        for path in [
            "/tmp/with space/firescale-junit-0000000001-000001.xml",
            r"C:\Users\John Smith\AppData\Local\Temp\firescale-junit-1.xml",
            "/tmp/it's/r.xml",
        ] {
            let value = addopts("-x --tb=short", std::path::Path::new(path));
            let words = shlex(&value);
            let expected = format!("--junitxml={}", path.replace('\\', "/"));
            assert_eq!(
                words,
                vec![
                    "-x",
                    "--tb=short",
                    "-o",
                    "junit_family=xunit1",
                    expected.as_str()
                ],
                "{value}"
            );
        }
    }

    #[test]
    fn report_paths_have_one_shape_and_never_repeat() {
        let a = junit_path();
        let b = junit_path();
        assert_ne!(a, b);
        let fold = |p: &std::path::Path| {
            p.to_string_lossy()
                .chars()
                .map(|c| if c.is_ascii_digit() { '#' } else { c })
                .collect::<String>()
        };
        assert_eq!(fold(&a), fold(&b));
    }

    #[test]
    fn junit_ids_use_the_file_and_handle_collection_errors() {
        let xml = r#"<testsuite name="pytest"><testcase classname="MyApp.test_m" name="test_x" file="MyApp/test_m.py"><failure message="x"/></testcase>
<testcase classname="" name="tests.test_broken"><error message="collection failure"/></testcase>
<testcase classname="api.tests.test_old.TestA" name="test_y" file="api/tests/test_old.py"><failure/></testcase></testsuite>"#;
        let failed: Vec<String> = parse_junit(xml).unwrap().failed.into_iter().collect();
        assert_eq!(
            failed,
            vec![
                "MyApp/test_m.py::test_x".to_owned(),
                "api/tests/test_old.py::TestA::test_y".to_owned(),
                "tests/test_broken.py".to_owned(),
            ]
        );
    }

    #[test]
    fn notices_and_printed_lines_are_not_failing_tests() {
        let jest = "\
PASS src/ok.test.js
  ● Console

    console.log
      FAIL looks like this

FAIL web src/math.test.js
  ● Console
  ● math › adds

Tests:       1 failed, 3 passed, 4 total
";
        let results = parse_jest(jest).unwrap();
        assert_eq!(
            results.failed.into_iter().collect::<Vec<_>>(),
            vec!["src/math.test.js › math › adds".to_owned()]
        );
        let vitest = "\
stdout | src/math.test.ts > math > logs
 FAIL  fake > printed by a test

⎯⎯⎯⎯⎯⎯⎯ Failed Tests 1 ⎯⎯⎯⎯⎯⎯⎯

 FAIL  src/math.test.ts > math > handles [ brackets ] in names
 FAIL  src/math.test.ts > outer [ src/math.test.ts ]

 Test Files  1 failed (1)
      Tests  1 failed | 2 passed (3)
";
        let results = parse_vitest(vitest).unwrap();
        assert_eq!(
            results.failed.into_iter().collect::<Vec<_>>(),
            vec![
                "src/math.test.ts > math > handles [ brackets ] in names".to_owned(),
                "src/math.test.ts > outer".to_owned()
            ]
        );
    }

    #[test]
    fn a_rename_matches_ids_from_the_project_or_the_repo_root() {
        let base: BTreeSet<String> = ["api/tests/test_old.py::test_a".to_owned()]
            .into_iter()
            .collect();
        let renames: Renames = [(
            "api/tests/test_old.py".to_owned(),
            "api/tests/test_new.py".to_owned(),
        )]
        .into_iter()
        .collect();
        assert!(follow_renames(&base, &renames, "api/").contains("api/tests/test_new.py::test_a"));
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
