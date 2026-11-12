// `fscale add rule`: a custom TypeScript rule with its SKILL.md and test.
import type { Generator } from './types.js';

/** A string literal in single quotes, as Prettier writes it. */
export function quote(text: string): string {
  return `'${text.replace(/\\/g, '\\\\').replace(/'/g, "\\'")}'`;
}

/** An identifier for the rule's import: `team/1st-rule` gives `rule1stRule`. */
export function importName(short: string): string {
  const camel = short.replace(/-([a-z0-9])/g, (_m: string, c: string) => c.toUpperCase());
  return `rule${camel.charAt(0).toUpperCase()}${camel.slice(1)}`;
}

function skill(id: string, short: string, description: string): string {
  // The rule's own description goes in the body: a plain YAML value
  // cannot hold every sentence (`: `, ` #`), and quotes are a formatter's choice.
  return `---
name: ${short}
description: Fixes violations of the firescale rule ${id}. Use when fscale check reports ${id}.
---

# ${id}

## What this rule enforces and why

${description}

Say why it matters: what goes wrong when the rule is broken.

## Manual fix

1. Describe the first step a person takes to fix a finding.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

Steps the agent applies. Each step must be safe and have one correct outcome.

1. Describe the step, or write "None." when every fix needs a person.

### REPORT

Items the agent only reports. It must not act on them.

- Anything that needs a human decision.

## Verify

Run this command. It must report no violations for this rule.

\`\`\`
fscale check --rules ${id}
\`\`\`
`;
}

export const customRule: Generator = {
  name: 'rule',
  description:
    'A custom TypeScript rule: rule.ts with its check, SKILL.md and a fixture test, in a rule pack (created when missing).',
  schema: {
    $schema: 'http://json-schema.org/draft-07/schema#',
    title: 'fscale add rule',
    description: 'Input for the custom rule generator.',
    type: 'object',
    additionalProperties: false,
    required: ['id', 'description'],
    properties: {
      id: {
        type: 'string',
        description: 'The rule id, <pack>/<rule>, lower case with hyphens: "team/no-todo-file".',
        // The SDK's own rule id pattern (define.ts).
        pattern: '^[a-z0-9]+(?:-[a-z0-9]+)*/[a-z0-9]+(?:-[a-z0-9]+)*$',
        maxLength: 80,
      },
      description: {
        type: 'string',
        description: 'One sentence on one line: what must be true in the repo.',
        // Something to say, and no line breaks or control characters.
        pattern: '^[^\\u0000-\\u001f]*\\S[^\\u0000-\\u001f]*$',
        minLength: 1,
        maxLength: 200,
      },
      severity: {
        type: 'string',
        description: 'How bad a violation is.',
        enum: ['blocker', 'high', 'medium', 'low'],
        default: 'medium',
      },
      category: {
        type: 'string',
        description: 'What kind of problem it is.',
        enum: ['security', 'reliability', 'consistency', 'maintainability'],
        default: 'maintainability',
      },
      packFolder: {
        type: 'string',
        description:
          'The pack folder, relative to the repo root with forward slashes. Default: firescale-rules.',
        // Relative, no `.` or `..` segment, no segment that ends in a dot,
        // and never inside .git or node_modules.
        pattern:
          '^(?!\\.\\.?(/|$))(?!.*/\\.\\.?(/|$))(?!.*\\.(/|$))(?!(.*/)?(\\.git|node_modules)(/|$))[A-Za-z0-9._-]+(/[A-Za-z0-9._-]+)*$',
        default: 'firescale-rules',
      },
    },
  },
  generate(input, ctx) {
    const id = String(input['id']);
    const [pack = '', short = ''] = id.split('/');
    const description = String(input['description']).trim();
    const folder = String(input['packFolder']);
    const ruleDir = `${folder}/rules/${short}`;
    const files = [
      {
        path: `${ruleDir}/rule.ts`,
        content: `import { defineRule } from '@firescale/sdk';

// ${description.replace(/\*\//g, '* /')}
export default defineRule({
  id: ${quote(id)},
  severity: ${quote(String(input['severity']))},
  category: ${quote(String(input['category']))},
  description: ${quote(description)},
  // Globs of the files the check reads: the result is cached until one changes.
  inputs: ['**/*'],
  async check(ctx) {
    // Return one finding per problem: { file, message, fixable: 'manual' }.
    // ctx.files.read(path), ctx.git.trackedFiles() and ctx.facts are available.
    const tracked = await ctx.git.trackedFiles();
    void tracked;
    return [];
  },
});
`,
      },
      { path: `${ruleDir}/SKILL.md`, content: skill(id, short, description) },
      {
        path: `${ruleDir}/rule.test.ts`,
        content: `import { checkRule } from '@firescale/sdk';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import rule from './rule.ts';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

describe(${quote(id)}, () => {
  it('passes on a repo that follows the rule', async () => {
    const repo = await fixtures.create({ files: { 'README.md': '# Example\\n' } });
    expect(await checkRule(rule, { root: repo.root })).toEqual([]);
  });

  it.todo('reports a repo that breaks the rule');
  it.todo('passes once the repo is fixed');
});
`,
      },
    ];
    const notes: string[] = [];
    const packFile = `${folder}/pack.ts`;
    if (ctx.exists(packFile)) {
      notes.push(
        `Add the rule to ${packFile}: import it from './rules/${short}/rule.ts' and list it with skillFile new URL('./rules/${short}/SKILL.md', import.meta.url).`,
      );
    } else {
      files.push({
        path: packFile,
        content: `import { definePack } from '@firescale/sdk';
import ${importName(short)} from './rules/${short}/rule.ts';

export const pack = definePack({
  name: ${quote(pack)},
  rules: [
    {
      rule: ${importName(short)},
      skillFile: new URL('./rules/${short}/SKILL.md', import.meta.url),
    },
  ],
});
`,
      });
      notes.push(
        `Turn the pack on: add "./${packFile}" to "packs" in your firescale config. It needs @firescale/sdk as a dev dependency.`,
      );
    }
    notes.push(
      `Write the check in ${ruleDir}/rule.ts, the failing and fixed cases in rule.test.ts, and the steps in SKILL.md.`,
    );
    return { files, notes };
  },
};
