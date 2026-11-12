//! What a running step has printed so far, read as it arrives (ADR 0025).
//!
//! The heartbeat line says what a long step is doing. Test runners such as
//! pytest and unittest print one character per test (`.`, `F`, `E`, `s`),
//! often with no line end until a file is done, so the last thing printed is
//! a marker like `E` that tells a person nothing. For those runners the
//! heartbeat gives a running count instead; for anything else it gives the
//! last line that contains words.
//!
//! The last lines are also kept, so verify can look for a service that is
//! not running while the step still runs.

use std::collections::VecDeque;

use crate::verify::strip_ansi;

/// How many complete lines are kept for [`Activity::recent`].
const RECENT_LINES: usize = 200;

/// Lines longer than this are cut when kept.
const MAX_LINE: usize = 500;

/// How long a line may be in a heartbeat.
const MAX_SHOWN: usize = 160;

/// A test runner whose progress markers are counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runner {
    /// pytest: `tests/test_a.py ..F.E [ 40%]`.
    Pytest,
    /// Python's unittest (also Django's `manage.py test`): `..F.E`.
    Unittest,
}

impl Runner {
    /// The runner a command line starts, when it is one of the known ones.
    #[must_use]
    pub fn from_command(command: &str) -> Option<Self> {
        let lower = command.to_ascii_lowercase();
        if lower.contains("pytest") || lower.contains("py.test") {
            Some(Self::Pytest)
        } else if lower.contains("unittest") || lower.contains("manage.py test") {
            Some(Self::Unittest)
        } else {
            None
        }
    }
}

/// Counts of the tests a runner has reported so far.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tally {
    pub passed: u64,
    pub failed: u64,
    pub errors: u64,
    pub skipped: u64,
}

impl Tally {
    fn add(&mut self, other: Self) {
        self.passed += other.passed;
        self.failed += other.failed;
        self.errors += other.errors;
        self.skipped += other.skipped;
    }

    /// True when no test has been counted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// `12 passed, 0 failed, 40 errors`, with skipped tests when there are
    /// any.
    #[must_use]
    pub fn describe(&self) -> String {
        let errors = if self.errors == 1 { "error" } else { "errors" };
        let mut text = format!(
            "{} passed, {} failed, {} {errors}",
            self.passed, self.failed, self.errors
        );
        if self.skipped > 0 {
            text.push_str(&format!(", {} skipped", self.skipped));
        }
        text
    }
}

/// What a line of output is.
enum Kind {
    /// Progress markers of a known runner, maybe after the file they belong to.
    Markers(Tally, Option<String>),
    /// A line a person can read.
    Words(String),
    /// Nothing worth showing: blank, or symbols only.
    Noise,
}

