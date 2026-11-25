//! Old tests, new code (ADR 0089; verification idea 1, first step).
//!
//! When the tamper guard has an `expected-changed`, `snapshot-changed` or
//! assertion finding in a test file the base commit has, the test step of
//! that file's unit runs once more, in a worktree of the head, with those
//! test files, every changed `conftest.py`, snapshot and runner config taken
//! from the base commit. A test of those files that fails there is evidence
//! for a reviewer: the change needed different tests to pass. It is added to
//! the findings of its file. A run that passes never lowers or removes a
//! finding, and nothing here ever makes verify fail: report only.
//!
//! Pytest, Vitest, Jest and `cargo test`: the runners whose results are read
//! test by test ([`TestRunner`]).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::blame::{self, BaseCommit, Worktree, WorktreeSetup};
use crate::git::run_git;
use crate::pertest::{self, TestRunner};
use crate::tamper::{Kind, OldTests, OldTestsStatus, TamperReport};
use crate::verify::{run_base_step, Planned, ProjectPlan, RunOptions, StepStatus, VerifyPlan};

/// The kinds whose files are run: a changed expected value, a rewritten
/// snapshot, and an assertion taken away or weakened.
const KINDS: [Kind; 4] = [
    Kind::ExpectedChanged,
    Kind::SnapshotChanged,
    Kind::AssertionRemoved,
    Kind::AssertionWeakened,
];

/// At most this many failing tests are named on one finding.
const MAX_EVIDENCE: usize = 5;

/// One unit whose test step runs: its folder (`.` for the root), its plan,
/// its command and runner, and the base's files it is run for.
struct Unit<'p> {
    folder: String,
    project: Option<&'p ProjectPlan>,
    command: String,
    runner: TestRunner,
    files: Vec<String>,
}

/// The place verify names a step by: `test`, `test in packages/api`.
fn place(folder: &str) -> String {
    if folder == "." {
        "test".to_owned()
    } else {
        format!("test in {folder}")
    }
}

/// True when the base commit has `path`.
fn on_base(root: &Path, base: &BaseCommit, path: &str) -> bool {
    run_git(
        root,
        &["cat-file", "-e", &format!("{}:./{path}", base.commit)],
    )
    .is_ok_and(|out| out.code == 0)
}

/// A stored snapshot: Jest's and Vitest's `__snapshots__/*.snap`, syrupy's
/// `__snapshots__/*.ambr`.
fn is_snapshot(path: &str) -> bool {
    path.contains("__snapshots__/")
        || Path::new(path)
            .extension()
            .is_some_and(|ext| ext == "snap" || ext == "ambr")
}

/// A file that tells a runner how to run: `conftest.py`, `pytest.ini`,
/// `vitest.config.*`, `vitest.workspace.*`, `jest.config.*`.
fn is_runner_config(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    matches!(name, "conftest.py" | "pytest.ini")
        || ["vitest.config.", "vitest.workspace.", "jest.config."]
            .iter()
            .any(|start| name.starts_with(start))
}

/// The test file a snapshot belongs to: `src/__snapshots__/a.test.ts.snap`
/// is `src/a.test.ts`'s; `tests/__snapshots__/test_x.ambr` is
/// `tests/test_x.py`'s.
fn snapshot_owner(path: &str) -> Option<String> {
    let (dir, name) = match path.rsplit_once("__snapshots__/") {
        Some((dir, name)) => (dir, name),
        None => return None,
    };
    if let Some(stem) = name.strip_suffix(".snap") {
        return Some(format!("{dir}{stem}"));
    }
    name.strip_suffix(".ambr")
        .map(|stem| format!("{dir}{stem}.py"))
}

/// A Rust file whose tests are beside the code they test (`src/lib.rs`):
/// the base's copy would bring the base's code with it, so it cannot be
/// run on the head's code. Files under `tests/` or `benches/` are tests
/// only.
fn holds_code(path: &str) -> bool {
    Path::new(path).extension().is_some_and(|ext| ext == "rs")
        && !path
            .split('/')
            .any(|part| part == "tests" || part == "benches")
}

