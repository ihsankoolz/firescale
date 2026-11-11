#!/usr/bin/env node
// The npm launcher for fscale.
//
// fscale itself is a native binary that ships in a per-platform package
// (@firescale/cli-<os>-<arch>), installed through optionalDependencies. This
// file finds that binary and runs it with the same arguments, stdio and exit
// code. It contains no firescale logic.
import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import os from 'node:os';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

/** Platform packages, keyed by `${process.platform}-${process.arch}`. */
export const PLATFORM_PACKAGES = {
  'linux-x64': '@firescale/cli-linux-x64',
  'linux-arm64': '@firescale/cli-linux-arm64',
  'darwin-x64': '@firescale/cli-darwin-x64',
  'darwin-arm64': '@firescale/cli-darwin-arm64',
  'win32-x64': '@firescale/cli-win32-x64',
  'win32-arm64': '@firescale/cli-win32-arm64',
};

const here = path.dirname(fileURLToPath(import.meta.url));

let musl;

/** True on Linux systems that use musl instead of glibc (for example Alpine). Worked out once. */
function isMusl() {
  if (musl !== undefined) return musl;
  musl = false;
  if (process.platform === 'linux') {
    // Alpine marks itself with this file; checking it avoids building a full report.
    if (existsSync('/etc/alpine-release')) {
      musl = true;
    } else if (process.report) {
      process.report.excludeNetwork = true;
      const report = process.report.getReport();
      const header = typeof report === 'object' && report !== null ? report.header : undefined;
      musl = header !== undefined && !header.glibcVersionRuntime;
    }
  }
  return musl;
}

/** The version of this firescale package, or undefined when it cannot be read. */
function ownVersion() {
  try {
    return JSON.parse(readFileSync(path.join(here, '..', 'package.json'), 'utf8')).version;
  } catch {
    return undefined;
  }
}

function fail(message) {
  process.stderr.write(`fscale: ${message}\n`);
  process.exit(2);
}

/** Finds the native binary, or exits with a message that says what to do. */
function findBinary() {
  const override = process.env.FSCALE_BINARY;
  if (override) {
    if (!existsSync(override)) {
      fail(`FSCALE_BINARY points to "${override}", which does not exist. Fix or unset it.`);
    }
    return override;
  }

  const key = `${process.platform}-${process.arch}`;
  const name = PLATFORM_PACKAGES[key];
  if (!name || isMusl()) {
    const what = isMusl() ? `${key} with musl libc` : key;
    fail(
      `there is no prebuilt fscale binary for ${what} yet. Build it from source (cargo build --release -p firescale-cli) and set FSCALE_BINARY to the result.`,
    );
  }

  const file = process.platform === 'win32' ? 'fscale.exe' : 'fscale';
  const version = ownVersion();
  // Look from this package first, then from the project, which covers package
  // managers that do not let a package see its optional dependencies. A binary
  // found through the project must be the same version as this launcher, or it
  // would be paired with the wrong Node host.
  const places = [
    { from: path.join(here, 'fscale.js'), sameVersionOnly: false },
    { from: path.join(process.cwd(), 'package.json'), sameVersionOnly: true },
  ];
  for (const { from, sameVersionOnly } of places) {
    try {
      const manifest = createRequire(from).resolve(`${name}/package.json`);
      if (sameVersionOnly && version !== undefined) {
        if (JSON.parse(readFileSync(manifest, 'utf8')).version !== version) continue;
      }
      const binary = path.join(path.dirname(manifest), 'bin', file);
      if (existsSync(binary)) return binary;
    } catch {
      // Not resolvable from here; try the next place.
    }
  }
  fail(
    `the package ${name} is not installed, so there is no fscale binary to run. It is an optional dependency of firescale: reinstall without --no-optional / --omit=optional, or install ${name} directly.`,
  );
}

const binary = findBinary();
if (process.env.FSCALE_LOG?.split(',').includes('launcher')) {
  process.stderr.write(`launcher: binary ${binary}\n`);
}
// Ctrl+C reaches the binary directly. Without these listeners this process
// would die first and leave the binary writing after the prompt came back.
process.on('SIGINT', () => {});
process.on('SIGTERM', () => {});
const result = spawnSync(binary, process.argv.slice(2), {
  stdio: 'inherit',
  windowsHide: true,
  env: {
    ...process.env,
    // Tell the binary where the Node host and Node itself are, so it never has to search.
    FSCALE_NODE_HOST: process.env.FSCALE_NODE_HOST ?? path.join(here, '..', 'dist', 'host.js'),
    FSCALE_NODE: process.env.FSCALE_NODE ?? process.execPath,
  },
});

if (result.error) {
  fail(`could not start ${binary}: ${result.error.message}. Reinstall firescale and try again.`);
}
if (result.signal) {
  // Report death by signal the way a shell does: 128 plus the signal number.
  // Re-raising the signal is not reliable, because Node ignores or repurposes some.
  process.exit(128 + (os.constants.signals[result.signal] ?? 1));
}
process.exit(result.status ?? 1);
