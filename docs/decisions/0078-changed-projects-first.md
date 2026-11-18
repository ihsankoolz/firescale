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
