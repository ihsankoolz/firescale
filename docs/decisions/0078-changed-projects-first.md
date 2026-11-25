# 0078 — With a known change set, the projects that hold a changed file start first

Status: accepted (2026-11-18, Track A of the run after 0.5.4; the owner's brief). Adds to ADR 0032 (`verify --affected`) and ADR 0023 (`--jobs`).

## Context

`docs/research/research-build-tools.md`, idea 5, first step. Units ran in folder order. A failure is most likely where the change is, so the unit that holds a changed file is the one to hear from first. In the course project the long suite ran wherever its folder name sorted.

## Decision

**With `--affected`, the projects that hold a changed file start first. The rest follow. Each group keeps the plan's order. Print order stays.** The repo root's own steps stay in front of every project, as before.

- A project holds a changed file when its node in the graph does: a detected project whose folder a changed file is in. A project the change only reaches through an edge (it imports a shared folder that changed) is not first. Nor is a folder named only under `verify.projects`, which has no node.
- **The repo root's own steps still start before any project.** Their key is made when they start and holds every file of the repo. A project that ran before them and wrote a file (a coverage report, a snapshot) would change that key.
- Only the order of starting changes. Every unit that ran before still runs. The summary, the report and `--json` list units in the plan's order, as they did.
- With `--jobs`, the workers take the units in the new order.
- When no unit is known to hold a changed file, the order is the plan's: without `--affected`, when everything is affected (a lockfile, a workflow, no commit to compare with), or when only a shared folder changed.

`Affected.direct` in `crates/core/src/graph.rs` holds the nodes with a changed file. `limit_to_affected` puts their units into `AffectedPlan.changed_first`. `run_verify_with` in `crates/core/src/verify.rs` starts units in that order and files each result under its place in the plan.

## Consequences

- The progress lines (`[verify] test in frontend: started`) come in the new order. Nothing else a person or a script reads changes.
- No pass and no affected set changes. A key is made from the files as they are when its step starts. So where one project's step writes a file that another project's key holds, the order can turn a hit into a miss, or a miss into a hit, on that run. That was already so with `--jobs`. `--budget` leaves out a step by its own kept time, so the order does not change what it leaves out.
- Units that share a database or a port may meet in a different order. That was already so with `--jobs`.

## Needs your decision

Nothing. Decided on your behalf: a unit reached only through an edge is not first (that is the "graph distance" of the later step); `--dry-run` does not show the start order.

## Not done

- No order among the first group beyond the plan's.
- A plain `fscale verify` with no `--affected` has no change set and keeps the plan's order, also when the working tree has changes.
- The base run of blame-aware verify keeps its own order, quickest first (ADR 0062).

## From the fresh review

It found no unit skipped, run twice or reported out of place. It found that the first form let a project start before the repo root's own steps, whose key holds every file of the repo; fixed, with a test seen failing first. It found two sentences here that said too much (no key changes; any project folder); corrected above.

## Later

- Then by graph distance from a changed file, then by kept time, quickest first.
- The start order in `--dry-run` and in `--json`.
- The same for a plain `fscale verify` with uncommitted changes.

## Tests

Written first and seen failing in WSL, three of three (the plan's order; no unit known to hold a file), in `crates/core/src/verify.rs`:

- `units_that_hold_a_changed_file_start_first_and_the_report_keeps_its_order`
- `with_jobs_the_changed_unit_is_among_the_first_started_and_the_report_keeps_its_order`
- `the_units_that_hold_a_changed_file_are_known_to_the_plan` (the course project's shape)
- After the review, seen failing first: `the_repo_root_s_own_steps_still_start_before_any_project`.

## Addendum: an expected time for a step with no kept time (build idea 14, first step)

Added 2026-11-25, Track A, item 11 (the owner's brief: this addendum, no ADR number of its own). Adds to ADR 0057 (`--budget`).

### Context

`docs/research/research-build-tools.md`, idea 14. Kept times live in `.firescale/step-times.json`, on one machine. A step with no kept time always runs under `--budget`, so a fresh clone or a fresh worktree gets no help from the budget on its first run: the course project's 1h 40m suite runs.

### Decision

**`verify.projects.<folder>.<step>.expected`: how long the step is expected to take. `--budget` uses it only when no time is kept for the step. A kept time always wins.**

- Written as the time limits are: `"1h 40m"`, `"90s"`, or a number of seconds. `false`, `0`, and any other value are config errors.
- Per project only. `verify.<step>.expected` is an error that names the per-project key; the repo root's own steps have none.
- A kept time wins, longer or shorter. Once the step passes in a run without `--budget`, its kept time is what `--budget` reads.
- **A file of kept times that could not be read is not "no kept time".** The expected time is not used then; the step runs and the run says so, as before (ADR 0057).
- Only `--budget` reads it: no slow-step line, no change to the base run's order (ADR 0062), no time limit. It is in no cache key (ADR 0058's table has its row: out).
- A step left out for its expected time is reported as any step `--budget` left out, with the setting named: `over the budget of 30m: it is expected to take 1h 40m (verify.projects["backend/ledger_service"].test.expected) and has no kept time`. The "Not run" paragraph ends with `Once it passes in a run of fscale verify without --budget, its kept time is used in place of the expected one.` The dry run lists it.
- **Verify's JSON is schema version 9.** Such a step has `expectedMs`, in `steps` and in `notRun`, and no `lastKnownMs` or `lastKnownFinished`: an expected time is not a time that was measured. In `notRun` an entry has either `lastKnownMs` and `lastKnownFinished`, or `expectedMs`. That makes two fields of `notRun` optional, which is the breaking part. Six places pinned version 8 and now say 9: `slow-steps.test.ts`, `python-env.test.ts`, `verify-run.test.ts`, `blame.test.ts`, the snapshot in `verify.test.ts.snap`, and `a_missing_module_is_setup_needed_with_the_install_command` in `verify.rs`. One more test pins the list of keys a project step may have, which now names `expected`: `names inputs among the keys of a project step` in `step-cache-inputs.test.ts`. Each is a deliberate change of the expected text, not a weaker test.

`BudgetTime`, `expected_key` in `crates/core/src/verify.rs`; `VerifyConfig::expected_for` and `project_expected`; `parse_expected` in `crates/core/src/config.rs`; `expected` in `VerifyProjectStepSetting`, `packages/sdk/src/types.ts`.

### Consequences

- A fresh clone with a committed hint leaves the slow step out of its first `--budget` run. The run is `PARTIAL`, never `OK`, and fails with `verify.failOnSkipped`, as for any step `--budget` leaves out. Without `--budget`, in CI and in the stop hook, nothing changes.
- A stale hint (the suite is now quick) leaves a step out until the step passes once in a run without `--budget`. It never makes a pass: the run is already `PARTIAL`.

### Not done

- The second half of idea 14: a note when a step's time limit is more than ten times its kept time.
- `fscale doctor`'s check of the stop hook's time limit (ADR 0083) reads kept times only, not expected ones.
- `fscale adopt` and `init` write no expected time.

### Tests

`packages/cli/src/duration-hint.test.ts`, written first and seen failing: 12 of 12 (the key was unknown, so every run stopped at the config with exit 2). The hint leaves a step out, names its key, and never reads OK; the same in JSON with `expectedMs` and no `lastKnownMs`, checked against the schema; the dry run lists it; a hint within the budget runs; without `--budget` nothing changes; a kept time within the budget wins over a long hint; a kept time over the budget wins over a short hint; after a pass the kept time is used; `false`, `0` and `"soon"` are refused; `verify.test.expected` is refused. A row in `configkey.rs`: a change of `expected` affects no step.
