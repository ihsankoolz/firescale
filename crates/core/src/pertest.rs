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
    /// `cargo test`, from its text output (ADR 0039).
    Cargo,
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
        } else if stems.windows(2).any(|pair| pair == ["cargo", "test"]) {
            Some(Self::Cargo)
        } else {
            None
        }
    }

    /// Reads this runner's results from a finished step: the JUnit report
    /// for pytest, the output for Vitest and Jest.
    #[must_use]
    pub fn read(self, stdout: &str, stderr: &str, junit: Option<&str>) -> Option<TestResults> {
        let output = || format!("{stdout}\n{stderr}");
        match self {
            Self::Pytest => junit.and_then(parse_junit).map(|mut results| {
                pytest_extras(&mut results, &output());
                results
            }),
            Self::Vitest => parse_vitest(&output()),
            Self::Jest => parse_jest(&output()),
            Self::Cargo => parse_cargo(stdout, stderr),
        }
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Pytest => "pytest",
            Self::Vitest => "Vitest",
            Self::Jest => "Jest",
            Self::Cargo => "cargo test",
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
    /// Why each failing test failed, where the runner says: its error's
    /// lines, without stack frames or code excerpts. A parser fills in the
    /// text; [`TestResults::hash_reasons`] then replaces it with hashes of
    /// the lines with what changes between runs taken out, so no text is
    /// kept (ADR 0043).
    pub reasons: BTreeMap<String, Vec<String>>,
    /// True when the output shows the results may not cover the whole run:
    /// more than one run of the runner in the step, or a test binary that
    /// ended without its results. Then per-test results cannot make a
    /// failure pre-existing on their own (ADR 0043).
    pub partial: bool,
}

impl TestResults {
    /// Replaces each reason's lines with short hashes of the lines,
    /// normalised by [`normalise_reason_line`] with `roots` (the folder the
    /// step ran in: the head, or a worktree) written as `<root>`.
    pub fn hash_reasons(&mut self, roots: &[String]) {
        for lines in self.reasons.values_mut() {
            let mut hashed: Vec<String> = lines
                .iter()
                .map(|line| normalise_reason_line(line, roots))
                .filter(|line| !line.is_empty())
                .map(|line| blake3::hash(line.as_bytes()).to_hex()[..16].to_owned())
                .collect();
            // The order of lines in a message is not the reason.
            hashed.sort();
            *lines = hashed;
        }
        self.reasons.retain(|_, lines| !lines.is_empty());
    }

    /// Adds lines to a reason, for failures reported one line at a time.
    fn append_reason(&mut self, id: &str, line: &str) {
        self.reasons
            .entry(id.to_owned())
            .or_default()
            .push(line.trim().to_owned());
    }

    fn add_reason(&mut self, id: &str, lines: &[String]) {
        let lines: Vec<String> = lines
            .iter()
            .map(|line| line.trim().to_owned())
            .filter(|line| !line.is_empty())
            .take(MAX_REASON_LINES)
            .collect();
        if !lines.is_empty() {
            // A runner that prints a summary of failures again keeps the first.
            self.reasons.entry(id.to_owned()).or_insert(lines);
        }
    }
}

/// At most this many lines of a test's error are its reason.
const MAX_REASON_LINES: usize = 40;

/// The id given to a run's unhandled errors (Vitest), which fail the step
/// beside its tests.
pub const UNHANDLED_ERRORS: &str = "(unhandled errors)";

/// The id given to a coverage threshold that is not met (Vitest, Jest),
/// which fails the step even when every test passed.
pub const COVERAGE_THRESHOLD: &str = "(coverage threshold)";

/// The roots of a run as they can appear in its output: as given, with
/// forward slashes, and resolved, longest first.
#[must_use]
pub fn roots_of(root: &std::path::Path) -> Vec<String> {
    let mut roots: Vec<String> = Vec::new();
    let shown = root.to_string_lossy().into_owned();
    roots.push(shown.replace('\\', "/"));
    roots.push(shown);
    if let Ok(real) = root.canonicalize() {
        let real = real.to_string_lossy().into_owned();
        let real = real
            .strip_prefix("\\\\?\\")
            .map_or(real.clone(), str::to_owned);
        roots.push(real.replace('\\', "/"));
        roots.push(real);
    }
    // Python's repr of a Windows path doubles its backslashes.
    let doubled: Vec<String> = roots
        .iter()
        .filter(|root| root.contains('\\'))
        .map(|root| root.replace('\\', "\\\\"))
        .collect();
    roots.extend(doubled);
    roots.retain(|root| root.len() > 1);
    roots.sort_by_key(|text| std::cmp::Reverse(text.len()));
    roots.dedup();
    roots
}