/// True when `text` has a run of two letters or more.
fn has_words(text: &str) -> bool {
    let mut run = 0;
    for c in text.chars() {
        if c.is_alphabetic() {
            run += 1;
            if run >= 2 {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

/// The counts of a string made only of progress markers, or `None` when it
/// holds anything else.
fn markers(text: &str) -> Option<Tally> {
    let mut tally = Tally::default();
    for c in text.chars() {
        match c {
            '.' => tally.passed += 1,
            'F' => tally.failed += 1,
            'E' => tally.errors += 1,
            's' => tally.skipped += 1,
            // Expected failures and unexpected successes are not counted.
            'x' | 'X' | 'u' => {}
            c if c.is_whitespace() => {}
            _ => return None,
        }
    }
    Some(tally)
}

/// A progress line of a known runner: optional file, markers, optional
/// `[ 40%]`. pytest prints the file name before its first marker.
fn marker_line(line: &str) -> Option<(Tally, Option<String>)> {
    let mut rest = line.trim();
    // `[ 40%]` or `[100%]` at the end.
    if let Some(open) = rest.rfind('[') {
        let inside = rest[open + 1..].trim_end();
        if inside.ends_with("%]")
            && inside[..inside.len() - 2]
                .trim()
                .chars()
                .all(|c| c.is_ascii_digit())
        {
            rest = rest[..open].trim_end();
        }
    }
    let mut file = None;
    if let Some((first, after)) = rest.split_once(' ').or(Some((rest, ""))) {
        let looks_like_file = first.ends_with(".py")
            || first.contains(".py::")
            || (first.contains('/') || first.contains('\\')) && has_words(first);
        if looks_like_file {
            file = Some(first.to_owned());
            rest = after.trim_start();
        }
    }
    if rest.is_empty() && file.is_none() {
        return None;
    }
    let tally = markers(rest)?;
    if file.is_none() && tally.is_empty() {
        return None;
    }
    Some((tally, file))
}

/// The output of one running command, read as it arrives.
#[derive(Debug, Clone)]
pub struct Activity {
    runner: Option<Runner>,
    /// The unfinished last line of stdout and of stderr.
    partial: [String; 2],
    /// Markers on the unfinished lines, not yet in `tally`.
    partial_tally: [Tally; 2],
    /// Markers on finished lines.
    tally: Tally,
    /// The file the latest markers belong to.
    file: Option<String>,
    /// The last line that contains words.
    last_words: Option<String>,
    /// True when the latest thing printed was progress markers.
    markers_last: bool,
    /// The last complete lines, oldest first.
    recent: VecDeque<String>,
    /// Goes up each time output arrives.
    generation: u64,
}

impl Activity {
    /// Reads the output of `command`.
    #[must_use]
    pub fn new(command: &str) -> Self {
        Self {
            runner: Runner::from_command(command),
            partial: [String::new(), String::new()],
            partial_tally: [Tally::default(); 2],
            tally: Tally::default(),
            file: None,
            last_words: None,
            markers_last: false,
            recent: VecDeque::new(),
            generation: 0,
        }
    }

    /// The runner whose markers are counted, if any.
    #[must_use]
    pub fn runner(&self) -> Option<Runner> {
        self.runner
    }

    /// Goes up each time output arrives.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The tests counted so far, finished lines and unfinished ones.
    #[must_use]
    pub fn tally(&self) -> Tally {
        let mut tally = self.tally;
        tally.add(self.partial_tally[0]);
        tally.add(self.partial_tally[1]);
        tally
    }

    /// The last complete lines, oldest first, without colour codes.
    #[must_use]
    pub fn recent(&self) -> Vec<String> {
        self.recent.iter().cloned().collect()
    }

    fn classify(&self, line: &str) -> Kind {
        let line = line.trim();
        if line.is_empty() {
            return Kind::Noise;
        }
        if self.runner.is_some() {
            if let Some((tally, file)) = marker_line(line) {
                return Kind::Markers(tally, file);
            }
        }
        if has_words(line) {
            Kind::Words(line.chars().take(MAX_SHOWN).collect())
        } else {
            Kind::Noise
        }
    }

    /// Adds a chunk of output from stdout (`stream` 0) or stderr (1).
    pub fn feed(&mut self, stream: usize, chunk: &[u8]) {
        let stream = stream.min(1);
        self.generation += 1;
        let text = String::from_utf8_lossy(chunk);
        let mut pending = std::mem::take(&mut self.partial[stream]);
        pending.push_str(&text);
        // A carriage return ends a line as far as a person can see.
        let mut lines: Vec<&str> = pending.split(['\n', '\r']).collect();
        let unfinished = lines.pop().unwrap_or_default().to_owned();
        for line in lines {
            let line = strip_ansi(line);
            if self.runner.is_none() && line.contains("test session starts") {
                self.runner = Some(Runner::Pytest);
            }
            match self.classify(&line) {
                Kind::Markers(tally, file) => {
                    self.tally.add(tally);
                    if file.is_some() {
                        self.file = file;
                    }
                    self.markers_last = true;
                }
                Kind::Words(words) => {
                    self.last_words = Some(words);
                    self.markers_last = false;
                }
                Kind::Noise => {}
            }
            let trimmed = line.trim_end();
            if !trimmed.is_empty() {
                if self.recent.len() == RECENT_LINES {
                    self.recent.pop_front();
                }
                self.recent
                    .push_back(trimmed.chars().take(MAX_LINE).collect());
            }
        }
        self.partial_tally[stream] = Tally::default();
        let shown = strip_ansi(&unfinished);
        match self.classify(&shown) {
            Kind::Markers(tally, file) => {
                self.partial_tally[stream] = tally;
                if file.is_some() {
                    self.file = file;
                }
                self.markers_last = true;
            }
            Kind::Words(words) => {
                self.last_words = Some(words);
                self.markers_last = false;
            }
            Kind::Noise => {}
        }
        // A runaway line with no end is not kept whole.
        self.partial[stream] = if unfinished.len() > 64 * 1024 {
            unfinished
                .chars()
                .rev()
                .take(MAX_LINE)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect()
        } else {
            unfinished
        };
    }

    /// What the heartbeat says the step is doing: `tests so far: 12 passed,
    /// 0 failed, 40 errors (in tests/test_b.py)` for a known runner whose
    /// markers came last, otherwise `last output: <the last line with
    /// words>`. `None` when nothing readable was printed.
    #[must_use]
    pub fn heartbeat(&self) -> Option<String> {
        let tally = self.tally();
        if self.runner.is_some() && self.markers_last && !tally.is_empty() {
            let file = self
                .file
                .as_ref()
                .map_or_else(String::new, |file| format!(" (in {file})"));
            return Some(format!("tests so far: {}{file}", tally.describe()));
        }
        self.last_words
            .as_ref()
            .map(|words| format!("last output: {words}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fed(command: &str, chunks: &[&str]) -> Activity {
        let mut activity = Activity::new(command);
        for chunk in chunks {
            activity.feed(0, chunk.as_bytes());
        }
        activity
    }

    #[test]
    fn pytest_markers_become_a_running_count() {
        // pytest writes one marker at a time, with no line end.
        let activity = fed(
            "python -m pytest",
            &[
                "============ test session starts ============\n",
                "platform linux -- Python 3.12.1, pytest-8.3.4\n",
                "collected 60 items\n\n",
                "tests/test_auth.py ..F.",
                "s  [ 10%]\ntests/test_orders.py ",
                "E",
                "E",
                "E",
            ],
        );
        assert_eq!(
            activity.heartbeat().as_deref(),
            Some("tests so far: 3 passed, 1 failed, 3 errors, 1 skipped (in tests/test_orders.py)")
        );
        assert_eq!(
            activity.tally(),
            Tally {
                passed: 3,
                failed: 1,
                errors: 3,
                skipped: 1
            }
        );
    }

    #[test]
    fn a_finished_marker_line_is_not_counted_twice() {
        let activity = fed("pytest", &["tests/a.py ..", "EE", " [100%]\n"]);
        assert_eq!(activity.tally().passed, 2);
        assert_eq!(activity.tally().errors, 2);
    }

    #[test]
    fn quiet_pytest_lines_without_a_file_are_counted() {
        let activity = fed("uv run pytest -q", &["..EE.F", "EEE"]);
        assert_eq!(
            activity.heartbeat().as_deref(),
            Some("tests so far: 3 passed, 1 failed, 5 errors")
        );
    }

    #[test]
    fn the_session_header_tells_pytest_apart_from_an_unknown_command() {
        let activity = fed(
            "npm run test:py",
            &["===== test session starts =====\n", "tests/a.py E"],
        );
        assert_eq!(activity.runner(), Some(Runner::Pytest));
        assert_eq!(
            activity.heartbeat().as_deref(),
            Some("tests so far: 0 passed, 0 failed, 1 error (in tests/a.py)")
        );
    }

    #[test]
    fn unittest_markers_on_stderr_are_counted() {
        let mut activity = Activity::new("python -m unittest discover");
        activity.feed(1, b"..E");
        activity.feed(1, b"F");
        assert_eq!(
            activity.heartbeat().as_deref(),
            Some("tests so far: 2 passed, 1 failed, 1 error")
        );
    }

    #[test]
    fn readable_output_after_markers_wins() {
        let activity = fed(
            "pytest",
            &[
                "tests/a.py ..E [100%]\n",
                "==== ERRORS ====\n",
                "Waiting for the database",
            ],
        );
        assert_eq!(
            activity.heartbeat().as_deref(),
            Some("last output: Waiting for the database")
        );
    }

    #[test]
    fn an_unknown_runner_shows_the_last_line_with_words() {
        let activity = fed(
            "node slow.js",
            &["building the index\n", "E\n", "...", "42"],
        );
        assert_eq!(
            activity.heartbeat().as_deref(),
            Some("last output: building the index")
        );
    }

    #[test]
    fn a_line_with_words_is_shown_as_it_is() {
        let activity = fed("node slow.js", &["working\n"]);
        assert_eq!(
            activity.heartbeat().as_deref(),
            Some("last output: working")
        );
    }

    #[test]
    fn nothing_readable_gives_nothing() {
        assert_eq!(fed("node x.js", &["...\n", "E"]).heartbeat(), None);
        assert_eq!(fed("pytest", &[]).heartbeat(), None);
    }

    #[test]
    fn colour_codes_and_carriage_returns_are_handled() {
        let activity = fed(
            "pytest",
            &[
                "\u{1b}[32mtests/a.py\u{1b}[0m ..\r\n",
                "\u{1b}[31mE\u{1b}[0m",
            ],
        );
        assert_eq!(activity.tally().passed, 2);
        assert_eq!(activity.tally().errors, 1);
    }

    #[test]
    fn pytest_failure_lines_are_words_not_markers() {
        // In the failure section, pytest starts explanation lines with "E".
        let activity = fed("pytest", &["E       assert 1 == 2\n"]);
        assert!(activity.tally().is_empty());
        assert_eq!(
            activity.heartbeat().as_deref(),
            Some("last output: E       assert 1 == 2")
        );
    }

    #[test]
    fn recent_lines_are_kept_without_colour_and_bounded() {
        let mut activity = Activity::new("x");
        for index in 0..(RECENT_LINES + 10) {
            activity.feed(0, format!("\u{1b}[1mline {index}\u{1b}[0m\n").as_bytes());
        }
        let recent = activity.recent();
        assert_eq!(recent.len(), RECENT_LINES);
        assert_eq!(recent.last().map(String::as_str), Some("line 209"));
        assert_eq!(recent.first().map(String::as_str), Some("line 10"));
    }

    #[test]
    fn runners_are_known_by_their_command() {
        assert_eq!(
            Runner::from_command(".venv/bin/python -m pytest"),
            Some(Runner::Pytest)
        );
        assert_eq!(
            Runner::from_command("python manage.py test"),
            Some(Runner::Unittest)
        );
        assert_eq!(Runner::from_command("npm run test:run"), None);
    }
}