/// True when the failing test `id` (as `runner` names it, relative to the
/// step's folder) is in `file` (repo-relative), or in the test file a
/// snapshot `file` belongs to.
fn in_file(runner: TestRunner, id: &str, folder: &str, file: &str) -> bool {
    let target = snapshot_owner(file).unwrap_or_else(|| file.to_owned());
    if runner == TestRunner::Cargo {
        // `cargo test` names the test binary, which for `tests/cart.rs` is
        // `cart`.
        let binary = id.split("::").next().unwrap_or(id);
        let stem = Path::new(&target)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned());
        return target.split('/').any(|part| part == "tests") && stem.as_deref() == Some(binary);
    }
    let Some(named) = pertest::file_of(runner, id) else {
        return false;
    };
    let named = named.replace('\\', "/");
    let named = named.trim_start_matches("./");
    let full = if folder == "." {
        named.to_owned()
    } else {
        format!("{folder}/{named}")
    };
    full == target || named == target
}

/// Writes the base commit's `path` into the worktree, or removes it there
/// when the base has none. Never through a link (as `overlay_head`).
fn put_base_file(root: &Path, base: &BaseCommit, into: &Path, path: &str) -> Result<(), String> {
    let to = path
        .split('/')
        .fold(into.to_path_buf(), |at, part| at.join(part));
    let parts: Vec<&str> = path.split('/').collect();
    let mut at = into.to_path_buf();
    for part in &parts[..parts.len().saturating_sub(1)] {
        at.push(part);
        if blame::is_link(&at) && fs::remove_file(&at).is_err() {
            let _ = fs::remove_dir(&at);
        }
    }
    if blame::is_link(&to) {
        let _ = fs::remove_file(&to);
    }
    if !on_base(root, base, path) {
        return match fs::remove_file(&to) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                Err(format!("{path} could not be removed: {error}"))
            }
            _ => Ok(()),
        };
    }
    let out = run_git(root, &["show", &format!("{}:./{path}", base.commit)])
        .map_err(|error| error.message().to_owned())?;
    if out.code != 0 {
        return Err(format!("git show could not read {path} on the base commit"));
    }
    let real_into = into
        .canonicalize()
        .map_err(|error| format!("the worktree cannot be read: {error}"))?;
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("{path}: {error}"))?;
        let inside = parent
            .canonicalize()
            .is_ok_and(|real| real.starts_with(&real_into));
        if !inside {
            return Err(format!("{path} would be written out of the worktree"));
        }
    }
    fs::write(&to, out.stdout).map_err(|error| format!("{path}: {error}"))
}

/// The unit of the plan that holds `file`: the deepest project folder it is
/// in, else the root.
fn unit_of<'p>(plan: &'p VerifyPlan, file: &str) -> (String, Option<&'p ProjectPlan>) {
    plan.projects
        .iter()
        .filter(|project| {
            project.root != "."
                && (file == project.root || file.starts_with(&format!("{}/", project.root)))
        })
        .max_by_key(|project| project.root.len())
        .map_or_else(
            || (".".to_owned(), None),
            |project| (project.root.clone(), Some(project)),
        )
}

fn not_judged(step: &str, files: Vec<String>, detail: impl Into<String>) -> OldTests {
    OldTests {
        step: step.to_owned(),
        status: OldTestsStatus::NotJudged,
        files,
        detail: detail.into(),
    }
}

