// `fscale add` and `fscale schema` (M14, ADR 0028).
import { readFile, readdir } from 'node:fs/promises';
import path from 'node:path';
import { validateSkill } from '@firescale/sdk';
import { fixtureSet } from '@firescale/sdk/testing';
import { Ajv } from 'ajv';
import { afterEach, describe, expect, it } from 'vitest';
import { EXIT_OK, EXIT_USAGE, runCli } from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

interface Diagnostic {
  ruleId: string;
  file: string;
}

/** Every file under `root` but `.git`, by path with forward slashes. */
async function snapshot(root: string): Promise<Map<string, string>> {
  const files = new Map<string, string>();
  async function walk(dir: string): Promise<void> {
    for (const entry of await readdir(dir, { withFileTypes: true })) {
      if (entry.name === '.git') continue;
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) await walk(full);
      else
        files.set(
          path.relative(root, full).split(path.sep).join('/'),
          await readFile(full, 'utf8'),
        );
    }
  }
  await walk(root);
  return files;
}

async function diagnostics(root: string): Promise<Diagnostic[]> {
  const result = await runCli(['check', '--json', '--no-cache'], root);
  return (JSON.parse(result.stdout) as { diagnostics: Diagnostic[] }).diagnostics;
}

const INPUTS: Record<string, Record<string, unknown>> = {
  package: { name: '@acme/utils', description: 'Small helpers.' },
  rule: { id: 'team/no-todo-file', description: 'No TODO.md files are tracked.', severity: 'low' },
};

describe('fscale schema', () => {
  it('lists the generators', async () => {
    const result = await runCli(['schema'], process.cwd());
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toMatchSnapshot();
  });

  it.each(Object.keys(INPUTS))(
    'prints a valid JSON Schema for %s that accepts the example input',
    async (name) => {
      const result = await runCli(['schema', name], process.cwd());
      expect(result.code).toBe(EXIT_OK);
      const schema = JSON.parse(result.stdout) as object;
      const ajv = new Ajv({ allErrors: true, strict: true });
      const validate = ajv.compile(schema);
      expect(validate(INPUTS[name])).toBe(true);
      expect(validate({ ...INPUTS[name], nope: 1 })).toBe(false);
      expect(result.stdout).toMatchSnapshot();
    },
  );

  it('names the generators when one does not exist', async () => {
    const result = await runCli(['schema', 'app'], process.cwd());
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain('Unknown generator "app". Generators: package, rule.');
  });
});

describe('fscale add', () => {
  it.each([
    ['package', { description: 'x' }, 'input.name is required'],
    ['package', { name: 'Bad Name' }, 'input.name must match'],
    ['package', { name: 'ok', folder: '../outside' }, 'input.folder must match'],
    ['package', { name: 'ok', extra: true }, 'input.extra is not a known field'],
    [
      'rule',
      { id: 'team/x', description: 'd', severity: 'huge' },
      'input.severity must be one of blocker, high, medium, low',
    ],
    ['rule', { id: 'no-pack', description: 'd' }, 'input.id must match'],
    ['rule', { id: 'team/x', description: 42 }, 'input.description must be a string'],
  ] as const)(
    'rejects %s input %j with an error naming the field',
    async (name, input, message) => {
      const repo = await fixtures.create({ files: { 'README.md': '# x\n' } });
      const before = await snapshot(repo.root);
      const result = await runCli(['add', name, '--json', JSON.stringify(input)], repo.root);
      expect(result.code).toBe(EXIT_USAGE);
      expect(result.stderr).toContain(message);
      expect(result.stderr).toContain(`fscale schema ${name}`);
      expect(await snapshot(repo.root)).toEqual(before);
    },
  );

  it('rejects input that is not JSON, or no input at all', async () => {
    const repo = await fixtures.create();
    const bad = await runCli(['add', 'package', '--json', '{name: x}'], repo.root);
    expect(bad.code).toBe(EXIT_USAGE);
    expect(bad.stderr).toContain('The --json input is not valid JSON');
    const none = await runCli(['add', 'package'], repo.root);
    expect(none.code).toBe(EXIT_USAGE);
    expect(none.stderr).toContain("fscale add <generator> --json '<input>'");
  });

  it.each(Object.keys(INPUTS))('%s: --dry-run prints a diff and changes nothing', async (name) => {
    const repo = await fixtures.create({ files: { 'README.md': '# x\n' } });
    const before = await snapshot(repo.root);
    const tracked = await repo.trackedFiles();
    const result = await runCli(
      ['add', name, '--json', JSON.stringify(INPUTS[name]), '--dry-run'],
      repo.root,
    );
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toContain('--- /dev/null');
    expect(result.stdout).toContain('Nothing was written.');
    expect(await snapshot(repo.root)).toEqual(before);
    expect(await repo.trackedFiles()).toEqual(tracked);
  });

  it.each(Object.keys(INPUTS))(
    '%s: writes the files (snapshot), and they add no violation to fscale check',
    async (name) => {
      const repo = await fixtures.create({
        files: {
          'README.md': '# x\n\n## Install\n\n```sh\nnpm install\n```\n\n## Usage\n\nRun it.\n',
          'package.json': '{ "name": "x", "workspaces": ["packages/*"] }\n',
        },
      });
      const before = await snapshot(repo.root);
      const found = await diagnostics(repo.root);
      const result = await runCli(['add', name, '--json', JSON.stringify(INPUTS[name])], repo.root);
      expect(result.code).toBe(EXIT_OK);
      expect(result.stdout).toMatchSnapshot('output');
      const after = await snapshot(repo.root);
      const created = [...after.keys()].filter((file) => !before.has(file)).sort();
      for (const file of created) expect(after.get(file)).toMatchSnapshot(file);
      // Nothing new to report for the files it wrote.
      const now = await diagnostics(repo.root);
      expect(now.filter((d) => created.includes(d.file))).toEqual([]);
      expect(now.length).toBeLessThanOrEqual(found.length);
      if (name === 'rule') {
        const skill = after.get('firescale-rules/rules/no-todo-file/SKILL.md') ?? '';
        expect(validateSkill(skill, { ruleId: 'team/no-todo-file' })).toEqual([]);
      }
    },
  );

  it('never writes over a file, and writes nothing when one is in the way', async () => {
    const repo = await fixtures.create({ files: { 'packages/utils/README.md': 'mine\n' } });
    const before = await snapshot(repo.root);
    const result = await runCli(['add', 'package', '--json', '{"name":"utils"}'], repo.root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain(
      'would write over packages/utils/README.md, so nothing was written',
    );
    expect(await snapshot(repo.root)).toEqual(before);
  });
});

describe('the fscale add skill', () => {
  it('is present for each agent after fscale sync', async () => {
    const repo = await fixtures.create({
      files: { 'firescale.config.json': JSON.stringify({ packs: ['core', 'agent-ready'] }) },
    });
    expect((await runCli(['sync'], repo.root)).code).toBe(EXIT_OK);
    for (const dir of ['.claude/skills', '.agents/skills', '.cursor/skills']) {
      const text = await readFile(path.join(repo.root, dir, 'firescale-add', 'SKILL.md'), 'utf8');
      expect(text).toMatch(/^---\nname: firescale-add\ndescription: /);
      expect(text).toContain('fscale add <generator> --json');
      expect(text).toContain('fscale schema <generator>');
    }
  });
});
