# 0040 — Base run fixes after M21: per-project lockfiles, and `fixed` needs a real failure

Status: accepted (2026-11-13, after M21, for 0.3.0). Refines ADR 0037; does not reverse it.

## Context

Two gaps in the base run of ADR 0037, found by the owner after M21:

1. **A changed lockfile was judged for the whole repo, and only by name, and only when the worktree was needed.** Any changed lockfile or `requirements*.txt` anywhere made every step's base "not judged", even in a project whose own dependencies had not changed. And the base-result store's key held the head's lockfiles but not its requirements lists, so after a `requirements.txt` change a result kept from before could still be used.
2. **`fixed` was given on the base's word alone.** A step that passed on the head and failed on the base was `fixed`, also when it failed on the base for a reason of the worktree (a build output, a generated file or a local setting that git ignores and the worktree lacks), not of the code. M21's own notes record this on this repo: its `pnpm test` can read `[fixed]` for that reason.

## Decision

### Lockfiles, per project

- A **dependency list** is a lockfile (`facts::LOCKFILE_NAMES`), `requirements*.txt`, `constraints*.txt`, or a `.txt` file in a folder named `requirements` (`facts::is_dependency_list`).
- A step's base cannot use the head's linked `node_modules` or virtual environment when a dependency list **that the step's cache key covers** (ADR 0036: its project's folder, what it depends on in the graph, the files directly in the folders above, files no project owns; the whole repo for the root's steps and for a step the cache does not key) differs between the base and the head.
- Then, if `verify.baseInstall` is set, it runs as before (with it, nothing is linked, and the install is the base's). Otherwise **that step has no base**: its base status is `setup-needed`, its failure blocks, and a note says which file changed and names `verify.baseInstall`. Other projects are judged as usual, and a worktree is made only if some step still needs one.
- The check is the repo root's: any changed dependency list counts for it, since custom TypeScript rules may load installed packages.
- The base store's key now holds the head's requirements lists as well as its lockfiles, so a kept result is not reused after one changes.

Simplest reading of "run verify.baseInstall if it is set": `baseInstall` keeps running whenever it is set, as ADR 0037 has it, rather than only when a lockfile changed. Running it only then would mix linked and installed folders in one worktree, which ADR 0037 ruled out (an install could write through a link into the head).

### `fixed` needs evidence

A passing step whose base result is `failed` is labelled `fixed` only when that failure is a result of the base's code. The evidence is a **control run**: the step runs once in a worktree of the head, which is the `HEAD` commit with the working tree's changes copied in (`git diff --name-only HEAD` and untracked files git does not ignore; deletions applied; `.firescale/` left out; ignored files not copied), given its dependencies the same way as the base worktree (linked folders, or `verify.baseInstall`). That worktree differs from the base worktree only in the code.

| Head   | Base                                                      | Control run in a worktree of the head                 | Label        |
| ------ | --------------------------------------------------------- | ----------------------------------------------------- | ------------ |
| passed | failed                                                    | passed                                                | `fixed`      |
| passed | failed                                                    | failed, setup needed, timed out, over budget, stopped | none, a note |
| passed | failed, and no test passed on the base (per-test results) | not run                                               | none, a note |

- The control result is kept in the base store under `control-<hash of the step's own cache key and the worktree's set-up>`, so a second run with the same files does not repeat it. A step the cache does not key runs the control every time.
- The control run counts against `verify.baseBudget`. Base steps and control steps both get `FSCALE_BASE_RUN=1`.
- The stop hook judges only failures (ADR 0039): it now looks up no base result for a passing step, so it never labels `fixed` and never runs a control.
- Rejected: reading the base's output for paths the change touched (a heuristic that fails both ways), and per-test evidence alone (it does not cover lint, typecheck or runners without per-test results, and a missing build output fails single tests too).

## Consequences

- A lockfile change in one service no longer blocks the judging of the others.
- `fixed` may be missing where it is true (the step also fails in any worktree, for example this repo's `pnpm test` without its build), never shown where it is not.
- A full `fscale verify` with a candidate for `fixed` makes a second worktree and, with `verify.baseInstall`, a second install. This happens only when a failure needed the base worktree anyway, and the result is kept.
- Proof: `blame.test.ts` ("a project's changed lockfile", "fixed needs a real failure on the base"), `facts::tests::dependency_lists_are_lockfiles_and_requirements_lists`.
