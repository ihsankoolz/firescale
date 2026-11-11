#!/usr/bin/env node
// The npm launcher for fscale.
//
// fscale itself is a native binary that ships in a per-platform package
// (@firescale/cli-<os>-<arch>), installed through optionalDependencies. This
// file finds that binary and runs it with the same arguments, stdio and exit
// code. It contains no firescale logic.
import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { createRequire } from 'node:module';
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

/** True on Linux systems that use musl instead of glibc (for example Alpine). */
function isMusl() {
  if (process.platform !== 'linux') return false;
  const report = process.report?.getReport();
  const header = typeof report === 'object' && report !== null ? report.header : undefined;
  return header !== undefined && !header.glibcVersionRuntime;
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
  // Look from this package first, then from the project, which covers package
  // managers that do not let a package see its optional dependencies.
  for (const from of [path.join(here, 'fscale.js'), path.join(process.cwd(), 'package.json')]) {
    try {
      const manifest = createRequire(from).resolve(`${name}/package.json`);
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
  // Pass the signal on so the caller sees the same outcome as a direct run.
  process.kill(process.pid, result.signal);
} else {
  process.exit(result.status ?? 1);
}
