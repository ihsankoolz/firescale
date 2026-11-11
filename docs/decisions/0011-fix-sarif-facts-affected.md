# 0011 — fix, dry run, SARIF, facts and --affected

Status: accepted (M4)

## Context

M4 adds `fscale fix` with `--dry-run`, SARIF output, `fscale facts` and `--affected`. Each left choices open.

## Decision

### Fix and dry run

- A fix does its work through its context: writing a file and untracking a path. The context records every such change. `fscale fix` prints the record, so the output is what actually happened, not what the rule claims.
- **A dry run is the same code with the context in record-only mode.** Nothing is written and the git index is not touched. This avoids a second "plan" implementation per rule that could drift from the real fix. A test asserts that a dry run and a real fix report identical changes.
- "Prints a diff" means a unified diff for every file that would be written. Removing a file from the git index is not a text change, so it is a `would untrack` line. The diff is a small hand-written line diff with one hunk per file; no dependency.
- `fscale fix` exits 0 when fixes ran, even if violations remain. `fscale check` is the command that judges the repo.
- Rules that are `off` are not fixed. `evaluated` rules are: report-only status is about failing the build, not about whether the user wants the fix.
- **Custom TypeScript rules:** the bridge gains a `fixRule` method. The SDK context records `files.write` and `git.untrack` calls and skips them in a dry run, so custom rules get `--dry-run` for free as long as they use the context.

### SARIF

- `fscale check --sarif` prints SARIF 2.1.0. `--json` and `--sarif` together are a usage error.
- Level mapping: a report-only (`evaluated`) rule is always `note`. An `enforced` rule is `error` for blocker and high, `warning` for medium, `note` for low.
- File locations are repo-relative, percent-encoded URI references with `uriBaseId` `%SRCROOT%`.
- The official schema (`sarif-schema-2.1.0.json` from the `oasis-tcs/sarif-spec` repository) is checked in under `packages/cli/test-fixtures/` and real output is validated against it in tests. It is a draft-04 schema; the test strips its `$schema` and `id` keys so the existing `ajv` dev dependency can compile it.

### Facts

- `fscale facts` reads marker files only. It never runs a package manager, loads the firescale config or starts Node.
- No TOML or YAML parser was added. `Cargo.toml` and `pnpm-workspace.yaml` are read by small line-based readers that handle the common forms and add a warning for what they skip (flow-style YAML, `**` globs). `docs/facts.md` lists the limits.
- The project graph is built from pnpm/npm/yarn workspaces and Cargo workspaces. Reading Nx's graph and uv workspaces is not done; `nx.json` is only reported as a tool.
- The JSON output has a schema at `packages/cli/schemas/facts-output.schema.json`, with `schemaVersion` 1.

### --affected

- `--affected` filters diagnostics; it does not skip rules. Rules like `core/no-committed-env` look at the whole repo, and the cache already avoids re-running unchanged rules.
- Without `--base`, the comparison is the working tree against `HEAD`, plus untracked files. With `--base <ref>`, commits since the merge base are added. There is no default base, because guessing `origin/main` is wrong for many repos.
- With no commit to compare against, everything counts as changed, with a note.

## Consequences

- A custom rule that edits files without the context cannot be previewed or reported. That is documented, not prevented.
- The line-based readers will misread unusual `Cargo.toml` or workspace files. The failure mode is a missing project, plus a warning where the reader can tell.
- `--affected` does not make a check faster.
- `fscale facts` output will grow (Nx graph, uv, more commands). New fields will be additive within `schemaVersion` 1.
