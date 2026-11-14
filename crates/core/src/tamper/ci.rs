//! CI steps removed or made unable to fail (M25, ADR 0045), in GitHub
//! Actions workflows.
//!
//! Every `run:` step is read command by command. A command that checks the
//! code (tests, lint, type checks, `fscale verify`; see [`check_of`]) counts
//! once for each kind of event its workflow runs on (pull requests, pushes,
//! calls from another workflow) when it **can fail**: not when its step or
//! job has `continue-on-error`, an `if:` that is literally false, or the
//! command is followed by `|| true` (or `|| :`, `|| exit 0`, `|| echo …`),
//! or `set +e` comes before it in the step. The counts are compared over
//! the whole change, so a step moved to another job or workflow is not a
//! finding; one deleted, or made unable to fail, is. A workflow whose `on:`
//! loses `pull_request` takes its checks away from pull requests, which is
//! a finding too.

use std::collections::BTreeSet;

use super::config::Measure;
use super::formats::{self, Entry, Format, Scalar};
use super::Kind;

/// True for a GitHub Actions workflow: a `.yml` or `.yaml` file directly
/// in `.github/workflows`.
pub(crate) fn reads(path: &str) -> bool {
    let Some(name) = path.strip_prefix(".github/workflows/") else {
        return false;
    };
    !name.contains('/') && (name.ends_with(".yml") || name.ends_with(".yaml"))
}

/// The kinds of event a workflow's checks count for. A `pull_request`
/// whose `types` leave out every update of the pull request (`opened`,
/// `synchronize`, `reopened`) does not count. A trigger with a `branches`,
/// `paths` or `*-ignore` filter is a class of its own ("… (some changes)"),
/// so adding a filter takes the checks away from every pull request, while
/// editing a filter, renaming the file or adding a filtered workflow does
/// not.
fn event_classes(entries: &[Entry]) -> BTreeSet<String> {
    let mut events: BTreeSet<String> = BTreeSet::new();
    for entry in entries
        .iter()
        .filter(|e| e.path.first().is_some_and(|k| k == "on" || k == "true"))
    {
        // `on: push`, `on: [push, pull_request]`, `on: { push: … }`.
        match entry.path.get(1) {
            None => {
                events.insert(entry.value.as_text());
            }
            Some(segment) if formats::is_index(segment) => {
                events.insert(entry.value.as_text());
            }
            Some(event) => {
                events.insert(event.clone());
            }
        }
    }
    let settings = |event: &str, key: &str| -> Vec<String> {
        entries
            .iter()
            .filter(|e| {
                e.path.len() >= 3 && e.path[0] == "on" && e.path[1] == event && e.path[2] == key
            })
            .map(|e| e.value.as_text())
            .collect()
    };
    let filtered = |event: &str| {
        ["branches", "paths", "branches-ignore", "paths-ignore"]
            .iter()
            .any(|key| {
                entries.iter().any(|e| {
                    e.path.len() >= 3
                        && e.path[0] == "on"
                        && e.path[1] == event
                        && e.path[2] == *key
                })
            })
    };
    // An unfiltered trigger also counts as a filtered one, so removing a
    // filter (running on more changes) is never a loss.
    let add = |classes: &mut BTreeSet<String>, name: &str, event: &str| {
        if !filtered(event) {
            classes.insert(name.to_owned());
        }
        classes.insert(format!("{name} (some changes)"));
    };
    let mut classes = BTreeSet::new();
    for event in &events {
        match event.as_str() {
            "pull_request" | "pull_request_target" => {
                let types = settings(event, "types");
                let updates = types.is_empty()
                    || types
                        .iter()
                        .any(|t| matches!(t.as_str(), "opened" | "synchronize" | "reopened"));
                if updates {
                    add(&mut classes, "pull requests", event);
                }
            }
            "merge_group" => {
                classes.insert("pull requests".to_owned());
            }
            "push" => {
                add(&mut classes, "pushes", event);
            }
            "workflow_call" => {
                classes.insert("calls".to_owned());
            }
            _ => {}
        }
    }
    classes
}

