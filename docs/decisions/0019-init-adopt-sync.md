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

1. **Files are created with `create_new`.** The call itself fails if anything is at the path. That holds for the config, the only file these commands write directly, and for the files the two new fixes create, through `RuleContext::create_tracked`.
2. **The two new fixes only create.** `core/ci-exists` writes `.github/workflows/ci.yml` only when `.github/workflows` holds no workflow file on disk, tracked or not, and no other CI system is configured. `core/readme-setup` writes `README.md` only when no entry at the root has a name starting with "readme", in any case. Each fix checks the disk again itself and does not rely on its check.
3. **What exists is never edited by them.** A README without install steps and a workflow without a pull request trigger stay `agent`. A file that exists but is untracked is `manual` ("run `git add`").
4. **No generic guard.** A rule "every old line must survive" enforced by the loop was considered and rejected: a legitimate merge into a JSON file changes neighbouring lines, and M12's managed block rewrites lines by design. The property is proven by tests instead: fixture repos with a README, a workflow, a `.gitignore` and an AGENTS.md containing CRLF, a BOM, no final newline, non-ASCII text and look-alike markers, compared byte for byte before and after `adopt`, `sync` and `init --force`, tracked, untracked and outside git.
5. **Nothing is deleted and nothing is committed.** A fix can write a file, untrack one (`core/no-committed-env`, the file stays on disk) or stage one it created.
6. **The git index is not a place to overwrite either.** A path git already tracks is never created or staged, on disk or not: a tracked file that is missing from disk may hold staged work that exists nowhere else. The rules report it as `manual` ("`git restore` it, or commit its deletion").
7. **No writing through links.** `RuleContext::write` and `create_tracked` refuse a path that is, or lies below, a symbolic link inside the repo. A linked `.github` or `.gitignore` would otherwise lead a fix to a file outside the repo.
8. **When in doubt, do nothing.** A folder that cannot be listed, a path a `.gitignore` rule ignores, an answer git cannot give: each makes the finding `agent` or `manual`, never `deterministic`. If `git add` fails after a file was created, the file is removed again, so a failed fix leaves nothing unreported.

Points 6 to 8 and the wording of point 1 come from the independent review before the pull request, which showed, with reproductions, that the first version could replace a staged README in the index, overwrite an untracked README in a folder it could not list, and write a workflow through a linked `.github`. Each has a test now.

### Fixes stage the file they create

`core/ci-exists` and `core/readme-setup` look at tracked files, on purpose: an untracked workflow never runs. A fix that created an untracked file would therefore not repair its own violation, and the loop would stop with "the fix changed nothing". Two ways out:

- make the checks count untracked files: rejected, it weakens the rules, and an existing test pins that an untracked workflow does not count;
- **let the fix `git add` the one file it created** (`RuleContext::track`, a new `Change::Track`): chosen. It is the mirror image of the untracking that already exists, it touches only a file firescale wrote a moment ago, and the output says `(staged with git add)`.

Both checks now also require the tracked file to be on disk. A file that is gone from disk and from the index is drift that `sync` repairs. A file that is gone from disk but still in the index is reported and left alone (point 6 above).

### `init`

- Works on the current folder (`--cwd` for another). It takes no folder argument and creates no folder: `mkdir x && cd x && fscale init`. Simplest reading of "an empty or new folder".
- **Empty** means: no entries, or only `.git`. Anything else and no firescale config: `init` refuses with exit code 2, changes nothing (it has not even run `git init`), and prints the two `adopt` commands. `--force` runs anyway and is exactly as careful as `adopt`.
- A folder that already has a config is a firescale repo: `init` keeps the config and runs the loop, like `sync`. `--packs` is an error there.
- An empty folder inside another git repo or firescale repo is refused too, unless forced: the files would land in a subfolder of that repo and be staged in its index.
- `git init` runs only when the folder is not inside a git work tree, and only after the packs, the config and the rules' order have been validated, so a refusal leaves no `.git` behind. The branch name is left to the user's git settings.
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

- The first line names the repo root, because for `adopt` and `sync` it can be a parent of the current folder.
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
- A dry run's scratch copy (ADR 0018) now also holds empty folders and symbolic links, as links, because the new fixes look at both. It still lacks files git ignores and the repo's own git settings.
- A repo with a linked `.gitignore` and a missing ignore entry stops `adopt` with an error instead of being fixed. The message says to replace the link or turn the rule off.
- Writes are not atomic: a fix that appends to a file writes it in place.
- In a dry run, custom TypeScript rules are not part of "Left for you", because the Node host checks the real repo. They are named instead.
- The generated workflow calls `npx firescale verify`, so it fails until 0.1.0 is on npm. That is the publishing checkpoint after M9.
- The pinned action SHAs in the generated workflow age. They are in one place (`crates/rules/src/core/ci-exists/mod.rs`) and pinned by a test, so updating them is one edit.
- `README*` rule inputs became case-insensitive, so the cache sees a `ReadMe.md`.
- `agent-ready` has no fixes until M12, so a default `init` lists its violations under "Left for you".
