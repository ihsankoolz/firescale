#!/usr/bin/env node
// Checks, from an empty folder, that `npx firescale@<version> --version`
// prints <version>. A new package can take a few minutes to become
// available, so it tries again for a while before giving up.
//
// Usage: node scripts/verify-published.mjs <version or tag> [--attempts <n>] [--wait <seconds>]
// FSCALE_NPX_COMMAND replaces the command that is run (used by the tests).
import { spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import process from 'node:process';
import { setTimeout as sleep } from 'node:timers/promises';

function fail(message) {
  process.stderr.write(`verify-published: ${message}\n`);
  process.exit(1);
}

const argv = process.argv.slice(2);
let version;
let attempts = 20;
let wait = 15;
for (let i = 0; i < argv.length; i += 1) {
  const arg = argv[i];
  if (arg === '--attempts' || arg === '--wait') {
    const value = Number.parseInt(argv[i + 1] ?? '', 10);
    if (!Number.isInteger(value) || value < 0) fail(`${arg} needs a whole number.`);
    if (arg === '--attempts') attempts = Math.max(1, value);
    else wait = value;
    i += 1;
  } else if (version === undefined && !arg.startsWith('-')) {
    version = arg.replace(/^v/, '');
  } else {
    fail(
      `unknown argument "${arg}". Usage: node scripts/verify-published.mjs <version> [--attempts <n>] [--wait <seconds>]`,
    );
  }
}
if (!version) fail('no version given. Usage: node scripts/verify-published.mjs <version>');
// The version goes into a command line, so it must be a plain version.
if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(version)) {
  fail(`"${version}" is not a version such as 0.1.1.`);
}

// An empty folder outside any repo, and a new, empty npm cache for every
// attempt. npm keeps a package's metadata for five minutes without asking
// again, so one shared cache would repeat the first "not found" for the whole
// retry window, and would reuse a half-finished install.
const command = process.env.FSCALE_NPX_COMMAND ?? `npx --yes firescale@${version} --version`;

let last = '';
let ok = false;
for (let attempt = 1; attempt <= attempts && !ok; attempt += 1) {
  const folder = mkdtempSync(path.join(os.tmpdir(), 'firescale-verify-'));
  const cache = mkdtempSync(path.join(os.tmpdir(), 'firescale-npm-cache-'));
  try {
    const result = spawnSync(command, {
      cwd: folder,
      encoding: 'utf8',
      shell: true,
      // A hung install counts as a failed attempt.
      timeout: 180_000,
      env: {
        ...process.env,
        npm_config_cache: cache,
        npm_config_prefer_online: 'true',
        npm_config_update_notifier: 'false',
      },
    });
    const printed = (result.stdout ?? '').trim();
    if (result.status === 0 && printed === version) {
      process.stdout.write(`firescale@${version} is installable: --version printed ${printed}\n`);
      ok = true;
    } else {
      last =
        result.status === 0
          ? `it printed "${printed}"`
          : `exit ${result.status ?? 'none, timed out or killed'}: ${(result.stderr ?? '').trim().split(/\r?\n/).slice(-3).join(' | ')}`;
      process.stdout.write(`attempt ${attempt} of ${attempts}: not yet (${last})\n`);
    }
  } finally {
    for (const dir of [folder, cache]) {
      rmSync(dir, { recursive: true, force: true, maxRetries: 5 });
    }
  }
  if (!ok && attempt < attempts) await sleep(wait * 1000);
}
if (!ok) {
  fail(
    `\`${command}\` did not print ${version} after ${attempts} attempts (${last}). Check https://www.npmjs.com/package/firescale and that all eight packages have version ${version}; then run this job again.`,
  );
}
