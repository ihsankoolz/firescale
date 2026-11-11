# Writing a rule

There are two kinds of rule (ADR 0008):

- **Built-in rules** are Rust, compiled into the `fscale` binary. They live in this repo.
- **Custom rules** are TypeScript, written with the SDK. They live in your repo and run through a Node process that `fscale` starts when needed.

Both need a `SKILL.md` with the same sections and tests with fixtures.

## Built-in rules (Rust)

```
crates/rules/src/<pack>/<rule-id>/
  mod.rs           metadata, check(), optional fix()
  SKILL.md         instructions for humans and agents, embedded with include_str!
  tests.rs         fixtures: passing repo, failing repo, fixed repo, fix run twice
```

Implement `firescale_core::rule::BuiltinRule` and add the rule to its pack in `crates/rules/src/<pack>/mod.rs`. `core/no-committed-env` is the reference. Tests build fixture repos with `firescale_core::testing::Fixture`. A Vitest test (`packages/cli/src/skills.test.ts`) validates every built-in SKILL.md and checks the rule is registered.

## Custom rules (TypeScript)

```
<your pack>/
  pack.ts                    export const pack = definePack({ name, rules })
  rules/<rule-id>/
    rule.ts                  metadata, check(), optional fix()
    SKILL.md                 instructions for humans and agents
    rule.test.ts             fixtures: passing repo, failing repo, fixed repo, fix run twice
```

List the pack in your config by path: `"packs": ["core", "./tools/rules/pack.ts"]`. A complete small example is in [`packages/cli/test-fixtures/custom-pack`](../packages/cli/test-fixtures/custom-pack).

```ts
// pack.ts
import { definePack } from '@firescale/sdk';
import noTodoFile from './rules/no-todo-file/rule.ts';

export const pack = definePack({
  name: 'team',
  rules: [
    { rule: noTodoFile, skillFile: new URL('./rules/no-todo-file/SKILL.md', import.meta.url) },
  ],
});
```

Rule ids must start with the pack's name. Anything a rule logs with `console.log` goes to stderr, so it never corrupts `--json` output. In M2 the bridge runs a custom rule's `check`; running a custom rule's `fix` arrives with `fscale fix`.

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
