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

| Key              | Type                    | Default    | Meaning                                                                              |
| ---------------- | ----------------------- | ---------- | ------------------------------------------------------------------------------------ |
| `packs`          | string[]                | `["core"]` | Rule packs that are on.                                                              |
| `rules`          | object keyed by rule id | `{}`       | Per rule: a status, or `{ status, options }`.                                        |
| `protectedPaths` | string[]                | `[]`       | Globs an agent fix run may never change. Stored now, used by the diff guard from M5. |

Rule status:

- `off`: the rule does not run.
- `evaluated` (default): violations are reported, the exit code stays 0.
- `enforced`: a violation makes `fscale check` exit 1.

Unknown keys, packs, rule ids and statuses are errors, so a typo cannot silently disable a rule.

The root is found by walking up from the current folder to the nearest folder that has a firescale config or a `.git` entry. The search stops at the git root. With neither, the root is the current folder.

`firescale.config.ts` is loaded by Node's built-in TypeScript support (Node 22.18 or newer), so it may only use erasable TypeScript syntax. The `import { defineConfig } from 'firescale'` line needs firescale installed in the repo; without it, export a plain object.
