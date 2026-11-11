import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { validateSkill } from '@firescale/sdk';
import { describe, expect, it } from 'vitest';
import { corePack } from './index.js';

const rulesDir = fileURLToPath(new URL('./rules/', import.meta.url));
const templateFile = fileURLToPath(new URL('../../sdk/templates/SKILL.md', import.meta.url));

describe('SKILL.md files', () => {
  it.each(corePack.rules.map((entry) => [entry.rule.id, entry] as const))(
    '%s has every required section',
    (_id, entry) => {
      const text = readFileSync(fileURLToPath(entry.skillFile), 'utf8');
      expect(validateSkill(text, { ruleId: entry.rule.id })).toEqual([]);
    },
  );

  it('the SDK template is itself valid', () => {
    expect(validateSkill(readFileSync(templateFile, 'utf8'))).toEqual([]);
  });

  it('every rule folder has rule.ts and SKILL.md and is registered in corePack', () => {
    const folders = readdirSync(rulesDir).filter((name) =>
      statSync(path.join(rulesDir, name)).isDirectory(),
    );
    expect(folders.length).toBeGreaterThan(0);

    const registered = new Set(
      corePack.rules.map((entry) => {
        const parts = fileURLToPath(entry.skillFile).split(path.sep);
        return parts[parts.length - 2];
      }),
    );
    for (const folder of folders) {
      expect(existsSync(path.join(rulesDir, folder, 'rule.ts')), `${folder}/rule.ts`).toBe(true);
      expect(existsSync(path.join(rulesDir, folder, 'SKILL.md')), `${folder}/SKILL.md`).toBe(true);
      expect(registered.has(folder), `${folder} is registered in corePack`).toBe(true);
    }
    expect(registered.size).toBe(folders.length);
  });

  it('every registered skill file exists', () => {
    for (const entry of corePack.rules) {
      expect(existsSync(fileURLToPath(entry.skillFile))).toBe(true);
    }
  });
});