/// Runs the base's tests on the head's code for the guard's findings, and
/// adds what it shows to `tamper`: an entry per unit in `old_tests`, and the
/// failing tests as evidence on the findings of their files. `deadline` is
/// the caller's own limit (the stop hook's); `verify.baseBudget` also
/// applies. Nothing it does changes which findings there are, or whether
/// they block.
#[allow(clippy::too_many_arguments)]
pub fn judge(
    root: &Path,
    base: &BaseCommit,
    changed: &BTreeSet<String>,
    tamper: &mut TamperReport,
    plan: &VerifyPlan,
    options: &RunOptions<'_>,
    deadline: Option<Instant>,
    say: &dyn Fn(&str),
) {
    if tamper.policy == crate::tamper::Policy::Off {
        return;
    }
    let mut files: BTreeSet<String> = BTreeSet::new();
    for finding in &tamper.findings {
        if KINDS.contains(&finding.kind)
            && !files.contains(&finding.file)
            && on_base(root, base, &finding.file)
        {
            files.insert(finding.file.clone());
        }
    }
    if files.is_empty() {
        return;
    }
    let budget = blame::budget_of(options).map(|limit| Instant::now() + limit);
    let deadline = match (deadline, budget) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };

    // Each file to the test step of its unit.
    let mut units: BTreeMap<String, Unit<'_>> = BTreeMap::new();
    let mut results: Vec<OldTests> = Vec::new();
    for file in files {
        let (folder, project) = unit_of(plan, &file);
        if holds_code(&file) {
            results.push(not_judged(
                &place(&folder),
                vec![file],
                "its tests are in the same file as the code they test, so the base's copy would bring the base's code with it",
            ));
            continue;
        }
        if let Some(unit) = units.get_mut(&folder) {
            unit.files.push(file);
            continue;
        }
        let steps = project.map_or(&plan.steps, |project| &project.steps);
        let command = steps.iter().find_map(|(name, planned)| match planned {
            Planned::Command(command) if name == "test" => Some(command.clone()),
            _ => None,
        });
        let Some(command) = command else {
            results.push(not_judged(
                &place(&folder),
                vec![file],
                "verify plans no test step for it",
            ));
            continue;
        };
        let dir = blame::folder_of(root, Some(&folder));
        let script = pertest::script_text(&dir, &command);
        let Some(runner) = TestRunner::of(&command, script.as_deref()) else {
            results.push(not_judged(
                &place(&folder),
                vec![file],
                format!(
                    "its test step (`{command}`) runs none of pytest, Vitest, Jest or cargo test"
                ),
            ));
            continue;
        };
        units.insert(
            folder.clone(),
            Unit {
                folder,
                project,
                command,
                runner,
                files: vec![file],
            },
        );
    }

    if !units.is_empty() {
        run_units(
            root,
            base,
            changed,
            tamper,
            plan,
            options,
            deadline,
            say,
            units,
            &mut results,
        );
    }
    tamper.old_tests = results;
}