/// True for an `if:` or `continue-on-error:` value that is literally true
/// (or, for `continue-on-error`, anything but literally false).
fn literal(value: &Scalar) -> Option<bool> {
    let text = value.as_text();
    let inner = text
        .trim()
        .strip_prefix("${{")
        .and_then(|t| t.strip_suffix("}}"))
        .unwrap_or(&text)
        .trim()
        .to_owned();
    match inner.as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => value.as_bool().filter(|_| matches!(value, Scalar::Bool(_))),
    }
}

/// Why a step or job never fails, from its `continue-on-error` and `if`.
fn never_fails(fields: &[&Entry]) -> Option<String> {
    for entry in fields {
        let key = entry.path.last().map(String::as_str);
        match key {
            Some("continue-on-error") => {
                // `${{ matrix.experimental }}` marks the matrix entries that
                // may fail, the usual and visible way to allow one; any other
                // value that is not literally false lets the step pass.
                let per_entry = entry.value.as_text().contains("matrix.");
                if literal(&entry.value) != Some(false) && !per_entry {
                    return Some(format!("continue-on-error: {}", entry.value.as_text()));
                }
            }
            Some("if") if literal(&entry.value) == Some(false) => {
                return Some(format!("if: {}", entry.value.as_text()));
            }
            _ => {}
        }
    }
    None
}

pub(crate) fn measures(_path: &str, text: &str) -> Result<Vec<Measure>, usize> {
    let entries = formats::read(Format::Yaml, text)?;
    let classes = event_classes(&entries);
    let mut measures = Vec::new();
    // `shell: bash` (on the step, the job or the workflow's defaults) runs
    // with `-o pipefail`; GitHub's default `bash -e {0}` does not.
    let workflow_bash = entries
        .iter()
        .any(|e| e.path == ["defaults", "run", "shell"] && e.value.as_text() == "bash");
    // Jobs and their steps.
    let jobs: BTreeSet<&String> = entries
        .iter()
        .filter(|e| e.path.first().is_some_and(|k| k == "jobs"))
        .filter_map(|e| e.path.get(1))
        .collect();
    for job in jobs {
        let in_job: Vec<&Entry> = entries
            .iter()
            .filter(|e| e.path.len() >= 3 && e.path[0] == "jobs" && &e.path[1] == job)
            .collect();
        let job_fields: Vec<&Entry> = in_job
            .iter()
            .copied()
            .filter(|e| e.path.len() == 3)
            .collect();
        let job_reason = never_fails(&job_fields);
        let job_bash = workflow_bash
            || in_job.iter().any(|e| {
                e.path.len() == 5
                    && e.path[2] == "defaults"
                    && e.path[4] == "shell"
                    && e.value.as_text() == "bash"
            });
        let steps: BTreeSet<&String> = in_job
            .iter()
            .filter(|e| e.path.len() >= 5 && e.path[2] == "steps")
            .map(|e| &e.path[3])
            .collect();
        for step in steps {
            let fields: Vec<&Entry> = in_job
                .iter()
                .copied()
                .filter(|e| e.path.len() == 5 && e.path[2] == "steps" && &e.path[3] == step)
                .collect();
            let step_reason = never_fails(&fields).or_else(|| job_reason.clone());
            let Some(run) = fields.iter().find(|e| e.path[4] == "run") else {
                continue;
            };
            let pipefail = job_bash
                || fields
                    .iter()
                    .any(|e| e.path[4] == "shell" && e.value.as_text() == "bash");
            let script = run.value.as_text();
            let mut set_plus_e = false;
            let lines = logical_lines(&script);
            let last = lines.len().saturating_sub(1);
            for (number, (offset, line)) in lines.iter().enumerate() {
                let offset = *offset;
                if line.split_whitespace().collect::<Vec<_>>() == ["set", "+e"] {
                    set_plus_e = true;
                    continue;
                }
                for (command, rest) in commands(line) {
                    let Some(check) = check_of(&command) else {
                        continue;
                    };
                    let after = rest.trim_start();
                    let reason = step_reason
                        .clone()
                        .or_else(|| set_plus_e.then(|| "set +e before it".to_owned()))
                        .or_else(|| swallowed(&rest))
                        .or_else(|| {
                            (after.starts_with('&') && !after.starts_with("&&"))
                                .then(|| "runs in the background with &".to_owned())
                        })
                        .or_else(|| {
                            (after.starts_with("&&") && number != last).then(|| {
                                "before && on a line that is not the last, where bash -e does not stop".to_owned()
                            })
                        })
                        .or_else(|| {
                            (after.starts_with('|') && !after.starts_with("||") && !pipefail).then(|| {
                                "piped, and without pipefail the last command of the pipe decides".to_owned()
                            })
                        });
                    let shown: String = line.trim().chars().take(160).collect();
                    for class in &classes {
                        let (value, what) = match &reason {
                            Some(reason) if !shown.contains(reason.as_str()) => {
                                (0, format!("{shown} (on {class}; cannot fail: {reason})"))
                            }
                            Some(_) => (0, format!("{shown} (on {class}; cannot fail)")),
                            None => (1, format!("{shown} (on {class})")),
                        };
                        measures.push(Measure::count(
                            Kind::CiWeakened,
                            format!("ci {check} on {class}"),
                            value,
                            run.line + offset,
                            what,
                        ));
                    }
                }
            }
        }
    }
    Ok(measures)
}

