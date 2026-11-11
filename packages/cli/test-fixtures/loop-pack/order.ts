// `order/a-derived` sorts first by id, but its fix needs the file that
// `order/z-base` creates, and says so with `after`.
import { definePack } from '@firescale/sdk';
import { fileRule, skillFile } from './shared.ts';

const base = fileRule({
  id: 'order/z-base',
  file: 'base.txt',
  wanted: async () => 'base\n',
});

const derived = fileRule({
  id: 'order/a-derived',
  after: ['order/z-base'],
  file: 'derived.txt',
  wanted: async (read) => {
    const source = await read('base.txt');
    return source === null ? null : `derived from: ${source}`;
  },
});

export const pack = definePack({
  name: 'order',
  rules: [
    { rule: derived, skillFile },
    { rule: base, skillFile },
  ],
});