/// Makes the worktree, takes the base's files into it, and runs each unit's
/// test step once.
#[allow(clippy::too_many_arguments)]
fn run_units(
    root: &Path,
    base: &BaseCommit,
    changed: &BTreeSet<String>,
    tamper: &mut TamperReport,
    plan: &VerifyPlan,
    options: &RunOptions<'_>,
    deadline: Option<Instant>,
    say: &dyn Fn(&str),
    units: BTreeMap<String, Unit<'_>>,
    results: &mut Vec<OldTests>,
) {
    let all_not_judged =
        |results: &mut Vec<OldTests>, units: &BTreeMap<String, Unit<'_>>, why: &str| {
            for unit in units.values() {
                results.push(not_judged(&place(&unit.folder), unit.files.clone(), why));
            }
        };
    if options.stop.load(Ordering::SeqCst) {
        all_not_judged(results, &units, "verify was stopped");
        return;
    }
    if blame::past(deadline) {
        all_not_judged(results, &units, "no time was left for it");
        return;
    }
    say("old tests, new code: running the base's versions of the changed test files on the head's code");
    let head = match run_git(root, &["rev-parse", "--verify", "HEAD"]) {
        Ok(out) if out.code == 0 => out.stdout.trim().to_owned(),
        _ => {
            all_not_judged(results, &units, "git could not name the head commit");
            return;
        }
    };
    let worktree = match Worktree::add(root, &head) {
        Ok(worktree) => worktree,
        Err(problem) => {
            all_not_judged(
                results,
                &units,
                &format!("a worktree of the head could not be made: {problem}"),
            );
            return;
        }
    };
    if let Err(problem) = blame::overlay_head(root, &worktree.root, deadline, options.stop) {
        let why = problem.unwrap_or_else(|| "no time was left for it".to_owned());
        all_not_judged(
            results,
            &units,
            &format!("the head's changes could not be copied into its worktree: {why}"),
        );
        return;
    }
    let links = WorktreeSetup {
        folders: if options.config.base_install.is_some() {
            Vec::new()
        } else {
            blame::dependency_folders(root)
        },
        env_files: blame::env_files(root, plan)
            .into_iter()
            .map(|(path, _)| path)
            .collect(),
    };
    if let Some(problem) =
        blame::prepare_worktree(root, options, &worktree, &links, deadline, say, "head")
    {
        all_not_judged(results, &units, &problem);
        return;
    }
    // The base's test files, and every changed conftest, snapshot and
    // runner config.
    let mut taken: BTreeSet<String> = units
        .values()
        .flat_map(|unit| unit.files.iter().cloned())
        .collect();
    taken.extend(
        changed
            .iter()
            .filter(|path| is_snapshot(path) || is_runner_config(path))
            .cloned(),
    );
    for path in &taken {
        if let Err(problem) = put_base_file(root, base, &worktree.root, path) {
            all_not_judged(
                results,
                &units,
                &format!("the base's {path} could not be put in the worktree: {problem}"),
            );
            return;
        }
    }

    for unit in units.values() {
        let step = place(&unit.folder);
        if options.stop.load(Ordering::SeqCst) {
            results.push(not_judged(&step, unit.files.clone(), "verify was stopped"));
            continue;
        }
        if blame::past(deadline) {
            results.push(not_judged(
                &step,
                unit.files.clone(),
                "no time was left for it",
            ));
            continue;
        }
        // Known to take longer than the time left (the stop hook's limit,
        // ADR 0083, or the base budget): not started.
        let known = options.times.and_then(|times| {
            times
                .last(
                    unit.project.map(|project| project.root.as_str()),
                    "test",
                    &unit.command,
                )
                .ok()
                .flatten()
        });
        if let (Some(known), Some(deadline)) = (known, deadline) {
            if Instant::now() + known.duration > deadline {
                results.push(not_judged(
                    &step,
                    unit.files.clone(),
                    format!(
                        "{}, longer than the time left (verify.baseBudget, or the stop hook's timeout)",
                        known.describe()
                    ),
                ));
                continue;
            }
        }
        say(&format!(
            "{step}: started with the base's tests: {}",
            unit.command
        ));
        let ran = run_base_step(
            &worktree.root,
            root,
            unit.project,
            "test",
            &unit.command,
            options,
            deadline,
        );
        let report = match ran {
            Ok(report) => report,
            Err(error) => {
                results.push(not_judged(
                    &step,
                    unit.files.clone(),
                    format!("it could not run: {}", error.message()),
                ));
                continue;
            }
        };
        match report.status {
            StepStatus::Passed => results.push(OldTests {
                step,
                status: OldTestsStatus::Passed,
                files: unit.files.clone(),
                detail: "the base's versions of these files pass on the head's code: the edits to them were not needed to pass (this lowers no finding)".to_owned(),
            }),
            StepStatus::Failed => {
                let failing: Vec<&String> = report
                    .test_results
                    .as_ref()
                    .map(|results| results.failed.iter().filter(|id| pertest::is_test(id)).collect())
                    .unwrap_or_default();
                let mut named = 0;
                for file in &unit.files {
                    let mine: Vec<&&String> = failing
                        .iter()
                        .filter(|id| in_file(unit.runner, id, &unit.folder, file))
                        .collect();
                    if mine.is_empty() {
                        continue;
                    }
                    named += mine.len();
                    let evidence: Vec<String> = mine
                        .iter()
                        .take(MAX_EVIDENCE)
                        .map(|id| {
                            let reason = report
                                .reason_text
                                .get(id.as_str())
                                .filter(|lines| !lines.is_empty())
                                .map_or_else(String::new, |lines| format!(": {}", lines.join(" / ")));
                            format!("the base's version fails on the head's code: {id}{reason}")
                        })
                        .collect();
                    for finding in tamper
                        .findings
                        .iter_mut()
                        .filter(|finding| &finding.file == file && KINDS.contains(&finding.kind))
                    {
                        finding.evidence.clone_from(&evidence);
                    }
                }
                if named > 0 {
                    results.push(OldTests {
                        step,
                        status: OldTestsStatus::Failed,
                        files: unit.files.clone(),
                        detail: format!(
                            "{named} test{} of the base's versions of these files fail{} on the head's code: the change needed different tests to pass",
                            if named == 1 { "" } else { "s" },
                            if named == 1 { "s" } else { "" }
                        ),
                    });
                } else if report.test_results.is_none() {
                    results.push(not_judged(
                        &step,
                        unit.files.clone(),
                        format!("it failed and its tests could not be read: they may not collect or build against the head's code ({})", report.detail),
                    ));
                } else {
                    results.push(not_judged(
                        &step,
                        unit.files.clone(),
                        "it failed, but no failing test is in these files",
                    ));
                }
            }
            _ => results.push(not_judged(&step, unit.files.clone(), report.detail.clone())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_belong_to_their_test_files() {
        assert_eq!(
            snapshot_owner("src/__snapshots__/a.test.ts.snap").as_deref(),
            Some("src/a.test.ts")
        );
        assert_eq!(
            snapshot_owner("tests/__snapshots__/test_x.ambr").as_deref(),
            Some("tests/test_x.py")
        );
        assert_eq!(snapshot_owner("src/a.test.ts"), None);
        assert!(is_snapshot("src/__snapshots__/a.test.ts.snap"));
        assert!(!is_snapshot("src/snap.ts"));
    }

    #[test]
    fn runner_config_and_rust_code_files() {
        assert!(is_runner_config("tests/conftest.py"));
        assert!(is_runner_config("vitest.config.mts"));
        assert!(is_runner_config("packages/a/jest.config.js"));
        assert!(!is_runner_config("src/config.ts"));
        assert!(holds_code("src/lib.rs"));
        assert!(!holds_code("tests/cart.rs"));
        assert!(!holds_code("tests/test_a.py"));
    }

    #[test]
    fn a_failing_test_is_matched_to_its_file() {
        assert!(in_file(
            TestRunner::Pytest,
            "tests/test_a.py::test_x",
            ".",
            "tests/test_a.py"
        ));
        assert!(in_file(
            TestRunner::Pytest,
            "tests/test_a.py::test_x",
            "api",
            "api/tests/test_a.py"
        ));
        assert!(!in_file(
            TestRunner::Pytest,
            "tests/test_b.py::test_x",
            ".",
            "tests/test_a.py"
        ));
        assert!(in_file(
            TestRunner::Vitest,
            "src/a.test.ts > math > adds",
            ".",
            "src/a.test.ts"
        ));
        assert!(in_file(
            TestRunner::Vitest,
            "src/a.test.ts > math > adds",
            ".",
            "src/__snapshots__/a.test.ts.snap"
        ));
        assert!(in_file(
            TestRunner::Jest,
            "src/a.test.js \u{203a} adds",
            ".",
            "src/a.test.js"
        ));
        assert!(in_file(
            TestRunner::Cargo,
            "cart::totals",
            ".",
            "tests/cart.rs"
        ));
        assert!(!in_file(
            TestRunner::Cargo,
            "cart::totals",
            ".",
            "src/cart.rs"
        ));
    }
}
