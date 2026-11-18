# 0077 — Affected covers the key: a property test, one widening, and every owner of an ambiguous import

Status: accepted (2026-11-18, Track A of the run after 0.5.4; the owner's brief). Adds to ADR 0034 (what a changed file affects) and ADR 0036 (what a step's key holds).

## Context

`docs/research/research-build-tools.md`, idea 11, the test only. `verify --affected` and the step cache each work out what a change reaches. Affected walks the project graph from the changed files. The cache hashes the files a unit's key covers. Nothing checked that the two agree. Where the key covers more than affected reaches, `--affected` leaves out a step whose inputs changed, and a plain `fscale verify` would run it again.

## Decision

**A property test: over fixture repos and random changes, every unit whose step key differs between the repo before the change and after it is in the affected set of that change.** `crates/core/src/affected_covers_key.rs`.

- Four fixtures: the course project's shape; a pnpm workspace with a root project; projects below a root that is no project, with a shared folder; a project at the root with projects of other kinds below it.
- 60 seeds for each. A seed makes one to three changes: edit a file, delete a file, or add one (source, config, a document, a picture) beside an existing file, at the root or in a new folder. A small generator in the test file gives the same changes for the same seed on every machine. No new dependency.
- For each unit of the plan, the key before and after, for the same command. A unit with no key is never served from the cache and is not compared.

**It found three cases. One is fixed here, by widening. Two are pinned and wait for you.**

**Fixed: the repo root's own steps now run when a project below the root changed.** Their key holds every file of the repo. Before, with a project at the root and a Python or Rust project below it, a change in the project below ran that project only. Now the root's steps run too, with the reason `the repo root's commands run from the repo root and can read every file below it, and a project below it changed`. This only adds runs. `limit_to_affected` in `crates/core/src/verify.rs`.

**An ambiguous Python import is an edge to every place that could hold it, with a warning** (build idea 6, item 2; added in its own pull request). Before, the first folder above the project that had the module won, and a second folder above, or another project's package of the same name, got no edge. A change there then affected nothing and left the importer's key as it was. Now `python_targets` in `crates/core/src/graph.rs` gives every folder above that has the module and every other Python project that has it as a top-level package. The importer gets an edge to each. When they are more than one node, the graph has a warning:

```
backend/svc/main.py:2: `from shared.db import URL` could come from 2 places in the repo (backend/shared/db.py, shared/db.py), and firescale cannot tell which one Python picks. It counts as depending on each.
```

This adds edges only. The importer's own code still comes first and gives no edge, as Python's own path has it. An edge is in the step's key and in `--affected`, so both get wider together.

## Consequences

- A project whose import could come from two places now has more in its key: its kept passes from before are not reused once, and a change in either place runs it.

- With a project at the repo root and other projects below it, `--affected` runs the root's own steps more often than before. It applies wherever the root has steps of its own that are tied to projects (package scripts, Cargo) and verify also runs a project below. The course project has no steps at the root, so nothing changes there.
- The property is now checked on every test run, 4 fixtures by 60 seeds, in about 7 seconds.
- The two gaps below stay, and each has a test that says so. A change that closes one makes that test fail and name what to do.

## The numbers

| Fixture                            | Keys compared | In gap 1 | In gap 2 | Broken |
| ---------------------------------- | ------------- | -------- | -------- | ------ |
| Course project                     | 355           | 7        | 0        | 0      |
| Workspace with a root project      | 60            | 7        | 0        | 0      |
| Projects below a plain root        | 173           | 13       | 0        | 0      |
| A root project with projects below | 175           | 8        | 28       | 0      |

Without the fix the last row has 14 broken cases, all of them the repo root's own steps. The first form of the test counted 3 of them as gap 1, because a document changed in the same seed; the fresh review found that.

## Needs your decision

Both gaps are places where the key holds a file and affected does not reach it. The brief allows one direction only: widen affected. I did not, because each widening changes what an earlier decision and its tests say.

