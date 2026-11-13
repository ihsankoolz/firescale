import { mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { runGit } from '@firescale/sdk';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { commitAll, EXIT_OK, EXIT_VIOLATIONS, runCli } from './test-helpers.js';

// The tamper guard (M23, ADR 0042), on the labelled corpus in
// bench/tamper-corpus.json: every kind of finding has caught cases and
// not-a-finding cases. bench/tamper-rates.mjs counts the same corpus.

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

interface Case {
  name: string;
  expect: string[];
  note: string;
  base: Record<string, string>;
  head: Record<string, string | null>;
}

interface Finding {
  kind: string;
  file: string;
  line: number;
  onBase: boolean;
  what: string;
}

interface Tamper {
  policy: string;
  base: string;
  files: number;
  findings: Finding[];
  notChecked?: string[];
  counts: Record<string, number>;
}

const CORPUS = new URL('../../../bench/tamper-corpus.json', import.meta.url);
const corpus = JSON.parse(await readFile(CORPUS, 'utf8')) as { cases: Case[] };

const config = (tamper?: string) =>
  `${JSON.stringify({
    packs: ['core'],
    verify: {
      lint: false,
      typecheck: false,
      test: 'node -e 0',
      ...(tamper ? { tamper } : {}),
    },
  })}\n`;

async function put(root: string, files: Record<string, string | null>): Promise<void> {
  for (const [file, content] of Object.entries(files)) {
    const target = path.join(root, ...file.split('/'));
    if (content === null) {
      await rm(target, { force: true });
    } else {
      await mkdir(path.dirname(target), { recursive: true });
      await writeFile(target, content);
    }
  }
}

/** A repo whose main branch has `base` and whose working tree, on a branch, has `head`. */
async function caseRepo(
  base: Record<string, string>,
  head: Record<string, string | null>,
  policy?: { base?: string; head?: string },
): Promise<string> {
  const repo = await fixtures.create({
    files: {
      'firescale.config.json': config(policy?.base),
      '.gitignore': '.firescale/\n',
      ...base,
    },
    track: 'no-git',
  });
  const root = repo.root;
  expect((await runGit(root, ['init', '--quiet', '--initial-branch=main'])).code).toBe(0);
  await commitAll(root, 'base');
  expect((await runGit(root, ['checkout', '--quiet', '-b', 'change'])).code).toBe(0);
  await put(root, head);
  if (policy?.head !== undefined) await put(root, { 'firescale.config.json': config(policy.head) });
  return root;
}

async function tamper(
  root: string,
): Promise<{ code: number; tamper: Tamper | undefined; stdout: string }> {
  const result = await runCli(['verify', '--json'], root);
  const report = JSON.parse(result.stdout) as { tamper?: Tamper };
  return { code: result.code, tamper: report.tamper, stdout: result.stdout };
}

const kinds = (report?: Tamper): string[] =>
  [...new Set((report?.findings ?? []).map((finding) => finding.kind))].sort();

describe('the tamper guard on the labelled corpus', () => {
  it('has a caught case and a not-a-finding case for every kind, in JavaScript and TypeScript', () => {
    for (const kind of ['test-deleted', 'test-skipped', 'assertion-removed']) {
      const caught = corpus.cases.filter((c) => c.expect.includes(kind));
      expect(
        caught.some((c) => c.name.startsWith('ts:')),
        kind,
      ).toBe(true);
      expect(
        caught.some((c) => c.name.startsWith('js:')),
        kind,
      ).toBe(true);
    }
    const clean = corpus.cases.filter((c) => c.expect.length === 0);
    expect(clean.some((c) => c.name.startsWith('ts:'))).toBe(true);
    expect(clean.some((c) => c.name.startsWith('js:'))).toBe(true);
  });

  for (const labelled of corpus.cases) {
    it(`${labelled.name}: ${labelled.expect.length === 0 ? 'no finding' : labelled.expect.join(', ')}`, async () => {
      const root = await caseRepo(labelled.base, labelled.head);
      const { code, tamper: report } = await tamper(root);
      expect(code).toBe(EXIT_OK);
      expect(report?.policy).toBe('evaluated');
      expect(report?.notChecked ?? []).toEqual([]);
      expect(kinds(report)).toEqual([...labelled.expect].sort());
    });
  }
});

describe('what a finding says', () => {
  it('names the file and line, on the base for a deletion and in the working tree for a skip', async () => {
    const labelled = corpus.cases.find((c) => c.name === 'ts: a test deleted');
    if (!labelled) throw new Error('corpus case missing');
    const root = await caseRepo(labelled.base, {
      ...labelled.head,
      'src/math.test.ts': (labelled.head['src/math.test.ts'] ?? '').replace(
        'it("adds"',
        'it.skip("adds"',
      ),
    });
    const { tamper: report } = await tamper(root);
    expect(report?.findings).toEqual([
      {
        kind: 'test-deleted',
        file: 'src/math.test.ts',
        line: 9,
        onBase: true,
        what: 'math > subtracts',
      },
      {
        kind: 'test-skipped',
        file: 'src/math.test.ts',
        line: 4,
        onBase: false,
        what: 'math > adds',
      },
      {
        kind: 'assertion-removed',
        file: 'src/math.test.ts',
        line: 10,
        onBase: true,
        what: 'expect(sub(3,1)).toBe(2)',
      },
    ]);
    const human = await runCli(['verify', '--no-cache'], root);
    expect(human.stdout.split('\n')[0]).toBe('verify: OK - tamper guard: 3 report-only findings');
    expect(human.stdout).toContain(
      '  test deleted: src/math.test.ts:9 (on the base commit): math > subtracts\n',
    );
    expect(human.stdout).toContain('  test skipped: src/math.test.ts:4: math > adds\n');
    expect(human.stdout).toContain(
      '  assertion removed: src/math.test.ts:10 (on the base commit): expect(sub(3,1)).toBe(2)\n',
    );
  });

  it('marks a file it cannot parse as not checked, never as clean', async () => {
    const labelled = corpus.cases.find((c) => c.name === 'ts: a test deleted');
    if (!labelled) throw new Error('corpus case missing');
    const root = await caseRepo(labelled.base, {
      'src/math.test.ts': 'it("adds", () => { expect(1).toBe(1) ',
    });
    const { tamper: report } = await tamper(root);
    expect(report?.notChecked).toEqual(['src/math.test.ts']);
    expect(report?.findings).toEqual([]);
    expect((await runCli(['verify'], root)).stdout).toContain(
      'Tamper guard: not checked, because tree-sitter found syntax errors in them on the base or now: src/math.test.ts',
    );
  });
});

describe('the policy comes from the base commit', () => {
  const deleted = () => {
    const labelled = corpus.cases.find((c) => c.name === 'ts: a test deleted');
    if (!labelled) throw new Error('corpus case missing');
    return labelled;
  };

  it('is report-only by default', async () => {
    const { base, head } = deleted();
    const { code, tamper: report } = await tamper(await caseRepo(base, head));
    expect(code).toBe(EXIT_OK);
    expect(report?.policy).toBe('evaluated');
    expect(report?.findings.length).toBeGreaterThan(0);
  });

  it('fails verify when the base enforces it, even if the change turns it off', async () => {
    const { base, head } = deleted();
    const root = await caseRepo(base, head, { base: 'enforced', head: 'off' });
    const { code, tamper: report } = await tamper(root);
    expect(code).toBe(EXIT_VIOLATIONS);
    expect(report?.policy).toBe('enforced');
    const human = await runCli(['verify', '--no-cache'], root);
    expect(human.stdout.split('\n')[0]).toBe('verify: FAILED at the tamper guard (2 findings)');
    expect(human.stdout).toContain('verify.tamper is "enforced" on the base commit');
  });

  it('does not enforce what only the change enforces', async () => {
    const { base, head } = deleted();
    const { code, tamper: report } = await tamper(await caseRepo(base, head, { head: 'enforced' }));
    expect(code).toBe(EXIT_OK);
    expect(report?.policy).toBe('evaluated');
  });

  it('is off when the base turns it off', async () => {
    const { base, head } = deleted();
    const { code, tamper: report } = await tamper(await caseRepo(base, head, { base: 'off' }));
    expect(code).toBe(EXIT_OK);
    expect(report?.policy).toBe('off');
    expect(report?.findings).toEqual([]);
  });

  it('blocks an agent at the stop hook when the base enforces it', async () => {
    const { base, head } = deleted();
    const root = await caseRepo(base, head, { base: 'enforced' });
    const hook = await runCli(
      ['hooks', 'run', '--agent', 'claude'],
      root,
      {},
      {
        input: JSON.stringify({ session_id: 's1', stop_hook_active: false }),
      },
    );
    // Claude Code is blocked with exit code 2 and the summary on stderr.
    expect(hook.code).toBe(2);
    expect(hook.stderr).toContain('FAILED at the tamper guard');
    expect(hook.stderr).toContain('test deleted: src/math.test.ts:9 (on the base commit)');
  });

  it('rejects a policy it does not know', async () => {
    const { base } = deleted();
    const root = await caseRepo(base, {}, { head: 'strict' });
    const result = await runCli(['verify'], root);
    expect(result.stderr).toContain('verify.tamper must be "off", "evaluated"');
  });
});
