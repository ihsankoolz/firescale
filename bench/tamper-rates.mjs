#!/usr/bin/env node
// Counts what the tamper guard (ADR 0042, ADR 0044) catches and gets wrong
// on the labelled corpus in bench/tamper-corpus.json.
//
// The corpus was written together with the guard, so these counts show that
// the guard does what its ADRs say. They are not a measured false-positive
// or miss rate on real changes: M26 measures that.
//
// Usage: node bench/tamper-rates.mjs [--bin target/debug/fscale]
//
// Each case becomes a git repo: its `base` files committed on main, its
// `head` files in the working tree of a branch. `fscale verify --json` runs
// there, and the kinds of finding it reports are compared with the case's
// label. A caught case's label lists the kinds it must give; a
// not-a-finding case's label is empty.

import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const binIndex = args.indexOf('--bin');
const bin = path.resolve(
  binIndex >= 0 ? args[binIndex + 1] : path.join(here, '..', 'target', 'debug', 'fscale'),
);
const corpus = JSON.parse(readFileSync(path.join(here, 'tamper-corpus.json'), 'utf8'));

const git = (cwd, ...argv) =>
  execFileSync(
    'git',
    [
      '-c',
      'user.name=bench',
      '-c',
      'user.email=bench@example.invalid',
      '-c',
      'commit.gpgsign=false',
      '-c',
      'core.autocrlf=false',
      ...argv,
    ],
    { cwd, stdio: 'pipe' },
  );

function put(root, files) {
  for (const [file, content] of Object.entries(files)) {
    const target = path.join(root, ...file.split('/'));
    if (content === null) {
      rmSync(target, { force: true });
    } else {
      mkdirSync(path.dirname(target), { recursive: true });
      writeFileSync(target, content);
    }
  }
}

const kinds = [
  'test-deleted',
  'test-skipped',
  'assertion-removed',
  'suppression-added',
  'not-checked',
];
const languages = { ts: 'TypeScript', js: 'JavaScript', py: 'Python', rs: 'Rust' };
const tally = Object.fromEntries(kinds.map((kind) => [kind, { cases: 0, caught: 0, missed: 0 }]));
const byLanguage = Object.fromEntries(
  Object.keys(languages).map((key) => [
    key,
    { caught: 0, cleanCases: 0, missed: 0, falsePositives: 0, cases: 0 },
  ]),
);
let cleanCases = 0;
let falsePositives = 0;
let cleanWithFinding = 0;
const wrong = [];

for (const labelled of corpus.cases) {
  const root = mkdtempSync(path.join(tmpdir(), 'firescale-tamper-'));
  try {
    put(root, {
      'firescale.config.json': JSON.stringify({
        packs: ['core'],
        verify: { lint: false, typecheck: false, test: 'node -e 0' },
      }),
      '.gitignore': '.firescale/\n',
      ...labelled.base,
    });
    git(root, 'init', '--quiet', '--initial-branch=main');
    git(root, 'add', '--all');
    git(root, 'commit', '--quiet', '--no-verify', '-m', 'base');
    git(root, 'checkout', '--quiet', '-b', 'change');
    put(root, labelled.head);
    let stdout;
    try {
      stdout = execFileSync(bin, ['verify', '--json'], { cwd: root, stdio: 'pipe' }).toString();
    } catch (error) {
      stdout = error.stdout?.toString() ?? '{}';
    }
    const report = JSON.parse(stdout);
    const found = new Set((report.tamper?.findings ?? []).map((finding) => finding.kind));
    const language = byLanguage[labelled.name.split(':')[0]];
    language.cases += 1;
    for (const kind of labelled.expect) {
      tally[kind].cases += 1;
      if (found.has(kind)) {
        tally[kind].caught += 1;
        language.caught += 1;
      } else {
        tally[kind].missed += 1;
        language.missed += 1;
        wrong.push(`missed ${kind}: ${labelled.name}`);
      }
    }
    const extra = [...found].filter((kind) => !labelled.expect.includes(kind));
    if (labelled.expect.length === 0) {
      cleanCases += 1;
      language.cleanCases += 1;
      if (found.size > 0) cleanWithFinding += 1;
    }
    falsePositives += extra.length;
    language.falsePositives += extra.length;
    for (const kind of extra) wrong.push(`false positive ${kind}: ${labelled.name}`);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

const caughtCases = corpus.cases.length - cleanCases;
console.log(
  `Tamper guard on bench/tamper-corpus.json (${corpus.cases.length} cases: ${caughtCases} with findings to catch, ${cleanCases} with none)`,
);
console.log('');
console.log('| Kind | Cases that must give it | Caught | Missed |');
console.log('| ---- | ----------------------- | ------ | ------ |');
for (const kind of kinds) {
  const { cases, caught, missed } = tally[kind];
  console.log(`| ${kind} | ${cases} | ${caught} | ${missed} |`);
}
console.log('');
console.log(
  '| Language | Cases | Not-a-finding cases | Findings caught | Missed | False positives |',
);
console.log(
  '| -------- | ----- | ------------------- | --------------- | ------ | --------------- |',
);
for (const [key, name] of Object.entries(languages)) {
  const row = byLanguage[key];
  console.log(
    `| ${name} | ${row.cases} | ${row.cleanCases} | ${row.caught} | ${row.missed} | ${row.falsePositives} |`,
  );
}
console.log('');
console.log(`False positives (a kind reported that the case does not have): ${falsePositives}`);
console.log(`Not-a-finding cases with any finding: ${cleanWithFinding} of ${cleanCases}`);
for (const line of wrong) console.log(`  ${line}`);
console.log('');
console.log(
  'These are counts on fixtures written with the guard, not a measured false-positive or miss rate (M26).',
);
process.exitCode = wrong.length === 0 ? 0 : 1;
