// Two rules that want different content in the same file. Each fix undoes the other.
import { definePack } from '@firescale/sdk';
import { fileRule, skillFile } from './shared.ts';

export const pack = definePack({
  name: 'fight',
  rules: [
    {
      rule: fileRule({ id: 'fight/spaces', file: 'style.txt', wanted: async () => 'spaces\n' }),
      skillFile,
    },
    {
      rule: fileRule({ id: 'fight/tabs', file: 'style.txt', wanted: async () => 'tabs\n' }),
      skillFile,
    },
  ],
});
