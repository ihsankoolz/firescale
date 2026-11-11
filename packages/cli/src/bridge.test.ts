import { cp, mkdir, readFile, symlink, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { fixtureSet, type FixtureRepo } from '@firescale/sdk/testing';
import { Ajv } from 'ajv';
import { afterEach, describe, expect, it } from 'vitest';
import type { CheckReport } from './report.js';
import {
  ENFORCED_CONFIG,
  EXIT_OK,
  EXIT_USAGE,
  EXIT_VIOLATIONS,
  FAKE_ENV,
  runCli,
} from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

const REPO_ROOT = fileURLToPath(new URL('../../../', import.meta.url));
const CUSTOM_PACK = fileURLToPath(new URL('../test-fixtures/custom-pack/', import.meta.url));
const SCHEMA = new URL('../schemas/check-output.schema.json', import.meta.url);
const SAMPLE = 'sample/no-todo-file';
const ENV_RULE = 'core/no-committed-env';
const NO_NODE = { FSCALE_NODE: path.join(REPO_ROOT, 'no-such-folder', 'node') };

/** Makes `@firescale/sdk` and `firescale` resolvable from the fixture. */
async function linkPackages(repo: FixtureRepo): Promise<void> {
  const modules = path.join(repo.root, 'node_modules');
  await mkdir(path.join(modules, '@firescale'), { recursive: true });
  // 'junction' works on Windows without admin rights; other platforms ignore the type.
  await symlink(
    path.join(REPO_ROOT, 'packages', 'sdk'),
    path.join(modules, '@firescale', 'sdk'),
    'junction',
  );
  await symlink(
    path.join(REPO_ROOT, 'packages', 'cli'),
    path.join(modules, 'firescale'),
    'junction',
  );
}

interface SampleOptions {
  config?: unknown;
  files?: Record<string, string>;
  configFile?: string;
}

/** A fixture repo that has the sample pack at tools/rules and linked packages. */
async function sampleRepo(options: SampleOptions = {}): Promise<FixtureRepo> {
  const config = options.config ?? {
    packs: ['core', './tools/rules/pack.ts'],
    rules: { [SAMPLE]: 'enforced' },
  };
  const repo = await fixtures.create({
    files: {
      'README.md': '# fixture\n',
      'TODO.md': '- item\n',
      [options.configFile ?? 'firescale.config.json']:
        typeof config === 'string' ? config : `${JSON.stringify(config, null, 2)}\n`,
      ...options.files,
    },
  });
  await cp(CUSTOM_PACK, path.join(repo.root, 'tools', 'rules'), { recursive: true });
  await linkPackages(repo);
  return repo;
}

function parseReport(stdout: string): CheckReport {
  return JSON.parse(stdout) as CheckReport;
}

async function validator() {
  const schema = JSON.parse(await readFile(SCHEMA, 'utf8')) as object;
  return new Ajv({ allErrors: true }).compile(schema);
}

describe('custom pack through the Node bridge', () => {
  it('runs a custom rule next to the built-in ones', async () => {
    const validate = await validator();
    const repo = await sampleRepo();
    const result = await runCli(['check', '--json'], repo.root);
    expect(result.code).toBe(EXIT_VIOLATIONS);

    // The rule calls console.log: stdout stays pure JSON, the text goes to stderr.
    const report = parseReport(result.stdout);
    expect(validate(report), JSON.stringify(validate.errors)).toBe(true);
    expect(result.stderr).toContain('sample/no-todo-file: checking');
    expect(result.stdout).not.toContain('checking');

    expect(report.rules.map((r) => [r.id, r.status])).toEqual([
      [ENV_RULE, 'evaluated'],
      [SAMPLE, 'enforced'],
    ]);
    const diagnostic = report.diagnostics.find((d) => d.file === 'TODO.md');
    expect(diagnostic).toMatchObject({ file: 'TODO.md', fixable: 'manual', status: 'enforced' });
  });

  it('passes rule options to the custom rule', async () => {
    const repo = await sampleRepo({
      config: {
        packs: ['core', './tools/rules/pack.ts'],
        rules: { [SAMPLE]: { status: 'enforced', options: { names: ['NOTES.md'] } } },
      },
      files: { 'NOTES.md': '- note\n' },
    });
    const result = await runCli(['check', '--json'], repo.root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const files = parseReport(result.stdout).diagnostics.map((d) => d.file);
    expect(files).toContain('NOTES.md');
    expect(files).not.toContain('TODO.md');
  });

  it('reports a rule error for invalid options with exit 2', async () => {
    const repo = await sampleRepo({
      config: {
        packs: ['core', './tools/rules/pack.ts'],
        rules: { [SAMPLE]: { status: 'enforced', options: { names: 'TODO.md' } } },
      },
    });
    const result = await runCli(['check'], repo.root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain('option "names" must be a list of file names');
  });

  it('explains a custom rule with its metadata and SKILL.md', async () => {
    const repo = await sampleRepo();
    const result = await runCli(['explain', SAMPLE], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toContain(SAMPLE);
    expect(result.stdout).toContain('Status:      enforced');
    expect(result.stdout).toContain('## What this rule enforces and why');
    expect(result.stdout).toContain('No `TODO.md` file is tracked by git.');
    expect(result.stdout).toContain('## Manual fix');
  });

  it('loads firescale.config.ts through Node', async () => {
    const repo = await sampleRepo({
      configFile: 'firescale.config.ts',
      config: [
        "import { defineConfig } from 'firescale';",
        '',
        "const status: 'enforced' | 'evaluated' = 'enforced';",
        'export default defineConfig({',
        "  packs: ['core', './tools/rules/pack.ts'],",
        `  rules: { '${SAMPLE}': status, '${ENV_RULE}': 'off' },`,
        '});',
        '',
      ].join('\n'),
    });
    const result = await runCli(['check', '--json'], repo.root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const report = parseReport(result.stdout);
    expect(report.rules.map((r) => [r.id, r.status])).toEqual([[SAMPLE, 'enforced']]);
  });
});

describe('no Node process for built-in rules', () => {
  it('runs built-in rules with a JSON config when Node cannot be started', async () => {
    const repo = await fixtures.create({
      files: { '.env': FAKE_ENV, 'firescale.config.json': ENFORCED_CONFIG },
    });
    const result = await runCli(['check', '--json'], repo.root, NO_NODE);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    expect(result.stderr).toBe('');
    const report = parseReport(result.stdout);
    expect(report.rules.map((r) => [r.id, r.status])).toEqual([[ENV_RULE, 'enforced']]);
    expect(report.diagnostics.map((d) => d.file)).toEqual(['.env']);
  });

  it('runs with no config at all when Node cannot be started', async () => {
    const repo = await fixtures.create({ files: { '.env': FAKE_ENV } });
    const result = await runCli(['check'], repo.root, NO_NODE);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toContain('.env');
    const explain = await runCli(['explain', ENV_RULE], repo.root, NO_NODE);
    expect(explain.code).toBe(EXIT_OK);
  });

  it('fails clearly when a custom pack needs Node and it cannot be started', async () => {
    const repo = await sampleRepo();
    const result = await runCli(['check'], repo.root, NO_NODE);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain('Could not start Node');
  });

  it('fails clearly when firescale.config.ts needs Node and it cannot be started', async () => {
    const repo = await fixtures.create({
      files: { 'firescale.config.ts': 'export default {};\n' },
    });
    const result = await runCli(['check'], repo.root, NO_NODE);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain('Could not start Node');
  });
});

describe('custom pack errors', () => {
  it('names a pack path that does not exist', async () => {
    const repo = await fixtures.create({
      files: { 'firescale.config.json': JSON.stringify({ packs: ['core', './tools/missing.ts'] }) },
    });
    await linkPackages(repo);
    const result = await runCli(['check'], repo.root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain('./tools/missing.ts does not exist');
  });

  it('rejects a custom pack named like a built-in pack', async () => {
    const repo = await fixtures.create({
      files: {
        'firescale.config.json': JSON.stringify({ packs: ['core', './tools/core-pack.ts'] }),
      },
    });
    await linkPackages(repo);
    await mkdir(path.join(repo.root, 'tools'), { recursive: true });
    await writeFile(
      path.join(repo.root, 'tools', 'core-pack.ts'),
      "import { definePack } from '@firescale/sdk';\nexport default definePack({ name: 'core', rules: [] });\n",
    );
    const result = await runCli(['check'], repo.root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toMatch(/^fscale: /);
    expect(result.stderr).toContain('is named "core", which is a built-in pack');
  });
});
