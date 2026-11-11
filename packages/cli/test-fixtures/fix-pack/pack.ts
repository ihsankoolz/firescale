// A sample custom pack whose rule has a deterministic fix. Used by the tests
// that run a custom rule's fix through the bridge.
import { definePack } from '@firescale/sdk';
import noTrackedLog from './rules/no-tracked-log/rule.ts';

export const pack = definePack({
  name: 'fixable',
  rules: [
    {
      rule: noTrackedLog,
      skillFile: new URL('./rules/no-tracked-log/SKILL.md', import.meta.url),
    },
  ],
});
