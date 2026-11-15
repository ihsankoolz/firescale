import path from 'node:path';
import { rm, writeFile } from 'node:fs/promises';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { courseProjectFiles, shimEnv, writeToolShims } from './course-fixture.js';
import { commitAll, EXIT_OK, runCli, stepCalls } from './test-helpers.js';

// What a change to the firescale config affects (ADR 0058): a step runs
// again, and is affected, only when a setting that can change its result
// changed.

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

const EVERY_PROJECT = [
  'audit_service',
  'auth_service',
  'frontend',
  'ledger_service',
  'market_service',
  'realtime_service',
];

const NOTHING_RAN = 'verify: OK (affected only) - 0 of 6 projects ran, 6 not affected';

const ONLY_OUT_SETTINGS =
  "note: firescale.config.json changed, but only in settings that cannot change a step's result (time limits, budgets and the like), so the change runs no project.";

/** The projects whose steps ran, from the stand-ins' log. */
function projectsIn(calls: string[]): string[] {
  return [...new Set(calls.map((call) => call.replace(/^.* in /, '')))].sort();
}

/**
 * The course project, committed, with stand-ins for python, npx and npm
 * that answer --version, and `files` on top.
 */
async function courseProject(files: Record<string, string> = {}) {
  const repo = await fixtures.create({ files: { ...courseProjectFiles(), ...files } });
  await commitAll(repo.root, 'initial');
  const tools = await fixtures.create({ files: {}, track: 'no-git' });
  await writeToolShims(tools.root, { versions: true });
  const log = path.join(tools.root, 'calls.log');
  const env = shimEnv(tools.root, log);
  const verify = async (...flags: string[]) => {
    await writeFile(log, '');
    const result = await runCli(['verify', ...flags], repo.root, env);
    return { ...result, calls: await stepCalls(log), headline: result.stdout.split('\n')[0] };
  };
  const config = (value: unknown) =>
    repo.write('firescale.config.json', `${JSON.stringify(value, null, 2)}\n`);
  return { repo, verify, config };
}

