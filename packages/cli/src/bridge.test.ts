import {
  chmod,
  copyFile,
  cp,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  stat,
  symlink,
  writeFile,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
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
  NODE_STUB,
  runCli,
} from './test-helpers.js';

const fixtures = fixtureSet();
const tempDirs: string[] = [];
afterEach(async () => {
  await fixtures.cleanup();
  for (const dir of tempDirs.splice(0)) {
    await rm(dir, { recursive: true, force: true, maxRetries: 5 });
  }
});

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

interface StubNode {
  marker: string;
  env: Record<string, string | undefined>;
}

/** A temp dir holding a copy of the node stand-in named `node`, first on PATH. */
async function stubNode(): Promise<StubNode> {
  const dir = await mkdtemp(path.join(tmpdir(), 'firescale-stub-'));
  tempDirs.push(dir);
  const names = process.platform === 'win32' ? ['node.exe', 'node'] : ['node'];
  for (const name of names) {
    const target = path.join(dir, name);
    await copyFile(NODE_STUB, target);
    await chmod(target, 0o755);
  }
  // On Windows the key may be `Path`: set the key that already exists.
  const pathKey = Object.keys(process.env).find((key) => key.toLowerCase() === 'path') ?? 'PATH';
  const marker = path.join(dir, 'marker.txt');
  return {
    marker,
    env: {
      [pathKey]: `${dir}${path.delimiter}${process.env[pathKey] ?? ''}`,
      FSCALE_NODE: undefined,
      FSCALE_STUB_MARKER: marker,
    },
  };
}

async function exists(file: string): Promise<boolean> {
  return stat(file).then(
    () => true,
    () => false,
  );
}

describe('no Node process for built-in rules', () => {
  it('runs check, check --json and explain with a JSON config without starting node', async () => {
    const stub = await stubNode();
    const repo = await fixtures.create({
      files: { '.env': FAKE_ENV, 'firescale.config.json': ENFORCED_CONFIG },
    });

    const json = await runCli(['check', '--json'], repo.root, stub.env);
    expect(json.code).toBe(EXIT_VIOLATIONS);
    expect(json.stderr).toBe('');
    const report = parseReport(json.stdout);
    expect(report.rules.map((r) => [r.id, r.status])).toEqual([[ENV_RULE, 'enforced']]);
    expect(report.diagnostics.map((d) => d.file)).toEqual(['.env']);

    const text = await runCli(['check'], repo.root, stub.env);
    expect(text.code).toBe(EXIT_VIOLATIONS);
    expect(text.stdout).toContain('.env');

    const explain = await runCli(['explain', ENV_RULE], repo.root, stub.env);
    expect(explain.code).toBe(EXIT_OK);
    expect(explain.stdout).toContain(ENV_RULE);

    expect(await exists(stub.marker)).toBe(false);
  });

  it('runs check, check --json and explain with no config without starting node', async () => {
    const stub = await stubNode();
    const repo = await fixtures.create({ files: { '.env': FAKE_ENV } });

    const text = await runCli(['check'], repo.root, stub.env);
    expect(text.code).toBe(EXIT_OK);
    expect(text.stdout).toContain('.env');

    const json = await runCli(['check', '--json'], repo.root, stub.env);
    expect(json.code).toBe(EXIT_OK);
    const report = parseReport(json.stdout);
    expect(report.rules.map((r) => r.id)).toContain(ENV_RULE);

    const explain = await runCli(['explain', ENV_RULE], repo.root, stub.env);
    expect(explain.code).toBe(EXIT_OK);
    expect(explain.stdout).toContain(ENV_RULE);

    expect(await exists(stub.marker)).toBe(false);
  });

  it('control: a custom pack does start node, and the stub intercepts it', async () => {
    const stub = await stubNode();
    const repo = await sampleRepo();
    const result = await runCli(['check'], repo.root, stub.env);
    expect(result.code).toBe(EXIT_USAGE);
    expect(await exists(stub.marker)).toBe(true);
  });

  it('control: firescale.config.ts starts node', async () => {
    const stub = await stubNode();
    const repo = await fixtures.create({
      files: { 'firescale.config.ts': 'export default {};\n' },
    });
    const result = await runCli(['check'], repo.root, stub.env);
    expect(result.code).toBe(EXIT_USAGE);
    expect(await exists(stub.marker)).toBe(true);
  });

  it('honours FSCALE_NODE for a .ts config', async () => {
    const stub = await stubNode();
    const repo = await fixtures.create({
      files: { 'firescale.config.ts': 'export default {};\n' },
    });
    // PATH is left alone: only FSCALE_NODE can lead to the stub.
    const env = { FSCALE_NODE: NODE_STUB, FSCALE_STUB_MARKER: stub.marker };
    const result = await runCli(['check'], repo.root, env);
    expect(result.code).toBe(EXIT_USAGE);
    expect(await exists(stub.marker)).toBe(true);
  });
});

describe('when Node cannot be started', () => {
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
  it('keeps a rule error readable when the pack loads its own copy of the SDK', async () => {
    // A real install has the SDK as files in node_modules, not a link to this checkout,
    // so the rule's FirescaleError class is a different object from the host's.
    const repo = await fixtures.create({
      files: {
        'TODO.md': '- item\n',
        'firescale.config.json': JSON.stringify({
          packs: ['./tools/rules/pack.ts'],
          rules: { [SAMPLE]: { options: { names: 'TODO.md' } } },
        }),
      },
    });
    await cp(CUSTOM_PACK, path.join(repo.root, 'tools', 'rules'), { recursive: true });
    const sdkCopy = path.join(repo.root, 'node_modules', '@firescale', 'sdk');
    await mkdir(sdkCopy, { recursive: true });
    const sdk = path.join(REPO_ROOT, 'packages', 'sdk');
    await cp(path.join(sdk, 'package.json'), path.join(sdkCopy, 'package.json'));
    await cp(path.join(sdk, 'dist'), path.join(sdkCopy, 'dist'), { recursive: true });

    const result = await runCli(['check'], repo.root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain('option "names" must be a list of file names');
    expect(result.stderr).not.toContain('crashed');
  });

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
