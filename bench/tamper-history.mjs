#!/usr/bin/env node
// Runs `fscale tamper --head <commit> --base <commit>^` over the last
// commits of a git repo's first-parent history, and writes one JSON line per
// commit: its findings, or the error. Reads git objects only: nothing of the
// repo is checked out, installed or run.
//
// Usage: node bench/tamper-history.mjs --bin <fscale> --repo <dir>
//          --name <owner/repo> --language <js|ts|py|rs> --commits <n> --out <file.jsonl>
//          [--all-parents]

import { execFileSync, spawnSync } from 'node:child_process';
import { appendFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';

const args = process.argv.slice(2);
const arg = (name, fallback) => {
  const at = args.indexOf(`--${name}`);
  return at >= 0 ? args[at + 1] : fallback;
};
const bin = path.resolve(arg('bin', 'target/release/fscale'));
const repo = path.resolve(arg('repo'));
const name = arg('name', path.basename(repo));
const language = arg('language', '?');
const count = Number(arg('commits', '150'));
const out = arg('out');
// Every non-merge commit, not only the first-parent line (for repos that
// merge branches rather than squash them).
const allParents = args.includes('--all-parents');
if (!out) throw new Error('--out <file.jsonl> is required');

const git = (...argv) =>
  execFileSync('git', ['-C', repo, ...argv], { stdio: ['ignore', 'pipe', 'pipe'] }).toString();
const commits = git(
  'log',
  ...(allParents ? [] : ['--first-parent']),
  '--no-merges',
  '--format=%H',
  '-n',
  String(count + 1),
)
  .split('\n')
  .filter(Boolean)
  // The oldest commit of a shallow clone has no parent here.
  .filter(
    (commit) =>
      spawnSync('git', ['-C', repo, 'rev-parse', '--verify', '--quiet', `${commit}^`]).status === 0,
  )
  .slice(0, count);

writeFileSync(out, '');
let findings = 0;
for (const commit of commits) {
  const started = process.hrtime.bigint();
  const run = spawnSync(bin, ['tamper', '--json', '--head', commit, '--base', `${commit}^`], {
    cwd: repo,
    env: { ...process.env, GITHUB_ACTIONS: '' },
    maxBuffer: 64 * 1024 * 1024,
  });
  const ms = Number((process.hrtime.bigint() - started) / 1_000_000n);
  const line = { repo: name, language, commit, ms };
  if (run.status === 0 || run.status === 1) {
    const report = JSON.parse(run.stdout.toString());
    line.files = report.files;
    line.findings = report.findings;
    line.notChecked = report.notChecked ?? [];
    findings += report.findings.length;
  } else {
    line.error = run.stderr.toString().trim();
  }
  appendFileSync(out, `${JSON.stringify(line)}\n`);
}
console.log(`${name}: ${commits.length} commits, ${findings} findings -> ${out}`);
