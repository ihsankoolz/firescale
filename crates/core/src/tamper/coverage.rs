//! Coverage thresholds (M25, ADR 0045): the floors a change lowers or
//! removes in the config files and command lines of the common coverage
//! tools, as [`Measure`]s of kind [`Kind::CoverageLowered`].
//!
//! - Jest: `coverageThreshold.<scope>.<metric>` in `jest.config.*` and in
//!   `package.json` under `jest`. A negative value ("at most N uncovered")
//!   is kept as it is, so `-10` is stricter than `-20`; a threshold that is
//!   gone is lower than any value.
//! - Vitest: `test.coverage.thresholds` (`<metric>`, `perFile`, `100`, glob
//!   scopes) and the older `test.coverage.<metric>`, in `vitest.config.*`,
//!   `vite.config.*` and `vitest.workspace.*`.
//! - nyc and c8: their rc files, `nyc.config.*` and `package.json` keys
//!   `nyc` and `c8`: the four metrics, `check-coverage`, `per-file`, `100`.
//!   A setting that is gone takes the tool's default (`lines` 90, the rest
//!   0, `check-coverage` off).
//! - coverage.py: `fail_under` in `.coveragerc`, `setup.cfg`, `tox.ini` and
//!   `pyproject.toml`, one key whichever file holds it.
//! - pytest-cov's `--cov-fail-under` in pytest `addopts`.
//! - `tarpaulin.toml` `fail-under`.
//! - Codecov: `coverage.status.{project,patch}.<name>` `target`, `threshold`
//!   (a higher allowed drop is weaker) and `informational`, and a status
//!   turned `off`.
//! - Command lines in `package.json` scripts, workflow `run:` steps,
//!   Makefiles, tox `commands` and pyproject task runners: pytest-cov,
//!   `coverage report --fail-under`, `cargo llvm-cov --fail-under-*`,
//!   `cargo tarpaulin --fail-under`, nyc and c8 flags and Vitest's
//!   `--coverage.thresholds.*`. Their keys name the tool and the flag.
//!
//! Percentages are kept ×100 (`82.5` is `8250`). A key names the folder the
//! tool runs in when it is not the repo root (`… in services/api`), so the
//! floors of two projects in one repo are compared apart while a setting
//! moved between the files one tool reads in one folder is not a finding.
//! Values that are not literal numbers (a variable) are skipped.

use super::config::Measure;
use super::formats::{self, Entry, Format, Scalar};
use super::Kind;
use crate::imports::js_dialect;

const METRICS: [&str; 4] = ["branches", "functions", "lines", "statements"];

/// The kinds of file this detector reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum File {
    PackageJson,
    Jest(Format),
    Vitest(Format),
    /// nyc or c8 settings; `None` for an rc file that may be JSON or YAML.
    Rc(&'static str, Option<Format>),
    CoverageRc,
    SetupCfg,
    ToxIni,
    PytestIni,
    Pyproject,
    Tarpaulin,
    Codecov,
    Workflow,
    Makefile,
}

fn split(path: &str) -> (&str, &str) {
    match path.rsplit_once('/') {
        Some((dir, name)) => (dir, name),
        None => ("", path),
    }
}

/// The format of a JS or JSON config file named `<stem>.<ext>`.
fn script_format(name: &str, stem: &str, json: bool) -> Option<Format> {
    let ext = name.strip_prefix(stem)?.strip_prefix('.')?;
    if ext.contains('.') {
        return None;
    }
    if ext.eq_ignore_ascii_case("json") {
        return json.then_some(Format::Json);
    }
    if ext.eq_ignore_ascii_case("jsx") || ext.eq_ignore_ascii_case("tsx") {
        return None;
    }
    js_dialect(name).map(Format::Js)
}

fn file_of(path: &str) -> Option<File> {
    let (dir, name) = split(path);
    let file = match name {
        "package.json" => File::PackageJson,
        ".nycrc" => File::Rc("nyc", None),
        ".nycrc.json" => File::Rc("nyc", Some(Format::Json)),
        ".nycrc.yml" | ".nycrc.yaml" => File::Rc("nyc", Some(Format::Yaml)),
        ".c8rc" | ".c8rc.json" => File::Rc("c8", Some(Format::Json)),
        ".coveragerc" => File::CoverageRc,
        "setup.cfg" => File::SetupCfg,
        "tox.ini" => File::ToxIni,
        "pytest.ini" => File::PytestIni,
        "pyproject.toml" => File::Pyproject,
        "tarpaulin.toml" | ".tarpaulin.toml" => File::Tarpaulin,
        "codecov.yml" | ".codecov.yml" | "codecov.yaml" | ".codecov.yaml" => File::Codecov,
        "Makefile" | "makefile" | "GNUmakefile" => File::Makefile,
        _ => {
            if (dir == ".github/workflows" || dir.ends_with("/.github/workflows"))
                && (name.ends_with(".yml") || name.ends_with(".yaml"))
            {
                File::Workflow
            } else if let Some(format) = script_format(name, "jest.config", true) {
                File::Jest(format)
            } else if let Some(format) = script_format(name, "vitest.config", false)
                .or_else(|| script_format(name, "vite.config", false))
                .or_else(|| script_format(name, "vitest.workspace", true))
            {
                File::Vitest(format)
            } else {
                File::Rc("nyc", Some(script_format(name, "nyc.config", false)?))
            }
        }
    };
    Some(file)
}

