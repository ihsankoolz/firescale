// Two rules that each must run after the other.
import { definePack } from '@firescale/sdk';
import { fileRule, skillFile } from './shared.ts';

export const pack = definePack({
  name: 'cycle',
  rules: [
    {
      rule: fileRule({
        id: 'cycle/one',
        after: ['cycle/two'],
        file: 'one.txt',
        wanted: async () => '1\n',
      }),
      skillFile,
    },
    {
      rule: fileRule({
        id: 'cycle/two',
        after: ['cycle/one'],
        file: 'two.txt',
        wanted: async () => '2\n',
      }),
      skillFile,
    },
  ],
});
