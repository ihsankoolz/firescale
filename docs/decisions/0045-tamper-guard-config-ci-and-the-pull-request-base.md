# 0045 — The tamper guard: config and CI detectors, and the base GitHub gives

Status: accepted (2026-11-14, M25). Extends ADR 0042 and ADR 0044. **Changes ADR 0041 on one point**: the generated workflow now uses `pull_request_target`, for one job that runs no code of the pull request (below, "Why `pull_request_target` here").

## Context

M25 asks for the detectors SPEC §8 lists beside tests and suppressions: lowered coverage thresholds, CI steps removed or made unable to fail, `--no-verify`, and (left over from M24) rules turned off in linter config. The owner added two things:

- In CI the base must come from the pull request's base as GitHub gives it, not from local git state, so a change cannot move what it is compared with; the generated workflow of M22 runs the guard that way. The ADR must say what is and is not protected locally, where an agent can change git state.
- A deleted or weakened verify step in the workflow itself is a finding, read from the base's copy of the workflow.

## What GitHub's docs say (read 2026-11-14)

From <https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows>:

- `pull_request`: `GITHUB_SHA` is the "Last merge commit on the `GITHUB_REF` branch", and `GITHUB_REF` the "PR merge branch `refs/pull/PULL_REQUEST_NUMBER/merge`". The workflow that runs is the one in that merge commit, so **a pull request can edit or delete the workflow that judges it**.
- `pull_request_target`: `GITHUB_SHA` is the "Last commit on default branch". "This event runs in the context of the default branch of the base repository", so the workflow that runs is the default branch's copy. "Avoid using this event if you need to build or run code from the pull request"; running untrusted code there can lead to "cache poisoning and granting unintended access to write privileges or secrets".
- The webhook docs (<https://docs.github.com/en/webhooks/webhook-events-and-payloads#pull_request>) say nothing about when `pull_request.base.sha` is taken, so firescale does not rely on it being the base branch's tip.
- `cache-mode` (<https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax>) takes `read`, `write`, `write-only` and `none`; `none` neither restores nor saves.
- "Pull request authors cannot approve their own pull requests" (<https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/reviewing-changes-in-pull-requests/approving-a-pull-request-with-required-reviews>); M26 uses that.
- The rulesets page lists no rule that requires a workflow a pull request cannot change; firescale does not rely on one.

## Decision

### One model for every config detector

Each detector turns a file it knows into **measures**: a named value where higher is stricter (a coverage floor of 80 % is `8000`, an ESLint rule at `error` is `2`, a CI check that can fail counts `1`, each `--no-verify` counts `-1`). The guard collects them from every changed file, on the base and in the working tree, and compares them **per key over the whole change**, as it compares tests (`crates/core/src/tamper/config.rs`). A key whose value is lower now than on the base is a finding, named where it now is (with "on the base: …, file:line") or, when it is gone, where it was on the base. A setting moved between files (an `.eslintrc.json` migrated to `eslint.config.js`, `fail_under` moved from `setup.cfg` to `pyproject.toml`, a CI step moved to another job) keeps its key and is not a finding.

Two ways of combining a key's values from several places: **max** (thresholds, rule levels, switches: the strictest place wins, and a key in no file takes the tool's default) and **sum** (CI checks that can fail, ignored codes, hook skips: every place counts).

**Precedence.** When a config file of coverage.py, Ruff or ESLint changes, the other files that tool may read in the same folder are read too, and only the one the tool uses counts: coverage.py reads `.coveragerc`, else the first of `setup.cfg`, `tox.ini`, `pyproject.toml` with its settings; Ruff the first of `.ruff.toml`, `ruff.toml`, `pyproject.toml`; ESLint 9 a flat `eslint.config.*` before any `.eslintrc*` or `package.json` `eslintConfig`. So adding a `.coveragerc` that hides `fail_under` in `pyproject.toml`, or an empty `eslint.config.js` next to `.eslintrc.json`, is a finding.

Config files are read by small, tolerant readers that flatten JSON (with comments and trailing commas), YAML (block style, flow lists and maps, block scalars), TOML, INI and JS/TS object literals (tree-sitter) into key paths with line numbers (`tamper/formats.rs`). No new dependency. A file a detector reads but cannot parse is a `not-checked` finding, as an unreadable test file is.

### The detectors