/// True for a repo-relative path (forward slashes) that can hold coverage
/// thresholds.
pub(crate) fn reads(path: &str) -> bool {
    file_of(path).is_some()
}

/// The coverage thresholds of a file, or the line where it cannot be read.
pub(crate) fn measures(path: &str, text: &str) -> Result<Vec<Measure>, usize> {
    let Some(file) = file_of(path) else {
        return Ok(Vec::new());
    };
    let (dir, _) = split(path);
    let mut out = Out {
        dir,
        lines: text.lines().collect(),
        measures: Vec::new(),
    };
    match file {
        File::PackageJson => {
            for entry in &formats::read(Format::Json, text)? {
                match entry.path.first().map(String::as_str) {
                    Some("jest") => out.jest(entry),
                    Some(tool @ ("nyc" | "c8")) => out.rc(tool, entry, 1),
                    Some("scripts") if entry.path.len() == 2 => {
                        if let Scalar::Str(command) = &entry.value {
                            out.commands(command, entry.line, dir);
                        }
                    }
                    _ => {}
                }
            }
        }
        File::Jest(format) => {
            for entry in &formats::read(format, text)? {
                out.jest(entry);
            }
        }
        File::Vitest(format) => {
            for entry in &formats::read(format, text)? {
                out.vitest(entry);
            }
        }
        File::Rc(tool, format) => {
            let entries = match format {
                Some(format) => formats::read(format, text)?,
                None => formats::read(Format::Json, text)
                    .or_else(|line| formats::read(Format::Yaml, text).map_err(|_| line))?,
            };
            for entry in &entries {
                out.rc(tool, entry, 0);
            }
        }
        File::CoverageRc | File::SetupCfg | File::ToxIni | File::PytestIni => {
            for entry in &formats::read(Format::Ini, text)? {
                let [section, key] = entry.path.as_slice() else {
                    continue;
                };
                let section = section.as_str();
                match key.as_str() {
                    "fail_under"
                        if section == "coverage:report"
                            || (file == File::CoverageRc && section == "report") =>
                    {
                        out.fail_under(entry, format!("fail_under = {}", entry.value.as_text()));
                    }
                    "addopts"
                        if (file == File::SetupCfg && section == "tool:pytest")
                            || (matches!(file, File::ToxIni | File::PytestIni)
                                && section == "pytest") =>
                    {
                        out.commands(&entry.value.as_text(), entry.line, dir);
                    }
                    "commands" if file == File::ToxIni && section.starts_with("testenv") => {
                        out.commands(&entry.value.as_text(), entry.line, dir);
                    }
                    _ => {}
                }
            }
        }
        File::Pyproject => {
            let entries = formats::read(Format::Toml, text)?;
            let mut addopts: Vec<&Entry> = Vec::new();
            for entry in &entries {
                let keys = keys(entry);
                match keys.as_slice() {
                    ["tool", "coverage", "report", "fail_under"] => {
                        out.fail_under(entry, format!("fail_under = {}", entry.value.as_text()));
                    }
                    ["tool", "pytest", "ini_options", "addopts"] => addopts.push(entry),
                    ["tool", "hatch", "envs", _, "scripts", ..]
                    | ["tool", "poe", "tasks", ..]
                    | ["tool", "pdm", "scripts", ..] => {
                        if let Scalar::Str(command) = &entry.value {
                            out.commands(command, entry.line, dir);
                        }
                    }
                    _ => {}
                }
            }
            if let Some(first) = addopts.first() {
                let joined: Vec<String> = addopts.iter().map(|e| e.value.as_text()).collect();
                out.commands(&joined.join(" "), first.line, dir);
            }
        }
        File::Tarpaulin => {
            for entry in &formats::read(Format::Toml, text)? {
                let keys = keys(entry);
                if let Some((&"fail-under", profile)) = keys.split_last() {
                    let Some(value) = percent(&entry.value) else {
                        continue;
                    };
                    let profile = if profile.is_empty() {
                        "default".to_owned()
                    } else {
                        profile.join(".")
                    };
                    out.max(
                        format!("tarpaulin fail-under ({profile})"),
                        true,
                        value,
                        0,
                        entry.line,
                        format!("fail-under = {}", entry.value.as_text()),
                    );
                }
            }
        }
        File::Codecov => {
            for entry in &formats::read(Format::Yaml, text)? {
                out.codecov(entry);
            }
        }
        File::Workflow => out.workflow(&formats::read(Format::Yaml, text)?),
        File::Makefile => {
            let mut logical = String::new();
            let mut start = 0;
            for (index, line) in text.lines().enumerate() {
                if logical.is_empty() {
                    start = index + 1;
                }
                let line = line.trim_end();
                if let Some(rest) = line.strip_suffix('\\') {
                    logical.push_str(rest);
                    logical.push(' ');
                    continue;
                }
                logical.push_str(line);
                let command = std::mem::take(&mut logical);
                if !command.trim_start().starts_with('#') {
                    out.commands(&command, start, dir);
                }
            }
            if !logical.is_empty() {
                out.commands(&logical, start, dir);
            }
        }
    }
    Ok(out.measures)
}

/// The path of an entry without list indexes.
fn keys(entry: &Entry) -> Vec<&str> {
    entry
        .path
        .iter()
        .filter(|segment| !formats::is_index(segment))
        .map(String::as_str)
        .collect()
}