/// A script's logical lines (continued with a trailing `\`), without
/// comment lines, with the offset of each from the script's first line.
pub(crate) fn logical_lines(script: &str) -> Vec<(usize, String)> {
    let mut lines = Vec::new();
    let mut current: Option<(usize, String)> = None;
    for (offset, raw) in script.lines().enumerate() {
        let trimmed = raw.trim();
        if current.is_none() && (trimmed.is_empty() || trimmed.starts_with('#')) {
            continue;
        }
        let (start, mut text) = current.take().unwrap_or((offset, String::new()));
        if let Some(body) = trimmed.strip_suffix('\\') {
            text.push_str(body);
            text.push(' ');
            current = Some((start, text));
        } else {
            text.push_str(trimmed);
            lines.push((start, text));
        }
    }
    if let Some(open) = current {
        lines.push(open);
    }
    lines
}

/// The commands of a logical line, split at `&&`, `||`, `;` and `|`
/// outside quotes, each with the text that follows it.
fn commands(line: &str) -> Vec<(String, String)> {
    let mut quote: Option<char> = None;
    let bytes: Vec<char> = line.chars().collect();
    let mut cuts: Vec<(usize, usize)> = Vec::new(); // (end of command, start of next)
    let mut index = 0;
    while index < bytes.len() {
        let c = bytes[index];
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '"' | '\'' => quote = Some(c),
                '#' if index == 0 || bytes[index - 1] == ' ' => break,
                '&' | '|' if bytes.get(index + 1) == Some(&c) => {
                    cuts.push((index, index + 2));
                    index += 1;
                }
                // `2>&1` and `&>` are redirections, not a background `&`.
                '&' if index > 0 && matches!(bytes[index - 1], '>' | '<') => {}
                '&' if bytes.get(index + 1) == Some(&'>') => {}
                ';' | '|' | '&' => cuts.push((index, index + 1)),
                _ => {}
            },
        }
        index += 1;
    }
    let end = index;
    let text = |from: usize, to: usize| bytes[from..to].iter().collect::<String>();
    let mut out = Vec::new();
    let mut from = 0;
    for (stop, next) in cuts {
        out.push((text(from, stop).trim().to_owned(), text(stop, end)));
        from = next;
    }
    out.push((text(from, end).trim().to_owned(), String::new()));
    out
}

