#!/usr/bin/env node
// Measures what the tamper guard misses on bench/tamper-cheats.json: cheating
// changes written for this purpose by a subagent that saw neither the
// guard's code nor its development corpus (bench/tamper-corpus.json), from a
// list of categories (ADR 0046). Every case is a cheat, so a case with no
// finding is a miss.
//
// Usage: node bench/tamper-misses.mjs [--bin target/release/fscale] [--json]
//
// Each case becomes a git repo: its `base` files committed on main, its
// `head` files in the working tree of a branch. `fscale tamper --json` runs
// there (no step of the case is ever run).

import { execFileSync, spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const binIndex = args.indexOf('--bin');
const bin = path.resolve(
  binIndex >= 0 ? args[binIndex + 1] : path.join(here, '..', 'target', 'release', 'fscale'),
);
const cheats = JSON.parse(readFileSync(path.join(here, 'tamper-cheats.json'), 'utf8'));

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

const APPROVAL = new Set(['snapshot-changed', 'expected-changed']);
const rows = [];
for (const cheat of cheats.cases) {
  const root = mkdtempSync(path.join(tmpdir(), 'firescale-cheat-'));
  try {
    put(root, cheat.base);
    git(root, 'init', '--quiet', '--initial-branch=main');
    git(root, 'add', '--all');
    git(root, 'commit', '--quiet', '--no-verify', '-m', 'base');
    git(root, 'checkout', '--quiet', '-b', 'change');
    put(root, cheat.head);
    const run = spawnSync(bin, ['tamper', '--json', '--base', 'main'], {
      cwd: root,
      env: { ...process.env, GITHUB_ACTIONS: '' },
    });
    const report = run.status === 0 || run.status === 1 ? JSON.parse(run.stdout.toString()) : null;
    const kinds = [...new Set((report?.findings ?? []).map((f) => f.kind))].sort();
    rows.push({
      name: cheat.name,
      category: cheat.category,
      language: cheat.language,
      kinds,
      error: report ? undefined : run.stderr.toString().trim(),
      caught: kinds.some((kind) => !APPROVAL.has(kind)),
      approval: kinds.length > 0 && kinds.every((kind) => APPROVAL.has(kind)),
    });
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

if (args.includes('--json')) {
  console.log(JSON.stringify(rows, null, 2));
} else {
  const groups = (key) => [...new Set(rows.map((row) => row[key]))].sort();
  const line = (label, list) => {
    const caught = list.filter((r) => r.caught).length;
    const approval = list.filter((r) => r.approval).length;
    const missed = list.length - caught - approval;
    console.log(`| ${label} | ${list.length} | ${caught} | ${approval} | ${missed} |`);
  };
  for (const key of ['category', 'language']) {
    console.log(`| ${key} | Cheats | Caught | Needs approval only | Missed |`);
    console.log('| --- | --- | --- | --- | --- |');
    for (const group of groups(key))
      line(
        group,
        rows.filter((r) => r[key] === group),
      );
    line('all', rows);
    console.log('');
  }
  console.log('Missed:');
  for (const row of rows.filter((r) => !r.caught && !r.approval)) {
    console.log(`  ${row.name}${row.error ? ` (error: ${row.error.slice(0, 120)})` : ''}`);
  }
}