/// A percentage ×100, from a literal number or a string like `80%`.
fn percent(value: &Scalar) -> Option<i64> {
    let number = match value {
        Scalar::Num(number) => *number,
        Scalar::Str(text) => text.trim().trim_end_matches('%').trim().parse().ok()?,
        _ => return None,
    };
    scaled(number)
}

fn scaled(number: f64) -> Option<i64> {
    let value = (number * 100.0).round();
    // Bounded, so the cast cannot saturate.
    (value.is_finite() && value.abs() < 1e15).then_some(value as i64)
}

fn boolean(value: &Scalar) -> Option<bool> {
    match value {
        Scalar::Bool(_) | Scalar::Str(_) => value.as_bool(),
        _ => None,
    }
}

/// `test.coverage.thresholds.lines: 80`, as a file writes it.
fn written(entry: &Entry) -> String {
    format!("{}: {}", entry.keys(), entry.value.as_text())
}

/// The folder `base` joined with a `cd` argument or a working directory;
/// one that cannot be known (a variable, an absolute path) is kept as
/// written, so it is the same on both sides.
fn join(base: &str, to: &str) -> String {
    let to = to.trim();
    if to.starts_with(['/', '$', '~', '\\']) || to.contains("${") || to == "-" {
        return to.to_owned();
    }
    let mut parts: Vec<&str> = base.split('/').filter(|p| !p.is_empty()).collect();
    for part in to.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return to.to_owned();
                }
            }
            _ => parts.push(part),
        }
    }
    parts.join("/")
}

/// The words of one simple command, quotes taken off.
fn words(segment: &str) -> Vec<&str> {
    segment
        .split_whitespace()
        .map(|word| word.trim_matches(['"', '\'']))
        .filter(|word| !word.is_empty())
        .collect()
}

/// The program a word names: `npx`, `node_modules/.bin/nyc` gives `nyc`.
fn program(word: &str) -> &str {
    word.rsplit(['/', '\\']).next().unwrap_or(word)
}

struct Out<'a> {
    dir: &'a str,
    lines: Vec<&'a str>,
    measures: Vec<Measure>,
}

