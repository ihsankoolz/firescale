#!/usr/bin/env node
// Prints the output blocks of README.md by running the built `fscale` on the
// repo's own test fixtures: the course-project fixture, the blame fixture of
// `packages/cli/src/blame.test.ts`, and one case of `bench/tamper-corpus.json`.
//
// Build first (`pnpm build`), then: node scripts/readme-output.mjs
//
// Each fixture is written to a new folder under the temp folder and removed
// at the end. Nothing in this repo is changed. Needs Node 22.18 or newer (it
// imports a TypeScript file) and git.

import { execFileSync, spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { courseProjectFiles } from '../packages/cli/src/course-fixture.ts';

const REPO = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const BIN = path.join(
  REPO,
  'target',
  'debug',
  process.platform === 'win32' ? 'fscale.exe' : 'fscale',
);
const made = [];

/** A new folder holding `files`. */
function folder(files) {
  const root = mkdtempSync(path.join(tmpdir(), 'firescale-readme-'));
  made.push(root);
  write(root, files);
  return root;
}

function write(root, files) {
  for (const [file, content] of Object.entries(files)) {
    const target = path.join(root, ...file.split('/'));
    mkdirSync(path.dirname(target), { recursive: true });
    writeFileSync(target, content);
  }
}

function git(root, ...args) {
  execFileSync(
    'git',
    [
      '-c',
      'user.name=fixture',
      '-c',
      'user.email=fixture@example.invalid',
      '-c',
      'commit.gpgsign=false',
      ...args,
    ],
    { cwd: root, stdio: 'ignore' },
  );
}

function commit(root, message) {
  git(root, 'add', '--all');
  git(root, 'commit', '--quiet', '--no-verify', '-m', message);
}

/** Runs fscale in `root` and prints the command, what it printed and its exit code. */
function show(title, root, args) {
  const result = spawnSync(BIN, args, { cwd: root, encoding: 'utf8' });
  if (result.error) {
    throw new Error(`Could not run ${BIN}: ${result.error.message}. Run \`pnpm build\` first.`);
  }
  console.log(`===== ${title}`);
  console.log(`$ fscale ${args.join(' ')}`);
  process.stdout.write(result.stdout.replaceAll('\r\n', '\n'));
  process.stdout.write(result.stderr.replaceAll('\r\n', '\n'));
  console.log(`(exit code ${result.status})\n`);
}

// 1. Rules: the course-project fixture, committed as it is.
const course = folder(courseProjectFiles());
git(course, 'init', '--quiet', '--initial-branch=main');
commit(course, 'the course project');
show('1. check', course, ['check']);
show('1. adopt --dry-run', course, ['adopt', '--dry-run']);

// 2. One definition of done: which projects a change reaches.
git(course, 'checkout', '--quiet', '-b', 'feature');
write(course, { 'backend/auth_service/main.py': 'from shared.db import URL\n\napp = 1\n' });
show('2. verify --affected --dry-run', course, ['verify', '--affected', '--dry-run']);

// 5. Doctor, on the same fixture. What it prints depends on this machine.
show('5. doctor', course, ['doctor']);

// 3. Blame: the fixture of blame.test.ts. The test step fails on the base
// commit; the change touches only what lint reads.
const stepScript = (file) =>
  `process.exit(require('fs').readFileSync(${JSON.stringify(file)}, 'utf8').includes('fail') ? 1 : 0);\n`;
const blame = folder({
  'firescale.config.json': JSON.stringify({
    packs: ['core'],
    verify: { lint: 'node lint.js', typecheck: false, test: 'node test.js' },
  }),
  '.gitignore': '.firescale/\n',
  'lint.js': stepScript('lint.txt'),
  'test.js': stepScript('test.txt'),
  'lint.txt': 'ok\n',
  'test.txt': 'fail\n',
  'package-lock.json': '{"lockfileVersion":3}\n',
});
git(blame, 'init', '--quiet', '--initial-branch=main');
commit(blame, 'base');
git(blame, 'checkout', '--quiet', '-b', 'feature');
write(blame, { 'lint.txt': 'ok, changed\n' });
show('3. verify, a failure that is on the base commit too', blame, ['verify']);
write(blame, { 'lint.txt': 'fail\n' });
show('3. verify, a failure the change made', blame, ['verify']);

// 4. Tamper guard: one case of the labelled corpus.
const CASE = 'ts: a test skipped with it.skip';
const corpus = JSON.parse(readFileSync(path.join(REPO, 'bench', 'tamper-corpus.json'), 'utf8'));
const found = (Array.isArray(corpus) ? corpus : corpus.cases).find((entry) => entry.name === CASE);
if (!found) throw new Error(`bench/tamper-corpus.json has no case "${CASE}".`);
const tamper = folder(found.base);
git(tamper, 'init', '--quiet', '--initial-branch=main');
commit(tamper, 'base');
git(tamper, 'checkout', '--quiet', '-b', 'feature');
write(tamper, found.head);
show(`4. tamper (corpus case "${CASE}")`, tamper, ['tamper']);

for (const root of made) rmSync(root, { recursive: true, force: true });
