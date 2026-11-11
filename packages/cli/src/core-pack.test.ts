import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import type { CheckReport } from './report.js';
import { ALL_CORE_RULES, EXIT_OK, REPO, runCli } from './test-helpers.js';

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

  it('passes on the firescale repo itself (dogfooding)', async () => {
    const result = await runCli(['check'], REPO);
    expect(result.stdout).toBe('OK: 8 rules checked, no violations.\n');
    expect(result.code).toBe(EXIT_OK);
  });
});