impl Out<'_> {
    /// Adds a measure; `scoped` keys name the folder when it is not the root.
    fn max(
        &mut self,
        key: String,
        scoped: bool,
        value: i64,
        absent: i64,
        line: usize,
        what: String,
    ) {
        let key = if scoped && !self.dir.is_empty() {
            format!("{key} in {}", self.dir)
        } else {
            key
        };
        self.measures.push(Measure::max(
            Kind::CoverageLowered,
            key,
            value,
            absent,
            line,
            what,
        ));
    }

    /// The 1-based line, at or after `from`, that holds `needle`.
    fn locate(&self, from: usize, needle: &str) -> usize {
        self.lines
            .iter()
            .enumerate()
            .skip(from.saturating_sub(1))
            .take(64)
            .find(|(_, line)| line.contains(needle))
            .map_or(from, |(index, _)| index + 1)
    }

    fn jest(&mut self, entry: &Entry) {
        let keys = keys(entry);
        let Some(at) = keys.iter().position(|key| *key == "coverageThreshold") else {
            return;
        };
        let [scope, metric] = keys[at + 1..] else {
            return;
        };
        if !METRICS.contains(&metric) {
            return;
        }
        let Some(value) = percent_number(&entry.value) else {
            return;
        };
        self.max(
            format!("jest {metric} ({scope})"),
            true,
            value,
            i64::MIN,
            entry.line,
            written(entry),
        );
    }

    fn vitest(&mut self, entry: &Entry) {
        let keys = keys(entry);
        let Some(at) = keys.iter().rposition(|key| *key == "coverage") else {
            return;
        };
        let rest = &keys[at + 1..];
        let (scope, setting) = match rest {
            [setting] => ("global", *setting),
            ["thresholds", setting] => ("global", *setting),
            ["thresholds", scope, setting] => (*scope, *setting),
            _ => return,
        };
        self.setting("vitest", scope, setting, entry, i64::MIN);
    }

    /// A metric, `perFile` or `100` of `tool` in `scope`.
    fn setting(&mut self, tool: &str, scope: &str, setting: &str, entry: &Entry, absent: i64) {
        if METRICS.contains(&setting) {
            if let Some(value) = percent_number(&entry.value) {
                self.max(
                    format!("{tool} {setting} ({scope})"),
                    true,
                    value,
                    absent,
                    entry.line,
                    written(entry),
                );
            }
        } else if setting == "perFile" {
            if let Some(on) = boolean(&entry.value) {
                self.max(
                    format!("{tool} perFile ({scope})"),
                    true,
                    i64::from(on),
                    0,
                    entry.line,
                    written(entry),
                );
            }
        } else if setting == "100" && boolean(&entry.value) == Some(true) {
            for metric in METRICS {
                self.max(
                    format!("{tool} {metric} ({scope})"),
                    true,
                    10_000,
                    absent,
                    entry.line,
                    written(entry),
                );
            }
        }
    }

    /// nyc or c8 settings, at `depth` (1 under a `package.json` key).
    fn rc(&mut self, tool: &str, entry: &Entry, depth: usize) {
        let keys = keys(entry);
        if keys.len() != depth + 1 {
            return;
        }
        let what = written(entry);
        match keys[depth] {
            metric if METRICS.contains(&metric) => {
                if let Some(value) = percent_number(&entry.value) {
                    let absent = if metric == "lines" { 9_000 } else { 0 };
                    self.max(
                        format!("{tool} {metric}"),
                        true,
                        value,
                        absent,
                        entry.line,
                        what,
                    );
                }
            }
            "check-coverage" | "checkCoverage" => {
                if let Some(on) = boolean(&entry.value) {
                    self.max(
                        format!("{tool} check-coverage"),
                        true,
                        i64::from(on),
                        0,
                        entry.line,
                        what,
                    );
                }
            }
            "per-file" | "perFile" => {
                if let Some(on) = boolean(&entry.value) {
                    self.max(
                        format!("{tool} per-file"),
                        true,
                        i64::from(on),
                        0,
                        entry.line,
                        what,
                    );
                }
            }
            "100" if boolean(&entry.value) == Some(true) => {
                for metric in METRICS {
                    let absent = if metric == "lines" { 9_000 } else { 0 };
                    self.max(
                        format!("{tool} {metric}"),
                        true,
                        10_000,
                        absent,
                        entry.line,
                        what.clone(),
                    );
                }
                self.max(
                    format!("{tool} check-coverage"),
                    true,
                    1,
                    0,
                    entry.line,
                    what,
                );
            }
            _ => {}
        }
    }

    fn fail_under(&mut self, entry: &Entry, what: String) {
        if let Some(value) = percent(&entry.value) {
            self.max(
                "coverage.py fail_under".to_owned(),
                true,
                value,
                0,
                entry.line,
                what,
            );
        }
    }

    fn codecov(&mut self, entry: &Entry) {
        let keys = keys(entry);
        let ["coverage", "status", status @ ("project" | "patch"), rest @ ..] = keys.as_slice()
        else {
            return;
        };
        match rest {
            [] => {
                let on = match &entry.value {
                    Scalar::Str(text) if text.eq_ignore_ascii_case("off") => false,
                    value => match boolean(value) {
                        Some(on) => on,
                        None => return,
                    },
                };
                self.max(
                    format!("codecov {status} status"),
                    false,
                    i64::from(on),
                    1,
                    entry.line,
                    written(entry),
                );
            }
            [name, "target"] => {
                if let Some(value) = percent(&entry.value) {
                    self.max(
                        format!("codecov {status} target ({name})"),
                        false,
                        value,
                        0,
                        entry.line,
                        written(entry),
                    );
                }
            }
            [name, "threshold"] => {
                if let Some(value) = percent(&entry.value) {
                    self.max(
                        format!("codecov {status} threshold ({name})"),
                        false,
                        -value,
                        0,
                        entry.line,
                        written(entry),
                    );
                }
            }
            [name, "informational"] => {
                if let Some(informational) = boolean(&entry.value) {
                    self.max(
                        format!("codecov {status} informational ({name})"),
                        false,
                        i64::from(!informational),
                        1,
                        entry.line,
                        written(entry),
                    );
                }
            }
            _ => {}
        }
    }

    /// The `run:` steps of a GitHub Actions workflow, in their
    /// `working-directory`.
    fn workflow(&mut self, entries: &[Entry]) {
        let folder = |prefix: &[String]| -> Option<String> {
            entries
                .iter()
                .find(|e| {
                    e.path.len() == prefix.len() + 1
                        && e.path.starts_with(prefix)
                        && e.path.last().is_some_and(|k| k == "working-directory")
                })
                .map(|e| e.value.as_text())
        };
        let defaults = |prefix: &[String]| -> Option<String> {
            entries
                .iter()
                .find(|e| {
                    e.path.len() == prefix.len() + 3
                        && e.path.starts_with(prefix)
                        && e.path[prefix.len()..] == ["defaults", "run", "working-directory"]
                })
                .map(|e| e.value.as_text())
        };
        for entry in entries {
            if !(entry.path.len() >= 3
                && entry.path.last().is_some_and(|k| k == "run")
                && entry.path[entry.path.len() - 3] == "steps")
            {
                continue;
            }
            let Scalar::Str(command) = &entry.value else {
                continue;
            };
            let step = &entry.path[..entry.path.len() - 1];
            let job = &entry.path[..entry.path.len() - 3];
            let dir = folder(step)
                .or_else(|| defaults(job))
                .or_else(|| defaults(&[]))
                .map_or_else(String::new, |dir| join("", &dir));
            self.commands(command, entry.line, &dir);
        }
    }

    /// The coverage flags of a shell script or command line run in `dir`,
    /// whose first line is `line`.
    fn commands(&mut self, script: &str, line: usize, dir: &str) {
        let mut cwd = dir.to_owned();
        for (index, text) in script.lines().enumerate() {
            for segment in text.split(['&', '|', ';']) {
                let words = words(segment);
                match words.as_slice() {
                    ["cd", to, ..] => {
                        cwd = join(&cwd, to);
                    }
                    [] => {}
                    _ => {
                        self.flags(&words, line + index, &cwd);
                    }
                }
            }
        }
    }

    /// The coverage flags of one simple command.
    fn flags(&mut self, words: &[&str], line: usize, cwd: &str) {
        let has = |names: &[&str]| words.iter().any(|w| names.contains(&program(w)));
        let tarpaulin = has(&["tarpaulin", "cargo-tarpaulin"]);
        let llvm_cov = has(&["llvm-cov", "cargo-llvm-cov"]);
        let coverage_py = words
            .iter()
            .position(|w| matches!(program(w), "coverage" | "coverage3"))
            .is_some_and(|at| words[at + 1..].contains(&"report"));
        let istanbul = if has(&["nyc"]) {
            Some("nyc")
        } else if has(&["c8"]) {
            Some("c8")
        } else {
            None
        };
        let vitest = has(&["vitest"]);

        let mut found: Vec<(String, i64, String, &str)> = Vec::new();
        for (at, word) in words.iter().enumerate() {
            let (flag, inline) = match word.split_once('=') {
                Some((flag, value)) if flag.starts_with("--") => (flag, Some(value)),
                _ => (*word, None),
            };
            let next = words.get(at + 1).filter(|w| !w.starts_with('-')).copied();
            let number = || -> Option<(i64, String)> {
                match inline {
                    Some(value) => Some((scaled(value.trim().parse().ok()?)?, (*word).to_owned())),
                    None => {
                        let value = next?;
                        Some((scaled(value.parse().ok()?)?, format!("{word} {value}")))
                    }
                }
            };
            let switch = || -> (bool, String) {
                match inline {
                    Some(value) => (
                        Scalar::Str(value.to_owned()).as_bool().unwrap_or(true),
                        (*word).to_owned(),
                    ),
                    None => match next {
                        Some(value @ ("true" | "false")) => {
                            (value == "true", format!("{word} {value}"))
                        }
                        _ => (true, (*word).to_owned()),
                    },
                }
            };
            let tool_flag = match flag {
                "--cov-fail-under" => Some("pytest-cov --cov-fail-under".to_owned()),
                "--fail-under" if tarpaulin => Some("cargo tarpaulin --fail-under".to_owned()),
                "--fail-under" if coverage_py => Some("coverage report --fail-under".to_owned()),
                "--fail-under-lines" | "--fail-under-functions" | "--fail-under-regions"
                    if llvm_cov =>
                {
                    Some(format!("cargo llvm-cov {flag}"))
                }
                "--lines" | "--branches" | "--functions" | "--statements" => {
                    istanbul.map(|tool| format!("{tool} {flag}"))
                }
                _ => None,
            };
            if let Some(key) = tool_flag {
                if let Some((value, what)) = number() {
                    found.push((key, value, what, word));
                }
                continue;
            }
            if let Some(tool) = istanbul {
                match flag {
                    "--check-coverage" | "check-coverage" => {
                        let (on, what) = if flag == "check-coverage" {
                            (true, (*word).to_owned())
                        } else {
                            switch()
                        };
                        found.push((
                            format!("{tool} --check-coverage"),
                            i64::from(on),
                            what,
                            word,
                        ));
                    }
                    "--no-check-coverage" => {
                        found.push((
                            format!("{tool} --check-coverage"),
                            0,
                            (*word).to_owned(),
                            word,
                        ));
                    }
                    "--100" if switch().0 => {
                        for metric in METRICS {
                            found.push((
                                format!("{tool} --{metric}"),
                                10_000,
                                (*word).to_owned(),
                                word,
                            ));
                        }
                        found.push((
                            format!("{tool} --check-coverage"),
                            1,
                            (*word).to_owned(),
                            word,
                        ));
                    }
                    _ => {}
                }
                continue;
            }
            if vitest {
                let Some(setting) = flag.strip_prefix("--coverage.") else {
                    continue;
                };
                let setting = setting.strip_prefix("thresholds.").unwrap_or(setting);
                if METRICS.contains(&setting) {
                    if let Some((value, what)) = number() {
                        let key = format!("vitest --coverage.thresholds.{setting}");
                        found.push((key, value, what, word));
                    }
                } else if setting == "100" {
                    let (on, what) = switch();
                    if on {
                        for metric in METRICS {
                            let key = format!("vitest --coverage.thresholds.{metric}");
                            found.push((key, 10_000, what.clone(), word));
                        }
                    }
                } else if setting == "perFile" {
                    let (on, what) = switch();
                    found.push((
                        "vitest --coverage.thresholds.perFile".to_owned(),
                        i64::from(on),
                        what,
                        word,
                    ));
                }
            }
        }
        for (key, value, what, word) in found {
            let key = if cwd.is_empty() {
                key
            } else {
                format!("{key} in {cwd}")
            };
            let line = self.locate(line, word);
            self.measures.push(Measure::max(
                Kind::CoverageLowered,
                key,
                value,
                0,
                line,
                what,
            ));
        }
    }
}

