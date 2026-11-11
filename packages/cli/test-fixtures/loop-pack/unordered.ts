// The same two rules as order.ts, without the `after` declaration: the loop
// still converges, in a second pass.
import { definePack } from '@firescale/sdk';
import { fileRule, skillFile } from './shared.ts';

const base = fileRule({ id: 'unordered/z-base', file: 'base.txt', wanted: async () => 'base\n' });

const derived = fileRule({
  id: 'unordered/a-derived',
  file: 'derived.txt',
  wanted: async (read) => {
    const source = await read('base.txt');
    return source === null ? null : `derived from: ${source}`;
  },
});

export const pack = definePack({
  name: 'unordered',
  rules: [
    { rule: derived, skillFile },
    { rule: base, skillFile },
  ],
});