/// True for a line of a stack trace: `at f (file:1:2)`, Vitest's
/// `❯ file:1:2`, Python's `File "x.py", line 3`.
fn is_stack_frame(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("at ")
        || line.starts_with("❯ ")
        || line.starts_with("File \"")
        || line.starts_with("stack backtrace:")
}

/// True for the first line of an error: `TypeError: x is not a function`,
/// `Error: boom`, `AssertionError: expected 3 to be 4`.
fn is_error_headline(line: &str) -> bool {
    let line = line.trim_start();
    let Some((head, _)) = line.split_once(':') else {
        return false;
    };
    !head.contains(' ')
        && (head.ends_with("Error") || head.ends_with("Exception") || head == "Error")
}

/// True for a line of a code excerpt: `  3| x`, `> 4 |   expect(x)`,
/// `    |   ^`.
fn is_code_frame(line: &str) -> bool {
    let line = line.trim_start();
    let line = line.strip_prefix('>').unwrap_or(line).trim_start();
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    let rest = line[digits..].trim_start();
    (digits > 0 || line.starts_with('|')) && rest.starts_with('|')
}

/// Replaces `needle` in `text`, in any letter case (Windows paths).
fn replace_any_case(text: &str, needle: &str, with: &str) -> String {
    if needle.is_empty() {
        return text.to_owned();
    }
    let lower_text = text.to_lowercase();
    let lower_needle = needle.to_lowercase();
    // Only when lowercasing keeps byte offsets, which holds for paths
    // without letters that change length.
    if lower_text.len() != text.len() || lower_needle.len() != needle.len() {
        return text.replace(needle, with);
    }
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(found) = lower_text[at..].find(&lower_needle) {
        out.push_str(&text[at..at + found]);
        out.push_str(with);
        at += found + needle.len();
    }
    out.push_str(&text[at..]);
    out
}

/// A word that names a time: `12ms`, `1.5 s`, `3 seconds`.
const TIME_UNITS: [&str; 15] = [
    "ms", "s", "sec", "secs", "second", "seconds", "m", "min", "mins", "minute", "minutes", "h",
    "us", "µs", "ns",
];

