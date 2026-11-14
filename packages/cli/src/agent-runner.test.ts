import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { runGit } from '@firescale/sdk';
import { fixtureSet } from '@firescale/sdk/testing';
import { Ajv } from 'ajv';
import { afterEach, describe, expect, it } from 'vitest';
import {
  commitAll,
  EXIT_OK,
  EXIT_USAGE,
  EXIT_VIOLATIONS,
  FAKE_AGENT,
  runCli,
} from './test-helpers.js';

// The guarded agent runner (M27, ADR 0047): `fscale fix --agent`, one test
// per guardrail of SPEC §7, against the stand-in agent of the testkit. No
// real model is ever called.

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

// Built from parts, so this repo's own core/env-example does not read the
// fixture's code as code of this repo.
const APP = `export const url = ${['process', 'env', 'API_URL'].join('.')};\nexport const add = (a, b) => a + b;\n`;
const TEST =
  'import { add } from "./app.js";\n\ntest("adds", () => {\n  expect(add(1, 2)).toBe(3);\n});\n';
const RULE = 'core/env-example';

const config = (agent: Record<string, number> = {}) =>
  `${JSON.stringify({
    packs: ['core'],
    verify: { lint: false, typecheck: false, test: false },
    agent: { timeoutSeconds: 20, ...agent },
  })}\n`;

interface Scenario {
  installed?: boolean;
  authenticated?: boolean;
  write?: Record<string, string | null>;
  commands?: string[][];
  result?: { ok: boolean; costUsd?: number; turns?: number; error?: string };
  sleepSecs?: number;
  exit?: number;
}

const GOOD: Scenario = {
  write: { '.env.example': 'API_URL=\n' },
  result: { ok: true, costUsd: 0.12, turns: 3 },
};

async function git(root: string, ...args: string[]): Promise<string> {
  const result = await runGit(root, ['-c', 'commit.gpgsign=false', ...args]);
  if (result.code !== 0) throw new Error(`git ${args.join(' ')}: ${result.stderr}`);
  return result.stdout.trim();
}

/** A repo on `main` whose code reads API_URL, which .env.example lacks. */
async function repo(files: Record<string, string> = {}, agent: Record<string, number> = {}) {
  const fixture = await fixtures.create({
    files: {
      'firescale.config.json': config(agent),
      '.gitignore': '.firescale/\n',
      'package.json': '{ "name": "demo", "type": "module" }\n',
      '.env.example': 'NODE_ENV=\n',
      'src/app.js': APP,
      'src/app.test.js': TEST,
      ...files,
    },
    track: 'no-git',
  });
  const root = fixture.root;
  await git(root, 'init', '--quiet', '--initial-branch=main');
  await git(root, 'config', 'user.name', 'Owner');
  await git(root, 'config', 'user.email', 'owner@example.invalid');
  await git(root, 'config', 'commit.gpgsign', 'false');
  await commitAll(root, 'base');
  return root;
}

async function fix(root: string, scenario: Scenario, extra: string[] = ['--rule', RULE]) {
  const dir = path.join(root, '..', `agent-${path.basename(root)}`);
  await mkdir(dir, { recursive: true });
  const file = path.join(dir, 'scenario.json');
  const log = path.join(dir, 'log.jsonl');
  await writeFile(file, JSON.stringify(scenario));
  const result = await runCli(['fix', '--agent', 'fake', ...extra], root, {
    FSCALE_FAKE_AGENT: FAKE_AGENT,
    FSCALE_FAKE_AGENT_SCENARIO: file,
    FSCALE_FAKE_AGENT_LOG: log,
    GITHUB_ACTIONS: undefined,
  });
  const logged = await readFile(log, 'utf8').catch(() => '');
  const requests = logged
    .split('\n')
    .filter(Boolean)
    .map((line) => JSON.parse(line) as { request: Request; cwd: string; gitConfigKeys: string[] });
  return { ...result, requests };
}

interface Request {
  cwd: string;
  rule: string;
  skill: string;
  prompt: string;
  diagnostics: { ruleId: string; file: string }[];
  policy: {
    allowedTools: string[];
    deniedCommands: string[];
    deniedPaths: string[];
    maxTurns: number;
    maxCostUsd: number;
    timeoutSecs: number;
    loadProjectConfig: boolean;
  };
}

async function ledger(root: string): Promise<Record<string, unknown>[]> {
  const text = await readFile(path.join(root, '.firescale', 'ledger.jsonl'), 'utf8').catch(
    () => '',
  );
  return text
    .split('\n')
    .filter(Boolean)
    .map((line) => JSON.parse(line) as Record<string, unknown>);
}

