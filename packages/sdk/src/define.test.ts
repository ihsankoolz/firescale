import { describe, expect, it } from 'vitest';
import { defineConfig, definePack, defineRule } from './define.js';
import { FirescaleError } from './errors.js';
import type { PackEntry, Rule } from './types.js';

function makeRule(overrides: Partial<Rule> = {}): Rule {
  return {
    id: 'demo/some-rule',
    severity: 'high',
    category: 'security',
    description: 'Something must be true.',
    inputs: ['**/*.ts'],
    check: () => [],
    ...overrides,
  };
}

function entry(rule: Rule): PackEntry {
  return { rule, skillFile: new URL('file:///tmp/SKILL.md') };
}

describe('defineRule', () => {
  it('accepts a valid rule and freezes it', () => {
    const rule = defineRule(makeRule());
    expect(rule.id).toBe('demo/some-rule');
    expect(Object.isFrozen(rule)).toBe(true);
    expect(Object.isFrozen(rule.inputs)).toBe(true);
  });

  it('accepts a rule with a fix', () => {
    expect(defineRule(makeRule({ fix: () => ({ changedFiles: [] }) })).fix).toBeTypeOf('function');
  });

  it.each(['nopack', 'Demo/rule', 'demo/Rule', 'demo/', '/rule', 'demo/a/b', 'demo/a_b', ''])(
    'rejects bad id %j',
    (id) => {
      expect(() => defineRule(makeRule({ id }))).toThrow(FirescaleError);
    },
  );

  it('rejects a bad severity', () => {
    expect(() => defineRule(makeRule({ severity: 'critical' as Rule['severity'] }))).toThrow(
      FirescaleError,
    );
  });

  it('rejects a bad category', () => {
    expect(() => defineRule(makeRule({ category: 'style' as Rule['category'] }))).toThrow(
      FirescaleError,
    );
  });

  it('rejects an empty description', () => {
    expect(() => defineRule(makeRule({ description: '   ' }))).toThrow(FirescaleError);
  });

  it('rejects bad inputs', () => {
    expect(() => defineRule(makeRule({ inputs: [1] as unknown as string[] }))).toThrow(
      FirescaleError,
    );
  });

  it('rejects a non-function check', () => {
    expect(() => defineRule(makeRule({ check: 'nope' as unknown as Rule['check'] }))).toThrow(
      FirescaleError,
    );
  });

  it('rejects a non-function fix', () => {
    expect(() =>
      defineRule(makeRule({ fix: 'nope' as unknown as NonNullable<Rule['fix']> })),
    ).toThrow(FirescaleError);
  });
});

describe('definePack', () => {
  it('accepts matching rules and freezes the pack', () => {
    const pack = definePack({ name: 'demo', rules: [entry(defineRule(makeRule()))] });
    expect(Object.isFrozen(pack)).toBe(true);
    expect(Object.isFrozen(pack.rules)).toBe(true);
    expect(pack.rules).toHaveLength(1);
  });

  it('rejects a bad pack name', () => {
    expect(() => definePack({ name: 'Bad Name', rules: [] })).toThrow(FirescaleError);
  });

  it('rejects a rule from another pack', () => {
    expect(() =>
      definePack({ name: 'demo', rules: [entry(makeRule({ id: 'other/some-rule' }))] }),
    ).toThrow(/another pack/);
  });

  it('rejects duplicate ids', () => {
    expect(() =>
      definePack({ name: 'demo', rules: [entry(makeRule()), entry(makeRule())] }),
    ).toThrow(/listed twice/);
  });
});

describe('defineConfig', () => {
  it('returns the config unchanged', () => {
    const config = { packs: ['core'] };
    expect(defineConfig(config)).toBe(config);
  });
});
