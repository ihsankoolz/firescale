//! A property test (ADR 0077): over fixture repos and random changes, every
//! unit whose step key differs between the repo before a change and after
//! it is in the affected set of that change.
//!
//! `verify --affected` and the step cache each work out what a change
//! reaches: one walks the project graph from the changed files, the other
//! hashes the files a unit's key covers. If the key covers more than
//! affected reaches, `--affected` leaves out a step whose inputs changed.
//! This test looks for such a case. The cases are made by a small generator
//! with fixed seeds, so every run tries the same ones.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::affected::Changed;
use crate::doctor::Machine;
use crate::facts::detect_facts_with;
use crate::graph::{affected, build_graph};
use crate::stepcache::{CacheSettings, StepCache};
use crate::testing::{course_project_files, Fixture, Track};
use crate::verify::{limit_to_affected, plan_verify, Planned, VerifyConfig};
use crate::walk::{walk_entries, EntryKind};

struct NoTools;
impl Machine for NoTools {
    fn run(&self, _: &Path, program: &str, _: &[&str]) -> Option<String> {
        Some(format!("{program} 1.0.0"))
    }
    fn env_set(&self, _: &str) -> bool {
        false
    }
    fn listening(&self, _: &str, _: u16) -> bool {
        false
    }
}

/// xorshift64*: the same numbers for the same seed, on every machine.
struct Random(u64);

