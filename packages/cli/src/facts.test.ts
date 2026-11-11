import { readFile } from 'node:fs/promises';
import { fixtureSet } from '@firescale/sdk/testing';
import { Ajv } from 'ajv';
import { afterEach, describe, expect, it } from 'vitest';
import { EXIT_OK, EXIT_USAGE, REPO, runCli } from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

const SCHEMA = new URL('../schemas/facts-output.schema.json', import.meta.url);

interface Facts {
  projects: { name: string; dependsOn: string[] }[];
}

async function validate(json: string): Promise<Facts> {
  const schema: object = JSON.parse(await readFile(SCHEMA, 'utf8'));
  const check = new Ajv({ strict: true }).compile(schema);
  const data: unknown = JSON.parse(json);
  const ok = check(data);
  expect(check.errors ?? []).toEqual([]);
  expect(ok).toBe(true);
  return data as Facts;
}

const MONOREPO = {
  'package.json': JSON.stringify({
    name: 'root',
    packageManager: 'pnpm@9.1.0',
    scripts: { build: 'x', test: 'y' },
  }),
  'pnpm-workspace.yaml': "packages:\n  - 'packages/*'\n",
  'packages/a/package.json': JSON.stringify({ name: '@x/a' }),
  'packages/b/package.json': JSON.stringify({
    name: '@x/b',
    dependencies: { '@x/a': 'workspace:*' },
  }),
};

describe('fscale facts', () => {
  it('prints schema-valid JSON for a pnpm monorepo', async () => {
    const repo = await fixtures.create({ files: MONOREPO, track: 'no-git' });
    const result = await runCli(['facts', '--json'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stderr).toBe('');
    await validate(result.stdout);
    expect(result.stdout).toMatchSnapshot();
  });

  it('works on an empty folder', async () => {
    const repo = await fixtures.create({ files: {}, track: 'no-git' });
    const result = await runCli(['facts', '--json'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    const facts = await validate(result.stdout);
    expect(facts.projects).toEqual([]);
  });

  it('prints human output', async () => {
    const repo = await fixtures.create({ files: MONOREPO, track: 'no-git' });
    const result = await runCli(['facts'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toMatchSnapshot();
  });

  it('takes no arguments', async () => {
    const repo = await fixtures.create({ files: {}, track: 'no-git' });
    const result = await runCli(['facts', 'extra'], repo.root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain('facts takes no arguments');
  });

  it('describes this repo', async () => {
    const result = await runCli(['facts', '--json'], REPO);
    expect(result.code).toBe(EXIT_OK);
    const facts = await validate(result.stdout);
    const rust = facts.projects.filter((p) =>
      ['firescale-cli', 'firescale-core', 'firescale-rules'].includes(p.name),
    );
    expect(rust.map((p) => p.name)).toEqual(['firescale-cli', 'firescale-core', 'firescale-rules']);
    const cli = rust.find((p) => p.name === 'firescale-cli');
    expect(cli?.dependsOn).toEqual(['firescale-core', 'firescale-rules']);
  });
});
