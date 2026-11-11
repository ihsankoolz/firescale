import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { fixtureSet } from '@firescale/sdk/testing';
import { Ajv } from 'ajv';
import { afterEach, describe, expect, it } from 'vitest';
import type { CheckReport } from './report.js';
import {
  ENFORCED_CONFIG,
  onlyEnvRule,
  onlyEnvRules,
  EXIT_OK,
  EXIT_USAGE,
  EXIT_VIOLATIONS,
  FAKE_ENV,
  runCli,
} from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

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
  return json.replace(/"version": "[^"]+"/, '"version": "<version>"');
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
    const result = await runCli(['check'], repo.root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    expect(result.stdout).toContain('.env');
    expect(result.stdout).toContain('FAILED');
  });

  it('reports but exits 0 by default, because the default status is evaluated', async () => {
    const repo = await failingRepo(onlyEnvRule('evaluated'));
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
    const repo = await failingRepo(onlyEnvRule('off'));
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
          `export default { rules: ${JSON.stringify(onlyEnvRules('off', { 'core/no-committed-env': '__STATUS__' })).replace('"__STATUS__"', '{ status }')} };`,
          '',
        ].join('\n'),
      },
    });
    const result = await runCli(['check', '--json'], repo.root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const report = JSON.parse(result.stdout) as CheckReport;
    expect(report.rules[0]?.status).toBe('enforced');
  });
});

describe('fscale check errors', () => {
  it('names a --cwd folder that does not exist', async () => {
    const repo = await fixtures.create();
    const missing = path.join(repo.root, 'nope');
    const result = await runCli(['check', '--cwd', missing]);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stdout).toBe('');
    expect(result.stderr).toBe(
      `fscale: Folder "${missing}" does not exist. Check the --cwd value.\n`,
    );
  });
});

describe('fscale check --cwd', () => {
  it('resolves ".." before looking for the repo root', async () => {
    // A config in a subfolder must not be picked up when --cwd points back out of it.
    const repo = await fixtures.create({
      files: {
        '.env': FAKE_ENV,
        'firescale.config.json': ENFORCED_CONFIG,
        'x/y/firescale.config.json': JSON.stringify({ packs: [] }),
      },
    });
    const inside = await runCli(['check'], path.join(repo.root, 'x', 'y'));
    expect(inside.stdout).toBe('OK: 0 rules checked, no violations.\n');
    const backOut = await runCli(
      ['check', '--cwd', path.join('..', '..')],
      path.join(repo.root, 'x', 'y'),
    );
    expect(backOut.code).toBe(EXIT_VIOLATIONS);
    expect(backOut.stdout).toContain('.env');
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

describe('fscale check --rules on an off rule', () => {
  const OFF = onlyEnvRule('off');

  it('skips the rule without --rules', async () => {
    const repo = await failingRepo(OFF);
    const result = await runCli(['check'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toBe('OK: 0 rules checked, no violations.\n');
  });

  it('evaluates it as report-only when named exactly', async () => {
    const repo = await failingRepo(OFF);
    const result = await runCli(['check', '--rules', 'core/no-committed-env'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toContain('.env');
    expect(result.stdout).toContain('evaluated');
    expect(result.stderr).toContain('note: core/no-committed-env is off');
  });

  it('reports it as evaluated in JSON, with the note on stderr', async () => {
    const schema = JSON.parse(await readFile(SCHEMA, 'utf8')) as object;
    const validate = new Ajv({ allErrors: true }).compile(schema);
    const repo = await failingRepo(OFF);
    const result = await runCli(['check', '--json', '--rules', 'core/no-committed-env'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    const report = JSON.parse(result.stdout) as CheckReport;
    expect(validate(report), JSON.stringify(validate.errors)).toBe(true);
    expect(report.rules.map((r) => [r.id, r.status])).toEqual([
      ['core/no-committed-env', 'evaluated'],
    ]);
    expect(result.stderr).toContain('note: core/no-committed-env is off');
  });

  it('does not let a glob switch an off rule on', async () => {
    const repo = await failingRepo(OFF);
    const result = await runCli(['check', '--rules', 'core/*'], repo.root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain('only matches rules that are off');
  });
});

describe('core/no-committed-env name variants', () => {
  it('matches the name case-insensitively and ignores look-alikes', async () => {
    // Never two names that differ only by case in one folder: macOS and Windows would merge them.
    const repo = await fixtures.create({
      files: {
        '.ENV': FAKE_ENV,
        'a/.Env.Local': FAKE_ENV,
        'sub/.ENV.PRODUCTION': FAKE_ENV,
        'b/.ENV.EXAMPLE': FAKE_ENV,
        'c/.envrc': FAKE_ENV,
        'firescale.config.json': ENFORCED_CONFIG,
      },
    });
    const result = await runCli(['check', '--json'], repo.root);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const report = JSON.parse(result.stdout) as CheckReport;
    expect(report.diagnostics.map((d) => d.file).sort()).toEqual([
      '.ENV',
      'a/.Env.Local',
      'sub/.ENV.PRODUCTION',
    ]);
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
    const repo = await fixtures.create({
      files: { 'README.md': '# fixture\n', 'firescale.config.json': onlyEnvRule('evaluated') },
    });
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
