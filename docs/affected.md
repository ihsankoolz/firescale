# `fscale check --affected`

`--affected` narrows a report to what a change touches. Every rule still runs on the whole repo; only the diagnostics are filtered. Use it in a pull request check or an agent hook, where findings in untouched code are noise.

```
fscale check --affected                      # uncommitted changes and untracked files
fscale check --affected --base origin/main   # plus everything committed since the merge base
```

## What counts as affected

A diagnostic is kept when its file:

1. changed: it differs from `HEAD` (staged or not), is untracked and not ignored, or, with `--base <ref>`, differs between the merge base of `<ref>` and `HEAD`; or
2. belongs to an affected project. A project is affected when one of its files changed, or when it depends, directly or through other projects, on a project that is.

Projects and their dependencies come from `fscale facts` (see [facts.md](facts.md)). A file belongs to the project with the deepest root that contains it, so a root package only owns files outside the other packages.

The summary, each rule's violation count, `ok` and the exit code all describe the narrowed report.

## Edge cases

- **No commit yet, or not a git repo:** there is nothing to compare with, so every file counts as changed and a note on stderr says so.
- **`--base` that git cannot resolve:** exit code 2 with a message naming the ref.
- **`--base` without `--affected`:** a usage error.
- **Deleted files** count as changed for their project, so the project's other findings are reported.

## Limits

- The project graph is the simple one built from workspace files. Nx's own graph is not read yet.
- Rules are not skipped for unaffected projects, so `--affected` changes what is reported, not how long the check takes. The cache already makes unchanged rules cheap.
