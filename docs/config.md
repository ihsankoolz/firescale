# Config

firescale reads one file at the repo root: `firescale.config.ts` or `firescale.config.json`. Having both is an error. With no config file, the `core` pack is on and every rule is `evaluated`.

```ts
// firescale.config.ts
import { defineConfig } from 'firescale';

export default defineConfig({
  packs: ['core'],
  rules: {
    'core/no-committed-env': 'enforced',
  },
  protectedPaths: ['infra/**'],
});
```

```json
{
  "packs": ["core"],
  "rules": {
    "core/no-committed-env": { "status": "enforced", "options": { "allow": [".env.example"] } }
  }
}
```

| Key              | Type                    | Default    | Meaning                                                                                                                                                                                                                                                                                                                                                                                                       |
| ---------------- | ----------------------- | ---------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `packs`          | string[]                | `["core"]` | Rule packs that are on: built-in names (`core`, `agent-ready`, `group-project`), or paths to custom packs.                                                                                                                                                                                                                                                                                                    |
| `rules`          | object keyed by rule id | `{}`       | Per rule: a status, or `{ status, options }`.                                                                                                                                                                                                                                                                                                                                                                 |
| `protectedPaths` | string[]                | `[]`       | Globs an agent fix run may never change. Stored now, used by the diff guard from M5.                                                                                                                                                                                                                                                                                                                          |
| `verify`         | object                  | `{}`       | For `fscale verify`: a command, `false` or `{ command, timeout }` per step (`lint`, `typecheck`, `test`), `timeout` and `idleTimeout` (ADR 0043), `ci`, `failOnSkipped`, `stopWhenServiceDown`, `stopAfterErrors`, `baseBudget`, `baseInstall` and `labelFixed` (blame-aware verify, ADR 0037, ADR 0043), `tamper` (`off`, `evaluated` or `enforced`; read from the base commit, ADR 0042), and per-project settings under `projects`. See [verify.md](verify.md). |

Rule status:

- `off`: the rule does not run.
- `evaluated` (default): violations are reported, the exit code stays 0.
- `enforced`: a violation makes `fscale check` exit 1.

Unknown keys, packs, rule ids and statuses are errors, so a typo cannot silently disable a rule.

The root is found by walking up from the current folder to the nearest folder that has a firescale config or a `.git` entry. The search stops at the git root. With neither, the root is the current folder.

## Custom rule packs

A `packs` entry that starts with `./` or `../` is a path, relative to the repo root, to a module that exports a rule pack written with the TypeScript SDK:

```json
{ "packs": ["core", "./tools/rules/pack.ts"] }
```

The module's default export, or an export named `pack`, must be a `definePack({ name, rules })` value. A custom pack cannot use the name of a built-in pack. See [writing-rules.md](writing-rules.md).

## When Node is started

`fscale` is a native binary. It starts a Node process only when the run needs TypeScript:

- the config file is `firescale.config.ts`, or
- `packs` lists a custom pack.

A repo with `firescale.config.json` (or no config) and only built-in packs never starts Node. When Node is needed, `fscale` uses `node` from `PATH`, or the executable named by the `FSCALE_NODE` environment variable, and the repo must have the `firescale` package installed.

`firescale.config.ts` is loaded by Node's built-in TypeScript support (Node 22.18 or newer), so it may only use erasable TypeScript syntax. The `import { defineConfig } from 'firescale'` line needs firescale installed in the repo; without it, export a plain object.

## The result cache

`fscale check` stores each built-in rule's result in `.firescale/cache/` and reuses it while the rule's input files, their git state, the rule's options and the firescale version are unchanged. firescale writes a `.gitignore` inside `.firescale/`, so the folder is never committed.

- `fscale check --no-cache` runs every rule again and neither reads nor writes the cache.
- `FSCALE_LOG=cache fscale check` prints `cache: hit`, `miss` or `skip` per rule on stderr.
- Custom TypeScript rules are not cached.
- Deleting `.firescale/` is always safe.

## `--rules` and rules that are off

`fscale check --rules <glob>` skips rules that are `off`. The exception is a rule named by its exact id, for example `fscale check --rules core/no-committed-env`: it runs even when it is `off`, as report-only, and a note on stderr says so. This keeps the Verify command in every SKILL.md usable.