impl Random {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, count: usize) -> usize {
        usize::try_from(self.next() % (count as u64)).unwrap()
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

/// A unit of a verify plan: the project's folder and kind, `None` for the
/// repo root's own steps.
type Unit = (Option<String>, Option<String>);

/// What one state of a repo gives: each unit's key (or why it has none),
/// and the units a set of changed files affects.
struct State {
    keys: BTreeMap<Unit, Result<String, String>>,
    affected: BTreeMap<Unit, bool>,
    /// The changed files that affected counts as inert (ADR 0034).
    inert: Vec<String>,
}

fn state(root: &Path, changed: &BTreeSet<String>) -> State {
    let facts = detect_facts_with(root, None).unwrap();
    let graph = build_graph(root, &facts);
    let mut plan = plan_verify(root, &facts, &VerifyConfig::default(), true, None);
    let set = affected(&graph, &Changed::Files(changed.clone()));
    limit_to_affected(&mut plan, &facts, &graph, &set, None);
    let affected: BTreeMap<Unit, bool> = plan
        .affected
        .as_ref()
        .unwrap()
        .units
        .iter()
        .map(|unit| ((unit.project.clone(), unit.kind.clone()), unit.affected))
        .collect();
    let read_env = |_: &str| None;
    let cache = StepCache::new(
        root,
        Some(graph),
        &CacheSettings {
            version: "0.0.0",
            read: true,
            sets_ci: true,
            settings: "",
            machine: &NoTools,
            read_env: &read_env,
        },
    );
    let mut units: Vec<Unit> = Vec::new();
    if plan
        .steps
        .iter()
        .any(|(_, planned)| !matches!(planned, Planned::NotNeeded(_)))
    {
        units.push((None, None));
    }
    for project in &plan.projects {
        units.push((Some(project.root.clone()), project.kind.clone()));
    }
    let keys = units
        .into_iter()
        .map(|(folder, kind)| {
            let dir = folder
                .as_deref()
                .filter(|folder| *folder != ".")
                .into_iter()
                .flat_map(|folder| folder.split('/'))
                .fold(root.to_path_buf(), |path, part| path.join(part));
            // The same command for every unit: only the files differ.
            let key = cache
                .key(
                    folder.as_deref(),
                    kind.as_deref(),
                    "test",
                    "node -e 0",
                    &dir,
                )
                .map(|key| key.key);
            ((folder, kind), key)
        })
        .collect();
    State {
        keys,
        affected,
        inert: set.inert_files,
    }
}

/// The files of the repo that a change may touch, as the snapshot lists
/// them.
fn files(root: &Path) -> Vec<String> {
    let mut files: Vec<String> = walk_entries(root)
        .unwrap()
        .into_iter()
        .filter(|(_, kind)| *kind != EntryKind::Folder)
        .map(|(path, _)| path)
        // Dependency, cache and build folders: git ignores them in a real
        // repo, and no change is made there.
        .filter(|path| !path.split('/').any(|part| SKIPPED.contains(&part)))
        .collect();
    files.sort();
    files
}

/// Folders of the fixtures that hold no source of the repo.
const SKIPPED: [&str; 9] = [
    "node_modules",
    "dist",
    ".venv",
    "__pycache__",
    ".pytest_cache",
    ".ruff_cache",
    "generated",
    "test-results",
    "target",
];

/// File names a change may add: source, config, documents, data.
const NEW_NAMES: [&str; 14] = [
    "new_module.py",
    "helper.ts",
    "notes.md",
    "data.json",
    "settings.toml",
    "picture.png",
    "Dockerfile.dev",
    "README.md",
    "conftest.py",
    "words.txt",
    ".env.example",
    "Makefile",
    "tool.sh",
    "LICENSE",
];

/// Folders a change may add a file to, beside the folders that hold a file.
const NEW_FOLDERS: [&str; 6] = [
    "docs",
    "scripts",
    "fixtures/data",
    "backend",
    "tools/gen",
    ".github/workflows",
];

/// One change: a file and its bytes after the change, `None` when the
/// change deleted it.
type Op = (String, Option<Vec<u8>>);

/// The path of `rel` under `root`.
fn at(root: &Path, rel: &str) -> std::path::PathBuf {
    rel.split('/')
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

/// Makes the changes again, on a fresh copy of the fixture.
fn replay(root: &Path, ops: &[&Op]) {
    for (file, bytes) in ops {
        let path = at(root, file);
        match bytes {
            Some(bytes) => {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, bytes).unwrap();
            }
            None => {
                // An earlier change that is left out may have made the file.
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// The paths a list of changes touched.
fn touched_by<'a>(ops: impl IntoIterator<Item = &'a Op>) -> BTreeSet<String> {
    ops.into_iter().map(|(file, _)| file.clone()).collect()
}

/// Makes one to three random changes under `root` and gives them in order.
fn change(root: &Path, random: &mut Random) -> Vec<Op> {
    let mut ops = Vec::new();
    for _ in 0..=random.below(3) {
        let existing = files(root);
        let at = |rel: &str| at(root, rel);
        let rel = match random.below(10) {
            // Edit a file.
            0..=3 => {
                let file = random.pick(&existing).clone();
                let mut text = std::fs::read(at(&file)).unwrap();
                text.extend_from_slice(b"\n# changed\n");
                std::fs::write(at(&file), text).unwrap();
                file
            }
            // Delete a file.
            4 | 5 => {
                let file = random.pick(&existing).clone();
                std::fs::remove_file(at(&file)).unwrap();
                file
            }
            // Add a file beside an existing one, at the root, or in a
            // folder of its own.
            _ => {
                let folder = match random.below(6) {
                    0 => String::new(),
                    1 | 2 => (*random.pick(&NEW_FOLDERS)).to_owned(),
                    _ => {
                        let beside = random.pick(&existing);
                        beside
                            .rsplit_once('/')
                            .map_or(String::new(), |(folder, _)| folder.to_owned())
                    }
                };
                let name = random.pick(&NEW_NAMES);
                let file = if folder.is_empty() {
                    (*name).to_owned()
                } else {
                    format!("{folder}/{name}")
                };
                let path = at(&file);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                let text = if name.ends_with(".py") {
                    "from shared.db import URL\n"
                } else {
                    "new\n"
                };
                std::fs::write(path, text).unwrap();
                file
            }
        };
        let bytes = std::fs::read(at(&rel)).ok();
        ops.push((rel, bytes));
    }
    ops
}

/// A workspace with a root project: the root's commands run its members.
fn workspace_files() -> Vec<(String, String)> {
    [
        (
            "package.json",
            r#"{"name":"mono","private":true,"scripts":{"test":"vitest run","lint":"eslint ."}}"#,
        ),
        ("pnpm-workspace.yaml", "packages:\n  - 'packages/*'\n"),
        ("pnpm-lock.yaml", "lockfileVersion: '9.0'\n"),
        ("README.md", "# Mono\n"),
        ("docs/guide.md", "# Guide\n"),
        ("scripts/release.js", "console.log('release');\n"),
        (".github/workflows/ci.yml", "on: [push]\n"),
        (
            "packages/ui/package.json",
            r#"{"name":"@mono/ui","dependencies":{"@mono/core":"workspace:*"}}"#,
        ),
        (
            "packages/ui/src/index.ts",
            "import { core } from '@mono/core';\nexport const ui = core;\n",
        ),
        ("packages/core/package.json", r#"{"name":"@mono/core"}"#),
        ("packages/core/src/index.ts", "export const core = 1;\n"),
        ("packages/core/README.md", "# Core\n"),
    ]
    .into_iter()
    .map(|(path, text)| (path.to_owned(), text.to_owned()))
    .collect()
}

/// Projects below a root that has no project of its own, with a folder
/// two of them share, documents and files that belong to no project.
fn split_files() -> Vec<(String, String)> {
    [
        ("README.md", "# Split\n"),
        ("LICENSE", "MIT\n"),
        ("docs/design.md", "# Design\n"),
        ("fixtures/words.txt", "one\ntwo\n"),
        ("scripts/seed.py", "print('seed')\n"),
        (".github/workflows/ci.yml", "on: [push]\n"),
        ("api/pyproject.toml", "[project]\nname = \"api\"\n"),
        ("api/main.py", "from common.util import helper\n"),
        (
            "api/tests/test_main.py",
            "def test_ok():\n    assert True\n",
        ),
        ("worker/pyproject.toml", "[project]\nname = \"worker\"\n"),
        ("worker/main.py", "from common.util import helper\n"),
        ("common/util.py", "def helper():\n    return 1\n"),
        ("common/__init__.py", ""),
        (
            "web/package.json",
            r#"{"name":"web","scripts":{"test":"vitest run"}}"#,
        ),
        ("web/src/app.ts", "export const app = 1;\n"),
        ("web/README.md", "# Web\n"),
        // A project inside another project's folder.
        (
            "api/plugins/pyproject.toml",
            "[project]\nname = \"plugins\"\n",
        ),
        ("api/plugins/plugin.py", "x = 1\n"),
    ]
    .into_iter()
    .map(|(path, text)| (path.to_owned(), text.to_owned()))
    .collect()
}

/// A project at the repo root, with projects of other kinds below it that
/// run their own steps, and files that belong to none of them.
fn rooted_files() -> Vec<(String, String)> {
    [
        (
            "package.json",
            r#"{"name":"site","private":true,"scripts":{"test":"vitest run"}}"#,
        ),
        ("src/index.ts", "export const site = 1;\n"),
        ("README.md", "# Site\n"),
        ("scripts/seed.py", "print('seed')\n"),
        ("assets/logo.png", "png\n"),
        ("backend/pyproject.toml", "[project]\nname = \"backend\"\n"),
        ("backend/main.py", "app = None\n"),
        (
            "backend/tests/test_main.py",
            "def test_ok():\n    assert True\n",
        ),
        (
            "tools/cli/Cargo.toml",
            "[package]\nname = \"cli\"\nversion = \"0.1.0\"\n",
        ),
        ("tools/cli/src/main.rs", "fn main() {}\n"),
    ]
    .into_iter()
    .map(|(path, text)| (path.to_owned(), text.to_owned()))
    .collect()
}

/// How many seeds each fixture is tried with.
const CASES: u64 = 60;

/// What the cases of one fixture gave.
struct Outcome {
    /// Every case that breaks the property and is not the known exception.
    broken: Vec<String>,
    /// The one known exception, kept by the owner's decision (ADR 0077,
    /// gap 1; ADR 0034): a changed file is one that affected counts as
    /// inert (a document, a picture, `docs/`), and a key holds it. Inert
    /// documents stay out of `--affected` on purpose.
    inert: Vec<String>,
    /// How many keys were compared.
    compared: usize,
}

/// Runs the cases of one fixture.
fn outcome(name: &str, files: &[(String, String)]) -> Outcome {
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    let mut outcome = Outcome {
        broken: Vec::new(),
        inert: Vec::new(),
        compared: 0,
    };
    for seed in 0..CASES {
        let fixture = Fixture::new(&borrowed, Track::NoGit).unwrap();
        let root = fixture.root();
        let before = state(root, &BTreeSet::new());
        let mut random = Random::new(seed);
        let ops = change(root, &mut random);
        let touched = touched_by(&ops);
        let after = state(root, &touched);
        for (unit, key) in &after.keys {
            // A unit with no key is never served from the cache, so
            // nothing is compared for it. Nor is a unit the change made.
            let (Ok(key), Some(Ok(old))) = (key, before.keys.get(unit)) else {
                continue;
            };
            outcome.compared += 1;
            if key == old || after.affected.get(unit) == Some(&true) {
                continue;
            }
            let case = format!(
                "{name}, seed {seed}: the key of {unit:?} differs after {touched:?}, and it is not affected ({:?}); inert: {:?}",
                after.affected.get(unit),
                after.inert
            );
            // The changes the known exception covers for this unit: inert
            // files (ADR 0034, kept by the owner's decision; ADR 0077). The
            // case is excused only when the other changes of the seed, made
            // alone on a fresh copy, leave the unit's key as it was: then
            // no other change is why the key differs.
            let inert = |file: &str| after.inert.iter().any(|inert| inert == file);
            let rest: Vec<&Op> = ops.iter().filter(|(file, _)| !inert(file)).collect();
            let explained = rest.len() < ops.len() && {
                let control = Fixture::new(&borrowed, Track::NoGit).unwrap();
                replay(control.root(), &rest);
                let alone = state(control.root(), &touched_by(rest.iter().copied()));
                alone.keys.get(unit) == Some(&Ok(old.clone()))
            };
            if explained {
                outcome.inert.push(case);
            } else {
                outcome.broken.push(case);
            }
        }
    }
    outcome
}

/// The property for one fixture: no case breaks it, apart from the one known
/// exception (inert documents, ADR 0034), and enough keys were compared for
/// that to mean something.
fn holds(name: &str, files: &[(String, String)], at_least: usize) {
    let outcome = outcome(name, files);
    // Shown with `--nocapture`: the numbers ADR 0077 quotes.
    println!(
        "{name}: {} keys compared, {} in the known exception (inert files), {} broken",
        outcome.compared,
        outcome.inert.len(),
        outcome.broken.len()
    );
    assert!(outcome.broken.is_empty(), "{}", outcome.broken.join("\n"));
    assert!(
        outcome.compared > at_least,
        "{name}: only {} keys were compared",
        outcome.compared
    );
}

#[test]
fn every_unit_whose_key_differs_is_affected_in_the_course_project() {
    holds("course project", &course_project_files(), 200);
}

#[test]
fn every_unit_whose_key_differs_is_affected_in_a_workspace_with_a_root_project() {
    holds("workspace", &workspace_files(), 30);
}

#[test]
fn every_unit_whose_key_differs_is_affected_in_projects_below_a_plain_root() {
    holds("split repo", &split_files(), 100);
}

#[test]
fn every_unit_whose_key_differs_is_affected_below_a_root_project() {
    holds("rooted repo", &rooted_files(), 60);
}

/// The one known exception, pinned so that it is seen (ADR 0077, gap 1;
/// kept by the owner's decision of 2026-11-25): a document below the repo
/// root is in the key of every project (a test may read it by its path,
/// ADR 0043), and affected counts it as inert (ADR 0034). `verify
/// --affected` then runs nothing, and a plain `fscale verify` runs every
/// project again. When this test fails, the exception was closed: take the
/// case out of [`Outcome::inert`] too.
#[test]
fn known_gap_a_document_below_the_root_is_in_the_keys_and_affects_nothing() {
    let files = split_files();
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    for file in ["docs/design.md", "fixtures/words.txt"] {
        let fixture = Fixture::new(&borrowed, Track::NoGit).unwrap();
        let root = fixture.root();
        let before = state(root, &BTreeSet::new());
        fixture.write(file, "changed\n").unwrap();
        let after = state(root, &BTreeSet::from([file.to_owned()]));
        assert_eq!(after.inert, [file], "{file}");
        assert_eq!(after.keys.len(), 3, "{file}: {:?}", after.keys.keys());
        for (unit, key) in &after.keys {
            assert_ne!(Some(key), before.keys.get(unit), "{file}: {unit:?}");
            assert_eq!(after.affected.get(unit), Some(&false), "{file}: {unit:?}");
        }
    }
    // The generator meets the gap in every fixture that has such a file.
    assert!(!outcome("split repo", &files).inert.is_empty());
}

/// Gap 2 of ADR 0077, closed by the owner's decision of 2026-11-25:
/// where the repo root is a project, a file in no project below it is in
/// the key of every project below, so it affects them too, as well as the
/// root project and the repo root's own steps.
#[test]
fn a_file_of_the_root_project_affects_the_projects_below_it_too() {
    let files = rooted_files();
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    for file in ["src/index.ts", "assets/logo.png", "scripts/seed.py"] {
        let fixture = Fixture::new(&borrowed, Track::NoGit).unwrap();
        let root = fixture.root();
        let before = state(root, &BTreeSet::new());
        fixture.write(file, "changed\n").unwrap();
        let after = state(root, &BTreeSet::from([file.to_owned()]));
        let unit = |folder: &str, kind: &str| (Some(folder.to_owned()), Some(kind.to_owned()));
        assert_eq!(after.affected.get(&(None, None)), Some(&true), "{file}");
        for below in [unit("backend", "python"), unit("tools/cli", "rust")] {
            assert_ne!(
                after.keys.get(&below),
                before.keys.get(&below),
                "{file}: {below:?}"
            );
            assert_eq!(after.affected.get(&below), Some(&true), "{file}: {below:?}");
        }
    }
}

/// The fix this test led to (ADR 0077): the repo root's own steps run when
/// a project below the root changed, since their key holds every file of
/// the repo. Seen failing first: `Some(false)`.
#[test]
fn the_roots_own_steps_run_when_a_project_below_the_root_changed() {
    let files = rooted_files();
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    for file in ["backend/main.py", "tools/cli/src/main.rs"] {
        let fixture = Fixture::new(&borrowed, Track::NoGit).unwrap();
        let root = fixture.root();
        let before = state(root, &BTreeSet::new());
        fixture.write(file, "changed\n").unwrap();
        let after = state(root, &BTreeSet::from([file.to_owned()]));
        let root_unit = (None, None);
        assert_ne!(after.keys.get(&root_unit), before.keys.get(&root_unit));
        assert_eq!(after.affected.get(&root_unit), Some(&true), "{file}");
    }
    // Nothing changed: nothing runs.
    let fixture = Fixture::new(&borrowed, Track::NoGit).unwrap();
    let idle = state(fixture.root(), &BTreeSet::new());
    assert!(
        idle.affected.values().all(|runs| !runs),
        "{:?}",
        idle.affected
    );
}

/// The generator gives the same changes for the same seed, and different
/// ones for different seeds: a failing case can be run again by its seed.
#[test]
fn the_changes_of_a_seed_are_the_same_every_time() {
    let files = split_files();
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    let touched = |seed: u64| {
        let fixture = Fixture::new(&borrowed, Track::NoGit).unwrap();
        touched_by(&change(fixture.root(), &mut Random::new(seed)))
    };
    assert_eq!(touched(7), touched(7));
    let distinct: BTreeSet<BTreeSet<String>> = (0..20).map(touched).collect();
    assert!(distinct.len() > 10, "{distinct:?}");
}
