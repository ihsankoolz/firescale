#!/usr/bin/env node
// Sets, or checks, the firescale version everywhere it is recorded:
//   Cargo.toml          [workspace.package] version
//   Cargo.lock          the entries of the workspace's own crates
//   package.json        the root and every packages/*/package.json
//
// Usage:
//   node scripts/bump-version.mjs <version>          write <version> everywhere
//   node scripts/bump-version.mjs --check            fail unless every place agrees
//   node scripts/bump-version.mjs --check --tag v<version>
//                                                    ... and agrees with the tag
//   Add --root <dir> to work on another checkout (used by the tests).
import { readdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

const USAGE =
  'Usage: node scripts/bump-version.mjs <version> | --check [--tag v<version>] [--root <dir>]';
// Plain semver, with an optional pre-release part. No build metadata: npm drops it.
const VERSION = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$/;

function fail(message) {
  process.stderr.write(`bump-version: ${message}\n`);
  process.exit(1);
}

function parseArgs(argv) {
  const options = { check: false, tag: undefined, root: undefined, version: undefined };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--check') {
      options.check = true;
    } else if (arg === '--tag' || arg === '--root') {
      const value = argv[i + 1];
      if (value === undefined || value.startsWith('--')) fail(`${arg} needs a value. ${USAGE}`);
      options[arg.slice(2)] = value;
      i += 1;
    } else if (arg.startsWith('-')) {
      fail(`unknown option "${arg}". ${USAGE}`);
    } else if (options.version === undefined) {
      options.version = arg;
    } else {
      fail(`unexpected argument "${arg}". ${USAGE}`);
    }
  }
  return options;
}

/** The `[workspace.package]` table of a Cargo.toml: its text and where it sits. */
function workspacePackage(cargo) {
  const match = /^\[workspace\.package\][^\n]*\n([\s\S]*?)(?=^\[|(?![\s\S]))/m.exec(cargo);
  if (!match) return undefined;
  return { body: match[1], start: match.index + match[0].length - match[1].length };
}

/** Names of the workspace's own crates, from each member's Cargo.toml. */
async function crateNames(root) {
  const dir = path.join(root, 'crates');
  const names = [];
  for (const entry of (await readdir(dir, { withFileTypes: true })).sort((a, b) =>
    a.name.localeCompare(b.name),
  )) {
    if (!entry.isDirectory()) continue;
    const text = await readFile(path.join(dir, entry.name, 'Cargo.toml'), 'utf8').catch(() => '');
    const name = /^\[package\][\s\S]*?^name\s*=\s*"([^"]+)"/m.exec(text)?.[1];
    if (name) names.push(name);
  }
  return names;
}

/** Every package.json that carries the version, repo-relative with forward slashes. */
async function manifests(root) {
  const files = ['package.json'];
  const dir = path.join(root, 'packages');
  for (const entry of (await readdir(dir, { withFileTypes: true })).sort((a, b) =>
    a.name.localeCompare(b.name),
  )) {
    if (!entry.isDirectory()) continue;
    const file = `packages/${entry.name}/package.json`;
    const exists = await readFile(path.join(root, ...file.split('/')), 'utf8').then(
      () => true,
      () => false,
    );
    if (exists) files.push(file);
  }
  return files;
}

