import { execFile } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { fixtureSet } from '@firescale/sdk/testing';
import { Ajv } from 'ajv';
import { afterEach, describe, expect, it } from 'vitest';
import type { CheckReport } from './check.js';
import { EXIT_OK, EXIT_USAGE, EXIT_VIOLATIONS } from './cli.js';
import { ENFORCED_CONFIG, FAKE_ENV, runCli } from './test-helpers.js';

const execFileAsync = promisify(execFile);
const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

const BIN = fileURLToPath(new URL('../dist/bin.js', import.meta.url));
const SCHEMA = new URL('../schemas/check-output.schema.json', import.meta.url);

/** A repo with two tracked env files, one nested, plus an allowed example. */
function failingRepo(config?: string) {
  return fixtures.create({
    files: {
      '.env': FAKE_ENV,
      '.env.example': 'EXAMPLE=\n',
      'apps/web/.env.local': FAKE_ENV,
      'README.md': '# fixture\n',
      ...(config === undefined ? {} : { 'firescale.config.json': config }),
    },
  });
}

/** Replaces the firescale version so snapshots survive a version bump. */
function stableVersion(json: string): string {
  return json.replace(/"version": "\d+\.\d+\.\d+"/, '"version": "<version>"');
}

describe('fscale check', () => {
  it('finds a tracked .env and exits 1 when the rule is enforced', async () => {
    const repo = await failingRepo(ENFORCED_CONFIG);
    const result = await runCli(['check'], repo.root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    expect(result.stderr).toBe('');
    expect(result.stdout).toMatchSnapshot();
  });

  it('exits 1 from the built binary too', async () => {
    const repo = await failingRepo(ENFORCED_CONFIG);
    const outcome = await execFileAsync(process.execPath, [BIN, 'check'], { cwd: repo.root }).then(
      () => ({ code: 0, stdout: '' }),
      (error: { code?: number; stdout?: string }) => ({
        code: error.code,
        stdout: error.stdout ?? '',
      }),
    );
    expect(outcome.code).toBe(EXIT_VIOLATIONS);
    expect(outcome.stdout).toContain('.env');
    expect(outcome.stdout).toContain('FAILED');
  });

  it('reports but exits 0 by default, because the default status is evaluated', async () => {
    const repo = await failingRepo();
    const result = await runCli(['check'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toMatchSnapshot();
  });

  it('exits 0 with a clean repo', async () => {
    const repo = await fixtures.create({
      files: { '.env.example': 'EXAMPLE=\n', 'firescale.config.json': ENFORCED_CONFIG },
    });
    const result = await runCli(['check'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toBe('OK: 1 rule checked, no violations.\n');
  });

  it('skips rules that are off', async () => {
    const repo = await failingRepo(JSON.stringify({ rules: { 'core/no-committed-env': 'off' } }));
    const result = await runCli(['check'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toBe('OK: 0 rules checked, no violations.\n');
  });

  it('passes rule options from the config to the rule', async () => {
    const config = {
      rules: {
        'core/no-committed-env': {
          status: 'enforced',
          options: { allow: ['.env', '.env.local', '.env.example'] },
        },
      },
    };
    const repo = await failingRepo(JSON.stringify(config));
    const result = await runCli(['check'], repo.root);
    expect(result.code).toBe(EXIT_OK);
  });

  it('finds the repo root from a subfolder and honours --cwd', async () => {
    const repo = await failingRepo(ENFORCED_CONFIG);
    const fromSubfolder = await runCli(['check'], path.join(repo.root, 'apps', 'web'));
    expect(fromSubfolder.code).toBe(EXIT_VIOLATIONS);
    const withFlag = await runCli(['check', '--cwd', repo.root]);
    expect(withFlag.code).toBe(EXIT_VIOLATIONS);
    expect(withFlag.stdout).toBe(fromSubfolder.stdout);
  });

  it('loads firescale.config.ts from the built binary', async () => {
    const repo = await fixtures.create({
      files: {
        '.env': FAKE_ENV,
        'firescale.config.ts': [
          "const status: 'enforced' | 'evaluated' = 'enforced';",
          "export default { rules: { 'core/no-committed-env': { status } } };",
          '',
        ].join('\n'),
      },
    });
    const outcome = await execFileAsync(process.execPath, [BIN, 'check', '--json'], {
      cwd: repo.root,
    }).then(
      () => ({ code: 0, stdout: '' }),
      (error: { code?: number; stdout?: string }) => ({
        code: error.code,
        stdout: error.stdout ?? '',
      }),
    );
    expect(outcome.code).toBe(EXIT_VIOLATIONS);
    const report = JSON.parse(outcome.stdout) as CheckReport;
    expect(report.rules[0]?.status).toBe('enforced');
  });
});

describe('fscale check --rules', () => {
  it('runs rules matching a glob', async () => {
    const repo = await failingRepo(ENFORCED_CONFIG);
    for (const glob of ['core/no-committed-env', 'core/*', '*/no-*', '**', 'x/y, core/*']) {
      const result = await runCli(['check', '--rules', glob], repo.root);
      expect(result.code, glob).toBe(EXIT_VIOLATIONS);
    }
  });

  it('fails with the available rules when nothing matches', async () => {
    const repo = await failingRepo(ENFORCED_CONFIG);
    const result = await runCli(['check', '--rules', 'nope/*'], repo.root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain('matches no rule');
    expect(result.stderr).toContain('core/no-committed-env');
  });
});

describe('fscale check --json', () => {
  it('matches the snapshot', async () => {
    const repo = await failingRepo(ENFORCED_CONFIG);
    const result = await runCli(['check', '--json'], repo.root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    expect(stableVersion(result.stdout)).toMatchSnapshot();
  });

  it('matches the snapshot for a clean repo', async () => {
    const repo = await fixtures.create({ files: { 'README.md': '# fixture\n' } });
    const result = await runCli(['check', '--json'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(stableVersion(result.stdout)).toMatchSnapshot();
  });

  it('validates against the documented JSON schema', async () => {
    const schema = JSON.parse(await readFile(SCHEMA, 'utf8')) as object;
    const validate = new Ajv({ allErrors: true }).compile(schema);
    const failing = await failingRepo(ENFORCED_CONFIG);
    const evaluated = await failingRepo();
    const clean = await fixtures.create({ files: { 'README.md': '# fixture\n' } });
    for (const repo of [failing, evaluated, clean]) {
      const result = await runCli(['check', '--json'], repo.root);
      const report: unknown = JSON.parse(result.stdout);
      expect(validate(report), JSON.stringify(validate.errors)).toBe(true);
    }
  });

  it('the schema rejects a report with a missing or unknown field', async () => {
    const schema = JSON.parse(await readFile(SCHEMA, 'utf8')) as object;
    const validate = new Ajv({ allErrors: true }).compile(schema);
    const repo = await failingRepo(ENFORCED_CONFIG);
    const report = JSON.parse((await runCli(['check', '--json'], repo.root)).stdout) as Record<
      string,
      unknown
    >;
    expect(validate({ ...report, extra: true })).toBe(false);
    const withoutOk = { ...report };
    delete withoutOk['ok'];
    expect(validate(withoutOk)).toBe(false);
  });

  it('only uses forward slashes in paths', async () => {
    const repo = await failingRepo(ENFORCED_CONFIG);
    const result = await runCli(['check', '--json'], repo.root);
    const report = JSON.parse(result.stdout) as CheckReport;
    expect(report.diagnostics.map((d) => d.file)).toEqual(['.env', 'apps/web/.env.local']);
    expect(result.stdout).not.toContain('\\\\');
  });
});
