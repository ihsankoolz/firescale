import { mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { runGit } from '@firescale/sdk';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { commitAll, EXIT_OK, EXIT_VIOLATIONS, runCli } from './test-helpers.js';

// The tamper guard (M23 and M24, ADR 0042 and ADR 0044), on the labelled
// corpus in bench/tamper-corpus.json: every kind of finding has caught cases
// and not-a-finding cases in every language. bench/tamper-rates.mjs counts
// the same corpus. The corpus was written with the guard, so these are
// fixtures, not a measured rate (M26 measures one).

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
  allowed?: { file: string; kinds: string[] }[];
  counts: Record<string, number>;
}

const CORPUS = new URL('../../../bench/tamper-corpus.json', import.meta.url);
const corpus = JSON.parse(await readFile(CORPUS, 'utf8')) as { cases: Case[] };

const config = (tamper?: unknown) =>
  `${JSON.stringify({
    packs: ['core'],
    // No step runs: the guard needs none, and a step per case would make
    // this file, which runs every corpus case, slower than it needs to be.
    verify: {
      lint: false,
      typecheck: false,
      test: false,
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
  policy?: { base?: unknown; head?: unknown },
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

const LANGUAGES = [
  'ts',
  'js',
  'py',
  'rs',
  'java',
  'kotlin',
  'swift',
  'dart',
  'c',
  'cpp',
  'cs',
  'go',
];
const KINDS = [
  'test-deleted',
  'test-skipped',
  'assertion-removed',
  'suppression-added',
  'not-checked',
];

// The config and CI detectors of M25 (ADR 0045), each with its own prefix.
const DETECTORS: Record<string, string> = {
  'cov:': 'coverage-lowered',
  'ci:': 'ci-weakened',
  'nv:': 'no-verify',
  'lint:': 'rule-disabled',
  'appr:': 'snapshot-changed',
};

describe('the tamper guard on the labelled corpus', () => {
  it('has a caught case and a not-a-finding case for each config and CI detector', () => {
    for (const [prefix, kind] of Object.entries(DETECTORS)) {
      const cases = corpus.cases.filter((c) => c.name.startsWith(`${prefix} `));
      expect(
        cases.some((c) => c.expect.includes(kind)),
        `${prefix} ${kind}`,
      ).toBe(true);
      expect(
        cases.some((c) => c.expect.length === 0),
        prefix,
      ).toBe(true);
    }
  });

  it('has a caught case for every kind, and not-a-finding cases, in every language', () => {
    for (const language of LANGUAGES) {
      const cases = corpus.cases.filter((c) => c.name.startsWith(`${language}:`));
      for (const kind of KINDS) {
        expect(
          cases.some((c) => c.expect.includes(kind)),
          `${language}: ${kind}`,
        ).toBe(true);
      }
      expect(
        cases.some((c) => c.expect.length === 0),
        language,
      ).toBe(true);
    }
  });

  for (const labelled of corpus.cases) {
    it(`${labelled.name}: ${labelled.expect.length === 0 ? 'no finding' : labelled.expect.join(', ')}`, async () => {
      const root = await caseRepo(labelled.base, labelled.head);
      const { code, tamper: report } = await tamper(root);
      expect(code).toBe(EXIT_OK);
      expect(report?.policy).toBe('evaluated');
      if (!labelled.expect.includes('not-checked')) {
        expect(report?.notChecked ?? []).toEqual([]);
      }
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
        what: 'expect(sub(3, 1)).toBe(2)',
      },
    ]);
    const human = await runCli(['verify', '--no-cache'], root);
    expect(human.stdout.split('\n')[0]).toBe('verify: OK - tamper guard: 3 report-only findings');
    expect(human.stdout).toContain(
      '  test deleted: src/math.test.ts:9 (on the base commit): math > subtracts\n',
    );
    expect(human.stdout).toContain('  test skipped: src/math.test.ts:4: math > adds\n');
    expect(human.stdout).toContain(
      '  assertion removed: src/math.test.ts:10 (on the base commit): expect(sub(3, 1)).toBe(2)\n',
    );
  });

  it('makes a file it cannot parse a finding of its own, which blocks when enforced', async () => {
    const labelled = corpus.cases.find((c) => c.name === 'ts: a test deleted');
    if (!labelled) throw new Error('corpus case missing');
    const head = { 'src/math.test.ts': 'it("adds", () => { expect(1).toBe(1) ' };
    const { tamper: report } = await tamper(await caseRepo(labelled.base, head));
    expect(report?.notChecked).toEqual(['src/math.test.ts']);
    expect(report?.findings).toEqual([
      {
        kind: 'not-checked',
        file: 'src/math.test.ts',
        line: 1,
        onBase: false,
        what: 'tree-sitter cannot read this file, so its tests and assertions were not compared',
      },
    ]);
    const enforced = await caseRepo(labelled.base, head, { base: 'enforced' });
    const blocked = await runCli(['verify', '--no-cache'], enforced);
    expect(blocked.code).toBe(EXIT_VIOLATIONS);
    expect(blocked.stdout.split('\n')[0]).toBe('verify: FAILED at the tamper guard (1 finding)');
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

  it('reads an enforced policy from a base config saved with a byte order mark', async () => {
    const { base, head } = deleted();
    const root = await caseRepo(base, {});
    await runGit(root, ['checkout', '--quiet', 'main']);
    await writeFile(path.join(root, 'firescale.config.json'), `\uFEFF${config('enforced')}`);
    await commitAll(root, 'enforce, from an editor that writes a BOM');
    await runGit(root, ['checkout', '--quiet', '-b', 'later']);
    await put(root, head);
    const { code, tamper: report } = await tamper(root);
    expect(report?.policy).toBe('enforced');
    expect(code).toBe(EXIT_VIOLATIONS);
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

describe('the policy keys come from the base commit too', () => {
  // A test file only `testPaths` makes one: `checks/` is not a test folder.
  const smoke = (tests: string[]) =>
    `import { add } from "../src/math";\n\n${tests
      .map((name) => `it("${name}", () => {\n  expect(add(1, 1)).toBe(2);\n});\n`)
      .join('\n')}`;
  const math = 'export const add = (a: number, b: number): number => a + b;\n';
  const base = {
    'src/math.ts': math,
    'checks/smoke.ts': smoke(['adds', 'adds again']),
  };
  const head = { 'checks/smoke.ts': smoke(['adds']) };

  it('reads a file named by the base testPaths as a test file, even when the change drops the key', async () => {
    const root = await caseRepo(base, head, {
      base: { testPaths: ['checks/**'] },
      head: 'evaluated',
    });
    const { tamper: report } = await tamper(root);
    expect(kinds(report)).toEqual(['assertion-removed', 'test-deleted']);
    expect(report?.findings.find((f) => f.kind === 'test-deleted')?.file).toBe('checks/smoke.ts');
  });

  it('does not read a file that only the change names in testPaths', async () => {
    const root = await caseRepo(base, head, { head: { testPaths: ['checks/**'] } });
    const { tamper: report } = await tamper(root);
    expect(report?.findings).toEqual([]);
  });

  it('leaves out what the base allows, and nothing that only the change allows', async () => {
    const tests = (names: string[]) =>
      `import { add } from "./math";\n\n${names
        .map((name) => `it("${name}", () => {\n  expect(add(2, 2)).toBe(4);\n});\n`)
        .join('\n')}`;
    const root = await caseRepo(
      {
        'src/math.ts': math,
        'src/math.test.ts': tests(['a', 'b']),
        'src/legacy/old.test.ts': tests(['c', 'd']).replaceAll('./math', '../math'),
      },
      {
        'src/math.test.ts': tests(['a']),
        'src/legacy/old.test.ts': tests(['c']).replaceAll('./math', '../math'),
      },
      { base: { allow: ['src/legacy/**'] }, head: { allow: ['src/**'] } },
    );
    const { tamper: report } = await tamper(root);
    expect(new Set(report?.findings.map((f) => f.file))).toEqual(new Set(['src/math.test.ts']));
    expect(kinds(report)).toEqual(['assertion-removed', 'test-deleted']);
    expect(report?.allowed).toEqual([
      {
        file: 'src/legacy/old.test.ts',
        kinds: [
          'test-deleted',
          'test-skipped',
          'assertion-removed',
          'suppression-added',
          'coverage-lowered',
          'ci-weakened',
          'no-verify',
          'rule-disabled',
          'snapshot-changed',
          'expected-changed',
          'not-checked',
        ],
      },
    ]);
    const human = await runCli(['verify', '--no-cache'], root);
    expect(human.stdout).toContain(
      'verify.tamper.allow on the base commit leaves out src/legacy/old.test.ts (test deleted, test skipped, assertion removed, suppression added, coverage lowered, CI weakened, hooks skipped, rule turned off, snapshot changed, expected value changed, not checked).',
    );
  });

  it('allows only the kinds an allow entry names', async () => {
    const generated = (body: string) => `${body}export const id = (x: number): number => x;\n`;
    const root = await caseRepo(
      {
        'src/math.ts': math,
        'src/gen/api.ts': generated(''),
        'src/gen/api.test.ts': smoke(['a', 'b']).replace('../src/math', '../math'),
      },
      {
        'src/gen/api.ts': generated('// eslint-disable-next-line no-console\nconsole.log(1);\n'),
        'src/gen/api.test.ts': smoke(['a']).replace('../src/math', '../math'),
      },
      { base: { allow: [{ path: 'src/gen/**', kinds: ['suppression-added'] }] } },
    );
    const { tamper: report } = await tamper(root);
    expect(kinds(report)).toEqual(['assertion-removed', 'test-deleted']);
  });

  it('blocks with the object form of an enforced policy', async () => {
    const root = await caseRepo(base, head, {
      base: { policy: 'enforced', testPaths: ['checks/**'] },
      head: { policy: 'off' },
    });
    const { code, tamper: report } = await tamper(root);
    expect(report?.policy).toBe('enforced');
    expect(code).toBe(EXIT_VIOLATIONS);
  });

  it('rejects a key it does not know, and says which it knows', async () => {
    const root = await caseRepo(base, {}, { head: { policy: 'enforced', exempt: ['x'] } });
    const result = await runCli(['verify'], root);
    expect(result.stderr).toContain(
      'unknown key "exempt" in verify.tamper. Allowed keys: policy, testPaths, allow',
    );
  });
});
