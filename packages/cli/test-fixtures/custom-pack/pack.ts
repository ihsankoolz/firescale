// A sample custom rule pack, used by the bridge tests. It is written exactly
// as a team would write its own pack with the firescale SDK.
import { definePack } from '@firescale/sdk';
import noTodoFile from './rules/no-todo-file/rule.ts';

export const pack = definePack({
  name: 'sample',
  rules: [
    {
      rule: noTodoFile,
      skillFile: new URL('./rules/no-todo-file/SKILL.md', import.meta.url),
    },
  ],
});
