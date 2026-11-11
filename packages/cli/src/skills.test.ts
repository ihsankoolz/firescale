import { readdir, readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { validateSkill } from '@firescale/sdk';
import { describe, expect, it } from 'vitest';
import { EXIT_OK, runCli } from './test-helpers.js';

const REPO_ROOT = fileURLToPath(new URL('../../../', import.meta.url));
const RULES_DIR = path.join(REPO_ROOT, 'crates', 'rules', 'src');

interface BuiltInSkill {
  ruleId: string;
  folder: string;
  file: string;
}

async function findBuiltInSkills(): Promise<BuiltInSkill[]> {
  const entries = await readdir(RULES_DIR, { recursive: true, withFileTypes: true });
  return entries
    .filter((entry) => entry.isFile() && entry.name === 'SKILL.md')
    .map((entry) => {
      const folder = entry.parentPath;
      // crates/rules/src/<pack>/<rule-id>/SKILL.md
      const ruleId = path.relative(RULES_DIR, folder).split(path.sep).join('/');
      return { ruleId, folder, file: path.join(folder, entry.name) };
    })
    .sort((a, b) => a.ruleId.localeCompare(b.ruleId));
}

const skills = await findBuiltInSkills();

describe('built-in rule skills', () => {
  it('finds at least one SKILL.md', () => {
    expect(skills.length).toBeGreaterThan(0);
    for (const skill of skills) expect(skill.ruleId).toMatch(/^[^/]+\/[^/]+$/);
  });

  it.each(skills)('$ruleId has a valid SKILL.md and the standard rule files', async (skill) => {
    expect(validateSkill(await readFile(skill.file, 'utf8'), { ruleId: skill.ruleId })).toEqual([]);
    const files = await readdir(skill.folder);
    expect(files).toContain('mod.rs');
    expect(files).toContain('tests.rs');
  });

  it.each(skills)('$ruleId is a registered rule that `fscale explain` can find', async (skill) => {
    const result = await runCli(['explain', skill.ruleId]);
    expect(result.stderr).toBe('');
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toContain(skill.ruleId);
  });
});

describe('other SKILL.md files', () => {
  it('the SDK template is valid', async () => {
    const template = path.join(REPO_ROOT, 'packages', 'sdk', 'templates', 'SKILL.md');
    expect(validateSkill(await readFile(template, 'utf8'))).toEqual([]);
  });

  it('the sample pack skill is valid', async () => {
    const sample = path.join(
      REPO_ROOT,
      'packages',
      'cli',
      'test-fixtures',
      'custom-pack',
      'rules',
      'no-todo-file',
      'SKILL.md',
    );
    expect(
      validateSkill(await readFile(sample, 'utf8'), { ruleId: 'sample/no-todo-file' }),
    ).toEqual([]);
  });
});
