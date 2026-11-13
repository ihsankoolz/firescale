# 0037 — Blame-aware verify: the base run

Status: accepted (2026-11-13, M19). Supersedes two points of ADR 0014: with a base, verify runs **every** step instead of stopping at the first failure, and `--base <ref>` works without `--affected`. Builds on ADR 0017 (labels on by default), ADR 0031 (the base run reuses the step cache) and ADR 0036 (the step cache).

## Context

SPEC §3.10: a person or an agent is held to the failures their change introduced. Until now any failure blocked, including one that already failed on the default branch. M19 compares each failure with the base commit.

## Decision

### Head and base

- The **head** is the working tree, uncommitted changes included: that is what an agent's stop hook judges.
- The **base** is the merge base of `HEAD` and `--base <ref>`, or of the default branch when `--base` is not given. The default branch is `origin/HEAD`, else the first of `origin/main`, `origin/master`, `main`, `master` that exists.
- **On the default branch itself, the base is its upstream** (`origin/main`), never the branch: a commit an agent made there in this session would otherwise become the base and hide its own failures. With no upstream there is no base, unless `--base` names one.
- There is **no base**, and verify behaves as before (every failure blocks), outside git, before the first commit, with no default branch, on a default branch with no upstream, and when the merge base is `HEAD` and the tree is clean ("nothing to compare"). firescale's own `.firescale/` folder never counts as a change; it now ignores itself (`.firescale/.gitignore`).
- A `--base` that git cannot compare with `HEAD` is a usage error (exit 2). `--base` works with and without `--affected`; with `--affected` it also sets what counts as changed, as before.

### Every step runs

With a base, a failure no longer stops the rest of its project, and a failed `check` no longer stops the other steps. Otherwise an old lint failure would hide a new test failure. Without a base, nothing changes.

### Where a step's base result comes from, cheapest first

1. **The same inputs.** When the step's full cache key can be made (every refusal of ADR 0036 holds: a service the repo does not start, a dynamic import, `paths` aliases, history-based commands, unreadable files, tool versions) and no file the key holds (ADR 0036: its project, what it depends on in the graph, the folders above, the files no project owns) differs between the base commit and the working tree (`git diff <base>` plus untracked files), its key is the same on both, so its head result is its base result. No base run. This is the M18 cache at work: "a project whose key is the same on the base and the head needs no base run". A step that the cache does not key never counts as having the same inputs. With `--no-cache`, neither this shortcut nor the store below is used.
2. **The base-result store**, `.firescale/base/<key>.json`, from an earlier run with the same base. Its key is the step's cache key with the base commit's files (git's blob ids for the same scope) in place of the working tree's, and the head's lockfiles, because the base run uses the head's installed dependencies. Only `passed` and `failed` are kept; a result older than 30 days is not used. The same base and the same lockfiles give no base run; a changed lockfile runs it again.
3. **A run in a worktree** of the base commit: `git worktree add --detach` in the temp folder, with the repo's own hooks off and Git LFS downloads skipped, removed afterwards (links first, then `git worktree remove --force`, then `prune`); a worktree a killed run left behind is removed by a later run after six hours. Only git's list of worktrees changes in the user's repo; the working tree and the index never do. Every git command firescale runs sets `GIT_OPTIONAL_LOCKS=0`, so even `git status` never rewrites `.git/index`. A step whose folder does not exist on the base, or that cannot be started there, is not judged.

Failing head steps run first, then passing ones (to find `fixed`), until the budget ends. **Passing steps run on the base only when a worktree is needed anyway** for a failure (or a failed check): a run with no failure costs no base run, and a passing step is labelled `fixed` only from a base result it already has (the same inputs never give `fixed`; a kept result can, for example after an earlier run in which the step failed on the head).

### The worktree's dependencies

**The head's dependency folders are linked in**, not installed: every ignored `node_modules`, `.venv` and `venv` of the head, as a symbolic link (Unix) or a junction (Windows, which needs no special rights). **A folder that holds a link into the repo is never linked**: a workspace package in `node_modules` (`node_modules/@scope/lib -> ../../packages/lib`) or an editable install in a virtual environment (a `.pth` or `.egg-link` naming a folder of the repo) would make the base run the head's code. A link that cannot be made means the base is not judged. PLAN.md said "run the repo's install command there"; this ADR chooses linking by default, because:

- an install needs the network and minutes, inside an agent's stop hook;
- a Python project with no virtual environment installs into the shared interpreter (ADR 0022), and installing the base's versions there would change the head's environment;
- the step cache already keys a result on what is installed.

The cost: the base runs with the head's dependency versions, and a workspace package linked inside `node_modules` points at the head's copy. **When a lockfile or a `requirements*.txt` differs between the base and the head**, the head's dependencies do not stand for the base's, so without `verify.baseInstall` the base results that would need a run say setup needed and the failures block as without a base (a dependency upgrade that breaks a test must not read as `pre-existing`). `verify.baseInstall` names a command to run at the worktree's root first (for example `pnpm install --frozen-lockfile --offline`, which fills `node_modules` from pnpm's store with no network); **with it, nothing is linked**, so an install can never write through a link into the head's folders. If it fails, the base results say setup needed and the failures block as without a base.