/// A threshold that must be a literal number (Jest, Vitest, nyc).
fn percent_number(value: &Scalar) -> Option<i64> {
    match value {
        Scalar::Num(number) => scaled(*number),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::config::{compare, Located};
    use super::super::Finding;
    use super::*;

    fn located(files: &[(&str, &str)]) -> Vec<Located> {
        files
            .iter()
            .flat_map(|(path, text)| {
                assert!(reads(path), "{path} is read");
                measures(path, text)
                    .unwrap_or_else(|line| panic!("{path} cannot be read at line {line}"))
                    .into_iter()
                    .map(|measure| Located {
                        file: (*path).to_owned(),
                        measure,
                    })
            })
            .collect()
    }

    fn findings(base: &[(&str, &str)], head: &[(&str, &str)]) -> Vec<Finding> {
        let findings = compare(&located(base), &located(head));
        assert!(findings.iter().all(|f| f.kind == Kind::CoverageLowered));
        findings
    }

    fn whats(base: &[(&str, &str)], head: &[(&str, &str)]) -> Vec<String> {
        findings(base, head).into_iter().map(|f| f.what).collect()
    }

    fn keys_of(path: &str, text: &str) -> Vec<(String, i64)> {
        measures(path, text)
            .expect("readable")
            .into_iter()
            .map(|m| (m.key, m.value))
            .collect()
    }

    #[test]
    fn reads_coverage_config_files_and_not_other_files() {
        for path in [
            "package.json",
            "packages/a/package.json",
            "jest.config.js",
            "jest.config.ts",
            "jest.config.json",
            "vitest.config.mts",
            "vite.config.ts",
            "vitest.workspace.json",
            ".nycrc",
            ".nycrc.yaml",
            "nyc.config.cjs",
            ".c8rc.json",
            ".coveragerc",
            "setup.cfg",
            "tox.ini",
            "pytest.ini",
            "services/api/pyproject.toml",
            "tarpaulin.toml",
            ".github/codecov.yml",
            ".github/workflows/ci.yml",
            "Makefile",
        ] {
            assert!(reads(path), "{path}");
        }
        for path in [
            "src/jest.config.helper.js",
            "jest.config.js.map",
            "src/index.ts",
            "README.md",
            "Cargo.toml",
            "docs/workflows/ci.yml",
            ".github/dependabot.yml",
            "config.json",
            "vite.config.jsx",
        ] {
            assert!(!reads(path), "{path}");
        }
    }

    #[test]
    fn jest_thresholds_lowered_or_removed_are_found() {
        let base = "module.exports = {\n  coverageThreshold: {\n    global: { lines: 80, branches: 70 },\n    './src/api/': { statements: -10 },\n  },\n};\n";
        let lowered = "module.exports = {\n  coverageThreshold: {\n    global: { lines: 60, branches: 70 },\n    './src/api/': { statements: -20 },\n  },\n};\n";
        assert_eq!(
            whats(&[("jest.config.js", base)], &[("jest.config.js", lowered)]),
            [
                "coverageThreshold.global.lines: 60 (on the base: coverageThreshold.global.lines: 80, jest.config.js:3)",
                "coverageThreshold../src/api/.statements: -20 (on the base: coverageThreshold../src/api/.statements: -10, jest.config.js:4)",
            ]
        );
        let removed = "module.exports = {\n  coverageThreshold: {\n    global: { lines: 80, branches: 70 },\n  },\n};\n";
        let found = findings(&[("jest.config.js", base)], &[("jest.config.js", removed)]);
        assert_eq!(found.len(), 1);
        assert!(found[0].on_base);
        assert_eq!(found[0].line, 4);
    }

    #[test]
    fn jest_thresholds_raised_moved_or_reformatted_are_not_findings() {
        let base = "module.exports = { coverageThreshold: { global: { lines: 80 } } };\n";
        let raised = "import type { Config } from 'jest';\nconst config: Config = {\n  coverageThreshold: {\n    global: {\n      lines: 85,\n    },\n  },\n  verbose: true,\n};\nexport default config;\n";
        assert!(findings(&[("jest.config.js", base)], &[("jest.config.ts", raised)]).is_empty());
        let in_package = "{\n  \"name\": \"a\",\n  \"jest\": {\n    \"coverageThreshold\": { \"global\": { \"lines\": 80 } }\n  }\n}\n";
        assert!(findings(&[("jest.config.js", base)], &[("package.json", in_package)]).is_empty());
        // Another package's floor is another key.
        assert_eq!(
            findings(
                &[("jest.config.js", base)],
                &[("packages/a/package.json", in_package)]
            )
            .len(),
            1
        );
        // A variable is skipped on both sides.
        let variable = "const floor = 80;\nmodule.exports = { coverageThreshold: { global: { lines: floor } } };\n";
        assert!(keys_of("jest.config.js", variable).is_empty());
    }

    #[test]
    fn vitest_thresholds() {
        let base = "import { defineConfig } from 'vitest/config';\nexport default defineConfig({\n  test: {\n    coverage: {\n      provider: 'v8',\n      thresholds: { lines: 90, perFile: true, 'src/core/**': { branches: 95 } },\n    },\n  },\n});\n";
        let lowered = base.replace("lines: 90", "lines: 75");
        assert_eq!(
            whats(&[("vitest.config.ts", base)], &[("vitest.config.ts", &lowered)]),
            ["test.coverage.thresholds.lines: 75 (on the base: test.coverage.thresholds.lines: 90, vitest.config.ts:6)"]
        );
        let not_per_file = base.replace("perFile: true", "perFile: false");
        assert_eq!(
            findings(
                &[("vitest.config.ts", base)],
                &[("vitest.config.ts", &not_per_file)]
            )
            .len(),
            1
        );
        let glob_gone = base.replace(", 'src/core/**': { branches: 95 }", "");
        assert_eq!(
            findings(
                &[("vitest.config.ts", base)],
                &[("vitest.config.ts", &glob_gone)]
            )[0]
            .what,
            "test.coverage.thresholds.src/core/**.branches: 95 is gone"
        );
        // The older form, raised, and moved to vite.config.ts.
        let older = "export default {\n  test: {\n    coverage: { lines: 95, perFile: true, thresholds: { 'src/core/**': { branches: 95 } } },\n  },\n};\n";
        assert!(findings(&[("vitest.config.ts", base)], &[("vite.config.ts", older)]).is_empty());
        // `100: true` is all four metrics at 100.
        let all = "export default { test: { coverage: { thresholds: { 100: true } } } };\n";
        let spelled = "export default { test: { coverage: { thresholds: { lines: 100, branches: 100, functions: 100, statements: 100 } } } };\n";
        assert!(findings(
            &[("vitest.config.ts", all)],
            &[("vitest.config.ts", spelled)]
        )
        .is_empty());
        assert_eq!(
            findings(&[("vitest.config.ts", all)], &[("vitest.config.ts", base)]).len(),
            4
        );
    }

    #[test]
    fn nyc_and_c8_settings() {
        let base = "{\n  \"check-coverage\": true,\n  \"lines\": 85,\n  \"branches\": 80\n}\n";
        let off = "{\n  \"check-coverage\": false,\n  \"lines\": 85,\n  \"branches\": 80\n}\n";
        assert_eq!(
            whats(&[(".nycrc", base)], &[(".nycrc", off)]),
            ["check-coverage: false (on the base: check-coverage: true, .nycrc:2)"]
        );
        let yaml = "check-coverage: true\nlines: 85\nbranches: 82\n";
        assert!(findings(&[(".nycrc", base)], &[(".nycrc.yml", yaml)]).is_empty());
        let package =
            "{\n  \"nyc\": { \"checkCoverage\": true, \"lines\": 85, \"branches\": 79 }\n}\n";
        assert_eq!(
            whats(&[(".nycrc", base)], &[("package.json", package)]),
            ["nyc.branches: 79 (on the base: branches: 80, .nycrc:4)"]
        );
        // nyc's default `lines` is 90: dropping `lines: 85` raises it.
        let no_lines = "{\n  \"check-coverage\": true,\n  \"branches\": 80\n}\n";
        assert!(findings(&[(".nycrc", base)], &[(".nycrc", no_lines)]).is_empty());
        // c8 is its own tool.
        let c8 = "{ \"check-coverage\": true, \"lines\": 95 }\n";
        assert_eq!(
            findings(&[(".c8rc.json", c8)], &[(".c8rc.json", "{}\n")]).len(),
            2
        );
        assert!(findings(&[(".c8rc.json", c8)], &[(".c8rc", c8)]).is_empty());
        // A JS config.
        let js = "module.exports = { 'check-coverage': true, lines: 85, branches: 80 };\n";
        assert!(findings(&[(".nycrc", base)], &[("nyc.config.js", js)]).is_empty());
    }

    #[test]
    fn coverage_py_fail_under_is_one_key_across_its_files() {
        let rc = "[run]\nbranch = True\n\n[report]\nfail_under = 90\nshow_missing = True\n";
        let setup = "[metadata]\nname = app\n\n[coverage:report]\nfail_under = 90\n";
        let pyproject = "[project]\nname = \"app\"\n\n[tool.coverage.report]\nfail_under = 90.0\n";
        let tox = "[tox]\nenvlist = py312\n\n[coverage:report]\nfail_under: 90\n";
        for (from, to) in [
            ((".coveragerc", rc), ("setup.cfg", setup)),
            (("setup.cfg", setup), ("pyproject.toml", pyproject)),
            (("pyproject.toml", pyproject), ("tox.ini", tox)),
        ] {
            assert!(
                findings(&[from], &[to]).is_empty(),
                "{} to {}",
                from.0,
                to.0
            );
        }
        let lowered = pyproject.replace("90.0", "72.5");
        assert_eq!(
            whats(&[("setup.cfg", setup)], &[("pyproject.toml", &lowered)]),
            ["fail_under = 72.5 (on the base: fail_under = 90, setup.cfg:5)"]
        );
        let found = findings(
            &[(".coveragerc", rc)],
            &[(".coveragerc", "[run]\nbranch = True\n")],
        );
        assert_eq!(found[0].what, "fail_under = 90 is gone");
        // Unrelated settings changed.
        let other = rc.replace(
            "show_missing = True",
            "show_missing = False\nskip_covered = True",
        );
        assert!(findings(&[(".coveragerc", rc)], &[(".coveragerc", &other)]).is_empty());
        // Another service's floor is another key.
        assert_eq!(
            keys_of("services/api/pyproject.toml", pyproject),
            [("coverage.py fail_under in services/api".to_owned(), 9000)]
        );
    }

    #[test]
    fn pytest_cov_fail_under_in_addopts_and_command_lines() {
        let ini = "[pytest]\naddopts = -q --cov=app --cov-fail-under=85\n";
        let setup = "[tool:pytest]\naddopts =\n    -q\n    --cov=app\n    --cov-fail-under 85\n";
        let pyproject = "[tool.pytest.ini_options]\naddopts = [\"-q\", \"--cov=app\", \"--cov-fail-under\", \"85\"]\n";
        assert!(findings(&[("pytest.ini", ini)], &[("setup.cfg", setup)]).is_empty());
        assert!(findings(&[("setup.cfg", setup)], &[("pyproject.toml", pyproject)]).is_empty());
        let lowered = setup.replace("85", "50");
        let found = findings(&[("setup.cfg", setup)], &[("setup.cfg", &lowered)]);
        assert_eq!(
            found[0].what,
            "--cov-fail-under 50 (on the base: --cov-fail-under 85, setup.cfg:5)"
        );
        assert_eq!(found[0].line, 5);
        // tox commands.
        let tox = "[testenv]\ndeps = pytest-cov\ncommands =\n    pytest --cov=app --cov-fail-under=85 {posargs}\n";
        assert!(findings(&[("pytest.ini", ini)], &[("tox.ini", tox)]).is_empty());
        assert_eq!(
            findings(
                &[("tox.ini", tox)],
                &[("tox.ini", &tox.replace("85", "60"))]
            )[0]
            .line,
            4
        );
    }

    #[test]
    fn command_lines_in_scripts_workflows_and_makefiles() {
        let workflow = "name: CI\non: [pull_request]\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@v4\n      - name: Rust coverage\n        run: |\n          cargo llvm-cov --workspace --fail-under-lines 80\n          cargo tarpaulin --fail-under=75\n      - run: coverage report --fail-under=90\n        working-directory: services/api\n      - run: cd services/web && npx vitest run --coverage.thresholds.lines=85\n";
        assert_eq!(
            keys_of(".github/workflows/ci.yml", workflow),
            [
                ("cargo llvm-cov --fail-under-lines".to_owned(), 8000),
                ("cargo tarpaulin --fail-under".to_owned(), 7500),
                (
                    "coverage report --fail-under in services/api".to_owned(),
                    9000
                ),
                (
                    "vitest --coverage.thresholds.lines in services/web".to_owned(),
                    8500
                ),
            ]
        );
        let lowered = workflow.replace("--fail-under-lines 80", "--fail-under-lines 40");
        let found = findings(
            &[(".github/workflows/ci.yml", workflow)],
            &[(".github/workflows/ci.yml", &lowered)],
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].line, 10);
        assert_eq!(
            found[0].what,
            "--fail-under-lines 40 (on the base: --fail-under-lines 80, .github/workflows/ci.yml:10)"
        );
        // The same steps moved to another workflow, with a step renamed.
        let moved = workflow.replace("Rust coverage", "Coverage");
        assert!(findings(
            &[(".github/workflows/ci.yml", workflow)],
            &[(".github/workflows/coverage.yaml", &moved)]
        )
        .is_empty());

        let package = "{\n  \"scripts\": {\n    \"test\": \"nyc --check-coverage --lines 90 mocha\",\n    \"cov\": \"c8 --100 node test.js\"\n  }\n}\n";
        let weaker = "{\n  \"scripts\": {\n    \"test\": \"nyc --lines=90 mocha\",\n    \"cov\": \"c8 --100 node test.js\",\n    \"lint\": \"eslint .\"\n  }\n}\n";
        assert_eq!(
            whats(&[("package.json", package)], &[("package.json", weaker)]),
            ["--check-coverage is gone"]
        );
        let makefile = "test:\n\tpytest --cov=app \\\n\t  --cov-fail-under=80\n";
        assert_eq!(
            findings(
                &[("Makefile", makefile)],
                &[("Makefile", "test:\n\tpytest --cov=app\n")]
            )
            .len(),
            1
        );
        assert_eq!(measures("Makefile", makefile).expect("readable")[0].line, 3);
        // Not a coverage tool: no measure.
        assert!(keys_of("Makefile", "lint:\n\twc --lines README.md\n").is_empty());
        assert!(keys_of(
            "package.json",
            "{ \"scripts\": { \"x\": \"head --lines 5 a\" } }"
        )
        .is_empty());
    }

    #[test]
    fn tarpaulin_and_codecov() {
        let tarpaulin = "[coverage]\nfail-under = 80\nout = [\"Html\"]\n";
        assert_eq!(
            whats(
                &[("tarpaulin.toml", tarpaulin)],
                &[(".tarpaulin.toml", &tarpaulin.replace("80", "60"))]
            ),
            ["fail-under = 60 (on the base: fail-under = 80, tarpaulin.toml:2)"]
        );
        let codecov = "coverage:\n  status:\n    project:\n      default:\n        target: 80%\n        threshold: 1%\n    patch:\n      default:\n        target: 90%\n";
        let raised = codecov.replace("80%", "82%");
        assert!(findings(
            &[("codecov.yml", codecov)],
            &[(".github/codecov.yml", &raised)]
        )
        .is_empty());
        let weaker = "coverage:\n  status:\n    project:\n      default:\n        target: 80%\n        threshold: 5%\n        informational: true\n    patch: off\n";
        assert_eq!(
            whats(&[("codecov.yml", codecov)], &[("codecov.yml", weaker)]),
            [
                "coverage.status.patch: off (not set on the base)",
                "coverage.status.patch.default.target: 90% is gone",
                "coverage.status.project.default.informational: true (not set on the base)",
                "coverage.status.project.default.threshold: 5% (on the base: coverage.status.project.default.threshold: 1%, codecov.yml:6)",
            ]
        );
        let auto = codecov.replace("80%", "auto");
        assert_eq!(
            findings(&[("codecov.yml", codecov)], &[("codecov.yml", &auto)]).len(),
            1
        );
    }

    #[test]
    fn files_that_cannot_be_read_and_files_without_settings() {
        assert_eq!(measures("package.json", "{\n  \"a\": \n}"), Err(3));
        assert!(measures("jest.config.js", "module.exports = { a: ").is_err());
        assert_eq!(
            measures("package.json", "{ \"scripts\": { \"test\": \"vitest\" } }"),
            Ok(Vec::new())
        );
        assert_eq!(
            measures("pyproject.toml", "[project]\nname = \"a\"\n"),
            Ok(Vec::new())
        );
        assert_eq!(measures("README.md", "anything"), Ok(Vec::new()));
    }
}
