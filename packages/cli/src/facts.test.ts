import { readFile } from 'node:fs/promises';
import { fixtureSet } from '@firescale/sdk/testing';
import { Ajv } from 'ajv';
import { afterEach, describe, expect, it } from 'vitest';
import { courseProjectFiles } from './course-fixture.js';
import { EXIT_OK, EXIT_USAGE, REPO, runCli } from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

const SCHEMA = new URL('../schemas/facts-output.schema.json', import.meta.url);

interface Facts {
  packageManagers: string[];
  languages: string[];
  tools: string[];
  projects: {
    name: string;
    root: string;
    kind: string;
    source: string;
    languages: string[];
    packageManager: string | null;
    tools: string[];
    commands: Record<string, string>;
    dependsOn: string[];
  }[];
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

  describe('in a repo with projects in subfolders', () => {
    it('reports each Python service and the frontend as its own project', async () => {
      const repo = await fixtures.create({ files: courseProjectFiles(), track: 'no-git' });
      const result = await runCli(['facts', '--json'], repo.root);
      expect(result.code).toBe(EXIT_OK);
      expect(result.stderr).toBe('');
      const facts = await validate(result.stdout);
      expect(result.stdout).toMatchSnapshot();
      expect(result.stdout).not.toContain('\\');

      expect(facts.packageManagers).toEqual(['npm', 'pip']);
      expect(facts.languages).toEqual(['javascript', 'python', 'typescript']);
      expect(facts.tools).toEqual(['docker-compose', 'github-actions']);
      const python = facts.projects.filter((p) => p.kind === 'python');
      expect(python.map((p) => p.root)).toEqual([
        'backend/audit_service',
        'backend/auth_service',
        'backend/ledger_service',
        'backend/market_service',
        'backend/realtime_service',
      ]);
      for (const service of python) {
        expect(service.languages).toEqual(['python']);
        expect(service.packageManager).toBe('pip');
        expect(service.tools).toContain('pytest');
        expect(service.commands['test']).toBe('python -m pytest');
        // Ruff only where it is configured.
        expect(service.tools.includes('ruff')).toBe(service.name === 'auth_service');
      }
      const node = facts.projects.filter((p) => p.kind === 'node');
      expect(node).toHaveLength(1);
      expect(node[0]).toMatchObject({
        name: 'frontend',
        root: 'frontend',
        source: 'discovered',
        languages: ['javascript', 'typescript'],
        packageManager: 'npm',
        tools: ['knip', 'oxlint', 'playwright', 'vite'],
        commands: {
          build: 'npm run build',
          lint: 'npx --no oxlint',
          typecheck: 'npx --no tsc --noEmit',
        },
      });
      // Nothing from node_modules, dist, test-results, caches or ignored folders.
      expect(facts.projects).toHaveLength(6);
    });

    it('prints the projects for people', async () => {
      const repo = await fixtures.create({ files: courseProjectFiles(), track: 'no-git' });
      const result = await runCli(['facts'], repo.root);
      expect(result.code).toBe(EXIT_OK);
      expect(result.stdout).toMatchSnapshot();
    });
  });
});