/// A line of a test's error with what changes from run to run taken out
/// (ADR 0043): the run's folders (`<root>`) and the temp folder
/// (`<temp>/*`), durations (`<time>`), clock times and dates, the line and
/// column numbers after a file name, `line 12` in Python tracebacks,
/// memory addresses, and runs of spaces. Other numbers stay: an expected
/// or received value is part of the reason.
#[must_use]
pub fn normalise_reason_line(line: &str, roots: &[String]) -> String {
    // Python's repr of a Windows path doubles its backslashes.
    let mut line = crate::verify::strip_ansi(line).trim().replace("\\\\", "\\");
    for root in roots {
        line = replace_any_case(&line, root, "<root>");
    }
    let temp = std::env::temp_dir();
    let mut temps = roots_of(&temp);
    for extra in ["/tmp", "/var/folders", "/private/var/folders"] {
        temps.push(extra.to_owned());
    }
    for temp in temps.iter().filter(|temp| temp.len() > 1) {
        let replaced = replace_any_case(&line, temp, "\u{1}");
        if replaced.contains('\u{1}') {
            // The folder or file just inside the temp folder is random.
            let mut out = String::new();
            let mut parts = replaced.split('\u{1}');
            out.push_str(parts.next().unwrap_or(""));
            for part in parts {
                out.push_str("<temp>");
                // Keep the separator, drop the random name after it.
                let mut cut = part.len();
                let mut seen_name = false;
                for (index, c) in part.char_indices() {
                    let separator = c == '/' || c == '\\';
                    if separator && !seen_name {
                        continue;
                    }
                    if separator || c.is_whitespace() || "'\"`),:;".contains(c) {
                        cut = index;
                        break;
                    }
                    seen_name = true;
                }
                out.push_str("/*");
                out.push_str(&part[cut..]);
            }
            line = out;
        }
    }
    let words = fold_times(&line);
    let words = fold_locations(&words);
    words.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Durations, clock times and dates as `<time>`; hex addresses as `0x#`.
fn fold_times(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        let starts_word = index == 0 || !chars[index - 1].is_alphanumeric();
        if c == '0' && starts_word && chars.get(index + 1) == Some(&'x') {
            let mut end = index + 2;
            while end < chars.len() && chars[end].is_ascii_hexdigit() {
                end += 1;
            }
            if end > index + 2 {
                out.push_str("0x#");
                index = end;
                continue;
            }
        }
        if c.is_ascii_digit() && starts_word {
            // A run of digits with `.`, `:`, `-`, `T` and `Z` between them:
            // a number, a clock time or a date.
            let mut end = index;
            while end < chars.len()
                && (chars[end].is_ascii_digit()
                    || (matches!(chars[end], '.' | ':' | '-' | 'T' | 'Z')
                        && chars.get(end + 1).is_some_and(char::is_ascii_digit)))
            {
                end += 1;
            }
            let number: String = chars[index..end].iter().collect();
            // A clock time with seconds, or a date with one: `14:08:27`,
            // `2026-11-13T10:22:33.123`, `2026-11-13 10:22:33`. Not
            // `10:30` or `127.0.0.1:5432`, which may be values.
            if is_clock(&number) || is_date_time(&number) {
                out.push_str("<time>");
                index = end;
                continue;
            }
            if is_date(&number) && chars.get(end) == Some(&' ') {
                let mut next = end + 1;
                while next < chars.len()
                    && (chars[next].is_ascii_digit()
                        || (matches!(chars[next], '.' | ':')
                            && chars.get(next + 1).is_some_and(char::is_ascii_digit)))
                {
                    next += 1;
                }
                let clock: String = chars[end + 1..next].iter().collect();
                if is_clock(&clock) {
                    out.push_str("<time>");
                    index = next;
                    continue;
                }
            }
            // A number followed by a unit of time that ends the word; a
            // one-letter unit only right after the number (`2m`, not `2 m`).
            let spaced = chars.get(end) == Some(&' ');
            let after = if spaced { end + 1 } else { end };
            let mut unit_end = after;
            while unit_end < chars.len() && chars[unit_end].is_alphabetic() {
                unit_end += 1;
            }
            let unit: String = chars[after..unit_end].iter().collect();
            let ends_word = chars.get(unit_end).is_none_or(|c| !c.is_alphanumeric());
            let one_letter = unit.chars().count() == 1 && unit != "s";
            if unit_end > after
                && ends_word
                && TIME_UNITS.contains(&unit.as_str())
                && !(spaced && one_letter)
                && !number.contains(':')
                && number.matches('.').count() <= 1
            {
                out.push_str("<time>");
                index = unit_end;
                continue;
            }
            out.push_str(&number);
            index = end;
            continue;
        }
        out.push(c);
        index += 1;
    }
    out
}

/// `14:08:27` or `14:08:27.123`: a clock time with seconds.
fn is_clock(text: &str) -> bool {
    let (main, fraction) = text.split_once('.').unwrap_or((text, "0"));
    let parts: Vec<&str> = main.split(':').collect();
    let two = |part: &str, max: u32| {
        part.len() == 2 && part.parse::<u32>().is_ok_and(|value| value < max)
    };
    parts.len() == 3
        && two(parts[0], 24)
        && two(parts[1], 60)
        && two(parts[2], 61)
        && !fraction.is_empty()
        && fraction.chars().all(|c| c.is_ascii_digit())
}

/// `2026-11-13`.
fn is_date(text: &str) -> bool {
    let parts: Vec<&str> = text.split('-').collect();
    parts.len() == 3
        && parts[0].len() == 4
        && parts[1].len() == 2
        && parts[2].len() == 2
        && parts
            .iter()
            .all(|part| part.chars().all(|c| c.is_ascii_digit()))
}

/// `2026-11-13T10:22:33`, with any fraction of a second.
fn is_date_time(text: &str) -> bool {
    text.split_once('T')
        .is_some_and(|(date, time)| is_date(date) && is_clock(time))
}

