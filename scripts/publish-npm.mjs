#!/usr/bin/env node
// Publishes the eight firescale packages from the tarballs the Release
// workflow built, in the order that never leaves `firescale` pointing at a
// package that is not there: the six platform packages, then @firescale/sdk,
// then firescale.
//
// Safe to run again: a package version that is already on the registry is
// skipped, so a release that stopped halfway can be finished.
//
// Authentication is npm trusted publishing (OIDC). There is no token: the
// job needs `id-token: write`, npm 11.5.1 or later and Node 22.14.0 or later.
//
// A pre-release version (one with a "-" part, such as 1.0.0-rc.1) is
// published under the dist-tag "next", never "latest". npm refuses a
// pre-release without a tag.
//
// Usage: node scripts/publish-npm.mjs --dir <folder with the artifacts> --version <version> [--dry-run]
//   --dry-run   look everything up and print the plan, publish nothing
//   <folder> holds one subfolder per artifact, named npm-<platform>.
// FSCALE_NPM_COMMAND replaces `npm` (used by the tests).
// The registry can be changed with FSCALE_NPM_REGISTRY (used by the tests).
import { spawnSync } from 'node:child_process';
import { readdir } from 'node:fs/promises';
import path from 'node:path';
import process from 'node:process';
import { setTimeout as sleep } from 'node:timers/promises';

const PLATFORMS = [
  'darwin-arm64',
  'darwin-x64',
  'linux-arm64',
  'linux-x64',
  'win32-arm64',
  'win32-x64',
];
/** The artifact whose copies of the two platform-independent tarballs are published. */
const SHARED_FROM = 'linux-x64';
const MIN_NPM = [11, 5, 1];
const MIN_NODE = [22, 14, 0];
const USAGE = 'Usage: node scripts/publish-npm.mjs --dir <folder> --version <version> [--dry-run]';

function fail(message) {
  process.stderr.write(`publish-npm: ${message}\n`);
  process.exit(1);
}

function parseArgs(argv) {
  const options = { dryRun: false };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--dry-run') {
      options.dryRun = true;
    } else if (arg === '--dir' || arg === '--version') {
      const value = argv[i + 1];
      if (value === undefined || value.startsWith('--')) fail(`${arg} needs a value. ${USAGE}`);
      options[arg.slice(2)] = value;
      i += 1;
    } else {
      fail(`unknown argument "${arg}". ${USAGE}`);
    }
  }
  if (!options.dir || !options.version) fail(`--dir and --version are both needed. ${USAGE}`);
  return options;
}

function atLeast(version, minimum) {
  const parts = version.split('.').map((part) => Number.parseInt(part, 10));
  for (let i = 0; i < 3; i += 1) {
    if ((parts[i] ?? 0) !== minimum[i]) return (parts[i] ?? 0) > minimum[i];
  }
  return true;
}

/** Every `.tgz` below `dir`, as `{ file, folder }` with `folder` the artifact it came from. */
async function tarballs(dir) {
  const found = [];
  const walk = async (current, depth) => {
    for (const entry of await readdir(current, { withFileTypes: true })) {
      const full = path.join(current, entry.name);
      if (entry.isDirectory() && depth < 3) await walk(full, depth + 1);
      else if (entry.isFile() && entry.name.endsWith('.tgz')) found.push(full);
    }
  };
  await walk(dir, 0).catch((error) => {
    fail(
      `cannot read the folder ${dir} (${error.message}). Pass the folder the artifacts were downloaded to.`,
    );
  });
  return found.sort();
}

/** The eight packages in publish order, each with the tarball to publish. */
function plan(files, version) {
  const pick = (name, tarball, artifact) => {
    const matches = files.filter(
      (file) =>
        path.basename(file) === tarball &&
        (artifact === undefined || file.split(path.sep).includes(`npm-${artifact}`)),
    );
    if (matches.length === 0) {
      fail(
        `no tarball ${tarball}${artifact ? ` in the artifact npm-${artifact}` : ''} for ${name}@${version}. Check that every build job finished and that the tag matches the version.`,
      );
    }
    return { name, file: matches[0] };
  };
  return [
    ...PLATFORMS.map((platform) =>
      pick(`@firescale/cli-${platform}`, `firescale-cli-${platform}-${version}.tgz`, platform),
    ),
    pick('@firescale/sdk', `firescale-sdk-${version}.tgz`, SHARED_FROM),
    pick('firescale', `firescale-${version}.tgz`, SHARED_FROM),
  ];
}

