// A rule whose `after` names a rule that does not exist.
import { definePack } from '@firescale/sdk';
import { fileRule, skillFile } from './shared.ts';

export const pack = definePack({
  name: 'unknown',
  rules: [
    {
      rule: fileRule({
        id: 'unknown/typo',
        after: ['core/gitignore-basic'],
        file: 'one.txt',
        wanted: async () => '1\n',
      }),
      skillFile,
    },
  ],
});
