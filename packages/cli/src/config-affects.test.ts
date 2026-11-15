import path from 'node:path';
import { readFile, rm, writeFile } from 'node:fs/promises';
import { runGit } from '@firescale/sdk';
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
      'verify: PARTIAL (affected only) - 1 of 6 projects ran, 5 not affected; 2 of 4 steps ran, 2 skipped',
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
    // (Their steps pass from the cache: these are the keys of the first run.)
    const on = await verify('--affected');
    expect(on.headline).toMatch(
      /^verify: PARTIAL \(affected only\) - 2 of 6 projects ran, 4 not affected; /,
    );
    expect(on.calls).toEqual([]);
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
    // (Its steps pass from the cache: these are the keys of the first run.)
    const removed = await verify('--affected');
    expect(removed.stdout).toContain('6 of 6 projects ran');
    expect(removed.stdout).toContain(
      "a setting of firescale.config.json that every step's result can depend on changed (verify.ci)",
    );
    expect(removed.calls).toEqual([]);
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
        // An unknown key is named apart from the settings this version knows.
        "a setting of firescale.config.json that every step's result can depend on changed (future (an unknown key))",
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

  // The owner's case again, with the config in .gitignore: git lists it
  // nowhere, and it is still compared with the defaults, setting by setting.
  it('runs no project for a git-ignored config that only sets two test timeouts', async () => {
    const { verify, config } = await courseProject({
      '.gitignore': 'generated/\nfirescale.config.json\n',
    });
    await config({
      verify: {
        projects: {
          'backend/ledger_service': { test: { timeout: '2h' } },
          frontend: { test: { timeout: '30m' } },
        },
      },
    });
    // (--base: with no changed file at all, --affected has nothing to
    // compare with and runs every project.)
    const affected = await verify('--affected', '--base', 'HEAD');
    expect(affected.code).toBe(EXIT_OK);
    expect(affected.headline).toBe(NOTHING_RAN);
    expect(affected.calls).toEqual([]);
  });

  // Renamed from .json to .ts: every project runs, so the note that the
  // change runs no project must not be said.
  it('does not say that the change runs no project after a rename from .json to .ts', async () => {
    const { repo, verify } = await courseProject({
      'firescale.config.json': '{ "verify": { "timeout": "1h" } }\n',
    });
    await rm(path.join(repo.root, 'firescale.config.json'));
    await repo.write('firescale.config.ts', 'export default { verify: { timeout: "1h" } };\n');
    const renamed = await verify('--affected');
    expect(renamed.stdout).toContain('6 of 6 projects ran');
    expect(projectsIn(renamed.calls)).toEqual(EVERY_PROJECT);
    expect(renamed.stdout).not.toContain(ONLY_OUT_SETTINGS);
    expect(renamed.stdout).not.toContain('runs no project');
  });
});

// The owner's second report: an untracked image.png at the root was not
// named as a file that cannot affect a build (ADR 0034, ADR 0043).
describe('the note that names inert files', () => {
  it('names an inert file even when another changed file runs every project', async () => {
    const { repo, verify } = await courseProject();
    await repo.write('image.png', 'not really a png\n');
    // Sorts before image.png, and belongs to no project: every project runs.
    await repo.write('deploy.sh', 'echo deploy\n');
    const result = await verify('--affected');
    expect(result.stdout).toContain('6 of 6 projects ran');
    expect(result.stdout).toContain(
      'note: 1 changed file (image.png) belongs to no project and cannot change a build or a test',
    );
  });

  it('names the image beside an untracked config that only sets time limits', async () => {
    const { repo, verify, config } = await courseProject();
    await repo.write('image.png', 'not really a png\n');
    await config({ verify: { projects: { frontend: { test: { timeout: '30m' } } } } });
    const result = await verify('--affected');
    expect(result.headline).toBe(NOTHING_RAN);
    expect(result.stdout).toContain(
      'note: 1 changed file (image.png) belongs to no project and cannot change a build or a test',
    );
  });
});