1. **A document that belongs to no project is in every key and affects nothing.** `README.md`, `docs/design.md`, `fixtures/words.txt`, a picture: ADR 0034 says a change to one runs nothing. The key of every project still holds it (ADR 0043: a test may read it by its path). So `verify --affected` runs nothing after a README edit, and a plain `fscale verify` runs every project again. Closing it by widening means a README edit runs everything, which undoes ADR 0034. Closing it the other way takes a file out of the key, which this run may not do. Pinned by `known_gap_a_document_below_the_root_is_in_the_keys_and_affects_nothing`.
2. **With a project at the repo root, a file in no project below it is the root project's only.** `src/index.ts` of the root project, `scripts/seed.py`, `assets/logo.png`: affected gives the file to the root project. The key of every project below holds it too. I tried the widening (such a file affects everything). It made two existing tests fail: `a_root_file_that_a_project_copies_affects_that_project_and_the_root_project` and `an_image_a_project_imports_or_a_root_project_holds_is_not_inert`. Both say that such a file runs the root project and what copies or imports it, and no more. I took the widening back and did not touch those tests. Pinned by `known_gap_a_file_of_the_root_project_is_in_the_keys_of_the_projects_below`.

Until you decide, the property test counts a case as a known gap only when the gap explains it: the seed's other changes, made alone on a fresh copy of the fixture, leave that unit's key as it was. A changed file is in gap 1 when affected lists it as inert, and in gap 2 for a project below the root when the repo root is a project and the file is outside every project below it. Any other case fails the test.

## Not done

- The fixtures are small and made by hand. No real repo is cloned.
- Changes to `firescale.config.json`, to a lockfile's contents by kind, renames, and changes of file mode are not generated as such. A rename is a delete and an add.
- Only the key for one command is compared. Named inputs (ADR 0076), env files (ADR 0074) and tool versions are the same before and after.
- The fixtures have no git repo, so the snapshot walks the folder. Ignored files are not part of the test.
- The test calls `affected`. The CLI calls `affected_except` with the agent hook files and the config's own effect set apart (ADR 0058), which needs a git base; that path is not in the property.
- A unit the change made, or one whose key the change took away, is not compared.
- Ambiguous imports: only Python, where the first match won. JavaScript imports resolve by a path or a declared package and had no first-wins rule.
- The warning is printed by `fscale graph`. `fscale verify` does not print it.
- The opposite property (a unit that is affected has a key that differs) is not tested and is not meant to hold: affected may run more.

## From the fresh review

It found no narrowing in `limit_to_affected`. It found that the test's known gaps were too wide: any case with an inert file among its changes was excused, also when another change was the cause. Fixed: a case is a known gap only when the other changes, made alone, leave the key as it was. It found one sentence here that said too much ("nothing changes with no project at the root"); corrected. The rest is under "Not done".

## Later

- The same property over the 15 real repos of the nightly run.
- More kinds of change: a rename, a new project, a changed import.
- Shrinking a failing case to its smallest change.
- Build idea 6, item 1: a report-only rule for a cross-project import with no manifest dependency behind it. Item 3: string imports as tier-4 edges.

## Tests

- `crates/core/src/affected_covers_key.rs`, written first. Seen failing in WSL before any change: three of three fixtures failed. The cause in those three was mostly the generator changing files under `node_modules` and other folders that hold no source, and gap 1. With those set apart and the fourth fixture added, `every_unit_whose_key_differs_is_affected_below_a_root_project` failed with 39 cases; 11 of them (14 as the test counts now) were the repo root's own steps, which the fix closes.
- `the_roots_own_steps_run_when_a_project_below_the_root_changed`: the fix by itself.
- The two `known_gap_` tests.
- Ambiguous imports, in `crates/core/src/graph.rs`, written first: `a_python_import_that_two_folders_above_could_hold_is_an_edge_to_each_with_a_warning` and `a_python_import_that_a_folder_above_and_another_project_could_hold_is_an_edge_to_each` were seen failing in WSL (no edge to the second place). `a_python_import_with_one_place_or_in_the_importer_s_own_code_has_no_warning` passed before and after: it guards what must not change.
