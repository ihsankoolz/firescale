# 0005 — Config format and loader

Status: accepted (M1)

## Context

SPEC §10 asks for one config file, `firescale.config.ts` or `.json`, holding the enabled packs, per-rule status and options, and protected paths. A `.ts` file has to be executed somehow.

## Decision

- **Loading `.ts`:** use `import()` and Node's built-in type stripping, which is on by default from Node 22.18. No loader dependency (jiti, tsx, esbuild). This is why `engines.node` is `>=22.18.0`.
- **Shape:** `{ packs?: string[], rules?: Record<ruleId, status | { status?, options? }>, protectedPaths?: string[] }`. `$schema` is tolerated in JSON.
- **Defaults:** packs `['core']`, status `evaluated` (SPEC §10), no protected paths.
- **Strict validation:** unknown top-level keys, packs, rule ids, rule keys and statuses are errors (exit code 2). A typo must not silently turn a rule off.
- **Both files present:** error. No precedence rule to remember.
- **Root discovery:** walk up from the working folder to the nearest config file, else the nearest `.git`, else stay put.
- **Packs:** only built-in packs resolve in M1 (`core`). Third-party packs are not designed yet.
- `protectedPaths` is parsed and validated now but only used from M5.

## Consequences

- `firescale.config.ts` is limited to erasable TypeScript syntax (no enums, no `paths` aliases). Fine for a config file.
- `import { defineConfig } from 'firescale'` only resolves when firescale is installed in the repo. Running through a global install or `npx` needs a plain exported object or the JSON form. The loader says so when the import fails.
- The `.ts` path is tested through the built binary, not in-process, because Vitest would transform the file itself and hide a Node loading problem.