/// Why what follows a command makes it unable to fail: `|| true`, `|| :`,
/// `|| exit 0`, `|| echo …`. (`; true` is not: with GitHub's `bash -e` the
/// step stops at the failing command before it.)
fn swallowed(rest: &str) -> Option<String> {
    let tail = rest.trim_start().strip_prefix("||")?;
    let next: Vec<&str> = tail.split_whitespace().take(2).collect();
    let swallow = matches!(
        next.as_slice(),
        ["true", ..] | [":", ..] | ["exit", "0", ..] | ["echo" | "printf", ..] | ["exit"]
    );
    swallow.then(|| format!("|| {}", next.join(" ")))
}

/// Script names that run checks.
fn is_check_script(name: &str) -> bool {
    let base = name.split([':', '-', '_', '.']).next().unwrap_or(name);
    matches!(
        base,
        "test"
            | "tests"
            | "lint"
            | "typecheck"
            | "type"
            | "types"
            | "tsc"
            | "check"
            | "checks"
            | "verify"
            | "ci"
            | "e2e"
            | "validate"
            | "spec"
            | "coverage"
    ) || matches!(name, "format:check" | "fmt:check" | "prettier:check")
}

/// The check a command runs, as `<tool> <subcommand>`, or `None` for a
/// command that checks nothing (an install, a build, an echo).
pub(crate) fn check_of(command: &str) -> Option<String> {
    let words = shell_words(command);
    let mut words: Vec<&str> = words.iter().map(String::as_str).collect();
    // Leading assignments and wrappers.
    loop {
        match words.first() {
            Some(word)
                if word.contains('=') && !word.starts_with('-') && !word.starts_with('=') =>
            {
                words.remove(0);
            }
            Some(&("sudo" | "time" | "env" | "exec" | "command" | "xvfb-run")) => {
                words.remove(0);
            }
            Some(&"timeout") => {
                words.remove(0);
                if words
                    .first()
                    .is_some_and(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()))
                {
                    words.remove(0);
                }
            }
            _ => break,
        }
    }
    let first = *words.first()?;
    let program = first.rsplit(['/', '\\']).next().unwrap_or(first);
    let program = program.strip_suffix(".exe").unwrap_or(program);
    // `firescale@0.5.0`, `eslint@9` (but not a scoped name's leading `@`).
    let program = match program.rfind('@') {
        Some(at) if at > 0 => &program[..at],
        _ => program,
    };
    let rest: Vec<&str> = words[1..].to_vec();
    let sub = rest.iter().copied().find(|w| !w.starts_with('-'));
    let has = |flag: &str| rest.contains(&flag);
    let runner = |words: &[&str]| -> Option<String> {
        // `npx [-y] tool …`, `pnpm exec tool …`, `uv run --group dev tool …`:
        // leading options, and the value of those that take one.
        const TAKES_VALUE: [&str; 16] = [
            "--group",
            "--only-group",
            "--with",
            "--python",
            "-p",
            "--package",
            "--project",
            "--directory",
            "--extra",
            "--env-file",
            "--from",
            "--index",
            "-C",
            "--filter",
            "-F",
            "--prefix",
        ];
        let mut rest = words.iter().copied().peekable();
        while let Some(word) = rest.peek().copied() {
            if !word.starts_with('-') {
                break;
            }
            rest.next();
            if TAKES_VALUE.contains(&word) {
                rest.next();
            }
        }
        check_of(&rest.collect::<Vec<_>>().join(" "))
    };
    match program {
        // A package script, whichever manager runs it: moving from npm to
        // pnpm keeps the key.
        "npm" | "pnpm" | "yarn" | "bun" => {
            let sub = sub?;
            let index = rest.iter().position(|w| *w == sub)?;
            match sub {
                "test" | "t" | "tst" => Some("script test".to_owned()),
                "run" | "run-script" => {
                    let script = rest
                        .get(index + 1..)?
                        .iter()
                        .find(|w| !w.starts_with('-'))?;
                    is_check_script(script).then(|| format!("script {script}"))
                }
                "exec" | "dlx" | "x" => runner(&rest[index + 1..]),
                // `pnpm <script>`, `yarn <script>` and `bun <script>` run a
                // script.
                script if program != "npm" && is_check_script(script) => {
                    Some(format!("script {script}"))
                }
                _ => None,
            }
        }
        "npx" | "pnpx" | "bunx" => runner(&rest),
        "uv" | "poetry" | "pipenv" | "pdm" | "hatch" | "rye" => {
            let index = rest.iter().position(|w| *w == "run")?;
            runner(&rest[index + 1..])
        }
        "python" | "python3" | "py" => {
            let index = rest.iter().position(|w| *w == "-m")?;
            let module = rest.get(index + 1)?;
            match *module {
                "unittest" => Some("python -m unittest".to_owned()),
                _ => check_of(&rest[index + 1..].join(" ")),
            }
        }
        "fscale" | "firescale" => matches!(sub?, "verify" | "check" | "tamper")
            .then(|| format!("fscale {}", sub.unwrap_or_default())),
        "vitest" | "jest" | "mocha" | "ava" | "tap" | "eslint" | "tsc" | "vue-tsc" | "oxlint"
        | "stylelint" | "pytest" | "py.test" | "tox" | "nox" | "flake8" | "mypy" | "pyright"
        | "basedpyright" | "pylint" | "bandit" | "golangci-lint" | "rspec" | "rubocop"
        | "phpunit" | "c8" | "nyc" => Some(program.to_owned()),
        "playwright" | "cypress" => {
            matches!(sub?, "test" | "run").then(|| format!("{program} {}", sub.unwrap_or_default()))
        }
        "prettier" | "black" | "isort" | "gofmt" => (has("--check")
            || has("-c")
            || has("--check-only")
            || has("--list-different")
            || has("-l"))
        .then(|| format!("{program} --check")),
        "biome" => matches!(sub?, "check" | "ci" | "lint")
            .then(|| format!("biome {}", sub.unwrap_or_default())),
        "ruff" => match sub? {
            "check" => Some("ruff check".to_owned()),
            "format" => has("--check").then(|| "ruff format --check".to_owned()),
            _ => None,
        },
        "cargo" => match sub? {
            "test" | "clippy" | "check" | "nextest" | "llvm-cov" | "tarpaulin" => {
                Some(format!("cargo {}", sub.unwrap_or_default()))
            }
            "fmt" => has("--check").then(|| "cargo fmt --check".to_owned()),
            _ => None,
        },
        "go" => matches!(sub?, "test" | "vet").then(|| format!("go {}", sub.unwrap_or_default())),
        "deno" => match sub? {
            "test" | "lint" | "check" => Some(format!("deno {}", sub.unwrap_or_default())),
            "fmt" => has("--check").then(|| "deno fmt --check".to_owned()),
            _ => None,
        },
        "node" => (has("--test")).then(|| "node --test".to_owned()),
        "make" | "just" | "task" | "mise" => {
            let target = rest
                .iter()
                .find(|w| !w.starts_with('-') && !w.contains('='))?;
            let target = if program == "mise" && *target == "run" {
                rest.iter().filter(|w| !w.starts_with('-')).nth(1)?
            } else {
                target
            };
            is_check_script(target).then(|| format!("{program} {target}"))
        }
        "mvn" | "mvnw" | "gradle" | "gradlew" => rest
            .iter()
            .find(|w| matches!(**w, "test" | "verify" | "check"))
            .map(|goal| format!("{program} {goal}")),
        "dotnet" | "swift" => (sub? == "test").then(|| format!("{program} test")),
        "bundle" => {
            let index = rest.iter().position(|w| *w == "exec")?;
            runner(&rest[index + 1..])
        }
        _ => None,
    }
}

