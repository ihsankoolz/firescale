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
use std::time::{Duration, Instant};

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
        // `tests/foo/main.rs` is the binary `foo`; cargo writes `-` as `_`.
        let binary = id.split("::").next().unwrap_or(id);
        let path = Path::new(&target);
        let stem = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .filter(|stem| stem != "main")
            .or_else(|| {
                path.parent()
                    .and_then(Path::file_name)
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .map(|stem| stem.replace('-', "_"));
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
    let real_into = into
        .canonicalize()
        .map_err(|error| format!("the worktree cannot be read: {error}"))?;
    // Never through a link that could not be taken away: the folder must be
    // inside the worktree before anything is removed or written there.
    let inside = |folder: &Path| {
        folder
            .canonicalize()
            .is_ok_and(|real| real.starts_with(&real_into))
    };
    if !on_base(root, base, path) {
        if to
            .parent()
            .is_some_and(|parent| parent.exists() && !inside(parent))
        {
            return Err(format!("{path} is not inside the worktree"));
        }
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
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("{path}: {error}"))?;
        if !inside(parent) {
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

/// What a run of the base's tests on the head's code works from: the repo,
/// the base commit, the files the change touched, the verify plan and its
/// options, the stop hook's limit, the base budget's end, and where
/// progress lines go.
#[derive(Clone, Copy)]
pub struct Run<'a, 'o> {
    pub root: &'a Path,
    pub base: &'a BaseCommit,
    pub changed: &'a BTreeSet<String>,
    pub plan: &'a VerifyPlan,
    pub options: &'a RunOptions<'o>,
    /// The stop hook's limit (ADR 0083), less its margin; `None` outside
    /// the hook. With one, nothing starts whose time is not kept (ADR 0090).
    pub deadline: Option<Instant>,
    /// When `verify.baseBudget` ends, as the base run set it (ADR 0090):
    /// `Some(None)` with no limit. `None` when no base run set it: the
    /// budget then starts now.
    pub budget_ends: Option<Option<Instant>>,
    pub say: &'a dyn Fn(&str),
}

/// The kept time of making and removing the worktree, beside the step
/// times (ADR 0090).
pub const WORKTREE_STEP: &str = "old tests: worktree";
const WORKTREE_COMMAND: &str = "a worktree of the head, made and removed";

/// Why nothing more could start: the budget the base run shares, or the
/// stop hook's limit.
fn out_of_time(budget_ends: Option<Instant>) -> &'static str {
    if blame::past(budget_ends) {
        "verify.baseBudget, which the base run shares, has run out"
    } else {
        "no time was left for it before the stop hook's limit"
    }
}

/// Runs the base's tests on the head's code for the guard's findings, and
/// adds what it shows to `tamper`: an entry per unit in `old_tests`, and the
/// failing tests as evidence on the findings. Nothing it does changes which
/// findings there are, or whether they block.
pub fn judge(run: &Run<'_, '_>, tamper: &mut TamperReport) {
    let Run {
        root,
        base,
        plan,
        options,
        ..
    } = *run;
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
    // One budget with the base run (ADR 0090).
    let budget_ends = run
        .budget_ends
        .unwrap_or_else(|| blame::budget_of(options).map(|limit| Instant::now() + limit));

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
        run_units(run, budget_ends, tamper, &units, &mut results);
    }
    tamper.old_tests = results;
}

/// The kept time of a command, if one is kept and readable.
fn kept(
    options: &RunOptions<'_>,
    project: Option<&str>,
    step: &str,
    command: &str,
) -> Option<Duration> {
    options
        .times
        .and_then(|times| times.last(project, step, command).ok().flatten())
        .map(|known| known.duration)
}

/// Makes the worktree, takes the base's files into it, and runs each unit's
/// test step once.
fn run_units(
    run: &Run<'_, '_>,
    budget_ends: Option<Instant>,
    tamper: &mut TamperReport,
    units: &BTreeMap<String, Unit<'_>>,
    results: &mut Vec<OldTests>,
) {
    let Run {
        root,
        base,
        changed,
        plan,
        options,
        deadline: hook,
        say,
        ..
    } = *run;
    let deadline = match (hook, budget_ends) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    let all_not_judged =
        |results: &mut Vec<OldTests>, units: &BTreeMap<String, Unit<'_>>, why: &str| {
            for unit in units.values() {
                results.push(not_judged(&place(&unit.folder), unit.files.clone(), why));
            }
        };
    if options.stop.load(Ordering::SeqCst) {
        all_not_judged(results, units, "verify was stopped");
        return;
    }
    if blame::past(deadline) {
        all_not_judged(results, units, out_of_time(budget_ends));
        return;
    }
    // The worktree's making and removal are timed and kept like a step
    // (ADR 0090). In the stop hook nothing starts unless every part of the
    // run has a kept time and all of it fits before the hook's limit: a hook
    // that its agent ends lets the agent stop unverified.
    let worktree_kept = kept(options, None, WORKTREE_STEP, WORKTREE_COMMAND);
    if let Some(limit) = hook {
        let steps: Option<Vec<Duration>> = units
            .values()
            .map(|unit| {
                kept(
                    options,
                    unit.project.map(|project| project.root.as_str()),
                    "test",
                    &unit.command,
                )
            })
            .collect();
        let (Some(worktree), Some(steps)) = (worktree_kept, steps) else {
            all_not_judged(
                results,
                units,
                "no time is kept yet for this run (the worktree's making and removal, and the test step), so it could go past the stop hook's limit; a verify outside the hook keeps them",
            );
            return;
        };
        let total = worktree + steps.iter().sum::<Duration>();
        if Instant::now() + total > limit {
            all_not_judged(
                results,
                units,
                &format!(
                    "it took {} when it last ran (the worktree and the test step), longer than the time left before the stop hook's limit",
                    crate::progress::format_duration(total)
                ),
            );
            return;
        }
    }
    // Room for removing the worktree after the last step.
    let reserve = worktree_kept.unwrap_or_default();
    say("old tests, new code: running the base's versions of the changed test files on the head's code");
    let setting_up = Instant::now();
    let head = match run_git(root, &["rev-parse", "--verify", "HEAD"]) {
        Ok(out) if out.code == 0 => out.stdout.trim().to_owned(),
        _ => {
            all_not_judged(results, units, "git could not name the head commit");
            return;
        }
    };
    let worktree = match Worktree::add(root, &head) {
        Ok(worktree) => worktree,
        Err(problem) => {
            all_not_judged(
                results,
                units,
                &format!("a worktree of the head could not be made: {problem}"),
            );
            return;
        }
    };
    if let Err(problem) = blame::overlay_head(root, &worktree.root, deadline, options.stop) {
        let why = problem.unwrap_or_else(|| out_of_time(budget_ends).to_owned());
        all_not_judged(
            results,
            units,
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
        all_not_judged(results, units, &problem);
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
                units,
                &format!("the base's {path} could not be put in the worktree: {problem}"),
            );
            return;
        }
    }
    let set_up = setting_up.elapsed();
    // Each step stops early enough to remove the worktree in time.
    let step_deadline = deadline.map(|at| at.checked_sub(reserve).unwrap_or(at));

    for unit in units.values() {
        let step = place(&unit.folder);
        if options.stop.load(Ordering::SeqCst) {
            results.push(not_judged(&step, unit.files.clone(), "verify was stopped"));
            continue;
        }
        if blame::past(step_deadline) {
            results.push(not_judged(
                &step,
                unit.files.clone(),
                out_of_time(budget_ends),
            ));
            continue;
        }
        // Known to take longer than the time left: not started.
        let known = kept(
            options,
            unit.project.map(|project| project.root.as_str()),
            "test",
            &unit.command,
        );
        if let (Some(known), Some(at)) = (known, step_deadline) {
            if Instant::now() + known > at {
                results.push(not_judged(
                    &step,
                    unit.files.clone(),
                    format!(
                        "it took {} when it last ran, longer than the time left ({})",
                        crate::progress::format_duration(known),
                        if blame::past(budget_ends.map(|end| end - known)) {
                            "verify.baseBudget, which the base run shares"
                        } else {
                            "the stop hook's limit"
                        }
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
            step_deadline,
        );
        match ran {
            Ok(report) => results.push(read_run(root, unit, &report, step, tamper)),
            Err(error) => results.push(not_judged(
                &step,
                unit.files.clone(),
                format!("it could not run: {}", error.message()),
            )),
        }
    }
    let removing = Instant::now();
    drop(worktree);
    if let Some(times) = options.times {
        times.record_ended(
            None,
            WORKTREE_STEP,
            WORKTREE_COMMAND,
            set_up + removing.elapsed(),
            crate::steptimes::Outcome::Passed,
        );
    }
}

/// What one unit's run showed, with evidence added to the findings: the
/// runner's results, not its exit code alone (review of ADR 0089). A run
/// with no failing test of the base's files adds nothing a person sees
/// (ADR 0090).
fn read_run(
    root: &Path,
    unit: &Unit<'_>,
    report: &crate::verify::StepReport,
    step: String,
    tamper: &mut TamperReport,
) -> OldTests {
    if !matches!(report.status, StepStatus::Passed | StepStatus::Failed) {
        return not_judged(&step, unit.files.clone(), report.detail.clone());
    }
    let Some(results) = report.test_results.as_ref() else {
        return not_judged(
            &step,
            unit.files.clone(),
            format!(
                "its tests could not be read: they may not collect or build against the head's code ({})",
                report.detail
            ),
        );
    };
    let line = |id: &str| {
        let reason = report
            .reason_text
            .get(id)
            .filter(|lines| !lines.is_empty())
            .map_or_else(String::new, |lines| format!(": {}", lines.join(" / ")));
        format!("{}{reason}", id.replace('\\', "/"))
    };
    let mut sure: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut unsure: Vec<(String, String)> = Vec::new();
    for id in results.failed.iter().filter(|id| pertest::is_test(id)) {
        match tie(root, &unit.folder, unit.runner, id, &unit.files) {
            Tie::File(file) => sure.entry(file).or_default().push(id.clone()),
            Tie::Unsure(why) => unsure.push((id.clone(), why)),
            Tie::Elsewhere => {}
        }
    }
    for (file, ids) in &sure {
        let evidence: Vec<String> = ids
            .iter()
            .take(MAX_EVIDENCE)
            .map(|id| format!("the base's version fails on the head's code: {}", line(id)))
            .collect();
        for finding in tamper
            .findings
            .iter_mut()
            .filter(|finding| &finding.file == file && KINDS.contains(&finding.kind))
        {
            finding.evidence.clone_from(&evidence);
        }
    }
    // A failing test that cannot be tied to one file for sure: under every
    // finding whose file it could be from, naming no file (ADR 0090).
    for (id, why) in unsure.iter().take(MAX_EVIDENCE) {
        let text = format!(
            "a test failed on the head's code with the base's test files, but it cannot be tied to one file for sure ({why}): {}",
            line(id)
        );
        for finding in tamper.findings.iter_mut().filter(|finding| {
            KINDS.contains(&finding.kind)
                && unit.files.contains(&finding.file)
                && in_file(unit.runner, id, &unit.folder, &finding.file)
        }) {
            finding.evidence.push(text.clone());
        }
    }
    let named: usize = sure.values().map(Vec::len).sum::<usize>() + unsure.len();
    if named > 0 {
        let untied = if unsure.is_empty() {
            String::new()
        } else {
            format!(
                "; {} of them cannot be tied to one file for sure",
                unsure.len()
            )
        };
        return OldTests {
            step,
            status: OldTestsStatus::Failed,
            files: unit.files.clone(),
            detail: format!(
                "{named} test{} of the base's versions of these files fail{} on the head's code: the change needed different tests to pass{untied}",
                if named == 1 { "" } else { "s" },
                if named == 1 { "s" } else { "" }
            ),
        };
    }
    if report.status == StepStatus::Failed {
        return not_judged(
            &step,
            unit.files.clone(),
            "it failed, but no failing test is in these files",
        );
    }
    // A pass: only when the results show a test of these files ran. Never
    // reassurance (ADR 0090): `ran-no-failure` is in JSON only.
    let collected = results.ran.iter().any(|id| {
        matches!(
            tie(root, &unit.folder, unit.runner, id, &unit.files),
            Tie::File(_)
        )
    });
    if !collected {
        return not_judged(
            &step,
            unit.files.clone(),
            "the run collected none of the base's tests of these files, as far as its results show",
        );
    }
    OldTests {
        step,
        status: OldTestsStatus::RanNoFailure,
        files: unit.files.clone(),
        detail: "it ran; no test of these files failed. This shows nothing about the change"
            .to_owned(),
    }
}

/// Which of the run's files a test the runner names belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Tie {
    /// This file, for sure.
    File(String),
    /// Maybe one of them, maybe not: why it cannot be told.
    Unsure(String),
    /// None of them.
    Elsewhere,
}

/// Folders a search for test files never enters.
const SKIPPED_FOLDERS: [&str; 4] = ["target", "node_modules", ".git", ".firescale"];

/// At most this many files are looked at for `cargo test`'s binaries.
const MAX_FILES: usize = 50_000;

/// Every file below `folder` of `root`, repo-relative, forward slashes,
/// leaving out build and dependency folders.
fn files_below(root: &Path, folder: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![(
        blame::folder_of(root, Some(folder)),
        if folder == "." {
            String::new()
        } else {
            format!("{folder}/")
        },
    )];
    while let Some((dir, prefix)) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if out.len() >= MAX_FILES {
                return out;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if !SKIPPED_FOLDERS.contains(&name.as_str()) {
                    stack.push((entry.path(), format!("{prefix}{name}/")));
                }
            } else if kind.is_file() {
                out.push(format!("{prefix}{name}"));
            }
        }
    }
    out
}