const branches = async (root: string) =>
  (await git(root, 'branch', '--list', '--format=%(refname:short)')).split('\n').filter(Boolean);

describe('fscale fix --agent: the guardrails of SPEC §7', () => {
  it('guardrail 1, pre-flight: refuses a missing or signed-out agent, a dirty tree and tracked secrets', async () => {
    const root = await repo();
    for (const [scenario, text] of [
      [{ installed: false }, 'did not answer --version'],
      [{ authenticated: false }, 'is not signed in'],
    ] as const) {
      const result = await fix(root, scenario);
      expect(result.code, text).toBe(EXIT_USAGE);
      expect(result.stderr).toContain(text);
      expect(result.requests).toEqual([]);
    }
    await writeFile(path.join(root, 'src', 'extra.js'), 'export {};\n');
    const dirty = await fix(root, GOOD);
    expect(dirty.code).toBe(EXIT_USAGE);
    expect(dirty.stderr).toContain('uncommitted changes');
    expect(dirty.requests).toEqual([]);

    const secret = await repo({ '.env': 'API_URL=https://secret.example\n' });
    const refused = await fix(secret, GOOD);
    expect(refused.code).toBe(EXIT_USAGE);
    expect(refused.stderr).toContain('.env is tracked by git and looks like a secret');
    expect(refused.requests).toEqual([]);
    expect(await ledger(secret)).toEqual([]);
  });

  it('guardrail 2, isolation: the agent works in a worktree on a new branch, never on yours', async () => {
    const root = await repo();
    const head = await git(root, 'rev-parse', 'HEAD');
    const result = await fix(root, GOOD);
    expect(result.code, result.stderr + result.stdout).toBe(EXIT_OK);
    // The agent ran elsewhere.
    expect(path.resolve(result.requests[0]?.cwd ?? root)).not.toBe(path.resolve(root));
    // Your branch, files and index are as they were.
    expect(await git(root, 'rev-parse', 'HEAD')).toBe(head);
    expect(await git(root, 'symbolic-ref', '--short', 'HEAD')).toBe('main');
    expect(await readFile(path.join(root, '.env.example'), 'utf8')).toBe('NODE_ENV=\n');
    expect(await git(root, 'status', '--porcelain')).toBe('');
    // The fix is on a branch of its own, and no worktree is left.
    const fixBranch = (await branches(root)).find((b) =>
      b.startsWith('fscale/fix/core-env-example-'),
    );
    expect(fixBranch).toBeDefined();
    expect(await git(root, 'show', `${fixBranch}:.env.example`)).toBe('API_URL=');
    expect((await git(root, 'worktree', 'list')).split('\n')).toHaveLength(1);
  });

  it("guardrail 3, scoped prompt: one rule's SKILL.md and that rule's diagnostics only", async () => {
    // README.md missing makes core/readme-setup fail too; it is not sent.
    const root = await repo();
    const result = await fix(root, GOOD);
    const request = result.requests[0]?.request;
    expect(request?.rule).toBe(RULE);
    expect(request?.skill).toContain('name: env-example');
    expect(request?.diagnostics.map((d) => d.ruleId)).toEqual([RULE]);
    expect(request?.prompt).toContain(RULE);
    expect(request?.prompt).not.toContain('core/readme-setup');
  });

  it("guardrail 4, tool limits: a strict allow-list, secrets denied, and never wider than the repo's own settings", async () => {
    const root = await repo({
      '.claude/settings.json': JSON.stringify({ permissions: { deny: ['Write', 'Bash(curl *)'] } }),
    });
    const result = await fix(root, GOOD);
    const policy = result.requests[0]?.request.policy;
    expect(policy?.allowedTools).toEqual(['Read', 'Glob', 'Grep', 'Edit']);
    expect(policy?.deniedCommands).toEqual(
      expect.arrayContaining(['git push', 'git tag', 'npm publish', 'Bash(curl *)', 'Write']),
    );
    expect(policy?.deniedPaths).toEqual(
      expect.arrayContaining([
        '.env',
        '**/.env',
        '.env.local',
        '*.pem',
        '*.key',
        '**/secrets/**',
        '**/.claude/**',
      ]),
    );

    // Settings that let no agent edit: the runner does not start.
    const readOnly = await repo({ '.codex/config.toml': 'sandbox_mode = "read-only"\n' });
    const refused = await fix(readOnly, GOOD);
    expect(refused.code).toBe(EXIT_USAGE);
    expect(refused.stderr).toContain('never has wider permissions than they give');
    expect(refused.requests).toEqual([]);
  });

  it('guardrail 5, budget limits: the limits are passed on, and a run over spend, turns or time fails', async () => {
    const root = await repo({}, { maxTurns: 5, maxCostUsd: 0.5, timeoutSeconds: 2 });
    const within = await fix(root, GOOD);
    expect(within.requests[0]?.request.policy).toMatchObject({
      maxTurns: 5,
      maxCostUsd: 0.5,
      timeoutSecs: 2,
    });
    for (const [scenario, text] of [
      [{ ...GOOD, result: { ok: true, costUsd: 0.75, turns: 2 } }, 'over the limit of $0.50'],
      [{ ...GOOD, result: { ok: true, costUsd: 0.1, turns: 9 } }, 'over the limit of 5'],
      [{ ...GOOD, sleepSecs: 6 }, 'time limit'],
    ] as const) {
      const result = await fix(root, scenario);
      expect(result.code, text).toBe(EXIT_VIOLATIONS);
      expect(result.stdout).toContain(text);
    }
    const reasons = (await ledger(root)).map((entry) => entry.reason ?? entry.result);
    expect(reasons).toEqual(['pass', 'over-budget', 'over-budget', 'timeout']);
  });

  it("guardrail 6, controlled config: the run says not to load the repo's own hooks, settings or MCP servers", async () => {
    const root = await repo({
      '.mcp.json': '{ "mcpServers": { "x": { "command": "evil" } } }\n',
    });
    const result = await fix(root, GOOD);
    expect(result.requests[0]?.request.policy.loadProjectConfig).toBe(false);
    // And pushes from inside the run go nowhere.
    expect(result.requests[0]?.gitConfigKeys).toEqual(
      expect.arrayContaining([
        'protocol.allow',
        'url.file:///firescale-runner-never-pushes/.pushInsteadOf',
      ]),
    );
  });

  it('guardrail 7, post-run diff guard: a protected path, or a file outside the rule, is rejected', async () => {
    const root = await repo();
    for (const [write, text] of [
      [{ '.env.example': 'API_URL=\n', '.env': 'API_URL=x\n' }, 'protected path .env'],
      [
        { '.env.example': 'API_URL=\n', '.claude/settings.json': '{}\n' },
        'protected path .claude/settings.json',
      ],
      [
        { '.env.example': 'API_URL=\n', 'firescale.config.json': config({ maxTurns: 99 }) },
        'protected path firescale.config.json',
      ],
      [
        { '.env.example': 'API_URL=\n', 'README.md': '# Demo\n' },
        'README.md, outside what the rule may change',
      ],
    ] as const) {
      const result = await fix(root, { ...GOOD, write });
      expect(result.code, text).toBe(EXIT_VIOLATIONS);
      expect(result.stdout).toContain(text);
    }
    expect((await branches(root)).filter((b) => b.startsWith('fscale/'))).toEqual([]);
  });

  it('guardrail 8, re-check: a good fix is committed, a bad fix is rolled back, and a skipped test is rejected by the tamper guard', async () => {
    const root = await repo();
    const bad = await fix(root, { write: { '.env.example': 'OTHER=\n' }, result: { ok: true } });
    expect(bad.code).toBe(EXIT_VIOLATIONS);
    expect(bad.stdout).toContain('violation of the rule remain');
    const cheat = await fix(root, {
      write: {
        '.env.example': 'API_URL=\n',
        'src/app.test.js': TEST.replace('test("adds"', 'test.skip("adds"'),
      },
      result: { ok: true },
    });
    expect(cheat.code).toBe(EXIT_VIOLATIONS);
    expect(cheat.stdout).toContain('the tamper guard found test skipped: src/app.test.js:3');
    expect((await branches(root)).filter((b) => b.startsWith('fscale/'))).toEqual([]);
    const good = await fix(root, GOOD);
    expect(good.code, good.stdout + good.stderr).toBe(EXIT_OK);
    expect(good.stdout).toContain(
      'PASSED - core/env-example is fixed on the branch fscale/fix/core-env-example-',
    );
    expect(good.stdout).toContain('nothing was pushed');
  });

  it('guardrail 9, one rule per commit, with trailers for the rule, the agent and the outcome', async () => {
    const root = await repo();
    await fix(root, GOOD);
    const branch = (await branches(root)).find((b) => b.startsWith('fscale/fix/')) ?? '';
    expect(await git(root, 'rev-list', '--count', `main..${branch}`)).toBe('1');
    const message = await git(root, 'log', '-1', '--format=%B', branch);
    expect(message).toContain('fix(core/env-example):');
    expect(message).toContain(
      'Firescale-Rule: core/env-example\nFirescale-Agent: fake\nFirescale-Outcome: pass',
    );
    expect(await git(root, 'log', '-1', '--format=%an <%ae>', branch)).toBe(
      'Owner <owner@example.invalid>',
    );
    expect(await git(root, 'diff', '--name-only', `main..${branch}`)).toBe('.env.example');
  });

  it('guardrail 10, ledger: a pass and a fail each append one schema-valid line, with no prompt, file contents or secrets', async () => {
    const root = await repo();
    await fix(root, GOOD);
    await fix(root, { write: { '.env.example': 'OTHER=\n' }, result: { ok: true, costUsd: 0.05 } });
    const lines = await ledger(root);
    expect(lines).toHaveLength(2);
    const schema = JSON.parse(
      await readFile(
        new URL('../../../docs/schemas/ledger-entry.schema.json', import.meta.url),
        'utf8',
      ),
    ) as object;
    const valid = new Ajv({ strict: true }).compile(schema);
    for (const line of lines) {
      expect(valid(line), JSON.stringify(valid.errors)).toBe(true);
      expect(line.schema).toBe(1);
      expect(line.time).toMatch(/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$/);
      expect(line.rule).toBe(RULE);
      expect(line.agent).toBe('fake');
      expect(typeof line.durationMs).toBe('number');
    }
    expect(lines[0]).toMatchObject({
      result: 'pass',
      filesChanged: 1,
      linesChanged: 2,
      costUsd: 0.12,
      turns: 3,
    });
    expect(lines[1]).toMatchObject({ result: 'fail', reason: 'still-failing', costUsd: 0.05 });
    const text = await readFile(path.join(root, '.firescale', 'ledger.jsonl'), 'utf8');
    for (const forbidden of [
      'API_URL',
      'OTHER=',
      'SKILL',
      'Fix the remaining violations',
      'process.env',
    ]) {
      expect(text).not.toContain(forbidden);
    }
  });

  it('never pushes or tags: a push from inside the run goes nowhere, and a tag fails the run', async () => {
    const root = await repo();
    const remote = path.join(root, '..', `remote-${path.basename(root)}.git`);
    await runGit(path.dirname(root), ['init', '--quiet', '--bare', remote]);
    await git(root, 'remote', 'add', 'origin', remote);
    const pushed = await fix(root, {
      ...GOOD,
      commands: [['git', 'push', 'origin', 'HEAD:refs/heads/agent']],
    });
    expect(pushed.code, pushed.stdout).toBe(EXIT_OK);
    expect((await runGit(path.dirname(root), ['--git-dir', remote, 'for-each-ref'])).stdout).toBe(
      '',
    );
    const tagged = await fix(root, { ...GOOD, commands: [['git', 'tag', 'v9.9.9']] });
    expect(tagged.code).toBe(EXIT_VIOLATIONS);
    expect(tagged.stdout).toContain('the agent created refs/tags/v9.9.9');
    expect(await git(root, 'tag', '--list')).toBe('');
  });

  it('nothing outside the worktree: a ref, the git config, a hook or your files changed fails the run and is put back', async () => {
    const root = await repo();
    const main = await git(root, 'rev-parse', 'main');
    const outside = path.join(root, 'src', 'app.js').replaceAll('\\', '/');
    for (const [commands, text] of [
      [
        [
          ['git', 'commit', '--allow-empty', '--quiet', '-m', 'x'],
          ['git', 'update-ref', 'refs/heads/main', 'HEAD'],
        ],
        'moved refs/heads/main',
      ],
      [[['git', 'config', 'core.hooksPath', '/dev/null']], "changed the repo's git config"],
      [[['git', 'branch', 'sneaky']], 'created refs/heads/sneaky'],
    ] as const) {
      const result = await fix(root, {
        ...GOOD,
        write: { ...GOOD.write, 'src/x.js': 'export {};\n' },
        commands: commands.map((c) => [...c]),
      });
      expect(result.code, text).toBe(EXIT_VIOLATIONS);
      expect(result.stdout).toContain(text);
    }
    expect(await git(root, 'rev-parse', 'main')).toBe(main);
    expect(await git(root, 'config', '--get', 'core.hooksPath').catch(() => '')).toBe('');
    expect(await branches(root)).toEqual(['main']);
    // A write through an absolute path into your checkout.
    const result = await fix(root, { ...GOOD, write: { ...GOOD.write, [outside]: 'changed\n' } });
    expect(result.code).toBe(EXIT_VIOLATIONS);
    expect(result.stdout).toContain('changed files in your checkout');
  });

  describe('after review', () => {
    it('a commit the agent makes itself, or a protected file renamed away, fails the run', async () => {
      const root = await repo({ '.husky/pre-commit': 'pnpm --silent fscale verify\n' });
      const committed = await fix(root, {
        write: { '.env.example': 'API_URL=\n', '.claude/settings.json': '{}\n' },
        commands: [
          ['git', 'add', '.claude/settings.json'],
          [
            'git',
            '-c',
            'user.name=a',
            '-c',
            'user.email=a@b.invalid',
            'commit',
            '--quiet',
            '-m',
            'sneaky',
          ],
        ],
        result: { ok: true },
      });
      expect(committed.code).toBe(EXIT_VIOLATIONS);
      expect(committed.stdout).toContain('made commits of its own');
      const renamed = await fix(root, {
        write: {
          '.env.example': 'API_URL=\n',
          '.husky/pre-commit': null,
          'src/hook.js': 'pnpm --silent fscale verify\n',
        },
        result: { ok: true },
      });
      expect(renamed.code).toBe(EXIT_VIOLATIONS);
      expect(renamed.stdout).toContain('protected path .husky/pre-commit');
      expect((await branches(root)).filter((b) => b.startsWith('fscale/'))).toEqual([]);
    });

    it('a remote with its own pushurl still gets no push', async () => {
      const root = await repo();
      const remote = path.join(root, '..', `remote-url-${path.basename(root)}.git`);
      await runGit(path.dirname(root), ['init', '--quiet', '--bare', remote]);
      await git(root, 'remote', 'add', 'origin', remote);
      await git(root, 'config', 'remote.origin.pushurl', remote);
      await fix(root, { ...GOOD, commands: [['git', 'push', 'origin', 'HEAD:refs/heads/agent']] });
      expect((await runGit(path.dirname(root), ['--git-dir', remote, 'for-each-ref'])).stdout).toBe(
        '',
      );
    });

    it("the repo's deny and ask rules narrow the run, and a repo that denies every edit is refused", async () => {
      const scoped = await repo({
        '.claude/settings.json': JSON.stringify({
          permissions: { deny: ['Edit(src/**)'], ask: ['Bash'] },
        }),
      });
      const result = await fix(scoped, {
        ...GOOD,
        write: { ...GOOD.write, 'src/app.js': APP + '// x\n' },
      });
      expect(result.code).toBe(EXIT_VIOLATIONS);
      expect(result.stdout).toContain('protected path src/app.js');
      expect(result.requests[0]?.request.policy.deniedCommands).toContain('Bash');
      const none = await repo({
        '.claude/settings.json': JSON.stringify({ permissions: { deny: ['Edit'] } }),
      });
      const refused = await fix(none, GOOD);
      expect(refused.code).toBe(EXIT_USAGE);
      expect(refused.stderr).toContain('deny every tool that edits files');
      expect(refused.requests).toEqual([]);
    });

    it('a secret in the code itself stops the run before it starts', async () => {
      const key = ['AKIA', 'IOSFODNN7', 'EXAMPLX'].join('');
      const root = await repo({ 'src/aws.js': `export const key = "${key}";\n` });
      const result = await fix(root, GOOD);
      expect(result.code).toBe(EXIT_USAGE);
      expect(result.stderr).toContain('core/no-secrets-in-source has findings');
      expect(result.requests).toEqual([]);
    });

    it('the ledger reason comes from the runner, not from what the agent says, and a cost below 0 is an error', async () => {
      const root = await repo();
      await fix(root, { ...GOOD, result: { ok: false, error: 'hit the time limit' } });
      await fix(root, { ...GOOD, result: { ok: true, costUsd: -50 } });
      const lines = await ledger(root);
      expect(lines.map((line) => line.reason)).toEqual(['agent-error', 'agent-error']);
      expect(lines.every((line) => line.costUsd === undefined)).toBe(true);
    });

    it('a worktree the agent locked is still removed when the run fails', async () => {
      const root = await repo();
      const result = await fix(root, {
        write: { '.env.example': 'OTHER=\n' },
        commands: [['git', 'worktree', 'lock', '.']],
        result: { ok: true },
      });
      expect(result.code).toBe(EXIT_VIOLATIONS);
      expect((await git(root, 'worktree', 'list')).split('\n')).toHaveLength(1);
      expect((await branches(root)).filter((b) => b.startsWith('fscale/'))).toEqual([]);
    });
  });
});
