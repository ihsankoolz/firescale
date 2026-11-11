import path from 'node:path';
import { FirescaleError } from '@firescale/sdk';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { EXIT_USAGE } from './cli.js';
import { findRoot, loadConfig, resolveConfig } from './config.js';
import { runCli } from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

const RULE = 'core/no-committed-env';

describe('resolveConfig', () => {
  it('defaults to the core pack with every rule evaluated', () => {
    const config = resolveConfig({}, null);
    expect(config.packs).toEqual(['core']);
    expect(config.protectedPaths).toEqual([]);
    expect(config.rules.map((r) => [r.rule.id, r.status, r.options])).toEqual([
      [RULE, 'evaluated', {}],
    ]);
  });

  it('accepts a status string or a { status, options } object', () => {
    expect(resolveConfig({ rules: { [RULE]: 'enforced' } }, null).rules[0]?.status).toBe(
      'enforced',
    );
    const withOptions = resolveConfig(
      { rules: { [RULE]: { status: 'off', options: { allow: ['.env.example'] } } } },
      null,
    );
    expect(withOptions.rules[0]?.status).toBe('off');
    expect(withOptions.rules[0]?.options).toEqual({ allow: ['.env.example'] });
    expect(resolveConfig({ rules: { [RULE]: { options: {} } } }, null).rules[0]?.status).toBe(
      'evaluated',
    );
  });

  it('keeps protectedPaths and allows $schema', () => {
    const config = resolveConfig({ $schema: 'x', protectedPaths: ['infra/**'] }, null);
    expect(config.protectedPaths).toEqual(['infra/**']);
  });

  it('turns off every rule of a pack that is not listed', () => {
    expect(resolveConfig({ packs: [] }, null).rules).toEqual([]);
  });

  it.each([
    ['a non-object', [], 'must contain an object'],
    ['an unknown top-level key', { rulez: {} }, 'unknown key "rulez"'],
    ['a bad packs value', { packs: 'core' }, '"packs" must be a list'],
    ['an unknown pack', { packs: ['nope'] }, 'Available packs: core'],
    ['an unknown rule', { rules: { 'core/nope': 'off' } }, `Rules in the enabled packs: ${RULE}`],
    ['a bad status', { rules: { [RULE]: 'on' } }, 'must be "off", "evaluated", "enforced"'],
    ['a bad status in an object', { rules: { [RULE]: { status: 'on' } } }, 'invalid status'],
    ['an unknown rule key', { rules: { [RULE]: { level: 'x' } } }, 'unknown key "level"'],
    ['bad options', { rules: { [RULE]: { options: [] } } }, 'options must be an object'],
    ['bad protectedPaths', { protectedPaths: 'x' }, '"protectedPaths" must be a list'],
  ])('rejects %s with a message that says what to do', (_name, raw, message) => {
    expect(() => resolveConfig(raw, 'firescale.config.json')).toThrow(FirescaleError);
    expect(() => resolveConfig(raw, 'firescale.config.json')).toThrow(message);
  });
});

describe('loadConfig', () => {
  it('uses defaults when there is no config file', async () => {
    const repo = await fixtures.create();
    const config = await loadConfig(repo.root);
    expect(config.file).toBeNull();
    expect(config.rules[0]?.status).toBe('evaluated');
  });

  it('loads firescale.config.json, with or without a BOM and with CRLF', async () => {
    const repo = await fixtures.create({
      files: {
        'firescale.config.json': `\uFEFF{\r\n  "rules": { "${RULE}": "enforced" }\r\n}\r\n`,
      },
    });
    const config = await loadConfig(repo.root);
    expect(config.file).toBe('firescale.config.json');
    expect(config.rules[0]?.status).toBe('enforced');
  });

  it('rejects invalid JSON', async () => {
    const repo = await fixtures.create({ files: { 'firescale.config.json': '{ nope' } });
    await expect(loadConfig(repo.root)).rejects.toThrow('is not valid JSON');
  });

  it('rejects having both config files', async () => {
    const repo = await fixtures.create({
      files: { 'firescale.config.json': '{}', 'firescale.config.ts': 'export default {};\n' },
    });
    await expect(loadConfig(repo.root)).rejects.toThrow('Keep one config file');
  });

  it('surfaces config errors through the CLI with exit code 2', async () => {
    const repo = await fixtures.create({ files: { 'firescale.config.json': '{"rulez":{}}' } });
    const result = await runCli(['check'], repo.root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toBe(
      'fscale: firescale.config.json: unknown key "rulez". Allowed keys: packs, rules, protectedPaths.\n',
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

describe('findRoot', () => {
  it('does not look for a config above the git root', async () => {
    const outer = await fixtures.create({
      files: { 'firescale.config.json': '{}', 'inner/file.txt': 'x\n' },
      track: 'no-git',
    });
    const { runGit } = await import('@firescale/sdk');
    const inner = path.join(outer.root, 'inner');
    await runGit(inner, ['init', '--quiet']);
    expect(await findRoot(inner)).toBe(inner);
  });

  it('prefers the nearest config file, then the git root, then the folder itself', async () => {
    const repo = await fixtures.create({ files: { 'a/b/file.txt': 'x\n' } });
    const nested = path.join(repo.root, 'a', 'b');
    expect(await findRoot(nested)).toBe(repo.root);

    await repo.write('a/firescale.config.json', '{}');
    expect(await findRoot(nested)).toBe(path.join(repo.root, 'a'));
  });
});