/// `file.ts:12:5` as `file.ts:#`, and `line 12` as `line #`: where a line
/// is moves when code is added above it.
fn fold_locations(line: &str) -> String {
    let mut out = String::new();
    for (position, word) in line.split(' ').enumerate() {
        if position > 0 {
            out.push(' ');
        }
        let previous_is_line = out.trim_end().ends_with("line") || out.trim_end().ends_with("Line");
        if previous_is_line
            && word
                .trim_end_matches(',')
                .chars()
                .all(|c| c.is_ascii_digit())
            && !word.is_empty()
        {
            out.push('#');
            out.push_str(word.trim_start_matches(|c: char| c.is_ascii_digit()));
            continue;
        }
        // A path with a file extension, then `:digits` once or twice.
        let Some(colon) = word.find(':').filter(|at| {
            let before = &word[..*at];
            let ext = before.rsplit('.').next().unwrap_or("");
            before.contains('.')
                && !ext.is_empty()
                && ext.chars().all(char::is_alphanumeric)
                && ext.chars().any(char::is_alphabetic)
                && word[*at + 1..].starts_with(|c: char| c.is_ascii_digit())
        }) else {
            out.push_str(word);
            continue;
        };
        out.push_str(&word[..colon]);
        out.push_str(":#");
        let rest = word[colon + 1..].trim_start_matches(|c: char| c.is_ascii_digit() || c == ':');
        out.push_str(rest);
    }
    out
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
            // The reason is the failure's message: pytest's
            // `AssertionError: assert 3 == 4`, without the traceback.
            let at = body
                .find("<failure")
                .or_else(|| body.find("<error"))
                .unwrap_or(0);
            let inner = &body[at..];
            let tag_end = inner.find('>').unwrap_or(inner.len());
            let message = attribute(&inner[..tag_end], "message").or_else(|| {
                let text = inner.get(tag_end + 1..)?;
                let text = &text[..text.find('<').unwrap_or(text.len())];
                unescape(text)
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                    .map(str::to_owned)
            });
            if let Some(message) = message {
                let lines: Vec<String> = message.lines().map(str::to_owned).collect();
                results.add_reason(&id, &lines);
            }
            results.failed.insert(id);
        } else if !body.contains("<skipped") {
            results.passed += 1;
        }
        rest = &after[tag_end + 1..];
    }
    Some(results)
}

