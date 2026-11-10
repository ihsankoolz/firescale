import { describe, expect, it, vi } from 'vitest';
import { defineRule } from './define.js';
import { FirescaleError } from './errors.js';
import { checkRule, fixRule } from './run.js';
import type { Finding, Rule } from './types.js';

const ROOT = process.cwd();

function makeRule(findings: Finding[], extra: Partial<Rule> = {}): Rule {
  return defineRule({
    id: 'demo/rule',
    severity: 'low',
    category: 'consistency',
    description: 'Demo.',
    inputs: [],
    check: () => findings,
    ...extra,
  });
}

function finding(overrides: Partial<Finding> = {}): Finding {
  return { message: 'm', file: 'a.txt', fixable: 'manual', ...overrides };
}

describe('checkRule', () => {
  it('stamps the rule id', async () => {
    const result = await checkRule(makeRule([finding()]), { root: ROOT });
    expect(result[0]?.ruleId).toBe('demo/rule');
  });

  it('normalises backslash paths to forward slashes', async () => {
    const result = await checkRule(makeRule([finding({ file: 'apps\\web\\x.ts' })]), {
      root: ROOT,
    });
    expect(result[0]?.file).toBe('apps/web/x.ts');
  });

  it('sorts by file, then line, then message', async () => {
    const rule = makeRule([
      finding({ file: 'b.txt', message: 'z' }),
      finding({ file: 'a.txt', message: 'y', range: { startLine: 5, endLine: 5 } }),
      finding({ file: 'a.txt', message: 'x', range: { startLine: 2, endLine: 3 } }),
      finding({ file: 'a.txt', message: 'a' }),
      finding({ file: 'b.txt', message: 'b' }),
    ]);
    const result = await checkRule(rule, { root: ROOT });
    expect(result.map((d) => `${d.file}:${d.range?.startLine ?? 0}:${d.message}`)).toEqual([
      'a.txt:0:a',
      'a.txt:2:x',
      'a.txt:5:y',
      'b.txt:0:b',
      'b.txt:0:z',
    ]);
  });

  it('keeps project and range on the diagnostic', async () => {
    const result = await checkRule(
      makeRule([finding({ project: 'web', range: { startLine: 1, endLine: 2 } })]),
      { root: ROOT },
    );
    expect(result[0]).toMatchObject({ project: 'web', range: { startLine: 1, endLine: 2 } });
  });

  it('rejects an invalid fixable', async () => {
    const rule = makeRule([finding({ fixable: 'auto' as Finding['fixable'] })]);
    await expect(checkRule(rule, { root: ROOT })).rejects.toThrow(FirescaleError);
  });

  it.each([
    { startLine: 0, endLine: 1 },
    { startLine: 3, endLine: 2 },
    { startLine: 1.5, endLine: 2 },
  ])('rejects bad range %j', async (range) => {
    const rule = makeRule([finding({ range })]);
    await expect(checkRule(rule, { root: ROOT })).rejects.toThrow(/range/);
  });

  it('rejects missing message or file', async () => {
    await expect(checkRule(makeRule([finding({ message: '' })]), { root: ROOT })).rejects.toThrow(
      /message/,
    );
    await expect(checkRule(makeRule([finding({ file: '' })]), { root: ROOT })).rejects.toThrow(
      /file/,
    );
  });

  it('rejects a file outside the repo root', async () => {
    const rule = makeRule([finding({ file: '../outside' })]);
    await expect(checkRule(rule, { root: ROOT })).rejects.toThrow(FirescaleError);
  });
});

describe('fixRule', () => {
  it('returns no changes when the rule has no fix', async () => {
    const rule = makeRule([finding({ fixable: 'deterministic' })]);
    expect(await fixRule(rule, { root: ROOT })).toEqual({ changedFiles: [] });
  });

  it('does not call fix when nothing is deterministic', async () => {
    const fix = vi.fn(() => ({ changedFiles: ['a.txt'] }));
    const rule = makeRule([finding({ fixable: 'agent' }), finding({ fixable: 'manual' })], { fix });
    expect(await fixRule(rule, { root: ROOT })).toEqual({ changedFiles: [] });
    expect(fix).not.toHaveBeenCalled();
  });

  it('passes only deterministic diagnostics and normalises, dedupes and sorts changed files', async () => {
    const fix = vi.fn<NonNullable<Rule['fix']>>(() => ({
      changedFiles: ['b\\x.txt', 'a.txt', 'b/x.txt'],
      notes: ['n'],
    }));
    const rule = makeRule(
      [finding({ fixable: 'deterministic' }), finding({ file: 'z.txt', fixable: 'manual' })],
      { fix },
    );
    const result = await fixRule(rule, { root: ROOT });
    expect(result).toEqual({ changedFiles: ['a.txt', 'b/x.txt'], notes: ['n'] });
    expect(fix.mock.calls[0]?.[1]).toHaveLength(1);
  });
});