/// The name of the test binary cargo builds from `path`: `tests/cart.rs`
/// and `tests/cart/main.rs` are `cart`; `-` is written `_`.
fn cargo_binary(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.split('/').collect();
    let name = match parts.as_slice() {
        [.., "tests", file] => file.strip_suffix(".rs")?,
        [.., "tests", dir, "main.rs"] => dir,
        _ => return None,
    };
    Some(name.replace('-', "_"))
}

/// The names cargo may give a binary of the crates below `folder`: each
/// `[package]`, `[lib]` and `[[test]]` name, with `-` written `_`.
fn crate_names(root: &Path, files: &[String]) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for manifest in files
        .iter()
        .filter(|path| path.rsplit('/').next() == Some("Cargo.toml"))
    {
        let Ok(text) = fs::read_to_string(blame::folder_of(root, Some(manifest))) else {
            continue;
        };
        let mut section = String::new();
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                section = line.to_owned();
                continue;
            }
            if !matches!(section.as_str(), "[package]" | "[lib]" | "[[test]]") {
                continue;
            }
            if let Some(value) = line
                .strip_prefix("name")
                .map(str::trim_start)
                .and_then(|rest| rest.strip_prefix('='))
            {
                let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
                names.insert(value.replace('-', "_"));
            }
        }
    }
    names
}

