# 0039 — Blame-aware verify: reruns and the stop hook

Status: accepted (2026-11-13, M21). Builds on ADR 0037 (the base run) and ADR 0038 (per-test labels).

## Context

M19 and M20 label failures; M21 makes the labels usable inside an agent's stop hook: a `new` failure may be a flaky test, and the stop hook must stay fast (on the course project a full verify took over ten minutes).

## Decision

### A new failure runs once more before it blocks

- After the labels, each step labelled `new` (failed, not timed out) runs again once on the head, as it ran the first time, without the step cache.
- If it passes, the step is **`flaky`** and does not block; its new tests are `flaky` too.
- With per-test results, only on positive evidence: the second run finished as a plain failure (not timed out, stopped or stopped early) with the runner's counts, passed at least as many more tests as there were new ones, failed only tests that already failed on the base, and what still fails is the base's own failure by the output evidence of ADR 0037. Then the step is `flaky`. Otherwise it stays `new`.
- **The check is never run again**: it is deterministic, and its result on the head is final.
- The second run shares the base run's time budget; with none left, a new failure blocks without one.
- **No flaky history is kept.** Nothing about a flaky result is written: not in `.firescale/`, not in the step cache (the second run's pass is never stored), not in the base store. The rerun is all there is (SPEC §8, §12).
- A step that timed out is not run again: a second timeout would double the wait.
- **A step whose files changed while verify ran is not run again**: when a file its cache key covers (ADR 0036; the whole repo for a step that is not cached) differs from when verify started, a first run that wrote a fixture or a generated file could make a second run pass for that reason, not by chance. It stays `new`, with a note.
- `flaky` does not block; a run whose only failures are `pre-existing` or `flaky` has the result `pre-existing` and exit code 0, and the headline names both.

### The stop hook

`fscale hooks run` uses blame-aware verify (it judges `ok`, so the labels apply) and runs **only what the change reaches**:

- The base is chosen first (ADR 0037). The changed files are those since the base commit, uncommitted ones included, so a commit the agent made on its branch counts as part of its change, and the check's command to run again names the same `--base`. **With no base, the hook runs everything**, as a full verify does: there is nothing to measure committed work against.
- Only the projects those files reach run, through M17's graph, shared-source nodes included; the repo root's own steps run when root files changed or a project they cover is affected (ADR 0032). The rest are `not affected`.
- The base run judges **failures only**: a passing step never runs on the base in the hook (no `fixed` labels there), so a hook with no failure makes no base run.
- The base run stays within `verify.baseBudget`; a failure it does not reach blocks as without a base.
- An agent is let go when every failure is `pre-existing` or `flaky`, and blocked on a `new` one (and on setup needed, as before).

### `cargo test` per-test labels

`cargo test`'s text output has been stable for years and needs no flag, so it is read like Vitest's and Jest's (ADR 0038). Cargo prints each test binary's `Running ... (target/debug/deps/<crate>-<hash>)` or `Doc-tests <crate>` line on stderr and its `running N tests` block and `test path ... ok` / `... FAILED` lines on stdout, so the k-th block is paired with the k-th binary. A test is `<crate>::<path>`, so the same name in two crates is two tests; a doc test's `(line N)` is left out, since lines move. `cargo nextest` is not read.

### A run that stopped at its first failure

`cargo test` stops after the first test binary that fails (without `--no-fail-fast`), as pytest does with `-x` or `--maxfail` and Jest or Vitest with `--bail`. Then an old failure early in the run hides any new failure after it, on the base and the head alike, and the outputs match. Such a failure is never `pre-existing` and never `flaky`: it blocks, with a note that says how to run it in full.

## After the review

An independent review before the pull request reproduced three ways an agent could stop with broken work: a new `check` violation was run again as `fscale check --affected` without the base, which reported nothing for a committed change, so it read as `flaky`; with no base, the hook looked only at uncommitted files, so committed work on a local `main` was never run; and the per-test `flaky` rule took a cut-short second run as evidence. It also found a first run that writes its own baseline, cargo's crate lines on stderr, doc tests' line numbers, fail-fast runs, and the base run's duration counting the reruns. All are fixed as described above, with tests (`stop-hook-blame.test.ts`, `pertest::tests`).

## Consequences

- A flaky test does not send an agent back; a test that fails twice does.
- The stop hook runs the affected projects and, only for their failures, the base: on the course-project shape, a change in one service runs one service's steps.
- Latency, cold and warm, is in `bench/RESULTS.md` (`bench/blame-bench.mjs`).
