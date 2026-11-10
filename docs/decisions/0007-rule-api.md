# 0007 — Final rule API and where rules live

Status: accepted (M1)

## Context

SPEC §5 sketches the rule shape and says the final API is decided in M1. A few points needed a concrete answer.

## Decision

1. **`check()` returns `Finding[]`, not `Diagnostic[]`.** A `Finding` is a `Diagnostic` without `ruleId`. The engine (`checkRule`) adds the id, normalises `file` to a forward-slash repo-relative path, validates the shape and sorts by file, line and message. Rule authors cannot get the id or the path format wrong, and output order is stable on every OS.
2. **`file` is required** on every diagnostic, as SPEC §5 lists it. A repo-level problem points at the file that should exist or change (for example `.gitignore`).
3. **Line range** is `range?: { startLine, endLine }`, 1-based and inclusive.
4. **`FixResult` is `{ changedFiles: string[], notes?: string[] }`.** An empty `changedFiles` on a second run is the idempotency contract that tests assert.
5. **`fix()` only receives diagnostics marked `fixable: 'deterministic'`**, and is not called when there are none.
6. **Context:** `check` gets read-only `files` and `git`; `fix` gets a context with `files.write` and `git.untrack` added. The split is enforced by types. `facts` exists but is empty until stack detection lands in M3.
7. **Rule folders live at `packages/rules-<pack>/src/rules/<rule-id>/`**, not `rules/<pack>/<rule-id>/` at the repo root. SPEC §9 puts packs in `packages/`, so the pack is the package and the folder layout inside it matches SPEC §5.
8. **SKILL.md is shipped next to the compiled rule.** A pack entry pairs a rule with its `skillFile` URL, and the pack's build copies every `SKILL.md` into `dist/`.
9. **`rule.ts` uses a default export**, as SPEC §5 shows. This is the one exception to the "named exports" style rule, and ESLint allows it only for `rule.ts` and tool config files.
10. **`core/no-committed-env` matches `.env` and `.env.<name>`** at any depth, not every name starting with `.env`. `.envrc` (direnv) is commonly committed on purpose. `.env.example` is allowed by default; the `allow` option replaces that list.
11. **Its fix** removes the files from the index with `git rm --cached` (the files stay on disk) and appends any missing ignore lines to the root `.gitignore`. It does not rewrite history; it returns a note telling the user to rotate secrets.
12. **`fscale check --rules <glob>`** is implemented now because every SKILL.md's Verify section uses it. `*` matches within an id segment, `**` across `/`, commas separate globs. `--affected` and `--sarif` wait for M2 and M3.
13. **Exit codes:** 0 pass, 1 an enforced rule failed, 2 usage, config or internal error.
14. **SKILL.md validation is a small hand-written parser** (`validateSkill`), not a YAML dependency. It reads single-line top-level frontmatter fields, which is all the spec's required fields need.
15. **Dev dependency:** `ajv` 8.20.0, in `packages/cli` only, to validate real `--json` output against the published JSON Schema in tests.

## Consequences

- The spec's `check: async (ctx) => Diagnostic[]` signature differs slightly from the real one (point 1). SPEC.md is left as written; this ADR records the difference.
- Rules run one after another in M1. Parallel scheduling and caching arrive with the Rust core in M2; `inputs` is already declared on each rule for that.
