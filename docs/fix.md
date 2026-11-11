# `fscale fix`

`fscale fix` applies the deterministic fix of every enabled rule that has one. A fix only does things that have one correct outcome; anything that needs judgement is left for a person or an agent, guided by the rule's SKILL.md.

```
fscale fix --dry-run     # show what would change, change nothing
fscale fix               # apply
fscale fix --rules "core/*"
```

- Rules that are `off` are skipped. Rules that are `evaluated` or `enforced` are fixed.
- Every fix is idempotent: running `fscale fix` again changes nothing and prints `Nothing to fix`.
- The exit code is 0 when the fixes ran, even if violations remain that have no deterministic fix. Run `fscale check` afterwards to see what is left.

## Dry run

`--dry-run` works out the changes without touching the working tree or the git index. It prints, per rule:

- one line per change (`would untrack`, `would edit`, `would create`),
- a unified diff for every file that would be written, with forward-slash paths and LF line endings. A new file is shown as a diff against `/dev/null`.

Removing a file from the git index is not a text change, so it appears as a `would untrack` line, not in the diff.

## Limits

- Fixes of custom TypeScript rules are not run yet. `fscale fix` prints a note for a custom rule that has one.
- The diff shows all of a file's changes in a single hunk.
