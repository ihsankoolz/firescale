# 0019 — `init`, `adopt` and `sync`

Status: accepted (M9). Builds on ADR 0015 (rules as the scaffold) and ADR 0018 (the fix loop). Changes one point of ADR 0012: two core rules that had no fix now have one.

## Context

M8 built the fix loop. M9 puts three commands on it: `init` for a new repo, `adopt` for an existing one, `sync` to repair drift. SPEC §15 fixes their meaning; this ADR records the choices the spec leaves open. The owner asked for extra care on three points: `adopt` must never destroy a person's work, `init` must refuse a folder that is not empty unless forced, and every command must print what it changed and what is left.

## Decision

### One code path

- All three commands call the same function, which calls the same planner and loop as `fscale fix` (`plan_fixes`, `FixPlan::run`, `render_fixes` in `crates/cli/src/main.rs`). The commands differ only in their checks before the loop and in whether a config is created. There is no template anywhere.
- They run every rule that is on. `--rules` is rejected; `fscale fix --rules` exists for that.
- An option that belongs to another command (`init --dry-run`, `adopt --force`, `sync --dry-run`) is an error that names the right command. A mistyped safety flag must not be silently ignored.

### Never destroying work

The guarantee is made of small, checkable parts, not of one clever guard:

1. **The config is created with `create_new`.** The call itself fails if the file exists. It is the only file these commands write directly.
2. **The two new fixes only create.** `core/ci-exists` writes `.github/workflows/ci.yml` only when `.github/workflows` holds no workflow file on disk, tracked or not, and no other CI system is configured. `core/readme-setup` writes `README.md` only when no entry at the root has a name starting with "readme", in any case. Each fix checks the disk again itself and does not rely on its check.
3. **What exists is never edited by them.** A README without install steps and a workflow without a pull request trigger stay `agent`. A file that exists but is untracked is `manual` ("run `git add`").
4. **No generic guard.** A rule "every old line must survive" enforced by the loop was considered and rejected: a legitimate merge into a JSON file changes neighbouring lines, and M12's managed block rewrites lines by design. The property is proven by tests instead: fixture repos with a README, a workflow, a `.gitignore` and an AGENTS.md containing CRLF, a BOM, no final newline, non-ASCII text and look-alike markers, compared byte for byte before and after `adopt`, `sync` and `init --force`, tracked, untracked and outside git.
5. **Nothing is deleted and nothing is committed.** A fix can write a file, untrack one (`core/no-committed-env`, the file stays on disk) or stage one it created.

### Fixes stage the file they create

`core/ci-exists` and `core/readme-setup` look at tracked files, on purpose: an untracked workflow never runs. A fix that created an untracked file would therefore not repair its own violation, and the loop would stop with "the fix changed nothing". Two ways out:

- make the checks count untracked files: rejected, it weakens the rules, and an existing test pins that an untracked workflow does not count;
- **let the fix `git add` the one file it created** (`RuleContext::track`, a new `Change::Track`): chosen. It is the mirror image of the untracking that already exists, it touches only a file firescale wrote a moment ago, and the output says `(staged with git add)`.

Both checks now also require the tracked file to be on disk, so that a deleted generated file is drift that `sync` repairs.

### `init`

- Works on the current folder (`--cwd` for another). It takes no folder argument and creates no folder: `mkdir x && cd x && fscale init`. Simplest reading of "an empty or new folder".
- **Empty** means: no entries, or only `.git`. Anything else and no firescale config: `init` refuses with exit code 2, changes nothing (it has not even run `git init`), and prints the two `adopt` commands. `--force` runs anyway and is exactly as careful as `adopt`.
- A folder that already has a config is a firescale repo: `init` keeps the config and runs the loop, like `sync`. `--packs` is an error there.
- An empty folder inside a firescale repo is refused too, unless forced, so that a nested repo is not started by accident.
- `git init` runs only when the folder is not inside a git work tree. The branch name is left to the user's git settings.
- `--packs` accepts built-in pack names only and is validated before anything is written. Default: `core,agent-ready` (SPEC §16).
- No `--dry-run`: in an empty folder everything is new, and in any other folder `adopt --dry-run` is the preview.

### `adopt`

- Without a config it writes one with `core` only: `agent-ready` is opt-in for an existing repo (SPEC §6, ADR 0014).
- `--dry-run` is the scratch-copy dry run of ADR 0018. The new config appears in it as a change like any other.
- It does not run `git init`. Rules already work outside git.

### `sync`

- Needs a config; otherwise it points at `adopt` and `init`.
- `--check` is a dry run that exits 1 when there is at least one change, and prints the diff.

### Output and exit codes

- Changes are printed with the renderer of `fscale fix`. The config appears under the heading `firescale`.
- Then **"Left for you"**: one more check on the result, listing every remaining violation with its fix hint and `fscale explain` pointer. Then **"To review"**: the notes of the fixes that ran, which is how a fix tells a person what to look at in a file it created.
- Exit code 0 when the loop settled, whatever is left: every rule starts as `evaluated`, and `fscale fix` behaves the same (ADR 0011). 1 only for `sync --check` with drift. 2 for refusals and errors.

### The generated files

- **Workflow.** `on: pull_request` and pushes to `main` and `master`; read-only permissions; checkout and setup-node pinned to the same commit SHAs as this repo's own CI (ADR 0009); an install step for npm (`npm ci` with a lockfile, else `npm install`) and pnpm; then `npx firescale verify`. Other package managers get a comment and a "To review" note instead of a guess.
- **README.** A title (the root project's name, or "Project name"; not the folder name, which differs in a dry run's scratch copy), `## Install` and `## Usage`, filled with the commands `fscale facts` knows and a sentence to replace where it knows none.
- Both files carry one comment saying firescale created them and that they now belong to the team. Neither is a managed file: once created, firescale never writes to it again.

## Consequences

- Two existing assertions changed on purpose, both from `Fixable::Agent` to `Fixable::Deterministic`: a missing workflow and a missing README are now repaired by a fix, as PLAN.md M9 requires. The assertion that an untracked workflow does not count is unchanged and still passes.
- `init` leaves two files staged (README, workflow) and two unstaged (`.gitignore`, the config). It is uneven, and it is printed. Staging everything would be `init` deciding what goes into a commit.
- A README or workflow a person wrote, committed, and then deleted from disk without committing the deletion is created again by `sync` as the starter version. Nothing is lost (`git restore` brings it back), and treating the two cases differently would need firescale to know who wrote a file.
- In a dry run, custom TypeScript rules are not part of "Left for you", because the Node host checks the real repo. They are named instead.
- The generated workflow calls `npx firescale verify`, so it fails until 0.1.0 is on npm. That is the publishing checkpoint after M9.
- The pinned action SHAs in the generated workflow age. They are in one place (`crates/rules/src/core/ci-exists/mod.rs`) and pinned by a test, so updating them is one edit.
- `README*` rule inputs became case-insensitive, so the cache sees a `ReadMe.md`.
- `agent-ready` has no fixes until M12, so a default `init` lists its violations under "Left for you".
