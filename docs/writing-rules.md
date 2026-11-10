# Writing a rule

A rule is a folder inside a rule pack:

```
packages/rules-<pack>/src/rules/<rule-id>/
  rule.ts          metadata, check(), optional fix()
  SKILL.md         instructions for humans and agents
  rule.test.ts     fixtures: passing repo, failing repo, fixed repo, fix run twice
```

Register it in the pack's `src/index.ts`. A test fails if a rule folder is not registered or has no `SKILL.md`.

## rule.ts

```ts
import { defineRule } from '@firescale/sdk';

export default defineRule({
  id: 'core/no-committed-env', // <pack>/<rule>
  severity: 'blocker', // blocker | high | medium | low
  category: 'security', // security | reliability | consistency | maintainability
  description: 'No .env files are tracked by git.',
  inputs: ['**/.env', '**/.env.*', '.gitignore'], // globs the check depends on
  async check(ctx) {
    return []; // Finding[]: { message, file, range?, project?, fixable }
  },
  async fix(ctx, diagnostics) {
    return { changedFiles: [] }; // optional; must be idempotent
  },
});
```

- `check(ctx)` is read-only. `ctx` has `root`, `options`, `facts`, `files.exists/read` and `git.isRepo/trackedFiles`.
- `check` returns findings without a rule id. The engine adds `ruleId`, normalises `file` to a forward-slash repo-relative path, and sorts the result.
- `fix(ctx, diagnostics)` also gets `files.write` and `git.untrack`. It only receives diagnostics marked `fixable: 'deterministic'`. Running it twice must change nothing the second time, and the second `changedFiles` must be empty.
- Throw `FirescaleError` for problems the user can fix (for example bad options). The message must name the problem and the next step.

## SKILL.md

Copy [`packages/sdk/templates/SKILL.md`](../packages/sdk/templates/SKILL.md). The frontmatter follows the Agent Skills spec (agentskills.io): `name` must equal the rule's short name, plus a `description`. No custom fields. The sections are fixed and checked by `validateSkill`:

1. `## What this rule enforces and why`
2. `## Manual fix`
3. `## Agent fix instructions`, with `### DETERMINISTIC` (steps the agent applies) and `### REPORT` (items the agent only reports)
4. `## Verify`, containing the exact command `fscale check --rules <pack>/<rule>`

## rule.test.ts

Fixture repos are built at test time with `@firescale/sdk/testing`:

```ts
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, expect, it } from 'vitest';
import rule from './rule.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

it('failing repo', async () => {
  const repo = await fixtures.create({ files: { '.env': 'EXAMPLE=1\n' } });
  expect(await repo.check(rule)).toHaveLength(1);
});
```

`fixtures.create({ files, track })` makes a temp git repo. `track` is `'all'` (default), `'none'`, `'no-git'` or a list of paths. The repo object has `check`, `fix`, `trackedFiles`, `read`, `write`, `track` and `root`. Never put anything that looks like a real secret in a fixture.