describe('what a config change affects (ADR 0058)', () => {
  // The owner's case: six projects, an untracked config with two test
  // timeouts and nothing else.
  it('runs no project for an untracked config that only sets two test timeouts', async () => {
    const { verify, config } = await courseProject();
    const first = await verify();
    expect(first.code).toBe(EXIT_OK);
    expect(projectsIn(first.calls)).toEqual(EVERY_PROJECT);

    await config({
      verify: {
        projects: {
          'backend/ledger_service': { test: { timeout: '2h' } },
          frontend: { test: { timeout: '30m' } },
        },
      },
    });
    const affected = await verify('--affected');
    expect(affected.code).toBe(EXIT_OK);
    expect(affected.headline).toBe(NOTHING_RAN);
    expect(affected.calls).toEqual([]);
    expect(affected.stdout).toContain(ONLY_OUT_SETTINGS);
    const plan = await verify('--affected', '--dry-run');
    expect(plan.stdout.split('\n')[0]).toBe(
      'verify --affected (dry run): 0 of 6 projects would run, 6 not affected. Nothing was run.',
    );

    // No step's key changed: a full run takes every pass from the cache.
    const full = await verify();
    expect(full.code).toBe(EXIT_OK);
    expect(full.calls).toEqual([]);
    expect(full.stdout).not.toContain('changed since the last run');

    // Another limit, a budget and every other setting that cannot change a
    // result: still nothing runs.
    await config({
      $schema: './node_modules/firescale/schema.json',
      protectedPaths: ['infra/**'],
      agent: { maxTurns: 5, maxCostUsd: 1, timeoutSeconds: 60 },
      verify: {
        timeout: '3h',
        idleTimeout: false,
        baseBudget: '10m',
        baseInstall: 'npm ci',
        labelFixed: true,
        tamper: 'enforced',
        test: { timeout: '4h' },
        projects: {
          'backend/ledger_service': { timeout: '1h', test: { timeout: '5h' } },
          frontend: { test: { timeout: false } },
        },
      },
    });
    expect((await verify()).calls).toEqual([]);
    const again = await verify('--affected');
    expect(again.headline).toBe(NOTHING_RAN);
    expect(again.calls).toEqual([]);
  });

  it('writing a default out in full, or reformatting, changes no key', async () => {
    const { repo, verify, config } = await courseProject({
      'firescale.config.json': '{ "verify": { "timeout": "2h" } }\n',
    });
    await verify();
    await config({
      packs: null,
      rules: null,
      verify: {
        ci: true,
        failOnSkipped: false,
        stopWhenServiceDown: true,
        timeout: '2h',
      },
    });
    expect((await verify()).calls).toEqual([]);
    expect((await verify('--affected')).headline).toBe(NOTHING_RAN);
    await repo.write('firescale.config.json', '{"verify":{"timeout":"2h"}}');
    expect((await verify()).calls).toEqual([]);
  });

  // The settings that can change every step's result.
  for (const [name, change] of [
    ['verify.ci', { verify: { ci: false } }],
    ['verify.failOnSkipped', { verify: { failOnSkipped: true } }],
    ['verify.stopWhenServiceDown', { verify: { stopWhenServiceDown: false } }],
    ['verify.stopAfterErrors', { verify: { stopAfterErrors: 25 } }],
    ['packs', { packs: ['core', 'agent-ready'] }],
    ['rules', { rules: { 'core/readme-setup': 'off' } }],
    [
      'rules.<id>.options',
      { rules: { 'core/env-example': { options: { excludePaths: ['x/'] } } } },
    ],
  ] as const) {
    it(`runs every step again, and every project under --affected, when ${name} changes`, async () => {
      const { verify, config } = await courseProject();
      await verify();
      await config(change);
      const affected = await verify('--affected');
      expect(projectsIn(affected.calls)).toEqual(EVERY_PROJECT);
      expect(affected.stdout).toContain('6 of 6 projects ran');
      expect(affected.stdout).not.toContain(ONLY_OUT_SETTINGS);
      // Those passes are kept under the new keys.
      const full = await verify('--no-cache');
      expect(projectsIn(full.calls)).toEqual(EVERY_PROJECT);
      expect((await verify()).calls).toEqual([]);
    });
  }

  it('runs every step again when a setting every step shares changes, with one note', async () => {
    const { verify, config } = await courseProject();
    await verify();
    await config({ verify: { ci: false } });
    const changed = await verify();
    expect(projectsIn(changed.calls)).toEqual(EVERY_PROJECT);
    expect(changed.stdout).toContain(
      "note: A setting of the firescale config that every step's result can depend on changed since the last run, and it is part of every step's cache key, so no pass kept before applies: every step runs again this time.",
    );
  });

  it('runs only the project whose command the config changes', async () => {
    const { verify, config } = await courseProject();
    await verify();
    await config({
      verify: { projects: { 'backend/ledger_service': { test: 'python -m pytest -q' } } },
    });
    const affected = await verify('--affected');
    expect(affected.headline).toBe(
      'verify: OK (affected only) - 1 of 6 projects ran, 5 not affected',
    );
    expect(affected.calls).toEqual(['python -m pytest -q in ledger_service']);
    expect(affected.stdout).toContain(
      '  backend/ledger_service: firescale.config.json changes what its test step runs',
    );
    // In a full run the other projects' keys are the same.
    await config({
      verify: {
        projects: { 'backend/ledger_service': { test: { command: 'python -m pytest -x' } } },
      },
    });
    expect((await verify()).calls).toEqual(['python -m pytest -x in ledger_service']);
    // The same command with a time limit beside it: nothing runs.
    await config({
      verify: {
        projects: {
          'backend/ledger_service': { test: { command: 'python -m pytest -x', timeout: '2h' } },
        },
      },
    });
    expect((await verify()).calls).toEqual([]);
  });

  it('runs nothing for a project or a step that the config turns off, and runs it when it is turned on again', async () => {
    const { repo, verify, config } = await courseProject();
    await verify();
    await config({
      verify: { projects: { 'backend/ledger_service': false, frontend: { lint: false } } },
    });
    const off = await verify('--affected');
    expect(off.calls).toEqual([]);
    expect(off.headline).toBe('verify: OK (affected only) - 0 of 5 projects ran, 5 not affected');
    expect((await verify()).calls).toEqual([]);

    // Turned on again, measured from a commit where they were off.
    await commitAll(repo.root, 'ledger and the frontend lint are off');
    await config({ verify: { timeout: '2h' } });
    const on = await verify('--affected');
    expect(projectsIn(on.calls)).toEqual(['frontend', 'ledger_service']);
    expect(on.stdout).toContain(
      '  backend/ledger_service: firescale.config.json changes what its lint, typecheck and test steps run',
    );
    expect(on.stdout).toContain(
      '  frontend: firescale.config.json changes what its lint step runs',
    );
  });

  it('runs the root step, and no project step, when the config gives the whole repo a command', async () => {
    const { repo, verify, config } = await courseProject();
    await verify();
    await config({ verify: { test: 'python -m pytest backend' } });
    const affected = await verify('--affected');
    expect(affected.calls).toEqual([`python -m pytest backend in ${path.basename(repo.root)}`]);
    expect(affected.stdout).toContain(
      "  the repo root's own steps: firescale.config.json changes what its test step runs",
    );
  });

  it('compares a committed config with the base commit, setting by setting', async () => {
    const { repo, verify, config } = await courseProject({
      'firescale.config.json': '{ "verify": { "timeout": "1h" } }\n',
    });
    await verify();
    await config({ verify: { timeout: '2h' } });
    await commitAll(repo.root, 'a longer limit');
    const limit = await verify('--affected', '--base', 'HEAD~1');
    expect(limit.headline).toBe(NOTHING_RAN);
    expect(limit.stdout).toContain(ONLY_OUT_SETTINGS);

    await config({ verify: { timeout: '2h', ci: false } });
    await commitAll(repo.root, 'no CI variable');
    const ci = await verify('--affected', '--base', 'HEAD~1');
    expect(projectsIn(ci.calls)).toEqual(EVERY_PROJECT);
    expect(ci.stdout).toContain(
      "a setting of firescale.config.json that every step's result can depend on changed (verify.ci)",
    );

    // Removed: the defaults are compared with what the base had.
    await rm(path.join(repo.root, 'firescale.config.json'));
    const removed = await verify('--affected');
    expect(projectsIn(removed.calls)).toEqual(EVERY_PROJECT);
    await commitAll(repo.root, 'no config');
    await config({ verify: { idleTimeout: '20m' } });
    expect((await verify('--affected')).headline).toBe(NOTHING_RAN);
  });

  it('runs every project when the base commit has a config this version cannot read', async () => {
    for (const [base, reason] of [
      [
        '{ "verify": { "timeout": "1h", "retries": 2 } }\n',
        'the firescale.config.json of the commit compared with could not be read by this version (firescale.config.json: unknown verify step "retries".',
      ],
      [
        '{ "future": true }\n',
        "a setting of firescale.config.json that every step's result can depend on changed (future)",
      ],
      [
        '{ not json\n',
        'the firescale.config.json of the commit compared with could not be read by this version (it is not valid JSON',
      ],
    ] as const) {
      const { verify, config } = await courseProject({ 'firescale.config.json': base });
      await config({ verify: { timeout: '1h' } });
      const affected = await verify('--affected');
      expect(projectsIn(affected.calls)).toEqual(EVERY_PROJECT);
      expect(affected.stdout).toContain(reason);
    }
  });

  it('keeps the whole-file rule for a firescale.config.ts', async () => {
    const { repo, verify } = await courseProject({
      'firescale.config.ts': 'export default { verify: { timeout: "1h" } };\n',
    });
    const first = await verify();
    expect(first.code).toBe(EXIT_OK);
    expect(projectsIn(first.calls)).toEqual(EVERY_PROJECT);
    expect((await verify()).calls).toEqual([]);
    await repo.write('firescale.config.ts', 'export default { verify: { timeout: "2h" } };\n');
    const affected = await verify('--affected');
    expect(projectsIn(affected.calls)).toEqual(EVERY_PROJECT);
    expect(affected.stdout).toContain('the firescale config firescale.config.ts changed');
    // Its bytes are in every key.
    await repo.write('firescale.config.ts', 'export default { verify: { timeout: "3h" } };\n');
    expect(projectsIn((await verify()).calls)).toEqual(EVERY_PROJECT);

    // Renamed to .json: the .ts file is among the changed files.
    await commitAll(repo.root, 'a ts config');
    await rm(path.join(repo.root, 'firescale.config.ts'));
    await repo.write('firescale.config.json', '{ "verify": { "timeout": "3h" } }\n');
    const renamed = await verify('--affected');
    expect(projectsIn(renamed.calls)).toEqual(EVERY_PROJECT);
    expect(renamed.stdout).toContain('the firescale config firescale.config.ts changed');
  });
});
