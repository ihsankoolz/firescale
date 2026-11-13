# 0037 — Blame-aware verify: the base run

Status: accepted (2026-11-13, M19). Supersedes two points of ADR 0014: with a base, verify runs **every** step instead of stopping at the first failure, and `--base <ref>` works without `--affected`. Builds on ADR 0017 (labels on by default), ADR 0031 (the base run reuses the step cache) and ADR 0036 (the step cache).

## Context

SPEC §3.10: a person or an agent is held to the failures their change introduced. Until now any failure blocked, including one that already failed on the default branch. M19 compares each failure with the base commit.

## Decision

### Head and base

- The **head** is the working tree, uncommitted changes included: that is what an agent's stop hook judges.
- The **base** is the merge base of `HEAD` and `--base <ref>`, or of the default branch when `--base` is not given. The default branch is `origin/HEAD`, else the first of `origin/main`, `origin/master`, `main`, `master` that exists.
- There is **no base**, and verify behaves as before (every failure blocks), outside git, before the first commit, with no default branch, and when the merge base is `HEAD` and the tree is clean ("nothing to compare"). On the default branch with uncommitted changes, the base is `HEAD`.
- A `--base` that git cannot compare with `HEAD` is a usage error (exit 2). `--base` works with and without `--affected`; with `--affected` it also sets what counts as changed, as before.

### Every step runs

With a base, a failure no longer stops the rest of its project, and a failed `check` no longer stops the other steps. Otherwise an old lint failure would hide a new test failure. Without a base, nothing changes.

### Where a step's base result comes from, cheapest first

1. **The same inputs.** When no file the step's cache key holds (ADR 0036: its project, what it depends on in the graph, the folders above, the files no project owns) differs between the base commit and the working tree (`git diff <base>` plus untracked files), its key is the same on both, so its head result is its base result. No base run. This is the M18 cache at work: "a project whose key is the same on the base and the head needs no base run". A step that the cache does not key (a service the repo does not start, a dynamic import, `paths` aliases) never counts as having the same inputs.
2. **The base-result store**, `.firescale/base/<key>.json`, from an earlier run with the same base. Its key is the step's cache key with the base commit's files (git's blob ids for the same scope) in place of the working tree's, and the head's lockfiles, because the base run uses the head's installed dependencies. Only `passed` and `failed` are kept; a result older than 30 days is not used. The same base and the same lockfiles give no base run; a changed lockfile runs it again.
3. **A run in a worktree** of the base commit: `git worktree add --detach` in the temp folder, removed afterwards (`git worktree remove --force`, then `prune`). Only git's list of worktrees changes in the user's repo; the working tree and the index never do.

Failing head steps run first, then passing ones (to find `fixed`), until the budget ends.

### The worktree's dependencies

**The head's dependency folders are linked in**, not installed: `node_modules`, `.venv` and `venv` of the repo root and of each project that runs, as a symbolic link (Unix) or a junction (Windows, which needs no special rights). PLAN.md said "run the repo's install command there"; this ADR chooses linking by default, because:

- an install needs the network and minutes, inside an agent's stop hook;
- a Python project with no virtual environment installs into the shared interpreter (ADR 0022), and installing the base's versions there would change the head's environment;
- the step cache already keys a result on what is installed.

The cost: the base runs with the head's dependency versions, and a workspace package linked inside `node_modules` points at the head's copy. **When a lockfile or a `requirements*.txt` differs between the base and the head**, the head's dependencies do not stand for the base's, so without `verify.baseInstall` the base results that would need a run say setup needed and the failures block as without a base (a dependency upgrade that breaks a test must not read as `pre-existing`). `verify.baseInstall` names a command to run at the worktree's root first (for example `pnpm install --frozen-lockfile --prefer-offline`); if it fails, the base results say setup needed and the failures block as without a base.

Each base step runs with `FSCALE_BASE_RUN=1`, so a repo (or a test's stand-in) can tell.

### The time budget

`verify.baseBudget` (default `5m`, `false` for none) caps the whole base run: worktree, install and steps. A base step gets its own time limit (ADR 0024) or what is left of the budget, whichever is shorter. When the budget runs out, the steps left (and one stopped by it) are `over-budget`. The stop hook never waits beyond it.

### Labels

| Head                | Base                                                        | Label             | Blocks            |
| ------------------- | ----------------------------------------------------------- | ----------------- | ----------------- |
| failed or timed out | passed                                                      | `new`             | yes               |
| failed or timed out | failed                                                      | `pre-existing`    | no                |
| passed              | failed                                                      | `fixed`           | —                 |
| failed or timed out | setup needed, timed out, over budget, stopped, or no result | none, with a note | yes               |
| setup needed        | anything                                                    | none              | as before (setup) |

- **A step that needs setup on the head is never labelled.** It is not a failure in the code, and it blocks with the setup message as before (ADR 0022).
- **When the base result says nothing about the code** (it needed setup, timed out, ran over the budget, or was stopped), the step gets no label and a note says why; the failure blocks, as it would without a base.
- **The check:** when it fails on the head, it runs on the base worktree too, and each enforced violation is compared by rule, file and message (not the line, so a violation that moved is the same one). If every one is on the base too, the check is `pre-existing`; otherwise `new`, and the violation lines say `[new]` or `[pre-existing]`.
- `flaky` is reserved for M21.

### Result, exit code and output

- Only `new` failures and unlabelled ones block. A run whose only failures are `pre-existing` has the result **`pre-existing`**, `ok: true`, exit code 0, and the headline `verify: OK - no new failures; pre-existing (fails on the base commit abc1234 too): test`. It never reads as a plain `OK`.
- A failing headline names new failures with `(new)` and lists pre-existing ones apart: `verify: FAILED at test (new); pre-existing (fails on the base commit abc1234 too, does not block): lint`.
- Each labelled row says `[new]`, `[pre-existing]` or `[fixed]`; a new failure gets a line saying it passes on the base, a pre-existing one a line saying it does not block (and no output tail); a line lists what the change fixed and how the base results were found.
- **Verify schema 6:** a step's `label` and `base` (`status`, `source`: `same-inputs`, `cache` or `ran`, and a `note`), the result `pre-existing`, and the report's `base` (`compared`, `reference`, `commit`, `reason`, counts, `budgetMs`, `durationMs`, `overBudget`).

### Not in M19

Per-test labels (M20); reruns, `flaky` and the stop hook's narrower run (M21). The stop hook already uses the labels, because it judges `ok`.

## Consequences

- An agent is no longer sent back for a failure it did not cause.
- The first run on a new base can take up to the budget longer; later runs with the same base and the same lockfiles reuse the stored results, and unchanged projects never run on the base at all.
- A base run uses the head's dependencies. When they changed (a lockfile or requirements list), the base is not judged without `verify.baseInstall`, and failures block as before.
