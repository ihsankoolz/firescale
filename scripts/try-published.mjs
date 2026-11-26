#!/usr/bin/env node
// Runs what npm serves as firescale@<spec>, the way a new user would: in a new
// empty folder, with a new empty npm cache, `npx --yes firescale@<spec>` with
// init, check, verify and doctor, in that order. Each must exit 0.
//
// The release's staging tests run it with `next`, the production smoke with
// `latest` (ADR 0092). In a folder made by init, doctor also checks that the
// agent hooks' `npx --yes firescale@<version>` starts.
//
// With --expect <version>, it first checks, in the same new cache, that
// `firescale@<spec> --version` prints that version, then installs
// firescale@<version> too. In a folder made by init, doctor runs
// `npx --no -- firescale@<version>`; npx keys its cache by the spec as typed,
// so firescale@next alone may leave nothing there for it to find.
//
// Usage: node scripts/try-published.mjs <dist-tag or version> [--expect <version>] [--keep]
// FSCALE_NPX_COMMAND replaces `npx --yes firescale@<spec>` (used by the tests);
// `{spec}` in it is replaced by the spec.
import { spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import process from 'node:process';

const COMMANDS = ['init', 'check', 'verify', 'doctor'];
const USAGE =
  'Usage: node scripts/try-published.mjs <dist-tag or version> [--expect <version>] [--keep]';
const VERSION = /^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/;

function fail(message) {
  process.stderr.write(`try-published: ${message}\n`);
  process.exit(1);
}

const argv = process.argv.slice(2);
const keep = argv.includes('--keep');
let expected;
const rest = [];
for (let i = 0; i < argv.length; i += 1) {
  if (argv[i] === '--keep') continue;
  if (argv[i] === '--expect') {
    expected = argv[i + 1] ?? '';
    if (!VERSION.test(expected)) fail(`"${expected}" is not a version such as 1.2.3. ${USAGE}`);
    i += 1;
    continue;
  }
  rest.push(argv[i]);
}
if (rest.length !== 1) fail(USAGE);
const spec = (rest[0] ?? '').replace(/^v(?=\d)/, '');
// The spec goes into a command line, so it must be a plain tag or version.
if (!/^[a-z][a-z0-9-]*$/.test(spec) && !VERSION.test(spec)) {
  fail(`"${rest[0]}" is not a dist-tag or a version (such as next, latest or 1.2.3). ${USAGE}`);
}
const npx = (version) =>
  process.env.FSCALE_NPX_COMMAND
    ? process.env.FSCALE_NPX_COMMAND.replaceAll('{spec}', version)
    : `npx --yes firescale@${version}`;
const label = `firescale@${spec}`;

const folder = mkdtempSync(path.join(os.tmpdir(), 'firescale-try-'));
const cache = mkdtempSync(path.join(os.tmpdir(), 'firescale-try-cache-'));
const tail = (text) =>
  (text ?? '')
    .trim()
    .split(/\r?\n/)
    .slice(-30)
    .map((line) => `      ${line}`)
    .join('\n');

const runs = [
  ...(expected
    ? [
        { spec, command: '--version' },
        { spec: expected, command: '--version' },
      ]
    : []),
  ...COMMANDS.map((command) => ({ spec, command })),
];

let failed;
try {
  for (const { spec: runSpec, command } of runs) {
    const started = Date.now();
    const runLabel = `firescale@${runSpec}`;
    const result = spawnSync(`${npx(runSpec)} ${command}`, {
      cwd: folder,
      encoding: 'utf8',
      shell: true,
      // A first install can be slow; nothing here should take ten minutes.
      timeout: 600_000,
      env: {
        ...process.env,
        npm_config_cache: cache,
        npm_config_prefer_online: 'true',
        npm_config_update_notifier: 'false',
        npm_config_fund: 'false',
        npm_config_audit: 'false',
      },
    });
    const seconds = ((Date.now() - started) / 1000).toFixed(1);
    const printed = (result.stdout ?? '').trim();
    if (result.status === 0 && command === '--version' && printed !== expected) {
      failed = `${runLabel} printed ${printed}, not ${expected}. The tag has not reached this runner yet, or points elsewhere: check the dist-tags on npmjs.com, then run this job again.`;
      break;
    }
    if (result.status === 0) {
      process.stdout.write(`PASS  ${runLabel} ${command} after ${seconds} s\n`);
      continue;
    }
    const code = result.status ?? 'none: timed out or killed';
    process.stdout.write(`FAIL  ${runLabel} ${command} (exit ${code}) after ${seconds} s\n`);
    process.stdout.write(`${tail(result.stdout)}\n${tail(result.stderr)}\n`);
    failed = `\`${runLabel} ${command}\` exited ${code} in a new folder made by \`${label} init\`. Read its output above. Do not promote this version.`;
    break;
  }
} finally {
  if (keep) process.stdout.write(`kept: ${folder}\n`);
  else rmSync(folder, { recursive: true, force: true, maxRetries: 5 });
  rmSync(cache, { recursive: true, force: true, maxRetries: 5 });
}
if (failed) fail(failed);
process.stdout.write(`${label}: init, check, verify and doctor all exit 0.\n`);