| Kind                               | Reads                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       | A finding when                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| ---------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `coverage-lowered` (`coverage.rs`) | Jest `coverageThreshold` (`jest.config.*`, `package.json`); Vitest `coverage.thresholds`; nyc and c8 rc files and `package.json`; coverage.py `fail_under` (`.coveragerc`, `setup.cfg`, `tox.ini`, `pyproject.toml`, one key); pytest-cov `--cov-fail-under` in `addopts`; `tarpaulin.toml`; Codecov `target`, `threshold`, `informational`; flags on command lines in scripts, workflows, Makefiles and tox (`--cov-fail-under`, `coverage report --fail-under`, `cargo llvm-cov --fail-under-*`, `cargo tarpaulin --fail-under`, nyc/c8 and Vitest flags) | a floor is lower or gone, `check-coverage` turned off, a status made informational                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| `ci-weakened` (`ci.rs`)            | GitHub Actions workflows: every `run:` step, command by command                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             | a check (tests, lint, type checks, `fscale verify`, `fscale tamper`; `check_of` lists them, package scripts by name whichever manager runs them) that could fail on pull requests or pushes no longer runs, or can no longer fail: `continue-on-error` (other than a `matrix.` expression), an `if:` that is literally false, `\|\| true`, `\|\| :`, `\|\| exit 0`, `\|\| echo …`, `set +e` before it, `&` (background), `&&` on a line that is not the script's last (bash `-e` does not stop there), a pipe without `pipefail`; or the workflow stops running on every pull request (`pull_request` gone, `types` without updates, a `branches`, `paths` or `*-ignore` filter added) |
| `no-verify` (`noverify.rs`)        | `package.json`, shell, PowerShell and batch scripts, Makefiles, justfiles, Taskfiles, Husky and lefthook files, workflows, `.gitlab-ci.yml`, and the agent hook files (`.claude/settings*.json`, `.codex/hooks.json`, `.cursor/hooks.json`)                                                                                                                                                                                                                                                                                                                 | more `--no-verify` on git commands, also shortened (`--no-veri…`; not `cargo publish --no-verify`, not inside a commit message), `git commit -n` (also `-anm`, or after the message), `git -c core.hooksPath=…` on a command, `core.hooksPath` pointed at `/dev/null`, Claude Code's `"disableAllHooks": true`; or fewer agent hooks that run `fscale hooks run` or `fscale verify` (the stop hook removed, or replaced by one that only names fscale)                                                                                                                                                                                                                                 |
| `rule-disabled` (`lintconfig.rs`)  | ESLint (all config forms, flat-config blocks and `overrides` as scopes), Biome, Ruff, Flake8, Pylint, mypy, Pyright, Cargo `[lints]` and `[workspace.lints]`, `tsconfig*.json`                                                                                                                                                                                                                                                                                                                                                                              | a rule turned off or lowered (`error → warn`, a new `off` for some files, a removed `error`), a code ignored or unselected, `strict` or a strict-family check turned off, a type-check mode lowered                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |

Each kind has caught and not-a-finding cases in `bench/tamper-corpus.json` (prefixes `cov:`, `ci:`, `nv:`, `lint:`); `tamper.test.ts` checks every prefix has both. A new `warn` is not a finding (it may turn a rule on); `ignore` patterns that leave whole files out of linting are not read.

**The CI check is read from both copies of the workflow**: the base commit's (from git) and the working tree's or head commit's. So a pull request that deletes the verify step, adds `continue-on-error` to it or drops `pull_request` from `on:` is a finding against the base's copy, wherever the guard runs.

### The base of a pull request, from GitHub

`fscale verify --pr-base` and `fscale tamper --pr-base` take the base from the run's event (`GITHUB_EVENT_PATH`, `GITHUB_SHA`), never from a ref in the clone (`tamper/github.rs`):

- **`pull_request`** (the merge commit is checked out): the base is the merge commit's **first parent**, read from the commit object, provided its second parent is the event's `pull_request.head.sha`. The event's `base.sha` must be in that history. If the checkout is the head itself, the base is `base.sha`.
- **`pull_request_target`**: the base branch's tip is `GITHUB_SHA` when the event's `base.sha` is in its history (a pull request into the default branch), else `base.sha`. `fscale tamper --pr-base` compares that tip with the tree a merge of the pull request's head into it gives (`git merge-tree --write-tree`, git objects only), and reads the policy from the tip: what merging would change on the base branch as it is now, whatever older commit the change branched from.
- Anything missing (not GitHub Actions, another event, a commit not in the clone, a shallow history) is an **error**, never a silent "no base": in CI, a guard that judged nothing must not read as a pass.
- Every git command firescale runs sets `GIT_NO_REPLACE_OBJECTS=1`, so a `git replace` made by the change's code cannot make the base commit read as another.