/// What pytest's JUnit report leaves out (ADR 0043): pytest-cov's
/// `Required test coverage of 80% not reached`, a failing test of its own,
/// and a step that ran pytest more than once (each run writes the same
/// report, so the last one hides the others).
fn pytest_extras(results: &mut TestResults, output: &str) {
    let mut sessions = 0;
    for raw in output.lines() {
        let line = clean(raw);
        if line.contains("test session starts") {
            sessions += 1;
        }
        if line.contains("Required test coverage of") && line.contains("not reached") {
            results.failed.insert(COVERAGE_THRESHOLD.to_owned());
            results.append_reason(COVERAGE_THRESHOLD, &line);
        }
    }
    if sessions > 1 {
        results.partial = true;
    }
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
    let mut in_unhandled = false;
    let mut summaries = 0;
    let mut block = Block::default();
    for raw in output.lines() {
        let line = clean(raw);
        if line.contains("Failed Tests") || line.contains("Failed Suites") {
            block.flush(&mut results);
            in_failures = true;
            in_unhandled = false;
            continue;
        }
        // Errors outside every test fail the run too (ADR 0043).
        if line.contains("Unhandled Errors") {
            block.flush(&mut results);
            in_failures = false;
            in_unhandled = true;
            results.failed.insert(UNHANDLED_ERRORS.to_owned());
            block.ids.push(UNHANDLED_ERRORS.to_owned());
            continue;
        }
        if line.contains("does not meet") && line.contains("threshold") {
            results.failed.insert(COVERAGE_THRESHOLD.to_owned());
            results.append_reason(COVERAGE_THRESHOLD, &line);
            continue;
        }
        if line.starts_with("Test Files ") {
            summaries += 1;
        }
        if line.starts_with("Test Files ") || line.starts_with("Tests ") {
            block.flush(&mut results);
            in_failures = false;
            in_unhandled = false;
        }
        if in_unhandled {
            // Unhandled errors' own banners are not their reason.
            if !line.contains("⎯⎯") {
                block.push(&line);
            }
            continue;
        }
        if in_failures && line.contains("⎯⎯") {
            block.flush(&mut results);
            continue;
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
                // FAIL lines one after the other share one error.
                if !block.lines.is_empty() || block.frames {
                    block.flush(&mut results);
                }
                block.ids.push(id.clone());
                results.failed.insert(id);
            }
            continue;
        }
        if in_failures {
            block.push(&line);
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
    results.partial = summaries > 1;
    block.flush(&mut results);
    (seen_summary || !results.failed.is_empty()).then_some(results)
}

/// The lines of one error in a runner's output, for the tests it belongs
/// to: everything up to the first stack frame, without code excerpts.
#[derive(Default)]
struct Block {
    ids: Vec<String>,
    lines: Vec<String>,
    /// True once a stack frame was seen: the rest is not the reason.
    frames: bool,
}

impl Block {
    fn push(&mut self, line: &str) {
        if self.ids.is_empty() {
            return;
        }
        // A second error of the same test (an `afterEach` failure, another
        // unhandled error) is part of its reason too.
        if self.frames && is_error_headline(line) {
            self.frames = false;
        }
        if self.frames {
            return;
        }
        if is_stack_frame(line) {
            self.frames = true;
            return;
        }
        if !is_code_frame(line) && !line.trim().is_empty() {
            self.lines.push(line.to_owned());
        }
    }

    fn flush(&mut self, results: &mut TestResults) {
        for id in &self.ids {
            results.add_reason(id, &self.lines);
        }
        *self = Self::default();
    }
}

/// Reads Jest's default reporter: a `FAIL path` line per failing file and a
/// `● suite › test` line per failing test, and its `Tests:` summary.
#[must_use]
pub fn parse_jest(output: &str) -> Option<TestResults> {
    let mut results = TestResults::default();
    let mut seen_summary = false;
    let mut file: Option<String> = None;
    let mut summaries = 0;
    let mut block = Block::default();
    for raw in output.lines() {
        let line = clean(raw);
        if line.starts_with("Jest: ") && line.contains("coverage threshold") {
            block.flush(&mut results);
            results.failed.insert(COVERAGE_THRESHOLD.to_owned());
            results.append_reason(COVERAGE_THRESHOLD, &line);
            continue;
        }
        if line.starts_with("Test Suites:") {
            summaries += 1;
        }
        let ends_block = line.starts_with("FAIL ")
            || line.starts_with("PASS ")
            || line.starts_with("● ")
            || line.starts_with("Test Suites:")
            || line.starts_with("Tests:")
            || line.starts_with("Summary of all failing tests");
        if ends_block {
            block.flush(&mut results);
        } else {
            block.push(&line);
        }
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
                    block.ids.push(file.clone());
                }
                continue;
            }
            if let Some(file) = &file {
                let id = format!("{file} › {}", name.trim());
                block.ids.push(id.clone());
                results.failed.insert(id);
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
    results.partial = summaries > 1;
    block.flush(&mut results);
    (seen_summary || !results.failed.is_empty()).then_some(results)
}

/// The test binary a cargo progress line names: `Running unittests
/// src/lib.rs (target/debug/deps/core-1a2b)` or the older `Running
/// target/debug/deps/core-1a2b` give `core`; `Doc-tests core` gives
/// `core (doc)`.
fn cargo_binary(line: &str) -> Option<String> {
    if let Some(name) = line.strip_prefix("Doc-tests ") {
        return Some(format!("{} (doc)", name.trim()));
    }
    let rest = line.strip_prefix("Running ")?;
    let path = rest.split_whitespace().last()?;
    let path = path.trim_start_matches('(').trim_end_matches(')');
    let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let file = file.strip_suffix(".exe").unwrap_or(file);
    Some(
        file.rsplit_once('-')
            .map_or(file, |(name, _)| name)
            .to_owned(),
    )
}

/// Reads `cargo test`'s text output: `test path ... ok` and `... FAILED`
/// lines, each under the test binary it belongs to, so that the same test
/// name in two crates is two tests: `firescale_core::verify::tests::a_test`.
/// Cargo prints a binary's `Running ...` or `Doc-tests ...` line on stderr
/// and its tests on stdout, so the k-th `running N tests` block of stdout
/// is the k-th binary of stderr. A doc test's `(line N)` is left out of its
/// id, since lines move.
#[must_use]
pub fn parse_cargo(stdout: &str, stderr: &str) -> Option<TestResults> {
    let binaries: Vec<String> = stderr
        .lines()
        .filter_map(|line| cargo_binary(&clean(line)))
        .collect();
    // One stream holding both (or a cargo that prints both on stdout).
    let together = binaries.is_empty();
    let mut results = TestResults::default();
    let mut binary = String::new();
    let mut block_index = 0;
    let mut results_seen = 0;
    let mut block = Block::default();
    let mut seen = false;
    for raw in stdout.lines() {
        let line = clean(raw);
        if together {
            if let Some(name) = cargo_binary(&line) {
                binary = name;
                continue;
            }
        } else if line.starts_with("running ")
            && (line.ends_with(" test") || line.ends_with(" tests"))
        {
            block.flush(&mut results);
            binary = binaries.get(block_index).cloned().unwrap_or_default();
            block_index += 1;
            continue;
        }
        if line.starts_with("test result:") {
            block.flush(&mut results);
            results_seen += 1;
            seen = true;
            continue;
        }
        // `---- tests::a stdout ----` starts a failing test's output.
        if let Some(name) = line
            .strip_prefix("---- ")
            .and_then(|rest| rest.strip_suffix(" stdout ----"))
        {
            block.flush(&mut results);
            let name = name.trim();
            block.ids.push(if binary.is_empty() {
                name.to_owned()
            } else {
                format!("{binary}::{name}")
            });
            continue;
        }
        if line == "failures:" {
            block.flush(&mut results);
            continue;
        }
        if !block.ids.is_empty() {
            // The thread line names the test and where it panicked; the
            // note about backtraces is the same for every failure.
            if !line.starts_with("note: ") {
                block.push(&line);
            }
            continue;
        }
        let Some(rest) = line.strip_prefix("test ") else {
            continue;
        };
        let Some((name, outcome)) = rest.rsplit_once(" ... ") else {
            continue;
        };
        let mut name = name.trim().to_owned();
        if name.ends_with(')') {
            if let Some(cut) = name.rfind(" (line ") {
                name.truncate(cut);
            }
        }
        let id = if binary.is_empty() {
            name
        } else {
            format!("{binary}::{name}")
        };
        match outcome.trim() {
            "ok" => results.passed += 1,
            "FAILED" => {
                results.failed.insert(id);
            }
            _ => {}
        }
    }
    // A test binary that crashed printed no `test result:` line.
    results.partial = !together && results_seen < binaries.len();
    block.flush(&mut results);
    (seen || !results.failed.is_empty()).then_some(results)
}

/// True when a test run stopped at its first failure, so a later new
/// failure cannot be ruled out: `cargo test` without `--no-fail-fast`
/// (one failed binary, and `error: test failed, to rerun pass ...`), pytest
/// with `-x`, `--exitfirst` or `--maxfail`, Jest or Vitest with `--bail`.
#[must_use]
pub fn stopped_at_first_failure(
    runner: TestRunner,
    command: &str,
    script: Option<&str>,
    output: &str,
) -> bool {
    let text = format!("{command} {}", script.unwrap_or(""));
    let words: Vec<&str> = text.split_whitespace().collect();
    match runner {
        TestRunner::Cargo => {
            output.contains("error: test failed, to rerun pass")
                && !output.contains("targets failed")
        }
        TestRunner::Pytest => words
            .iter()
            .any(|word| *word == "-x" || *word == "--exitfirst" || word.starts_with("--maxfail")),
        TestRunner::Vitest | TestRunner::Jest => {
            words.iter().any(|word| word.starts_with("--bail"))
        }
    }
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
        assert_eq!(TestRunner::of("cargo build", None), None);
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
    fn a_reason_leaves_out_what_changes_between_runs_and_keeps_values() {
        let roots = vec!["/home/me/repo".to_owned(), "C:\\Users\\me\\repo".to_owned()];
        let same = |a: &str, b: &str| {
            assert_eq!(
                normalise_reason_line(a, &roots),
                normalise_reason_line(b, &roots),
                "{a} | {b}"
            );
        };
        let differ = |a: &str, b: &str| {
            assert_ne!(
                normalise_reason_line(a, &roots),
                normalise_reason_line(b, &roots)
            );
        };
        same("took 12ms", "took 1.5 s");
        same(
            "at 14:08:27 on 2026-11-13T10:22:33Z",
            "at 09:01:02 on 2026-11-14T08:00:01Z",
        );
        same(
            "logged 2026-11-13 10:22:33.120",
            "logged 2026-11-14 08:00:01.5",
        );
        differ(
            "expected '10:30' to be '10:00'",
            "expected '25:99' to be '10:00'",
        );
        differ("connect 127.0.0.1:5432", "connect 127.0.0.1:6379");
        differ("call 555-123-4567", "call 555-123-9999");
        differ("found 2 h1", "found 3 h1");
        differ("expected 2026-11-13", "expected 2026-11-14");
        same(
            "open 'C:\\\\Users\\\\me\\\\repo\\\\x.json'",
            "open 'C:\\Users\\me\\repo\\x.json'",
        );
        same(
            "open '/home/me/repo/data.json'",
            "open 'c:\\users\\me\\repo/data.json'",
        );
        same("src/a.test.ts:5:15 failed", "src/a.test.ts:12:3 failed");
        same("File x.py, line 12", "File x.py, line 120");
        same("object at 0x7f3a2b", "object at 0x10ff00");
        let temp = std::env::temp_dir();
        same(
            &format!(
                "open '{}'",
                temp.join("vitest-abc123").join("g.json").display()
            ),
            &format!(
                "open '{}'",
                temp.join("vitest-zzz9").join("g.json").display()
            ),
        );
        differ("expected 3 to be 4", "expected 5 to be 4");
        differ(
            "TypeError: x is not a function",
            "AssertionError: expected 3 to be 4",
        );
        differ("{ a: 1 }", "{ a: 2 }");
    }

    #[test]
    fn each_runner_gives_each_failing_test_its_reason() {
        let vitest = "\
⎯⎯⎯⎯⎯⎯⎯ Failed Tests 3 ⎯⎯⎯⎯⎯⎯⎯

 FAIL  src/math.test.ts > math > adds
 FAIL  src/math.test.ts > math > adds again
AssertionError: expected 3 to be 4
 ❯ src/math.test.ts:5:15
      5|     expect(x).toBe(4);
⎯⎯⎯⎯⎯⎯⎯⎯⎯[1/2]⎯
 FAIL  src/math.test.ts > math > divides
TypeError: x is not a function
      3| let x;
 Test Files  1 failed (1)
      Tests  3 failed | 1 passed (4)
";
        let mut results = parse_vitest(vitest).unwrap();
        assert_eq!(
            results.reasons["src/math.test.ts > math > adds"],
            vec!["AssertionError: expected 3 to be 4".to_owned()]
        );
        assert_eq!(
            results.reasons["src/math.test.ts > math > adds again"],
            results.reasons["src/math.test.ts > math > adds"]
        );
        assert_eq!(
            results.reasons["src/math.test.ts > math > divides"],
            vec!["TypeError: x is not a function".to_owned()]
        );
        results.hash_reasons(&[]);
        assert!(results
            .reasons
            .values()
            .flatten()
            .all(|line| line.len() == 16));

        let unhandled = "\
⎯⎯⎯⎯ Unhandled Errors ⎯⎯⎯⎯
Vitest caught 1 unhandled error during the test run.
Error: boom
 ❯ src/a.ts:1:1
 Test Files  1 passed (1)
      Tests  2 passed (2)
";
        let results = parse_vitest(unhandled).unwrap();
        assert!(results.failed.contains(UNHANDLED_ERRORS));
        assert_eq!(
            results.reasons[UNHANDLED_ERRORS],
            vec![
                "Vitest caught 1 unhandled error during the test run.".to_owned(),
                "Error: boom".to_owned()
            ]
        );
        // A second error, after the first one's stack, is part of the reason.
        let two = unhandled.replace(
            " Test Files",
            "TypeError: x is not a function\n ❯ src/b.ts:2:2\n Test Files",
        );
        assert!(parse_vitest(&two).unwrap().reasons[UNHANDLED_ERRORS]
            .contains(&"TypeError: x is not a function".to_owned()));
        // Every coverage line counts, and two runs in one step are partial.
        let coverage = "ERROR: Coverage for lines (50%) does not meet global threshold (80%)\nERROR: Coverage for functions (40%) does not meet global threshold (80%)\n Test Files  1 passed (1)\n      Tests  2 passed (2)\n Test Files  1 passed (1)\n      Tests  1 passed (1)\n";
        let results = parse_vitest(coverage).unwrap();
        assert_eq!(results.reasons[COVERAGE_THRESHOLD].len(), 2);
        assert!(results.partial);
        assert!(!parse_vitest(vitest).unwrap().partial);
        let jest = "\
FAIL src/math.test.js
  ● math › adds

    expect(received).toBe(expected) // Object.is equality

    Expected: 4
    Received: 3

      3 | test('adds', () => {
    > 4 |   expect(1 + 2).toBe(4);
        |                 ^

      at Object.toBe (src/math.test.js:4:17)

Tests:       1 failed, 3 passed, 4 total
Jest: \"global\" coverage threshold for lines (80%) not met: 50%
";
        let results = parse_jest(jest).unwrap();
        assert_eq!(
            results.reasons["src/math.test.js › math › adds"],
            vec![
                "expect(received).toBe(expected) // Object.is equality".to_owned(),
                "Expected: 4".to_owned(),
                "Received: 3".to_owned(),
            ]
        );
        assert!(results.failed.contains(COVERAGE_THRESHOLD));

        let junit = r#"<testsuite><testcase classname="tests.test_a" name="test_x" file="tests/test_a.py"><failure message="AssertionError: assert 3 == 4">trace</failure></testcase></testsuite>"#;
        let results = TestRunner::Pytest
            .read(
                "==== test session starts ====\nFAIL Required test coverage of 80% not reached. Total coverage: 61.20%\n",
                "",
                Some(junit),
            )
            .unwrap();
        assert!(results.failed.contains(COVERAGE_THRESHOLD));
        assert!(!results.partial);
        let twice = TestRunner::Pytest
            .read(
                "== test session starts ==\n== test session starts ==\n",
                "",
                Some(junit),
            )
            .unwrap();
        assert!(twice.partial);
        assert_eq!(
            results.reasons["tests/test_a.py::test_x"],
            vec!["AssertionError: assert 3 == 4".to_owned()]
        );

        let cargo_out = "\
running 1 test
test tests::a ... FAILED

failures:

---- tests::a stdout ----

thread 'tests::a' panicked at src/lib.rs:10:5:
assertion failed: 1 == 2
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    tests::a

test result: FAILED. 0 passed; 1 failed
";
        let cargo_err = "     Running unittests src/lib.rs (target/debug/deps/core-1a2b)\n";
        // A second binary that crashed before its results: partial.
        let crashed = format!(
            "{cargo_err}     Running tests/api.rs (target/debug/deps/api-9f8e)\nerror: test failed, to rerun pass `--test api`\n\nCaused by:\n  process didn't exit successfully (signal: 11, SIGSEGV: invalid memory reference)\n"
        );
        assert!(parse_cargo(cargo_out, &crashed).unwrap().partial);
        let results = parse_cargo(cargo_out, cargo_err).unwrap();
        assert!(!results.partial);
        assert_eq!(
            results.reasons["core::tests::a"],
            vec![
                "thread 'tests::a' panicked at src/lib.rs:10:5:".to_owned(),
                "assertion failed: 1 == 2".to_owned(),
            ]
        );
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
    fn cargo_test_output_gives_ids_per_crate() {
        let output = "\
     Running unittests src/lib.rs (target/debug/deps/firescale_core-5992d7290be67e72)

running 3 tests
test verify::tests::a ... ok
test verify::tests::b ... FAILED
test verify::tests::c ... ignored

failures:

test result: FAILED. 1 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out

     Running unittests src/main.rs (target/debug/deps/fscale-0a1b2c3d)

running 1 test
test verify::tests::b ... FAILED

test result: FAILED. 0 passed; 1 failed
   Doc-tests firescale_core

running 1 test
test src/lib.rs - doc (line 3) ... ok
";
        let results = parse_cargo(output, "").unwrap();
        assert_eq!(results.passed, 2);
        assert_eq!(
            results.failed.into_iter().collect::<Vec<_>>(),
            vec![
                "firescale_core::verify::tests::b".to_owned(),
                "fscale::verify::tests::b".to_owned()
            ]
        );
        assert_eq!(
            TestRunner::of("cargo test --workspace", None),
            Some(TestRunner::Cargo)
        );
        assert_eq!(
            TestRunner::of("npm test", Some("cargo test && vitest run")),
            Some(TestRunner::Vitest)
        );
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

    #[test]
    fn cargo_streams_are_paired_and_doc_test_lines_left_out() {
        let stderr = "\
   Compiling a v0.1.0
     Running unittests src/lib.rs (target/debug/deps/a-1111)
     Running unittests src/lib.rs (target/debug/deps/b-2222)
   Doc-tests a
error: test failed, to rerun pass `-p b --lib`
";
        let stdout = "running 1 test
test tests::same ... ok

test result: ok. 1 passed; 0 failed

running 1 test
test tests::same ... FAILED

test result: FAILED. 0 passed; 1 failed

running 1 test
test src/lib.rs - f (line 10) ... FAILED
";
        let results = parse_cargo(stdout, stderr).unwrap();
        assert_eq!(results.passed, 1);
        assert_eq!(
            results.failed.into_iter().collect::<Vec<_>>(),
            vec![
                "a (doc)::src/lib.rs - f".to_owned(),
                "b::tests::same".to_owned()
            ]
        );
        assert!(stopped_at_first_failure(
            TestRunner::Cargo,
            "cargo test",
            None,
            stderr
        ));
        assert!(!stopped_at_first_failure(
            TestRunner::Cargo,
            "cargo test --no-fail-fast",
            None,
            "error: 2 targets failed:\n error: test failed, to rerun pass"
        ));
        assert!(stopped_at_first_failure(
            TestRunner::Pytest,
            "pytest -x",
            None,
            ""
        ));
        assert!(stopped_at_first_failure(
            TestRunner::Jest,
            "npm test",
            Some("jest --bail"),
            ""
        ));
        assert_eq!(
            cargo_binary("Running target/debug/deps/old_style-abcd"),
            Some("old_style".to_owned())
        );
    }
}