/// Ties a test the runner names (failing, or passing in `ran`) to one of
/// `files`, only when no other file could hold it (ADR 0090). Pytest,
/// Vitest and Jest name a file; `cargo test` names a binary, which a
/// crate's library, a `[[test]]` target or another crate's file of the same
/// name could also be.
fn tie(root: &Path, folder: &str, runner: TestRunner, id: &str, files: &[String]) -> Tie {
    if runner != TestRunner::Cargo {
        let mine: Vec<&String> = files
            .iter()
            .filter(|file| in_file(runner, id, folder, file))
            .collect();
        return match mine.as_slice() {
            [] => Tie::Elsewhere,
            [one] => Tie::File((*one).clone()),
            many => Tie::Unsure(format!("{} of the files could hold it", many.len())),
        };
    }
    let binary = id.split("::").next().unwrap_or(id);
    if !files
        .iter()
        .any(|file| cargo_binary(file).as_deref() == Some(binary))
    {
        return Tie::Elsewhere;
    }
    let below = files_below(root, folder);
    if crate_names(root, &below).contains(binary) {
        return Tie::Unsure(format!(
            "cargo names the test binary `{binary}`, and a crate is named `{binary}` too: its library's own tests run under that name"
        ));
    }
    let holders: Vec<&String> = below
        .iter()
        .filter(|path| cargo_binary(path).as_deref() == Some(binary))
        .collect();
    match holders.as_slice() {
        [one] if files.contains(one) => Tie::File((*one).clone()),
        [_] | [] => Tie::Elsewhere,
        many => Tie::Unsure(format!(
            "cargo names the test binary `{binary}`, and {} files could build it",
            many.len()
        )),
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

    fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (path, text) in files {
            let at = dir.path().join(path);
            fs::create_dir_all(at.parent().unwrap()).unwrap();
            fs::write(at, text).unwrap();
        }
        dir
    }

    #[test]
    fn a_cargo_test_is_tied_to_a_file_only_when_no_other_could_be_it() {
        let cand = |files: &[&str]| files.iter().map(|f| (*f).to_owned()).collect::<Vec<_>>();
        // One crate `shop`, one `tests/cart.rs`: sure.
        let clear = tree(&[
            ("Cargo.toml", "[package]\nname = \"shop\"\n"),
            ("tests/cart.rs", ""),
        ]);
        assert_eq!(
            tie(
                clear.path(),
                ".",
                TestRunner::Cargo,
                "cart::totals",
                &cand(&["tests/cart.rs"])
            ),
            Tie::File("tests/cart.rs".to_owned())
        );
        // The crate is named `cart` too: its library's own tests run as `cart`.
        let clash = tree(&[
            ("Cargo.toml", "[package]\nname = \"cart\"\n"),
            ("tests/cart.rs", ""),
        ]);
        assert!(matches!(
            tie(clash.path(), ".", TestRunner::Cargo, "cart::totals", &cand(&["tests/cart.rs"])),
            Tie::Unsure(why) if why.contains("a crate is named `cart`")
        ));
        // Two crates' `tests/x.rs` under one unit.
        let two = tree(&[
            ("a/Cargo.toml", "[package]\nname = \"a\"\n"),
            ("a/tests/x.rs", ""),
            ("b/Cargo.toml", "[package]\nname = \"b\"\n"),
            ("b/tests/x.rs", ""),
        ]);
        assert!(matches!(
            tie(two.path(), ".", TestRunner::Cargo, "x::totals", &cand(&["a/tests/x.rs"])),
            Tie::Unsure(why) if why.contains("2 files")
        ));
        // A binary no candidate could be.
        assert_eq!(
            tie(
                clear.path(),
                ".",
                TestRunner::Cargo,
                "other::t",
                &cand(&["tests/cart.rs"])
            ),
            Tie::Elsewhere
        );
        // Pytest names the file: sure.
        assert_eq!(
            tie(
                clear.path(),
                ".",
                TestRunner::Pytest,
                "tests/test_a.py::t",
                &cand(&["tests/test_a.py"])
            ),
            Tie::File("tests/test_a.py".to_owned())
        );
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
        // Cargo names a test binary with `_` for `-`, and `tests/foo/main.rs`
        // after its folder (review of ADR 0089).
        assert!(in_file(
            TestRunner::Cargo,
            "my_file::totals",
            ".",
            "tests/my-file.rs"
        ));
        assert!(in_file(
            TestRunner::Cargo,
            "foo::totals",
            ".",
            "tests/foo/main.rs"
        ));
    }
}