The generated workflow's `pull-request` job now runs `fscale verify --affected --pr-base` (it was `--base "origin/$BASE_REF"`, a ref that the change's install scripts could move before verify started).

### `fscale tamper`

The guard alone, with no step run: `fscale tamper [--base <ref>] [--head <commit>] [--pr-base] [--json]`. With `--head` or `--pr-base` the head is **read from git objects** (`Head::Commit`): nothing is checked out, written or run, so a commit can be judged without trusting it. Exit 1 when the base's policy is `enforced` and there are findings. In GitHub Actions each finding is also printed as an annotation (`::warning` or `::error`), every value escaped so text from the change cannot add a workflow command. M26 runs it over real history.

### The tamper-guard workflow, which a pull request cannot change

`core/ci-exists` now writes a second file, `.github/workflows/tamper-guard.yml`, beside `ci.yml` (and `fscale sync` adds it to a repo whose `ci.yml` is firescale's and unedited). It is started only by `pull_request_target`:

```yaml
on:
  pull_request_target:
jobs:
  tamper-guard:
    runs-on: ubuntu-latest
    cache-mode: none
    permissions:
      contents: read
    steps:
      - uses: actions/checkout@… # the base branch
        with: { fetch-depth: 0 }
      - run: git fetch --no-tags origin "$HEAD_SHA" # objects only
      - uses: actions/setup-node@…
      - run: npx --yes firescale@<the version that wrote the file> tamper --pr-base
```

GitHub takes it from the default branch's copy, so a pull request can neither edit nor delete it. It is a file of its own because GitHub reports a skipped job as a success, even for a required check: a `tamper-guard` job skipped by an `if:` in the `pull_request` run of a shared file would pass the check without running. It checks out the base, fetches the pull request's commits as objects, and runs a firescale pinned in the base's file. It never checks out, installs or runs the pull request's files, has no cache (`cache-mode: none`) and a read-only token. Its policy comes from the base commit like every other run's.

#### Why `pull_request_target` here (changing ADR 0041)

ADR 0041 kept `pull_request_target` out because a job that builds a pull request's code with the base's privileges is the classic way to poison a cache or leak a token. This job builds nothing: the pull request's files are data, read from git objects by a program the base chose. That is the use GitHub's docs leave open ("Avoid using this event if you need to build or run code from the pull request"). The other two jobs are unchanged, and a test now checks the opposite of the old one: `pull_request_target` starts only `tamper-guard`, which has `cache-mode: none`, `contents: read`, no cache step, no `ref:` on its checkout, and runs exactly `git fetch` and `npx --yes firescale@<version> tamper --pr-base` (`shared-cache.test.ts`, `core::ci_exists::tests`). The concurrency group now includes the event name, so this run and the `pull_request` run of the same pull request do not cancel each other.

To make it a gate, the owner marks the `tamper-guard` check as required in branch protection and sets `verify.tamper` to `"enforced"` on the default branch. A pull request can still add a job of the same name to a workflow of its own; whoever relies on the check should look at the `Tamper guard` workflow's run.

### What is protected locally, and what is not

Locally the guard runs inside verify and the stop hook, in a clone the agent can write. It is a tripwire there, not a boundary:

| What an agent with a shell can do                                                                                                                              | Locally                                                                                                                                    |
| -------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| Edit `verify.tamper` (policy, `allow`, `testPaths`) in the change                                                                                              | **protected**: read from the base commit (ADR 0042, 0044)                                                                                  |
| `git replace` the base commit with another                                                                                                                     | **protected**: `GIT_NO_REPLACE_OBJECTS=1` on every git command                                                                             |
| Hide a change from `git diff` with `git update-index --skip-worktree` or `--assume-unchanged`                                                                  | **protected**: such files are compared anyway (outside a sparse checkout), and they count as a change when choosing the base               |
| Delete, skip or weaken a CI step, a coverage floor, a lint rule, add `--no-verify`, remove the stop hook in agent settings                                     | **reported**, against the base's copies                                                                                                    |
| Move the base ref (`git update-ref refs/remotes/origin/main HEAD`), reset or commit on a local default branch with no upstream, rewrite history under the base | **not protected**: the base is resolved from local refs, so the comparison moves with them, or there is no base and the guard does not run |
| Remove the stop hook itself, or call the agent's tools without it                                                                                              | **not protected** once removed: the removal is a finding only on a later run                                                               |
| Replace the firescale binary in `node_modules`, or edit objects in `.git` by hand                                                                              | **not protected**                                                                                                                          |

So locally the guard catches a weakening an agent makes in the files of its change, and says so; it cannot stop an agent that sets out to change what it is compared with. **CI is where it is enforced**: the `tamper-guard` job's base comes from GitHub's event, its definition from the default branch, and nothing of the pull request runs before or during it. The `pull-request` job's verify also uses `--pr-base`, which removes the moved-ref trick there, but that job runs the change's install scripts and tests first, so it is no stronger than the change it judges.

## After the review

An independent review of the diff before the pull request reproduced these, all fixed with tests and corpus cases:

- **The CI guard compared with the merge base** and read the policy there, so a pull request branched from an old commit (before a job gained tests and before `enforced`) was judged by the old file and the old policy. It now compares the base tip with the merged tree and reads the policy from the tip.
- **A skipped job counts as a passed required check** (GitHub's docs): the guard had to leave `ci.yml` for a file only `pull_request_target` starts.
- **Ways to stop checks without a finding**: trigger filters and `types`, `&`, `&&` on a non-final line; the stop hook replaced by `echo fscale`, `disableAllHooks`, `-n` after the message, `--no-verif`, `git -c core.hooksPath=…`; a new config file of higher precedence.
- **False positives**: npm to pnpm, `continue-on-error: ${{ matrix.experimental }}`, `; exit 0` (bash `-e` stops first), `--no-verify` in a commit message, and a renamed workflow with a push branch filter.
- **Crashes and slowness**: unbounded nesting in the YAML flow and TOML readers overflowed the stack; a long TOML or YAML array took quadratic time. Both readers now stop at a depth of 100 and track brackets as lines are added.
- **Real history** (a smoke run before the review): a workflow whose list sat on the line after its key could not be read.

## Known limits

- **Net counts** still miss a swap within a key: one CI check removed while another of the same tool is added elsewhere; one ignored code swapped for another.
- **CI**: only GitHub Actions `run:` steps are read. Checks run by an action (`uses: golangci/golangci-lint-action`), a reusable workflow called from another (`workflow_call` counts as its own event class; removing the call is not seen), GitLab, CircleCI and other CI systems are not. `if:` expressions that are false but not literally (`if: github.event_name == 'never'`), `shell:` overrides and matrix `fail-fast` are not worked out. A check moved from a workflow into a script that the workflow runs reads as removed.
- **Coverage**: values that are not literals (a variable) are skipped; `target: auto` in Codecov is not a number; a floor moved from a config file to a command-line flag has a different key and reads as removed.
- **Lint config**: Ruff's `[tool.ruff.lint] select` beside an older top-level `select` in the same file is not resolved; ignoring a code under `select = ["ALL"]` is a finding even when the same change selects more than before; `extends` and presets are not followed (a rule removed from the config, left to a preset, reads as a lowered level only when the base set it to `error`); rule options (`max-len` raised) are not compared; moving `[lints]` from a crate to `[workspace.lints]` reads as a removed `deny`.
- **Hook skips**: `HUSKY=0` and pre-commit's `SKIP=` are not counted (both are routine in CI); a script in another language that runs `git commit --no-verify` is not read.
- **The `pull_request` job** runs the change's code, as before; only `tamper-guard` is outside its reach.

## Measuring

The corpus counts in `bench/RESULTS.md` are still fixture counts, written with the detectors. M26 measures false positives on real history and misses on a corpus written for that purpose.

## Consequences

- A change that lowers a coverage floor, removes or neuters a CI check (including the verify step and the tamper guard's own job), skips git hooks, removes the agent stop hook or turns a lint or type-check rule off is named with file and line against the base.
- In CI, the base and the guard's own definition come from GitHub and the default branch, not from anything the pull request can change.
- `fscale tamper` judges a commit without checking it out, which M26 uses on real history.
- Repos with firescale's previous workflow get the new one from `fscale sync` while they have not edited it.
