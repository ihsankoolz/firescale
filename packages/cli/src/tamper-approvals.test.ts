import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { runGit } from '@firescale/sdk';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { commitAll, EXIT_OK, EXIT_USAGE, EXIT_VIOLATIONS, runCli } from './test-helpers.js';

// Approvals (M26, ADR 0046): a changed snapshot and a changed expected value
// need a person's approval. Approvals are read only from GitHub's reviews,
// in a pull_request_target run where none of the change's code has run;
// nothing the change writes counts as one.

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

const CART =
  'export const total = (items: number[]): number => items.reduce((a, b) => a + b, 0);\n';
const TEST = (value: number) =>
  `import { total } from "./cart";\n\nit("adds the items", () => {\n  expect(total([10, 20])).toBe(${value});\n});\n\nit("renders", () => {\n  expect(String(total([1]))).toMatchSnapshot();\n});\n`;
const SNAP = (value: string) => `// Jest Snapshot v1\n\nexports[\`renders 1\`] = \`"${value}"\`;\n`;
const SNAP_PATH = 'src/__snapshots__/cart.test.ts.snap';

const config = `${JSON.stringify({ packs: ['core'], verify: { lint: false, typecheck: false, test: false, tamper: 'enforced' } })}\n`;

async function git(root: string, ...args: string[]): Promise<string> {
  const result = await runGit(root, [
    '-c',
    'user.name=fixture',
    '-c',
    'user.email=fixture@example.invalid',
    '-c',
    'commit.gpgsign=false',
    ...args,
  ]);
  if (result.code !== 0) throw new Error(`git ${args.join(' ')}: ${result.stderr}`);
  return result.stdout.trim();
}

interface Run {
  root: string;
  base: string;
  head: string;
  older: string;
  env: Record<string, string>;
  reviews: (list: unknown[]) => Promise<string>;
}

/**
 * main has the cart, its test and a snapshot. The pull request (by
 * `owner`) changes either the expected value or the snapshot, in two
 * commits; the run is a pull_request_target run on main.
 */
async function pullRequest(
  change: 'expected' | 'snapshot',
  extra: Record<string, string> = {},
): Promise<Run> {
  const repo = await fixtures.create({
    files: {
      'firescale.config.json': config,
      'src/cart.ts': CART,
      'src/cart.test.ts': TEST(30),
      [SNAP_PATH]: SNAP('1'),
    },
    track: 'no-git',
  });
  const root = repo.root;
  await git(root, 'init', '--quiet', '--initial-branch=main');
  await commitAll(root, 'base');
  const base = await git(root, 'rev-parse', 'HEAD');
  await git(root, 'checkout', '--quiet', '-b', 'change');
  await writeFile(path.join(root, 'src', 'cart.ts'), CART.replace('0);', '1);'));
  await commitAll(root, 'off by one');
  const older = await git(root, 'rev-parse', 'HEAD');
  if (change === 'expected') {
    await writeFile(path.join(root, 'src', 'cart.test.ts'), TEST(31));
  } else {
    await writeFile(path.join(root, ...SNAP_PATH.split('/')), SNAP('2'));
  }
  for (const [file, text] of Object.entries(extra)) {
    await mkdir(path.dirname(path.join(root, ...file.split('/'))), { recursive: true });
    await writeFile(path.join(root, ...file.split('/')), text);
  }
  await commitAll(root, 'Make the tests pass\n\nApproved-by: reviewer');
  const head = await git(root, 'rev-parse', 'HEAD');
  await git(root, 'checkout', '--quiet', 'main');
  const outside = path.join(root, '..', `pr-${path.basename(root)}`);
  await mkdir(outside, { recursive: true });
  const event = path.join(outside, 'event.json');
  await writeFile(
    event,
    JSON.stringify({
      pull_request: { base: { sha: base }, head: { sha: head }, user: { login: 'owner' } },
    }),
  );
  const env = {
    GITHUB_ACTIONS: 'true',
    GITHUB_EVENT_NAME: 'pull_request_target',
    GITHUB_EVENT_PATH: event,
    GITHUB_SHA: base,
  };
  const reviews = async (list: unknown[]): Promise<string> => {
    const file = path.join(outside, 'reviews.json');
    await writeFile(file, JSON.stringify(list));
    return file;
  };
  return { root, base, head, older, env, reviews };
}

const review = (
  login: string,
  commit: string,
  association = 'MEMBER',
  state = 'APPROVED',
  type = 'User',
) => ({
  user: { login, type },
  state,
  commit_id: commit,
  author_association: association,
});

