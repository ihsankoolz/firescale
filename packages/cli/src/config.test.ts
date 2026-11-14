import path from 'node:path';
import { runGit } from '@firescale/sdk';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import type { CheckReport } from './report.js';
import {
  ALL_CORE_RULES,
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

const RULE = 'core/no-committed-env';

/** A repo with a tracked .env and the given config file content. */
function repoWith(config: unknown | undefined) {
  return fixtures.create({
    files: {
      '.env': FAKE_ENV,
      ...(config === undefined
        ? {}
        : {
            'firescale.config.json': typeof config === 'string' ? config : JSON.stringify(config),
          }),
    },
  });
}

function statusOf(report: CheckReport, id: string): string | undefined {
  return report.rules.find((r) => r.id === id)?.status;
}

async function checkJson(root: string): Promise<{ code: number; report: CheckReport }> {
  const result = await runCli(['check', '--json'], root);
  return { code: result.code, report: JSON.parse(result.stdout) as CheckReport };
}

/** A config that runs only core/no-committed-env, with this value for it. */
const onlyEnv = (value: unknown) => ({ rules: onlyEnvRules('off', { [RULE]: value }) });

describe('config values', () => {
  it('defaults to the core pack with every rule evaluated', async () => {
    const repo = await repoWith(undefined);
    const { code, report } = await checkJson(repo.root);
    expect(code).toBe(EXIT_OK);
    expect(report.rules.map((r) => [r.id, r.status])).toEqual(
      ALL_CORE_RULES.map((id) => [id, 'evaluated']),
    );
  });

  it('accepts a status string or a { status, options } object', async () => {
    const enforced = await checkJson((await repoWith(onlyEnv('enforced'))).root);
    expect(enforced.code).toBe(EXIT_VIOLATIONS);
    expect(enforced.report.rules[0]?.status).toBe('enforced');

    const allowed = await repoWith(onlyEnv({ status: 'enforced', options: { allow: ['.env'] } }));
    const withOptions = await checkJson(allowed.root);
    expect(withOptions.code).toBe(EXIT_OK);
    expect(withOptions.report.rules[0]?.status).toBe('enforced');
    expect(withOptions.report.diagnostics).toEqual([]);

    const off = await repoWith(onlyEnv({ status: 'off', options: { allow: ['.env.example'] } }));
    const offResult = await runCli(['check'], off.root);
    expect(offResult.code).toBe(EXIT_OK);
    expect(offResult.stdout).toBe('OK: 0 rules checked, no violations.\n');

    const optionsOnly = await checkJson((await repoWith(onlyEnv({ options: {} }))).root);
    expect(optionsOnly.report.rules[0]?.status).toBe('evaluated');
  });

  it('keeps protectedPaths and allows $schema', async () => {
    const repo = await repoWith({ $schema: 'x', protectedPaths: ['infra/**'] });
    const result = await runCli(['check'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stderr).toBe('');
  });

  it('turns off every rule of a pack that is not listed', async () => {
    const { code, report } = await checkJson((await repoWith({ packs: [] })).root);
    expect(code).toBe(EXIT_OK);
    expect(report.rules).toEqual([]);
  });

  it.each([
    ['a non-object', [], 'must contain an object'],
    ['an unknown top-level key', { rulez: {} }, 'unknown key "rulez"'],
    ['a bad packs value', { packs: 'core' }, '"packs" must be a list'],
    ['an unknown pack', { packs: ['nope'] }, 'Available packs: core'],
    [
      'an unknown rule',
      { rules: { 'core/nope': 'off' } },
      `Rules in the enabled packs: ${ALL_CORE_RULES.join(', ')}`,
    ],
    ['a bad status', { rules: { [RULE]: 'on' } }, 'must be "off", "evaluated", "enforced"'],
    ['a bad status in an object', { rules: { [RULE]: { status: 'on' } } }, 'invalid status'],
    ['an unknown rule key', { rules: { [RULE]: { level: 'x' } } }, 'unknown key "level"'],
    ['bad options', { rules: { [RULE]: { options: [] } } }, 'options must be an object'],
    ['bad protectedPaths', { protectedPaths: 'x' }, '"protectedPaths" must be a list'],
  ])('rejects %s with a message that says what to do', async (_name, raw, message) => {
    const repo = await repoWith(raw);
    const result = await runCli(['check'], repo.root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stdout).toBe('');
    expect(result.stderr).toMatch(/^fscale: /);
    expect(result.stderr).toContain('firescale.config.json');
    expect(result.stderr).toContain(message);
  });
});

describe('loading the config file', () => {
  it('uses defaults when there is no config file', async () => {
    const repo = await repoWith(undefined);
    const result = await runCli(['check'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toContain(RULE);
  });

  it('loads firescale.config.json, with or without a BOM and with CRLF', async () => {
    const plain = JSON.stringify(onlyEnv('enforced'), null, 2).replace(/\n/g, '\r\n') + '\r\n';
    for (const content of [plain, `\uFEFF${plain}`]) {
      const repo = await repoWith(content);
      const { code, report } = await checkJson(repo.root);
      expect(code).toBe(EXIT_VIOLATIONS);
      expect(report.rules[0]?.status).toBe('enforced');
    }
  });

  it('rejects invalid JSON', async () => {
    const repo = await repoWith('{ nope');
    const result = await runCli(['check'], repo.root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain('is not valid JSON');
  });

  it('rejects having both config files', async () => {
    const repo = await fixtures.create({
      files: { 'firescale.config.json': '{}', 'firescale.config.ts': 'export default {};\n' },
    });
    const result = await runCli(['check'], repo.root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain('Keep one config file');
  });

  it('surfaces config errors through the CLI with exit code 2', async () => {
    const repo = await fixtures.create({ files: { 'firescale.config.json': '{"rulez":{}}' } });
    const result = await runCli(['check'], repo.root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toBe(
      'fscale: firescale.config.json: unknown key "rulez". Allowed keys: packs, rules, protectedPaths, verify, agent.\n',
    );
  });
});

describe('loadConfig error paths', () => {
  it('reports an unreadable config without a stack trace', async () => {
    const repo = await fixtures.create({ files: { 'firescale.config.json/keep.txt': 'x\n' } });
    const result = await runCli(['check'], repo.root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toMatch(/^fscale: Could not read firescale\.config\.json \(/);
    expect(result.stderr).not.toContain('    at ');
  });
});

describe('finding the repo root', () => {
  it('does not look for a config above the git root', async () => {
    const outer = await fixtures.create({
      files: { 'firescale.config.json': ENFORCED_CONFIG, 'inner/.env': FAKE_ENV },
      track: 'no-git',
    });
    const inner = path.join(outer.root, 'inner');
    await runGit(inner, ['init', '--quiet']);
    await runGit(inner, ['add', '--force', '--', '.env']);
    const { code, report } = await checkJson(inner);
    expect(code).toBe(EXIT_OK);
    expect(statusOf(report, RULE)).toBe('evaluated');
    expect(report.diagnostics.filter((d) => d.ruleId === RULE).map((d) => d.file)).toEqual([
      '.env',
    ]);
  });

  it('prefers the nearest config file, then the git root', async () => {
    const repo = await fixtures.create({
      files: { 'a/b/file.txt': 'x\n', '.env': FAKE_ENV },
    });
    const nested = path.join(repo.root, 'a', 'b');

    // No config anywhere: the git root is the root, so its .env is found from a subfolder.
    const noConfig = await checkJson(nested);
    expect(noConfig.report.diagnostics.filter((d) => d.ruleId === RULE).map((d) => d.file)).toEqual(
      ['.env'],
    );
    expect(statusOf(noConfig.report, RULE)).toBe('evaluated');

    // A config at the git root applies from a subfolder.
    await repo.write('firescale.config.json', ENFORCED_CONFIG);
    const rootConfig = await checkJson(nested);
    expect(rootConfig.code).toBe(EXIT_VIOLATIONS);

    // A nearer config wins over the one at the git root.
    await repo.write('a/firescale.config.json', onlyEnvRule('off'));
    const nearest = await checkJson(nested);
    expect(nearest.code).toBe(EXIT_OK);
    expect(nearest.report.rules).toEqual([]);
    expect((await checkJson(repo.root)).code).toBe(EXIT_VIOLATIONS);
  });
});