/** True when `name@version` is on the registry. Any answer but 200 or 404 is an error. */
async function isPublished(registry, name, version) {
  const url = `${registry}/${name.replace('/', '%2f')}/${version}`;
  let lastError = '';
  for (let attempt = 1; attempt <= 3; attempt += 1) {
    try {
      const response = await globalThis.fetch(url, { headers: { accept: 'application/json' } });
      if (response.status === 200) return true;
      if (response.status === 404) return false;
      lastError = `HTTP ${response.status}`;
    } catch (error) {
      lastError = error instanceof Error ? error.message : String(error);
    }
    if (attempt < 3) await sleep(attempt * 2000);
  }
  return fail(
    `could not find out whether ${name}@${version} is on the registry (${lastError}). Nothing more was published. Run the job again.`,
  );
}

function npm(args, options = {}) {
  const override = process.env.FSCALE_NPM_COMMAND;
  if (override) {
    // A stand-in for npm: a Node script that gets the same arguments.
    return spawnSync(process.execPath, [override, ...args], { encoding: 'utf8', ...options });
  }
  // On Windows npm is a .cmd file and needs a shell, so an argument with a
  // space in it must be quoted by hand.
  const windows = process.platform === 'win32';
  const quoted = windows ? args.map((arg) => (/\s/.test(arg) ? `"${arg}"` : arg)) : args;
  return spawnSync('npm', quoted, { encoding: 'utf8', shell: windows, ...options });
}

const options = parseArgs(process.argv.slice(2));
const registry = (process.env.FSCALE_NPM_REGISTRY ?? 'https://registry.npmjs.org').replace(
  /\/+$/,
  '',
);
const steps = plan(await tarballs(path.resolve(options.dir)), options.version);

if (!options.dryRun) {
  const node = process.versions.node;
  if (!atLeast(node, MIN_NODE)) {
    fail(
      `npm trusted publishing needs Node ${MIN_NODE.join('.')} or later, and this is ${node}. Use a newer Node in the publish job.`,
    );
  }
  const npmVersion = npm(['--version']).stdout?.trim() ?? '';
  if (!atLeast(npmVersion, MIN_NPM)) {
    fail(
      `npm trusted publishing needs npm ${MIN_NPM.join('.')} or later, and this is ${npmVersion || 'unknown'}. Use a Node version that ships it, or run \`npm install -g npm@latest\` first.`,
    );
  }
}

// npm refuses to publish a pre-release as "latest".
const distTag = options.version.includes('-') ? ['--tag', 'next'] : [];

let published = 0;
let skipped = 0;
for (const step of steps) {
  const label = `${step.name}@${options.version}`;
  if (await isPublished(registry, step.name, options.version)) {
    process.stdout.write(`skip     ${label} (already on the registry)\n`);
    skipped += 1;
    continue;
  }
  if (options.dryRun) {
    process.stdout.write(`publish  ${label} from ${path.basename(step.file)} (dry run)\n`);
    continue;
  }
  process.stdout.write(`publish  ${label} from ${path.basename(step.file)}\n`);
  // A hung publish must not hold the runner: ten minutes is far more than one needs.
  const result = npm(['publish', step.file, '--access', 'public', ...distTag], {
    stdio: 'inherit',
    timeout: 600_000,
  });
  if (result.status !== 0) {
    // A second run of this job may have published it in the meantime.
    if (await isPublished(registry, step.name, options.version)) {
      process.stdout.write(`skip     ${label} (it is on the registry now)\n`);
      skipped += 1;
      continue;
    }
    fail(
      `npm publish failed for ${label}. ${published} package(s) were published before it in this run. Fix the cause (RELEASING.md lists the usual ones: a missing or expired trusted publisher shows as ENEEDAUTH) and run the job again: packages that are already published are skipped.`,
    );
  }
  published += 1;
}
process.stdout.write(
  options.dryRun
    ? `Dry run: ${steps.length - skipped} to publish, ${skipped} already on the registry.\n`
    : `Done: ${published} published, ${skipped} already on the registry.\n`,
);
