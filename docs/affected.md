# `fscale check --affected`

`--affected` narrows a report to what a change touches. Every rule still runs on the whole repo; only the diagnostics are filtered. Use it in a pull request check or an agent hook, where findings in untouched code are noise.

```
fscale check --affected                      # uncommitted changes and untracked files
fscale check --affected --base origin/main   # plus everything committed since the merge base
```

## What counts as affected

A diagnostic is kept when its file:

1. changed: it differs from `HEAD` (staged or not), is untracked and not ignored, or, with `--base <ref>`, differs between the merge base of `<ref>` and `HEAD`; or
2. belongs to an affected project. A project is affected when one of its files changed, or when it depends, directly or through other projects, on a project that is; or
3. belongs to no project below the repo root, and a changed file is one of the `inputs` of the diagnostic's rule. A change in one place can cause a finding on a repo-level file that did not change: a new `lint` script in `web/package.json` makes `AGENTS.md` out of date. Without this the stop hook, which runs `--affected`, would let an agent stop with that finding. It keeps more than needed when a rule reads many files: after any change, `core/no-secrets-in-source` (inputs `**/*`) keeps its findings on repo-level files. Findings inside a project that is not affected are still left out.

For point 3, two cases count as reading every changed file:

- **A custom rule** (a rule of a pack named in the config, not built in). Its result is never cached, so nothing checks its `inputs`; an empty or short list does not show what it reads.
- **Every rule, when `firescale.config.json` or `firescale.config.ts` at the firescale root changed.** The config turns rules and packs on and sets their options, so a rule that was just turned on reports files that did not change.

Projects and their dependencies come from `fscale facts` (see [facts.md](facts.md)). A file belongs to the project with the deepest root that contains it, so a root package only owns files outside the other packages.

The summary, each rule's violation count, `ok` and the exit code all describe the narrowed report.

## Edge cases

- **No commit yet, or not a git repo:** there is nothing to compare with, so every file counts as changed and a note on stderr says so.
- **`--base` that git cannot resolve:** exit code 2 with a message naming the ref.
- **`--base` without `--affected`:** a usage error for `check`. `fscale verify --base <ref>` works without `--affected`: it chooses the base commit that failures are compared with (ADR 0037).
- **Deleted files** count as changed for their project, so the project's other findings are reported.

## `fscale verify --affected`

`verify --affected` uses the same changed files and narrows the `check` step the same way. For lint, typecheck and test it goes further and uses the full project graph ([graph.md](graph.md)): only the projects a change reaches run, and the rest are reported as `not affected`. When unsure it runs more: a file no project owns, a root lockfile, a CI file, a Compose file or a `firescale.config.ts` affects every project, and so does a clean tree with no `--base`. A change to `firescale.config.json` reaches a project only through a setting that can change a step's result (ADR 0058; "What a config change affects" in verify.md). See "Affected only" in [verify.md](verify.md). With `--base`, the same ref also sets the base commit that blame-aware verify compares failures with ("Only new failures block" in verify.md); without it, that base is the default branch.

`fscale check --affected` still keeps the rules of this page: its project graph is the one of `dependsOn` (workspace members by name), and a clean tree reports nothing. Its job is to hide findings in untouched code, where reporting too little costs nothing.

## Limits

- `check --affected` uses the simple graph built from workspace files; `verify --affected` uses the full one. Nx's own graph is not read by either.
- Rules are not skipped for unaffected projects, so `--affected` changes what is reported, not how long the check takes. The cache already makes unchanged rules cheap.
- A finding on a file inside a project that is not affected is left out even when a change elsewhere caused it. A built-in rule with no `inputs` (`group-project/branch-name`, which reads the branch name and no file) is kept by point 3 only when the firescale config changed.
