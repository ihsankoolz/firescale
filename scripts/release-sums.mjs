#!/usr/bin/env node
// SHA-256 sums of the release tarballs, so every job of the Release workflow
// uses the bytes the build job made (ADR 0092).
//
//   write <dir>   writes <dir>/SHA256SUMS for every .tgz in <dir>, in the
//                 format `sha256sum -c` reads: "<hex>  <file name>", sorted.
//   check <dir>   finds every SHA256SUMS in <dir> and the folders below it (the
//                 downloaded artifacts: one folder per platform), and fails
//                 unless each listed tarball is there with the same bytes and
//                 every tarball is listed.
//
// Usage: node scripts/release-sums.mjs <write|check> <dir>
import { createHash } from 'node:crypto';
import { readdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import process from 'node:process';

const SUMS = 'SHA256SUMS';
const USAGE = 'Usage: node scripts/release-sums.mjs <write|check> <dir>';

function fail(message) {
  process.stderr.write(`release-sums: ${message}\n`);
  process.exit(1);
}

/** Forward slashes in every path this script prints. */
const shown = (root, file) => path.relative(root, file).split(path.sep).join('/');

async function sha256(file) {
  return createHash('sha256')
    .update(await readFile(file))
    .digest('hex');
}

async function entries(dir) {
  try {
    return await readdir(dir, { withFileTypes: true });
  } catch (error) {
    return fail(
      `cannot read the folder ${dir} (${error.message}). Pass the folder that holds the tarballs.`,
    );
  }
}

async function write(dir) {
  const names = (await entries(dir))
    .filter((entry) => entry.isFile() && entry.name.endsWith('.tgz'))
    .map((entry) => entry.name)
    .sort();
  if (names.length === 0) fail(`no .tgz in ${dir}. Run scripts/pack-npm.mjs first.`);
  const lines = [];
  for (const name of names) lines.push(`${await sha256(path.join(dir, name))}  ${name}`);
  await writeFile(path.join(dir, SUMS), `${lines.join('\n')}\n`);
  process.stdout.write(`${SUMS}: ${names.length} tarballs in ${dir}\n`);
}

/** Every folder at or below `dir` (three levels), with its file names. */
async function folders(dir) {
  const found = [];
  const walk = async (current, depth) => {
    const list = await entries(current);
    found.push({ dir: current, files: list.filter((e) => e.isFile()).map((e) => e.name) });
    if (depth < 3) {
      for (const entry of list) {
        if (entry.isDirectory()) await walk(path.join(current, entry.name), depth + 1);
      }
    }
  };
  await walk(dir, 0);
  return found;
}

async function check(root) {
  const problems = [];
  let matched = 0;
  let lists = 0;
  for (const { dir, files } of await folders(root)) {
    const tarballs = files.filter((name) => name.endsWith('.tgz'));
    if (!files.includes(SUMS)) {
      for (const name of tarballs) {
        problems.push(`${shown(root, path.join(dir, name))} has no ${SUMS} beside it`);
      }
      continue;
    }
    lists += 1;
    const listed = new Map();
    for (const line of (await readFile(path.join(dir, SUMS), 'utf8')).split(/\r?\n/)) {
      if (line.trim() === '') continue;
      const match = /^([0-9a-f]{64}) [ *](.+)$/.exec(line);
      if (!match) {
        problems.push(
          `${shown(root, path.join(dir, SUMS))} has a line that is not "<sha256>  <file>": ${line}`,
        );
        continue;
      }
      if (listed.has(match[2])) {
        problems.push(`${shown(root, path.join(dir, match[2]))} is listed twice in ${SUMS}`);
        continue;
      }
      listed.set(match[2], match[1]);
    }
    if (listed.size === 0) {
      problems.push(`${shown(root, path.join(dir, SUMS)) || SUMS} lists no tarball`);
    }
    for (const [name, sum] of listed) {
      const file = path.join(dir, name);
      if (!files.includes(name)) {
        problems.push(`${shown(root, file)} is in ${SUMS} but missing`);
      } else if ((await sha256(file)) !== sum) {
        problems.push(`${shown(root, file)} does not match ${SUMS}`);
      } else {
        matched += 1;
      }
    }
    for (const name of tarballs) {
      if (!listed.has(name))
        problems.push(`${shown(root, path.join(dir, name))} is not in ${SUMS}`);
    }
  }
  if (lists === 0 && problems.length === 0) {
    problems.push(`no ${SUMS} in ${root} or below it`);
  }
  if (problems.length > 0) {
    fail(
      `${problems.join('\n  ')}\nThe tarballs are not the bytes the build job made. Do not publish them: run the build again.`,
    );
  }
  process.stdout.write(`${matched} tarballs match ${SUMS} (${lists} folders)\n`);
}

const [command, dir, ...rest] = process.argv.slice(2);
if (!dir || rest.length > 0) fail(USAGE);
if (command === 'write') await write(path.resolve(dir));
else if (command === 'check') await check(path.resolve(dir));
else fail(`unknown command "${command}". ${USAGE}`);