interface Report {
  findings: { kind: string; file: string; line: number; what: string }[];
  approvals?: { head: string; by: string[] };
}

async function guard(run: Run, list: unknown[]) {
  const file = await run.reviews(list);
  const json = await runCli(
    ['tamper', '--json', '--pr-base', '--approvals', file],
    run.root,
    run.env,
  );
  const human = await runCli(['tamper', '--pr-base', '--approvals', file], run.root, run.env);
  return {
    code: json.code,
    report: JSON.parse(json.stdout) as Report,
    human: human.stdout,
    stderr: json.stderr,
  };
}

describe.each([
  [
    'expected',
    'expected-changed',
    'expect(total([10, 20])).toBe(31) (on the base: expect(total([10, 20])).toBe(30), line 4)',
  ],
  ['snapshot', 'snapshot-changed', 'snapshot `renders 1` was rewritten'],
] as const)('a changed %s', (change, kind, what) => {
  it('blocks until a person approves the pull request on GitHub', async () => {
    const run = await pullRequest(change);
    const none = await guard(run, []);
    expect(none.code, none.stderr).toBe(EXIT_VIOLATIONS);
    expect(none.report.findings.map((f) => f.kind)).toEqual([kind]);
    expect(none.report.findings[0]?.what).toBe(what);
    expect(none.human.split('\n')[0]).toBe('tamper: FAILED - 1 finding');
    expect(none.human).toContain("[needs an approving review of the pull request's last commit]");

    const approved = await guard(run, [review('reviewer', run.head)]);
    expect(approved.code, approved.stderr).toBe(EXIT_OK);
    expect(approved.report.approvals).toEqual({ head: run.head, by: ['reviewer'] });
    expect(approved.human).toContain('[approved by reviewer]');
  });

  it('does not count an approval the change could make itself', async () => {
    const run = await pullRequest(change, {
      // Files and words in the change that look like approvals are never read.
      '.firescale/approvals.json': JSON.stringify([review('reviewer', 'x'.repeat(40))]),
      '.github/approvals.yml': 'approved-by: reviewer\n',
    });
    for (const [why, list] of [
      ['the pull request author', [review('owner', run.head, 'OWNER')]],
      ['an older commit of the pull request', [review('reviewer', run.older)]],
      ['someone without write access', [review('passer-by', run.head, 'CONTRIBUTOR')]],
      ['a bot', [review('helper[bot]', run.head, 'MEMBER', 'APPROVED', 'Bot')]],
      [
        'an approval taken back',
        [review('reviewer', run.head), review('reviewer', run.head, 'MEMBER', 'DISMISSED')],
      ],
    ] as const) {
      const result = await guard(run, [...list]);
      expect(result.code, why).toBe(EXIT_VIOLATIONS);
      expect(result.report.approvals?.by, why).toEqual([]);
    }
    // A file of approvals is read only in a pull_request_target run: in the
    // pull request's own job, or on a laptop, the change could write it.
    const file = await run.reviews([review('reviewer', run.head)]);
    for (const env of [
      { ...run.env, GITHUB_EVENT_NAME: 'pull_request' },
      { GITHUB_ACTIONS: undefined },
    ]) {
      const refused = await runCli(['tamper', '--pr-base', '--approvals', file], run.root, env);
      expect(refused.code).toBe(EXIT_USAGE);
      expect(refused.stderr).toContain(
        '--approvals is read only with --pr-base in a pull_request_target run',
      );
    }
    const local = await runCli(
      ['tamper', '--approvals', file, '--base', 'main', '--head', run.head],
      run.root,
    );
    expect(local.code).toBe(EXIT_USAGE);
  });

  it('is reported, not blocking, where the change runs, since nothing can approve there', async () => {
    const run = await pullRequest(change);
    await git(run.root, 'checkout', '--quiet', 'change');
    const verify = await runCli(['verify', '--json', '--base', 'main'], run.root);
    expect(verify.code, verify.stderr).toBe(EXIT_OK);
    const report = (JSON.parse(verify.stdout) as { tamper: Report }).tamper;
    expect(report.findings.map((f) => f.kind)).toEqual([kind]);
    const human = await runCli(['verify', '--no-cache', '--base', 'main'], run.root);
    expect(human.stdout).toContain(
      "[needs a person's approval, given in the pull request; it does not block here]",
    );
  });
});
