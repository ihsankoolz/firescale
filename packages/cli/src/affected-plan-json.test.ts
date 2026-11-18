import { readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { runGit } from '@firescale/sdk';
import { fixtureSet, type FixtureRepo } from '@firescale/sdk/testing';
import { Ajv } from 'ajv';
import { afterEach, describe, expect, it } from 'vitest';
import { EXIT_OK, EXIT_USAGE, runCli } from './test-helpers.js';

// `fscale verify --affected --dry-run --json`: the affected plan as JSON,
// with a typed reason for each unit (ADR 0079).

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

interface Reason {
  type: string;
  text: string;
  files?: string[];
  on?: string;
  sources?: string[];
  file?: string;
}

interface Plan {
  schemaVersion: number;
  base: string | null;
  changedFiles: number;
  everything: Reason | null;
  units: {
    project?: string;
    kind?: string;
    affected: boolean;
    reason: Reason;
    steps: { name: string; command: string }[];
  }[];
  notes: string[];
  warnings: string[];
}

const pkg = (name: string, deps: Record<string, string> = {}) =>
  `${JSON.stringify(
    { name, version: '1.0.0', scripts: { test: 'node -e 0' }, dependencies: deps },
    null,
    2,
  )}\n`;

/** Three services with no project at the root: b uses a, c stands alone. */
async function services(): Promise<FixtureRepo> {
  const repo = await fixtures.create({
    files: {
      'README.md': '# Services\n',
      'a/package.json': pkg('a'),
      'a/index.js': 'export const a = 1;\n',
      'b/package.json': pkg('b', { a: 'file:../a' }),
      'b/index.js': 'export const b = 1;\n',
      'c/package.json': pkg('c'),
      'c/index.js': 'export const c = 1;\n',
    },
  });
  const result = await runGit(repo.root, [
    '-c',
    'user.name=fixture',
    '-c',
    'user.email=fixture@example.invalid',
    '-c',
    'commit.gpgsign=false',
    'commit',
    '--quiet',
    '--no-verify',
    '-m',
    'initial',
  ]);
  expect(result.code).toBe(0);
  return repo;
}

async function plan(repo: FixtureRepo): Promise<Plan> {
  const result = await runCli(['verify', '--affected', '--dry-run', '--json'], repo.root);
  expect(result.stderr).toBe('');
  expect(result.code).toBe(EXIT_OK);
  return JSON.parse(result.stdout) as Plan;
}

async function schema(): Promise<object> {
  return JSON.parse(
    await readFile(
      new URL('../../../docs/schemas/affected-plan.schema.json', import.meta.url),
      'utf8',
    ),
  ) as object;
}

describe('verify --affected --dry-run --json', () => {
  it('gives each unit a typed reason: a changed file, a dependency, or not affected', async () => {
    const repo = await services();
    await writeFile(path.join(repo.root, 'a', 'index.js'), 'export const a = 2;\n');
    const found = await plan(repo);
    expect(found.schemaVersion).toBe(1);
    expect(found.base).toBeNull();
    expect(found.changedFiles).toBe(1);
    expect(found.everything).toBeNull();
    const unit = (project: string) => found.units.find((unit) => unit.project === project);
    expect(unit('a')).toMatchObject({
      kind: 'node',
      affected: true,
      reason: { type: 'changed-file', files: ['a/index.js'], text: 'changed: a/index.js' },
      steps: [{ name: 'test', command: 'npm run test' }],
    });
    expect(unit('b')).toMatchObject({
      affected: true,
      reason: { type: 'depends-on', on: 'a' },
    });
    expect(unit('b')?.reason.sources).toContain('package.json');
    expect(unit('b')?.reason.text).toContain('depends on a');
    expect(unit('c')).toMatchObject({
      affected: false,
      reason: { type: 'not-affected', text: 'no change reaches this project' },
    });
    expect(found.units.map((unit) => unit.project)).toEqual(['a', 'b', 'c']);
  });

  it('says with a type why everything runs: a CI file, or a file of no project', async () => {
    const repo = await services();
    await repo.write('.github/workflows/ci.yml', 'on: [push]\n');
    const ci = await plan(repo);
    expect(ci.everything).toMatchObject({ type: 'ci-file', file: '.github/workflows/ci.yml' });
    expect(ci.units.every((unit) => unit.affected && unit.reason.type === 'ci-file')).toBe(true);

    const other = await services();
    await other.write('scripts/seed.js', 'console.log(1);\n');
    const unowned = await plan(other);
    expect(unowned.everything).toMatchObject({ type: 'unowned-file', file: 'scripts/seed.js' });
  });

  it('runs nothing for a document, and says so in a note', async () => {
    const repo = await services();
    await writeFile(path.join(repo.root, 'README.md'), '# Changed\n');
    const found = await plan(repo);
    expect(found.everything).toBeNull();
    expect(found.units.every((unit) => !unit.affected)).toBe(true);
    expect(found.notes.join('\n')).toContain('README.md');
  });

  it('matches its schema, and the schema refuses a reason with no type', async () => {
    const valid = new Ajv({ strict: true }).compile(await schema());
    const repo = await services();
    await writeFile(path.join(repo.root, 'a', 'index.js'), 'export const a = 2;\n');
    const found = await plan(repo);
    expect(valid(found), JSON.stringify(valid.errors)).toBe(true);
    await repo.write('.github/workflows/ci.yml', 'on: [push]\n');
    const everything = await plan(repo);
    expect(valid(everything), JSON.stringify(valid.errors)).toBe(true);
    const broken = structuredClone(found) as unknown as { units: { reason: object }[] };
    broken.units[0]!.reason = { text: 'changed' };
    expect(valid(broken)).toBe(false);
  });

  it('prints nothing but the JSON and runs no command', async () => {
    const repo = await services();
    await writeFile(path.join(repo.root, 'a', 'index.js'), 'export const a = 2;\n');
    const result = await runCli(['verify', '--affected', '--dry-run', '--json'], repo.root);
    expect(result.stdout.trimStart().startsWith('{')).toBe(true);
    expect(() => JSON.parse(result.stdout) as unknown).not.toThrow();
  });

  it('is still refused without --affected', async () => {
    const repo = await services();
    const result = await runCli(['verify', '--dry-run', '--json'], repo.root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain('--affected');
  });
});