// Why a time limit is in no key (ADR 0058): a step it stops is never kept,
// and a pass kept under another limit is still true of the code.
describe('time limits and the step cache', () => {
  // The slow test belongs to a project in a folder below the root: the key
  // of a step at the root holds the config's bytes, so every edit of a
  // limit would run it again (see "a step that can read the config as a
  // file" below).
  it('never keeps a step that a limit stopped, and serves a kept pass after the limit is lowered', async () => {
    const repo = await fixtures.create({
      files: {
        'app/package.json': `${JSON.stringify({ name: 'app', private: true })}\n`,
        'app/slow.js': "setTimeout(() => console.log('done'), 2000);\n",
      },
    });
    const limits = (limit: Record<string, string>) =>
      repo.write(
        'firescale.config.json',
        JSON.stringify({
          verify: {
            lint: false,
            typecheck: false,
            projects: { app: { test: 'node slow.js' } },
            ...limit,
          },
        }),
      );
    for (const limit of ['timeout', 'idleTimeout']) {
      await limits({ [limit]: '1s' });
      const stopped = await runCli(['verify'], repo.root, {});
      expect(stopped.stdout).toMatch(/^ {4}test +TIMED OUT /m);
      expect(stopped.stdout).not.toContain('from the cache');
    }
    // A longer limit: the step was not kept as anything, so it runs.
    await limits({ timeout: '1m', idleTimeout: '1m' });
    const passed = await runCli(['verify'], repo.root, {});
    expect(passed.code).toBe(EXIT_OK);
    expect(passed.stdout.split('\n')[0]).toBe('verify: OK');
    // The limits lowered again: the pass is served, not run to time out.
    for (const limit of ['timeout', 'idleTimeout']) {
      await limits({ [limit]: '1s' });
      const served = await runCli(['verify'], repo.root, {});
      expect(served.code).toBe(EXIT_OK);
      expect(served.stdout.split('\n')[0]).toBe(
        'verify: OK (1 of 2 steps passed from the cache, not run now)',
      );
    }
  });
});