const VERSION_LINE = /^(\s*"version"\s*:\s*")([^"]*)(")/m;

/** Reads every recorded version. Returns `[{ file, what, version }]`. */
async function readAll(root) {
  const found = [];
  const read = (file) => readFile(path.join(root, ...file.split('/')), 'utf8');

  const cargo = await read('Cargo.toml');
  const table = workspacePackage(cargo);
  const cargoVersion = table && /^version\s*=\s*"([^"]*)"/m.exec(table.body)?.[1];
  if (cargoVersion === undefined) {
    fail('Cargo.toml has no version under [workspace.package]. Add one and try again.');
  }
  found.push({ file: 'Cargo.toml', what: '[workspace.package] version', version: cargoVersion });

  const lock = await read('Cargo.lock');
  for (const name of await crateNames(root)) {
    const entry = new RegExp(`^name = "${name}"\\r?\\nversion = "([^"]*)"`, 'm').exec(lock);
    if (!entry) {
      fail(`Cargo.lock has no entry for the crate "${name}". Run \`cargo build\` and try again.`);
    }
    found.push({ file: 'Cargo.lock', what: name, version: entry[1] });
  }

  for (const file of await manifests(root)) {
    const version = VERSION_LINE.exec(await read(file))?.[2];
    if (version === undefined) fail(`${file} has no "version" field. Add one and try again.`);
    found.push({ file, what: '"version"', version });
  }
  return found;
}

async function writeAll(root, version) {
  const changed = [];
  const update = async (file, edit) => {
    const full = path.join(root, ...file.split('/'));
    const before = await readFile(full, 'utf8');
    const after = edit(before);
    if (after !== before) {
      // Only the version text changes, so line endings and formatting stay as they are.
      await writeFile(full, after);
      changed.push(file);
    }
  };

  await update('Cargo.toml', (text) => {
    const table = workspacePackage(text);
    const body = table.body.replace(/^(version\s*=\s*")[^"]*(")/m, `$1${version}$2`);
    return text.slice(0, table.start) + body + text.slice(table.start + table.body.length);
  });
  const names = await crateNames(root);
  await update('Cargo.lock', (text) =>
    names.reduce(
      (lock, name) =>
        lock.replace(
          new RegExp(`^(name = "${name}"\\r?\\nversion = ")[^"]*(")`, 'm'),
          `$1${version}$2`,
        ),
      text,
    ),
  );
  for (const file of await manifests(root)) {
    await update(file, (text) => text.replace(VERSION_LINE, `$1${version}$3`));
  }
  return changed;
}

const options = parseArgs(process.argv.slice(2));
const root = path.resolve(
  options.root ?? path.join(path.dirname(fileURLToPath(import.meta.url)), '..'),
);

if (options.check) {
  if (options.version !== undefined) fail(`--check takes no version. ${USAGE}`);
  const found = await readAll(root);
  const expected = found[0].version;
  const wrong = found.filter((entry) => entry.version !== expected);
  if (wrong.length > 0) {
    const lines = found.map((entry) => `  ${entry.version}  ${entry.file} (${entry.what})`);
    fail(
      `the version is not the same everywhere:\n${lines.join('\n')}\nRun \`node scripts/bump-version.mjs <version>\` to set them all.`,
    );
  }
  if (options.tag !== undefined && options.tag !== `v${expected}`) {
    fail(
      `the tag "${options.tag}" does not match the version ${expected} in Cargo.toml and the package manifests. Tag the commit v${expected}, or bump the version first with \`node scripts/bump-version.mjs <version>\` and tag the new commit.`,
    );
  }
  process.stdout.write(`${expected}\n`);
} else {
  if (options.tag !== undefined) fail(`--tag only works with --check. ${USAGE}`);
  if (options.version === undefined) fail(`no version given. ${USAGE}`);
  if (!VERSION.test(options.version)) {
    fail(
      `"${options.version}" is not a version such as 0.1.1 or 1.0.0-rc.1. Write it without a leading "v".`,
    );
  }
  const changed = await writeAll(root, options.version);
  const after = await readAll(root);
  const stale = after.filter((entry) => entry.version !== options.version);
  if (stale.length > 0) {
    fail(
      `could not set the version in: ${stale.map((entry) => `${entry.file} (${entry.what})`).join(', ')}. Edit those by hand.`,
    );
  }
  if (changed.length === 0) {
    process.stdout.write(`Already at ${options.version}. Nothing changed.\n`);
  } else {
    process.stdout.write(
      `Set the version to ${options.version} in:\n${changed.map((file) => `  ${file}`).join('\n')}\n`,
    );
  }
}
