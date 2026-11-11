import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { ALL_CORE_RULES, ENFORCED_CONFIG, EXIT_OK, EXIT_USAGE, runCli } from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

describe('fscale explain', () => {
  it('prints what the rule checks, why, and how to fix it by hand', async () => {
    const repo = await fixtures.create({ files: { 'firescale.config.json': ENFORCED_CONFIG } });
    const result = await runCli(['explain', 'core/no-committed-env'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toMatchSnapshot();
    expect(result.stdout).toContain('Status:      enforced');
    expect(result.stdout).toContain('## What this rule enforces and why');
    expect(result.stdout).toContain('## Manual fix');
    expect(result.stdout).not.toContain('\r');
    expect(result.stdout).not.toMatch(/^---$/m);
  });

  it('accepts the short rule name', async () => {
    const repo = await fixtures.create();
    const full = await runCli(['explain', 'core/no-committed-env'], repo.root);
    const short = await runCli(['explain', 'no-committed-env'], repo.root);
    expect(short.stdout).toBe(full.stdout);
  });

  it('lists the available rules when the rule is unknown', async () => {
    const repo = await fixtures.create();
    const result = await runCli(['explain', 'core/nope'], repo.root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toBe(
      `fscale: Unknown rule "core/nope". Available rules: ${ALL_CORE_RULES.join(', ')}.\n`,
    );
  });

  it('needs exactly one rule', async () => {
    const result = await runCli(['explain']);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain('explain needs exactly one rule id');
  });
});