// After the review of ADR 0058: a step whose folder holds the config can
// read it as a file (`prettier --check .`, a JSON linter). For such a step
// the config stays a file: its bytes are in the key and a change to it is
// a change to that step's project.
describe('a step that can read the config as a file', () => {
  const EXIT_BLOCK = 2;

  /** Fails when the config is there and is not formatted (no line end). */
  const FORMAT_CHECK = [
    "const fs = require('fs');",
    "const text = fs.existsSync('firescale.config.json') ? fs.readFileSync('firescale.config.json', 'utf8') : '\\n';",
    "if (!text.endsWith('\\n')) { console.log('firescale.config.json is not formatted'); process.exit(1); }",
    '',
  ].join('\n');

  /** A repo on a branch `feature` off `main`, with `files` committed on main. */
  async function branchRepo(files: Record<string, string>) {
    const repo = await fixtures.create({
      files: { '.gitignore': '.firescale/\n', 'lint.js': FORMAT_CHECK, ...files },
      track: 'no-git',
    });
    const root = repo.root;
    expect((await runGit(root, ['init', '--quiet', '--initial-branch=main'])).code).toBe(0);
    await commitAll(root, 'base');
    expect((await runGit(root, ['checkout', '--quiet', '-b', 'feature'])).code).toBe(0);
    const verify = async (...flags: string[]) => {
      const result = await runCli(['verify', ...flags], root, {});
      return { ...result, headline: result.stdout.split('\n')[0] };
    };
    return { repo, root, verify };
  }

  it('runs the root project again for an untracked config that its lint reads, and blocks the stop hook', async () => {
    const { repo, root, verify } = await branchRepo({
      'package.json': `${JSON.stringify({ name: 'app', private: true, scripts: { lint: 'node lint.js' } })}\n`,
    });
    expect((await verify()).stdout).toMatch(/ {2}lint +passed /);

    // Only a time limit, written on one line with no line end.
    await repo.write('firescale.config.json', '{"verify":{"test":{"timeout":"2h"}}}');
    const full = await verify();
    expect(full.stdout).toMatch(/ {2}lint +FAILED +\[new\] /);
    expect(full.stdout).not.toContain('from the cache');
    expect(full.headline).toMatch(/^verify: FAILED at lint \(new\)/);

    const affected = await verify('--affected');
    expect(affected.headline).toMatch(/^verify: FAILED at lint \(new\)/);
    expect(affected.stdout).toContain(
      "  the repo root's own steps: changed: firescale.config.json",
    );
    expect(affected.stdout).not.toContain('runs no project');

    // With nothing kept, the failure is still new: the base has no config.
    await rm(path.join(root, '.firescale'), { recursive: true, force: true });
    expect((await verify()).headline).toMatch(/^verify: FAILED at lint \(new\)/);

    const hook = await runCli(
      ['hooks', 'run', '--agent', 'claude'],
      root,
      {},
      {
        input: JSON.stringify({ session_id: 'config-file', stop_hook_active: false }),
      },
    );
    expect(hook.code).toBe(EXIT_BLOCK);
    expect(hook.stderr).toContain('lint');
  });

  it("runs the repo root's own steps again for a white space change of a tracked config", async () => {
    const config = { verify: { lint: 'node lint.js', typecheck: false, test: false } };
    const { repo, verify } = await branchRepo({
      'firescale.config.json': `${JSON.stringify(config, null, 2)}\n`,
    });
    expect((await verify()).headline).toBe('verify: OK');
    expect((await verify()).headline).toBe(
      'verify: OK (1 of 2 steps passed from the cache, not run now)',
    );
    await repo.write('firescale.config.json', JSON.stringify(config));
    const full = await verify();
    expect(full.headline).toMatch(/^verify: FAILED at lint \(new\)/);
    const affected = await verify('--affected');
    expect(affected.headline).toMatch(/^verify: FAILED at lint \(new\)/);
    expect(affected.stdout).toContain(
      "  the repo root's own steps: firescale.config.json changed, and these steps can read every file of the repo, the config among them",
    );
  });

  // The rule's two sides in one repo: the root project can read the file,
  // the project in a folder below it cannot.
  it("runs the root project, and no project below it, for a change of the config's bytes only", async () => {
    const { repo, verify } = await branchRepo({
      'package.json': `${JSON.stringify({ name: 'app', private: true, scripts: { lint: 'node lint.js' } })}\n`,
      'web/package.json': `${JSON.stringify({ name: 'web', private: true, scripts: { lint: 'node check.js' } })}\n`,
      'web/check.js': "console.log('ok');\n",
    });
    const first = await verify();
    expect(first.stdout).toMatch(/^ {2}lint +passed /m);
    expect(first.stdout).toMatch(/^ {4}lint +passed /m);

    // Only a time limit, well formatted: the root's lint still passes.
    await repo.write('firescale.config.json', '{ "verify": { "timeout": "2h" } }\n');
    const full = await verify();
    // The root project's lint ran again; the one below was served.
    expect(full.stdout).toMatch(/^ {2}lint +passed /m);
    expect(full.stdout).toMatch(/^ {4}lint +cached /m);

    await repo.write('firescale.config.json', '{"verify":{"timeout":"2h"}}\n');
    const affected = await verify('--affected');
    expect(affected.stdout).toContain('1 of 2 projects ran, 1 not affected');
    expect(affected.stdout).toContain(
      "  the repo root's own steps: changed: firescale.config.json",
    );
    expect(affected.stdout).toContain('  in web: not affected');
    expect(affected.stdout).not.toContain('runs no project');
  });

  // After the second review: git lists no file it ignores. So an ignored
  // config was a file of no key, and was in no list of changed files: its
  // settings were not compared under --affected and in the stop hook.
  describe('when git ignores the config', () => {
    const IGNORES = '.firescale/\nfirescale.config.json\n';
    const ROOT_LINT = { verify: { lint: 'node lint.js', typecheck: false, test: false } };
    const FORMATTED = `${JSON.stringify(ROOT_LINT, null, 2)}\n`;
    const NOT_FORMATTED = JSON.stringify(ROOT_LINT);
    // Only a time limit: no setting of it reaches a step.
    const LIMIT = '{ "verify": { "timeout": "2h" } }\n';
    const LIMIT_ON_ONE_LINE = '{"verify":{"timeout":"2h"}}';
    const IGNORED_CONFIG_CHANGED =
      "  the repo root's own steps: firescale.config.json, which git ignores, changed since the last run that passed, and these steps can read every file of the repo, the config among them";
    const stopHook = (root: string) =>
      runCli(
        ['hooks', 'run', '--agent', 'claude'],
        root,
        {},
        { input: JSON.stringify({ session_id: 'ignored-config', stop_hook_active: false }) },
      );

    it("runs the repo root's own steps again in a full run for a change of the bytes of an ignored config", async () => {
      const { repo, verify } = await branchRepo({ '.gitignore': IGNORES });
      await repo.write('firescale.config.json', FORMATTED);
      expect((await verify()).headline).toBe('verify: OK');
      await repo.write('firescale.config.json', NOT_FORMATTED);
      const full = await verify();
      expect(full.stdout).not.toContain('from the cache');
      // (Git sees no change from the base commit, so no label: it blocks.)
      expect(full.headline).toBe('verify: FAILED at lint');
    });

    /**
     * A root project whose lint reads the ignored config, which only sets a
     * time limit, and a changed file that runs nothing: --affected has a
     * change to measure, and the config's bytes are the only thing that can
     * run a step.
     */
    const rootProject = async () => {
      const made = await branchRepo({
        '.gitignore': IGNORES,
        'package.json': `${JSON.stringify({ name: 'app', private: true, scripts: { lint: 'node lint.js' } })}\n`,
      });
      await made.repo.write('firescale.config.json', LIMIT);
      await made.repo.write('notes.md', 'a note\n');
      return made;
    };

    it('runs the root project once under --affected for an ignored config that no run has seen, and not again', async () => {
      const { verify } = await rootProject();
      // Nothing is known of an earlier run: the safe side, once.
      const once = await verify('--affected');
      expect(once.headline).toBe(
        'verify: PARTIAL (affected only) - 1 of 1 project ran; 2 of 4 steps ran, 2 skipped',
      );
      expect(once.stdout).toContain(IGNORED_CONFIG_CHANGED);
      // The same bytes as that passing run saw: nothing runs.
      const same = await verify('--affected');
      expect(same.headline).toBe('verify: OK (affected only) - 0 of 1 project ran, 1 not affected');
    });

    it('runs the root project under --affected when an ignored config is not as the last passing run saw it', async () => {
      const { repo, verify } = await rootProject();
      expect((await verify()).code).toBe(EXIT_OK);
      await repo.write('firescale.config.json', LIMIT_ON_ONE_LINE);
      const affected = await verify('--affected');
      expect(affected.headline).toMatch(/^verify: FAILED at lint \(new\)/);
      expect(affected.stdout).toContain(IGNORED_CONFIG_CHANGED);
    });

    it('blocks the stop hook when an ignored config is not as the last passing run saw it', async () => {
      const { repo, root, verify } = await rootProject();
      expect((await verify()).code).toBe(EXIT_OK);
      await repo.write('firescale.config.json', LIMIT_ON_ONE_LINE);
      const hook = await stopHook(root);
      expect(hook.code).toBe(EXIT_BLOCK);
      expect(hook.stderr).toContain('lint');
    });

    // After the third review: a run in which --budget left the lint out is
    // PARTIAL, and it wrote the config's new bytes down though no step had
    // run with them. --affected and the stop hook then let the failure by.
    it('keeps no record of an ignored config from a run in which --budget left a step out', async () => {
      const { repo, root, verify } = await rootProject();
      expect((await verify()).code).toBe(EXIT_OK);
      // As if every step had taken 1h 40m when it last ran.
      const times = path.join(root, '.firescale', 'step-times.json');
      const kept = JSON.parse(await readFile(times, 'utf8')) as {
        steps: { durationMs: number }[];
      };
      expect(kept.steps.length).toBeGreaterThan(0);
      for (const step of kept.steps) step.durationMs = 100 * 60 * 1000;
      await writeFile(times, JSON.stringify(kept, null, 2));
      await repo.write('firescale.config.json', LIMIT_ON_ONE_LINE);
      // On a person's machine: --budget is refused in CI.
      const budget = await runCli(['verify', '--budget', '30m'], root, {
        CI: undefined,
        GITHUB_ACTIONS: undefined,
      });
      expect(budget.stdout.split('\n')[0]).toMatch(
        /^verify: PARTIAL - .*1 not run \(over the --budget of 30m\)$/,
      );
      // The lint has not run with these bytes: it still has to. (The stop
      // hook asks the same question; its test is the one above.)
      const affected = await verify('--affected');
      expect(affected.headline).toMatch(/^verify: FAILED at lint \(new\)/);
      expect(affected.stdout).toContain(IGNORED_CONFIG_CHANGED);
      // Three whole runs, the last with a run of the base commit.
    }, 90_000);

    /** Two projects below the root; `web` has a check that fails when the root has a config. */
    const twoProjects = () => {
      const pkg = (name: string) =>
        `${JSON.stringify({ name, private: true, scripts: { test: 'node t.js' } })}\n`;
      return branchRepo({
        '.gitignore': IGNORES,
        'api/package.json': pkg('api'),
        'api/t.js': '\n',
        'web/package.json': pkg('web'),
        'web/t.js': '\n',
        'web/no-config.js':
          "process.exit(require('fs').existsSync('../firescale.config.json') ? 1 : 0);\n",
      });
    };

    const WEB_COMMAND = JSON.stringify({
      verify: { projects: { web: { test: 'node no-config.js' } } },
    });

    it('runs the project whose command an ignored config changes under --affected', async () => {
      const { repo, verify } = await twoProjects();
      await repo.write('api/t.js', '// changed\n');
      await repo.write('firescale.config.json', WEB_COMMAND);
      const affected = await verify('--affected');
      expect(affected.stdout).toContain('2 of 2 projects ran');
      expect(affected.stdout).toContain(
        '  web: firescale.config.json changes what its test step runs',
      );
      expect(affected.headline).toMatch(/^verify: FAILED at test in web \(new\)/);
    });

    it('blocks the stop hook when an ignored config gives a project a command that fails', async () => {
      const { repo, root } = await twoProjects();
      await repo.write('api/t.js', '// changed\n');
      await repo.write('firescale.config.json', WEB_COMMAND);
      const hook = await stopHook(root);
      expect(hook.code).toBe(EXIT_BLOCK);
      expect(hook.stderr).toContain('test');
    });

    for (const [setting, name] of [
      [{ ci: false }, 'verify.ci'],
      [{ failOnSkipped: true }, 'verify.failOnSkipped'],
    ] as const) {
      it(`runs every project under --affected when an ignored config changes ${name}`, async () => {
        const { repo, verify } = await twoProjects();
        await repo.write('api/t.js', '// changed\n');
        await repo.write('firescale.config.json', JSON.stringify({ verify: setting }));
        const affected = await verify('--affected');
        expect(affected.stdout).toContain('2 of 2 projects ran');
        expect(affected.stdout).toContain(
          `a setting of firescale.config.json that every step's result can depend on changed (${name})`,
        );
      });
    }

    it('runs no project under --affected for an ignored config that only sets a time limit', async () => {
      const { repo, verify } = await twoProjects();
      await repo.write('api/t.js', '// changed\n');
      await repo.write('firescale.config.json', JSON.stringify({ verify: { timeout: '2h' } }));
      const limit = await verify('--affected');
      expect(limit.stdout).toContain('1 of 2 projects ran, 1 not affected');
      expect(limit.stdout).toContain('  in web: not affected');
      expect(limit.stdout).not.toContain('runs no project');
    });
  });
});
