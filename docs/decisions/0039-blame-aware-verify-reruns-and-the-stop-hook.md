# 0039 — Blame-aware verify: reruns and the stop hook

Status: accepted (2026-11-13, M21). Builds on ADR 0037 (the base run) and ADR 0038 (per-test labels).

## Context

M19 and M20 label failures; M21 makes the labels usable inside an agent's stop hook: a `new` failure may be a flaky test, and the stop hook must stay fast (on the course project a full verify took over ten minutes).

## Decision

### A new failure runs once more before it blocks

- After the labels, each step labelled `new` (failed, not timed out) runs again once on the head, as it ran the first time, without the step cache.
- If it passes, the step is **`flaky`** and does not block; its new tests are `flaky` too.
- With per-test results: if every new test passes the second time and what still fails is the base's own failure (the output evidence of ADR 0037 holds for the second run's output), the step is `flaky`. Otherwise it stays `new`.
- **No flaky history is kept.** Nothing about a flaky result is written: not in `.firescale/`, not in the step cache (the second run's pass is never stored), not in the base store. The rerun is all there is (SPEC §8, §12).
- A step that timed out is not run again: a second timeout would double the wait.
- **A step whose files changed while verify ran is not run again**: when a file its cache key covers (ADR 0036; the whole repo for a step that is not cached) differs from when verify started, a first run that wrote a fixture or a generated file could make a second run pass for that reason, not by chance. It stays `new`, with a note.
- `flaky` does not block; a run whose only failures are `pre-existing` or `flaky` has the result `pre-existing` and exit code 0, and the headline names both.

### The stop hook

`fscale hooks run` uses blame-aware verify (it judges `ok`, so the labels apply) and runs **only what the change reaches**:

- The base is chosen first (ADR 0037). The changed files are those since the base commit, uncommitted ones included, so a commit the agent made on its branch counts as part of its change. With no base, they are the uncommitted ones (as `--affected` alone).
- Only the projects those files reach run, through M17's graph, shared-source nodes included; the repo root's own steps run when root files changed or a project they cover is affected (ADR 0032). The rest are `not affected`.
- The base run judges **failures only**: a passing step never runs on the base in the hook (no `fixed` labels there), so a hook with no failure makes no base run.
- The base run stays within `verify.baseBudget`; a failure it does not reach blocks as without a base.
- An agent is let go when every failure is `pre-existing` or `flaky`, and blocked on a `new` one (and on setup needed, as before).

### `cargo test` per-test labels

`cargo test`'s text output (`test path ... ok` / `... FAILED` under each `Running ... (target/debug/deps/<crate>-<hash>)` or `Doc-tests <crate>` line) has been stable for years and needs no flag, so it is read like Vitest's and Jest's (ADR 0038). A test is `<crate>::<path>`, so the same name in two crates is two tests. `cargo nextest` is not read.

## Consequences

- A flaky test does not send an agent back; a test that fails twice does.
- The stop hook runs the affected projects and, only for their failures, the base: on the course-project shape, a change in one service runs one service's steps.
- Latency, cold and warm, is in `bench/RESULTS.md` (`bench/blame-bench.mjs`).
