// Fixes that are fine, and that a careless loop would call broken.
// `quiet/twice` writes the same final content once per violation.
// `quiet/own-way` repairs its file without using its context, so the loop
// sees a violation and no recorded change.
import { writeFile } from 'node:fs/promises';
import path from 'node:path';
import { defineRule, definePack } from '@firescale/sdk';
import { skillFile } from './shared.ts';

const WANTED = 'alpha\nbeta\n';

const twice = defineRule({
  id: 'quiet/twice',
  severity: 'low',
  category: 'consistency',
  description: 'list.txt lists alpha and beta.',
  inputs: ['list.txt'],
  async check(ctx) {
    const current = (await ctx.files.exists('list.txt')) ? await ctx.files.read('list.txt') : '';
    return ['alpha', 'beta']
      .filter((name) => !current.split('\n').includes(name))
      .map((name) => ({
        file: 'list.txt',
        message: `list.txt does not list ${name}.`,
        fixable: 'deterministic' as const,
      }));
  },
  async fix(ctx, diagnostics) {
    // Once per violation, on purpose: the second write changes nothing.
    for (let done = 0; done < diagnostics.length; done += 1) {
      await ctx.files.write('list.txt', WANTED);
    }
    return { changedFiles: ['list.txt'] };
  },
});

const ownWay = defineRule({
  id: 'quiet/own-way',
  severity: 'low',
  category: 'consistency',
  description: 'own.txt exists.',
  inputs: ['own.txt'],
  async check(ctx) {
    if (await ctx.files.exists('own.txt')) return [];
    return [{ file: 'own.txt', message: 'own.txt is missing.', fixable: 'deterministic' }];
  },
  async fix(ctx) {
    await writeFile(path.join(ctx.root, 'own.txt'), 'made without the context\n');
    return { changedFiles: [] };
  },
});

export const pack = definePack({
  name: 'quiet',
  rules: [
    { rule: twice, skillFile },
    { rule: ownWay, skillFile },
  ],
});