/// Splits a command into words, honouring single and double quotes.
pub(crate) fn shell_words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for c in command.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => current.push(c),
            None => match c {
                '"' | '\'' => {
                    quote = Some(c);
                    started = true;
                }
                c if c.is_whitespace() => {
                    if started {
                        words.push(std::mem::take(&mut current));
                        started = false;
                    }
                }
                c => {
                    current.push(c);
                    started = true;
                }
            },
        }
    }
    if started {
        words.push(current);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::super::config::{compare, Located};
    use super::*;

    fn located(file: &str, text: &str) -> Vec<Located> {
        measures(file, text)
            .expect("readable")
            .into_iter()
            .map(|measure| Located {
                file: file.to_owned(),
                measure,
            })
            .collect()
    }

    fn findings(base: &str, head: &str) -> Vec<String> {
        compare(
            &located(".github/workflows/ci.yml", base),
            &located(".github/workflows/ci.yml", head),
        )
        .into_iter()
        .map(|f| {
            format!(
                "{}:{} {}",
                f.line,
                if f.on_base { "base" } else { "now" },
                f.what
            )
        })
        .collect()
    }

    const BASE: &str = "on:\n  pull_request:\n  push:\n    branches: [main]\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@v4\n      - run: pnpm install --frozen-lockfile\n      - run: pnpm lint\n      - name: Tests\n        run: |\n          pnpm build\n          pnpm test\n";

    #[test]
    fn recognises_checks() {
        for (command, check) in [
            ("pnpm test", "script test"),
            ("npm run test:unit -- --run", "script test:unit"),
            ("npx --yes firescale verify --affected", "fscale verify"),
            (
                "npx --yes firescale@0.5.0 tamper --pr-base",
                "fscale tamper",
            ),
            ("CI=1 uv run pytest -q", "pytest"),
            ("python -m pytest tests", "pytest"),
            ("cargo test --workspace", "cargo test"),
            ("cargo fmt --all --check", "cargo fmt --check"),
            ("yarn typecheck", "script typecheck"),
            ("make lint", "make lint"),
            ("./node_modules/.bin/eslint .", "eslint"),
        ] {
            assert_eq!(check_of(command).as_deref(), Some(check), "{command}");
        }
        for command in [
            "pnpm install",
            "cargo build --release",
            "echo test",
            "prettier --write .",
            "npm run build",
            "ruff format .",
        ] {
            assert_eq!(check_of(command), None, "{command}");
        }
    }

    #[test]
    fn a_deleted_check_step_is_a_finding_and_a_moved_one_is_not() {
        let head = BASE.replace("      - run: pnpm lint\n", "");
        assert_eq!(
            findings(BASE, &head),
            ["11:base pnpm lint (on pull requests) is gone"]
        );
        // The lint step moved after the tests, and into the same script.
        let moved = BASE
            .replace("      - run: pnpm lint\n", "")
            .replace("pnpm test\n", "pnpm test\n          pnpm lint\n");
        assert!(findings(BASE, &moved).is_empty());
        // A flag added changes nothing.
        assert!(findings(BASE, &BASE.replace("pnpm test\n", "pnpm test --coverage\n")).is_empty());
    }

    #[test]
    fn a_check_made_unable_to_fail_is_a_finding() {
        let swallowed = BASE.replace("pnpm test\n", "pnpm test || true\n");
        assert_eq!(
            findings(BASE, &swallowed),
            ["15:now pnpm test || true (on pull requests; cannot fail)",]
        );
        let continued = BASE.replace(
            "      - run: pnpm lint\n",
            "      - run: pnpm lint\n        continue-on-error: true\n",
        );
        assert_eq!(
            findings(BASE, &continued)[0],
            "11:now pnpm lint (on pull requests; cannot fail: continue-on-error: true)"
        );
        let off = BASE.replace(
            "    runs-on: ubuntu-latest\n",
            "    runs-on: ubuntu-latest\n    if: ${{ false }}\n",
        );
        assert_eq!(findings(BASE, &off).len(), 2, "one per check");
        let plus_e = BASE.replace("pnpm build\n", "set +e\n          pnpm build\n");
        assert_eq!(
            findings(BASE, &plus_e)[0],
            "16:now pnpm test (on pull requests; cannot fail: set +e before it)"
        );
        // `|| exit 1` still fails.
        assert!(findings(BASE, &BASE.replace("pnpm test\n", "pnpm test || exit 1\n")).is_empty());
    }

    #[test]
    fn a_workflow_that_stops_running_on_pull_requests_is_a_finding() {
        let head = BASE.replace("  pull_request:\n", "");
        let found = findings(BASE, &head);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found
            .iter()
            .all(|f| f.contains("(on pull requests) is gone")));
        // A trigger added, or a branch filter edited, is not.
        assert!(findings(BASE, &BASE.replace("[main]", "[main, release]")).is_empty());
    }

    #[test]
    fn narrower_triggers_are_findings() {
        for (from, to) in [
            (
                "  pull_request:\n",
                "  pull_request:\n    branches: [nope]\n",
            ),
            (
                "  pull_request:\n",
                "  pull_request:\n    paths: [\"nothing/**\"]\n",
            ),
            (
                "  pull_request:\n",
                "  pull_request:\n    types: [closed]\n",
            ),
            (
                "  pull_request:\n",
                "  pull_request:\n    paths-ignore: [\"src/**\"]\n",
            ),
        ] {
            assert!(!findings(BASE, &BASE.replace(from, to)).is_empty(), "{to}");
        }
        // Editing an existing filter, or adding the types every pull request
        // update has, is not.
        let filtered = BASE.replace(
            "  pull_request:\n",
            "  pull_request:\n    branches: [main]\n",
        );
        assert!(findings(&filtered, &filtered.replace("[main]", "[main, next]")).is_empty());
        let typed = BASE.replace(
            "  pull_request:\n",
            "  pull_request:\n    types: [opened, synchronize]\n",
        );
        assert!(findings(BASE, &typed).is_empty());
    }

    #[test]
    fn shell_forms_that_cannot_fail_are_findings_and_the_ones_that_can_are_not() {
        for (to, why) in [
            ("pnpm test &\n          wait\n", "background"),
            ("pnpm test && echo ok\n          echo done\n", "bash -e"),
            ("pnpm test | tee out.log\n", "pipefail"),
        ] {
            let found = findings(BASE, &BASE.replace("pnpm test\n", to));
            assert!(found.iter().any(|f| f.contains(why)), "{to}: {found:?}");
        }
        for to in [
            "pnpm test && echo ok\n",
            "pnpm test 2>&1\n",
            "pnpm test; exit 0\n",
            "pnpm test || exit 1\n",
        ] {
            assert!(
                findings(BASE, &BASE.replace("pnpm test\n", to)).is_empty(),
                "{to}"
            );
        }
        // With `shell: bash` the pipe fails as its first command does.
        let bash = BASE.replace(
            "      - name: Tests\n",
            "      - name: Tests\n        shell: bash\n",
        );
        assert!(findings(
            &bash,
            &bash.replace("pnpm test\n", "pnpm test | tee out.log\n")
        )
        .is_empty());
    }

    #[test]
    fn honest_workflow_edits_are_not_findings() {
        // npm to pnpm keeps the scripts.
        let npm = BASE
            .replace("pnpm", "npm run")
            .replace("npm run install --frozen-lockfile", "npm ci");
        assert!(
            findings(&npm, BASE).is_empty(),
            "{:?}",
            findings(&npm, BASE)
        );
        // A matrix entry allowed to fail.
        let matrix = BASE.replace(
            "    runs-on: ubuntu-latest\n",
            "    runs-on: ubuntu-latest\n    continue-on-error: ${{ matrix.experimental }}\n",
        );
        assert!(findings(BASE, &matrix).is_empty());
    }
}
