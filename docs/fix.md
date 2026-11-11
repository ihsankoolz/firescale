# `fscale fix`

`fscale fix` applies the deterministic fix of every enabled rule that has one. A fix only does things that have one correct outcome; anything that needs judgement is left for a person or an agent, guided by the rule's SKILL.md.

```
fscale fix --dry-run     # show what would change, change nothing
fscale fix               # apply
fscale fix --rules "core/*"
```

- Rules that are `off` are skipped. Rules that are `evaluated` or `enforced` are fixed.
- Every fix is idempotent: running `fscale fix` again changes nothing and prints `Nothing to fix`.
- The exit code is 0 when the fixes ran and settled, even if violations remain that have no deterministic fix. Run `fscale check` afterwards to see what is left. It is 2 when the fixes cannot settle (see below).

## The fix loop

`fscale fix` does not run each fix once. It runs them in passes, until a pass finds nothing left to fix (ADR 0018). `init`, `adopt` and `sync` run the same loop over every rule that is on ([init.md](init.md)).

- **Order.** A rule can say that its fix must run after another rule's, with `after` (see [writing-rules.md](writing-rules.md)). Otherwise fixes run in order of rule id. `--rules` does not change the order of the rules it selects, and a dependency it leaves out simply does not run.
- **Passes.** A fix that needs something another fix creates, without saying so, gets it one pass later.
- **Bad declarations stop the run before anything is written:** an `after` id that no rule has, and rules that must each run after another of them. Turning one rule of such a cycle `off` breaks it.

The loop stops with an error, and exit code 2, when the fixes do not settle:

| What happened                                                                        | The error names                          |
| ------------------------------------------------------------------------------------ | ---------------------------------------- |
| A fix changed nothing twice in a row, but its rule still reports fixable violations. | The rule.                                |
| A rule wrote the same content to a file again, after another rule changed it.        | The file, and every rule that writes it. |
| Files are still changing after 10 passes.                                            | The rules and files of the last pass.    |

In each case the changes already made stay in your working tree and are listed in the error. Review them with `git status` and `git diff`. The usual cause is a bug in a custom rule; turn the rule `off` to continue.

## Dry run

`--dry-run` copies the repo to a scratch folder, runs the real loop there, and deletes the folder. Your working tree and git index are not touched. Because the fixes really run, each one sees what the fixes before it wrote, and the preview shows what `fscale fix` will do. It prints, per rule, in the order the rules run:

- one line per change (`would untrack`, `would edit`, `would create`),
- a unified diff for every file that would be written, with forward-slash paths and LF line endings. A new file is shown as a diff against `/dev/null`. When two rules change the same file, the second diff starts from the first one's result.

Removing a file from the git index is not a text change, so it appears as a `would untrack` line, not in the diff.

The copy holds every file git does not ignore, plus every tracked file. Ignored files such as `node_modules` are not copied. The copy is its own git repo, and git's `GIT_DIR` and `GIT_INDEX_FILE` variables are ignored during a dry run, so it is safe to run from a git hook.

## Custom rules

A custom TypeScript rule's `fix()` runs in the Node host, through the SDK. The SDK records every `ctx.files.write` and `ctx.git.untrack` call, and those records are what `fscale fix` reports. A fix that changes things by any other route (for example `node:fs` directly) still works, but its changes are not listed and a dry run cannot show them, so use the context. In a dry run the fix works on the scratch copy, while the pack itself is loaded from your repo.

## Limits

- The diff shows all of a file's changes in a single hunk.
- A dry run of a large repo takes as long as copying its tracked and unignored files.
- A fix whose result depends on a file git ignores and does not track can preview differently from the real run, because that file is not in the copy.
- If `fscale` is killed during a dry run, a `firescale-dry-run-*` folder is left in the temp folder. It is safe to delete.
