import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import type { CheckReport } from './report.js';
import { ALL_AGENT_READY_RULES, ALL_CORE_RULES, EXIT_OK, REPO, runCli } from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

describe('the core pack', () => {
  it('lists exactly the eight core rules, all evaluated, in an empty repo with no config', async () => {
    const repo = await fixtures.create();
    const result = await runCli(['check', '--json'], repo.root);
    const report = JSON.parse(result.stdout) as CheckReport;
    expect(report.rules.map((r) => [r.id, r.status])).toEqual(
      ALL_CORE_RULES.map((id) => [id, 'evaluated']),
    );
    expect(report.rules).toHaveLength(8);
  });

  it.each(ALL_CORE_RULES)('explains %s', async (id) => {
    const repo = await fixtures.create();
    const result = await runCli(['explain', id], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toContain(id);
  });

  it('passes on the firescale repo itself, with the agent-ready pack on too (dogfooding)', async () => {
    const result = await runCli(['check'], REPO);
    expect(result.stdout).toBe('OK: 12 rules checked, no violations.\n');
    expect(result.code).toBe(EXIT_OK);
  });
});

describe('the agent-ready pack', () => {
  it('is off until the config lists it', async () => {
    const repo = await fixtures.create({
      files: { 'firescale.config.json': JSON.stringify({ packs: ['core', 'agent-ready'] }) },
    });
    const result = await runCli(['check', '--json'], repo.root);
    const report = JSON.parse(result.stdout) as CheckReport;
    expect(report.rules.map((r) => r.id)).toEqual([...ALL_AGENT_READY_RULES, ...ALL_CORE_RULES]);
    expect(report.rules.every((r) => r.status === 'evaluated')).toBe(true);
    expect(result.code).toBe(EXIT_OK);
  });

  it('reports an empty repo as not agent-ready, and passes once the hook is installed', async () => {
    const repo = await fixtures.create({
      files: {
        'firescale.config.json': JSON.stringify({
          packs: ['agent-ready'],
          rules: Object.fromEntries(ALL_AGENT_READY_RULES.map((id) => [id, 'enforced'])),
        }),
      },
    });
    const before = await runCli(['check'], repo.root);
    expect(before.code).toBe(1);
    expect(before.stdout).toMatchSnapshot();

    await runCli(['hooks', 'install', '--agent', 'claude'], repo.root);
    await repo.write('AGENTS.md', '# Agents\n\nNothing to build yet.\n');
    const after = await runCli(['check'], repo.root);
    expect(after.stdout).toBe('OK: 4 rules checked, no violations.\n');
    expect(after.code).toBe(EXIT_OK);
  });

  it.each(ALL_AGENT_READY_RULES)('explains %s', async (id) => {
    const repo = await fixtures.create({
      files: { 'firescale.config.json': JSON.stringify({ packs: ['agent-ready'] }) },
    });
    const result = await runCli(['explain', id], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toContain(id);
  });

  it('says how to turn the pack on when a rule of it is explained while it is off', async () => {
    const repo = await fixtures.create();
    const result = await runCli(['explain', 'agent-ready/verify-hook'], repo.root);
    expect(result.code).toBe(2);
    expect(result.stderr).toBe(
      'fscale: Rule "agent-ready/verify-hook" is in the "agent-ready" pack, which is not on. Add "agent-ready" to "packs" in your firescale config, then run this again.\n',
    );
  });
});
