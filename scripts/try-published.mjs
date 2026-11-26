#!/usr/bin/env node
// Runs what npm serves as firescale@<spec>, the way a new user would: in a new
// empty folder, with a new empty npm cache, `npx --yes firescale@<spec>` with
// init, check, verify and doctor, in that order. Each must exit 0.
//
// The release's staging tests run it with `next`, the production smoke with
// `latest` (ADR 0092). In a folder made by init, doctor also checks that the
// agent hooks' `npx --yes firescale@<version>` starts.
//
// Usage: node scripts/try-published.mjs <dist-tag or version> [--keep]
// FSCALE_NPX_COMMAND replaces `npx --yes firescale@<spec>` (used by the tests).
import { spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import process from 'node:process';

const COMMANDS = ['init', 'check', 'verify', 'doctor'];
const USAGE = 'Usage: node scripts/try-published.mjs <dist-tag or version> [--keep]';

function fail(message) {
  process.stderr.write(`try-published: ${message}\n`);
  process.exit(1);
}

const argv = process.argv.slice(2);
const keep = argv.includes('--keep');
const rest = argv.filter((arg) => arg !== '--keep');
if (rest.length !== 1) fail(USAGE);
const spec = (rest[0] ?? '').replace(/^v(?=\d)/, '');
// The spec goes into a command line, so it must be a plain tag or version.
if (!/^[a-z][a-z0-9-]*$/.test(spec) && !/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(spec)) {
  fail(`"${rest[0]}" is not a dist-tag or a version (such as next, latest or 1.2.3). ${USAGE}`);
}
const prefix = process.env.FSCALE_NPX_COMMAND || `npx --yes firescale@${spec}`;
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

let failed;
try {
  for (const command of COMMANDS) {
    const started = Date.now();
    const result = spawnSync(`${prefix} ${command}`, {
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
    if (result.status === 0) {
      process.stdout.write(`PASS  ${label} ${command} after ${seconds} s\n`);
      continue;
    }
    const code = result.status ?? 'none: timed out or killed';
    process.stdout.write(`FAIL  ${label} ${command} (exit ${code}) after ${seconds} s\n`);
    process.stdout.write(`${tail(result.stdout)}\n${tail(result.stderr)}\n`);
    failed = `\`${label} ${command}\` exited ${code} in a new folder made by \`${label} init\`. Read its output above. Do not promote this version.`;
    break;
  }
} finally {
  if (keep) process.stdout.write(`kept: ${folder}\n`);
  else rmSync(folder, { recursive: true, force: true, maxRetries: 5 });
  rmSync(cache, { recursive: true, force: true, maxRetries: 5 });
}
if (failed) fail(failed);
process.stdout.write(`${label}: init, check, verify and doctor all exit 0.\n`);