**pnpm refuses a linked `node_modules`** for recursive runs (`pnpm -r`, `ERR_PNPM_UNSAFE_TASK_RUN_STATE_PATH`). A base step that fails with that error is setup needed, not a failure, so a head failure is never `pre-existing` because of it; the note names `verify.baseInstall`. A pnpm workspace needs `verify.baseInstall` for its recursive steps to be judged. This repo sets `"baseInstall": "pnpm install --frozen-lockfile --offline && pnpm build"` (about 25 s), since its Vitest suite also runs the built binary.

Each base step runs with `FSCALE_BASE_RUN=1`, so a repo (or a test's stand-in) can tell.

### The time budget

`verify.baseBudget` (default `5m`, `false` for none) caps the whole base run: worktree, install and steps. A base step gets its own time limit (ADR 0024) or what is left of the budget, whichever is shorter. When the budget runs out, the steps left (and one stopped by it) are `over-budget`. The stop hook never waits beyond it.

### Labels, by evidence

A failure lets an agent stop only with evidence that it is old. **A failure is `pre-existing` when it fails on the base too and its output on the head is contained in the base's**: each non-empty line of the head's output, normalised (the run's root folder written as `<root>`, digits folded to `#` so counts, times and line numbers do not matter, spaces squeezed), appears in the base's output at least as often. Only short hashes of those lines are kept (in memory, and in the base store), never the text.

So a base that fails another way, a missing module, a broken environment, an untracked build output the worktree lacks, never hides a head failure, and neither does one old failure in a step where the change added another (an extra `FAIL add` line). This is a step-level measure; M20 compares failing tests by name for pytest, Vitest and Jest.

| Head                | Base                                                        | Label          | Blocks            |
| ------------------- | ----------------------------------------------------------- | -------------- | ----------------- |
| failed              | passed                                                      | `new`          | yes               |
| failed              | failed, and the head's output is within the base's          | `pre-existing` | no                |
| failed              | failed another way                                          | none, a note   | yes               |
| timed out           | anything but passed                                         | none, a note   | yes               |
| timed out           | passed                                                      | `new`          | yes               |
| passed              | failed                                                      | `fixed`        | —                 |
| failed or timed out | setup needed, timed out, over budget, stopped, or no result | none, a note   | yes               |
| setup needed        | anything                                                    | none           | as before (setup) |

- **A step that needs setup on the head is never labelled.** It is not a failure in the code, and it blocks with the setup message as before (ADR 0022).
- **When the base result says nothing about the code**, the step gets no label and a note says why; the failure blocks, as it would without a base.
- **The check:** when it fails on the head, it runs on the base worktree too, and each enforced violation is compared by rule, file and message (not the line, so a violation that moved is the same one), counting repeats. If every one is on the base too, as often, the check is `pre-existing`; otherwise `new`, and the violation lines say `[new]` or `[pre-existing]`.
- `flaky` is reserved for M21.
- Ctrl+C during the base run makes the run `stopped`, as before.

### Result, exit code and output

- Only `new` failures and unlabelled ones block. A run whose only failures are `pre-existing` has the result **`pre-existing`**, `ok: true`, exit code 0, and the headline `verify: OK - no new failures; pre-existing (fails on the base commit abc1234 too): test`. It never reads as a plain `OK`.
- A failing headline names new failures with `(new)` and lists pre-existing ones apart: `verify: FAILED at test (new); pre-existing (fails on the base commit abc1234 too, does not block): lint`.
- Each labelled row says `[new]`, `[pre-existing]` or `[fixed]`; a new failure gets a line saying it passes on the base, a pre-existing one a line saying it does not block (and no output tail); a line lists what the change fixed and how the base results were found.
- **Verify schema 6:** a step's `label` and `base` (`status`, `source`: `same-inputs`, `cache` or `ran`, and a `note`), the result `pre-existing`, and the report's `base` (`compared`, `reference`, `commit`, `reason`, counts, `budgetMs`, `durationMs`, `overBudget`).

### Not in M19

Per-test labels (M20); reruns, `flaky` and the stop hook's narrower run (M21). The stop hook already uses the labels, because it judges `ok`.

## After the review

An independent review before the pull request reproduced eight ways a failure the change caused would stop blocking: a step-level label that let one old failure hide a new one in the same step; a linked `node_modules` with workspace links, which made the base run the head's library; base environments that differ (ignored build outputs, a nested root); a project new on the head, whose base run errored and made the stop hook let the agent go; the same-inputs shortcut skipping the cache key's refusals; a commit on a local `main` becoming the base; firescale's own `.firescale/` making a clean tree look dirty; and `baseInstall` writing through links into the head. It also found `git status` rewriting `.git/index`, leaked worktrees, Ctrl+C reported as `failed`, and parts of the base run outside the budget. All are fixed as described above, with tests (`blame.test.ts`, `blame::tests`).

## Consequences

- An agent is no longer sent back for a failure it did not cause.
- The first run on a new base can take up to the budget longer; later runs with the same base and the same lockfiles reuse the stored results, and unchanged projects never run on the base at all.
- A base run uses the head's dependencies. When they changed (a lockfile or requirements list), the base is not judged without `verify.baseInstall`, and failures block as before.
