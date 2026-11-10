import { definePack, type PackEntry, type Rule } from '@firescale/sdk';
import noCommittedEnv from './rules/no-committed-env/rule.js';

/** Pairs a rule with the SKILL.md in its folder. */
function entry(folder: string, rule: Rule): PackEntry {
  return { rule, skillFile: new URL(`./rules/${folder}/SKILL.md`, import.meta.url) };
}

/** The core rule pack. */
export const corePack = definePack({
  name: 'core',
  rules: [entry('no-committed-env', noCommittedEnv)],
});
