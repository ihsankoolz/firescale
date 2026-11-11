// `restless/counter` has a fix that is not idempotent: every run adds a line
// and the rule is never satisfied. `restless/lazy` has a fix that does nothing.
import { defineRule, definePack } from '@firescale/sdk';
import { skillFile } from './shared.ts';

const counter = defineRule({
  id: 'restless/counter',
  severity: 'low',
  category: 'consistency',
  description: 'Never satisfied.',
  inputs: ['count.txt'],
  check: () => [{ file: 'count.txt', message: 'Add another line.', fixable: 'deterministic' }],
  async fix(ctx) {
    const current = (await ctx.files.exists('count.txt')) ? await ctx.files.read('count.txt') : '';
    await ctx.files.write('count.txt', `${current}line\n`);
    return { changedFiles: ['count.txt'] };
  },
});

const lazy = defineRule({
  id: 'restless/lazy',
  severity: 'low',
  category: 'consistency',
  description: 'Reports a violation its fix does not repair.',
  inputs: ['lazy.txt'],
  check: () => [{ file: 'lazy.txt', message: 'lazy.txt is missing.', fixable: 'deterministic' }],
  fix: () => ({ changedFiles: [] }),
});

export const pack = definePack({
  name: 'restless',
  rules: [
    { rule: counter, skillFile },
    { rule: lazy, skillFile },
  ],
});
