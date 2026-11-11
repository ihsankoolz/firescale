# 0018 — The fix loop: `after`, the pass limit, and the dry run

Status: accepted (M8). Changes two points of ADR 0011: how a dry run works, and when `fscale fix` exits non-zero.

## Context

ADR 0015 makes the rules the scaffold: `init`, `adopt` and `sync` are "run every rule's fix until `check` passes". That needs an engine that runs fixes in a sensible order, repeats until nothing is left, and refuses to pretend when fixes do not settle. Until M8, `fscale fix` ran each fix once, in id order, and a dry run of one fix could not see what an earlier fix would have written (a limit recorded in M4).

## Decision

### `after`

- A rule may list the ids of rules whose fixes must run before its own:
  - built-in rules: `after: vec!["core/gitignore-basics".to_owned()]` in `RuleMeta`,
  - custom rules: `after: ['core/gitignore-basics']` in `defineRule`. The SDK rejects a malformed id and a rule that lists itself.
- `after` only orders fixes. It does not change when a rule is checked, and it is not part of the cache key.
- Among rules that are free to run, the lowest id goes first, so the order is the same on every machine.
- An entry that names a rule which exists but is not running (its pack is off, the rule is `off`, or `--rules` leaves it out) places no constraint. A rule pack must be able to say "after `core/ci-exists`" without breaking for a user who turned that rule off.
- An entry that names no rule at all is an error. So is a cycle among the rules that are on. Both are found over every rule that is on, whatever `--rules` selects, and before any fix runs. Turning one rule of a cycle `off` breaks the cycle.

### The loop

- One pass runs the fix of every selected rule that has one, once, in order. Each fix first runs its own check, as before, and repairs the deterministic violations it finds.
- Passes repeat until a pass finds no deterministic violation. So a run that changes something always ends with one pass that changes nothing: that pass is the proof that the fixes have settled.
- A dependency that is not declared with `after` still works: the dependent fix does its work one pass later. `after` saves the pass and makes the order explicit.
- The loop stops with an error, and `fscale fix` exits 2, in three cases:
  1. **A stuck fix.** A rule's fix reports no change in two passes in a row, while the rule still reports violations marked `deterministic`. The rule's fix does not do what its check asks. The error names the rule. One such pass is not enough: a custom fix that writes files without using its context reports no change and has still done its work, which the next pass's check shows.
  2. **Fixes that undo each other.** A rule writes content to a file that it already wrote there earlier in the run, and another rule has written the same file in between. The error names the file and every rule that wrote it. This is caught in the second pass, without waiting for the limit. One rule rewriting its own file is not a fight; if it never settles, the pass limit stops it.
  3. **The pass limit, 10.** Anything else that keeps changing files, such as a fix that appends on every run. The error names the rules and files of the last pass. Ten is far above any real chain: a chain of N undeclared dependencies needs N passes, and declared ones need one.
- A write whose content is what the file already holds is not a change. It is not recorded, not reported, and does not count as progress. This is true of `RuleContext::write` and of the SDK's `ctx.files.write`.
- When the loop stops with an error in a real run, the changes made so far stay on disk and are listed in the error, with the advice to review them with git. Nothing is rolled back: firescale does not own the user's working tree, and a partial repair is more useful than a silent undo.
- The limit is a constant, not a setting. A repo that needs more than ten passes has a rule to repair, not a number to raise.

### Exit code

`fscale fix` still exits 0 when the fixes ran and settled, even if violations remain that only a person or an agent can fix (ADR 0011). What is new: it exits 2 when the loop cannot settle, where it used to exit 0 with "Nothing to fix" for a stuck fix.

### Dry run

- `fscale fix --dry-run` copies the repo to a scratch folder in the OS temp folder, runs the **real** loop there, prints what changed, and deletes the folder. The user's working tree and git index are never touched.
- Why a copy and not a virtual overlay: rules read the repo through their context, through git, through `fscale facts`, and straight from disk. Only a real folder gives all of them the same view of what earlier fixes wrote, including custom TypeScript rules that use `node:fs`. With a copy, "the dry run shows what the real run does" holds by construction.
- What is copied: every file git does not ignore, plus every tracked file. Ignored, untracked files (`node_modules`, build output) are not. If the repo is a git repo, the copy is one too (`git init`), with the same paths in its index, so `git ls-files` answers the same.
- **Git is cut off from the real repo.** A dry run started from a git hook inherits `GIT_DIR`, `GIT_INDEX_FILE` and similar variables, which tell every git command which repository and index to use, whatever folder it runs in. With them, an untracking fix in the copy would change the real index. So once the copy exists, every git command of the run, in the binary and in the Node host, has those variables removed. A test runs a dry run with them set.
- The scratch path is not canonicalized, so that git and Node are not given a `\\?\` path on Windows.
- Custom packs are loaded from the real repo, where their dependencies are installed, and their fixes are pointed at the copy.
- The output groups changes by rule, in the order the rules first changed something. A file one rule writes in several passes is shown once, as a diff from before its first write to after its last. Two rules that change the same file each get their own diff, and the second starts from the first's result.
- The `dry_run` mode of `RuleContext` and of the SDK's fix context still exists for tests and for callers that run one fix. `fscale fix` no longer uses it.

### No new dependency

The scratch folder is made with the standard library. `tempfile` stays a test-only dependency.

## Consequences

- A dry run costs a copy of the repo's tracked and unignored files, and a `git update-index` over the tracked ones. On a very large repo that takes seconds, not milliseconds. It is not on the path of `check`, `verify` or a real `fix`.
- In the copy, a file that git ignores and does not track is absent. A fix whose result depends on such a file (for example one that reads `node_modules`) can preview differently from the real run. No built-in fix does.
- A tracked file that is missing from disk is kept in the copy's index as an empty file, so rules that look at what is tracked still see it.
- If `fscale` is killed during a dry run, the scratch folder is left in the temp folder.
- An `after` id that no rule has stops all of `fscale fix`, also when the id belongs to a custom pack that this repo has not listed. Built-in ids are always known.
- `core/no-committed-env` now declares `after: core/gitignore-basics`, the first real use: its fix adds lines to the `.gitignore` that the other creates. A test in `crates/rules` checks that every built-in `after` names a built-in rule and that there is no cycle.
- A real run makes one extra pass of checks after its last change. For the three built-in fixes that is a few milliseconds.
- A rule that marks a violation `deterministic` and then does not fix it now fails `fscale fix` loudly. That is intended: such a rule would otherwise make `init` report success on a repo that does not pass.
- An independent review before the pull request found that a dry run could change the real git index under inherited git variables, that the fight detector misfired on one rule writing its file twice, and that a fix working outside its context failed the run. All three are fixed, with tests, and the decisions above describe the fixed behaviour.
- One existing test changed its fixture. "shows a new file as a diff against /dev/null" used two rules and asserted that `core/no-committed-env` creates `.gitignore` with four lines. That was the old, inaccurate preview: in a real run `core/gitignore-basics` creates the file first and `core/no-committed-env` edits it. The test now turns on only `core/no-committed-env`, so its assertion is unchanged and true, and a new test covers the two-rule case.
- The bridge's `fixRule` request gained `packRoot` and its reply gained `attempted`. Both are optional additions, so the protocol version stays 1.
