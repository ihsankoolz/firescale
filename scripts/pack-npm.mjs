#!/usr/bin/env node
// Builds the npm tarballs for one platform:
//   firescale (JS launcher + Node host), @firescale/sdk, @firescale/cli-<os>-<arch> (the binary).
//
// Usage: node scripts/pack-npm.mjs [--binary <path>] [--platform <os>-<arch>] [--out <dir>]
import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { chmod, copyFile, cp, mkdir, readFile, rm, stat, writeFile } from 'node:fs/promises';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

const PLATFORMS = [
  'linux-x64',
  'linux-arm64',
  'darwin-x64',
  'darwin-arm64',
  'win32-x64',
  'win32-arm64',
];

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

function fail(message) {
  process.stderr.write(`pack-npm: ${message}\n`);
  process.exit(1);
}

function parseArgs(argv) {
  const options = {};
  for (let i = 0; i < argv.length; i += 1) {
    const flag = argv[i];
    if (flag !== '--binary' && flag !== '--platform' && flag !== '--out') {
      fail(
        `unknown argument "${flag}". Usage: node scripts/pack-npm.mjs [--binary <path>] [--platform <os>-<arch>] [--out <dir>]`,
      );
    }
    const value = argv[i + 1];
    if (value === undefined || value.startsWith('--')) fail(`${flag} needs a value.`);
    options[flag.slice(2)] = value;
    i += 1;
  }
  return options;
}

async function exists(file) {
  try {
    await stat(file);
    return true;
  } catch {
    return false;
  }
}

async function readJson(file) {
  return JSON.parse(await readFile(file, 'utf8'));
}

async function workspaceVersion() {
  const cargo = await readFile(path.join(root, 'Cargo.toml'), 'utf8');
  const section = /^\[workspace\.package\]\s*$([\s\S]*?)(?=^\[|(?![\s\S]))/m.exec(cargo)?.[1];
  const version = section && /^version\s*=\s*"([^"]+)"/m.exec(section)?.[1];
  if (!version) fail('Cargo.toml has no version under [workspace.package]. Add one and try again.');
  return version;
}

/** Runs `npm pack` in `cwd`, writing the tarball to `out`. Returns the tarball file name. */
function npmPack(cwd, out) {
  // On Windows npm is a .cmd file and needs a shell, so a path with spaces must be quoted.
  const destination = process.platform === 'win32' && /\s/.test(out) ? `"${out}"` : out;
  const stdout = execFileSync('npm', ['pack', '--pack-destination', destination], {
    cwd,
    encoding: 'utf8',
    shell: process.platform === 'win32',
    stdio: ['ignore', 'pipe', 'inherit'],
  });
  const lines = stdout.trim().split(/\r?\n/);
  const name = lines[lines.length - 1]?.trim();
  if (!name) fail(`npm pack printed no file name in ${cwd}. Run it there by hand to see why.`);
  return name;
}

const options = parseArgs(process.argv.slice(2));
const platform = options.platform ?? `${process.platform}-${process.arch}`;
if (!PLATFORMS.includes(platform)) {
  fail(`unknown platform "${platform}". Use one of: ${PLATFORMS.join(', ')}.`);
}
const [os, cpu] = platform.split('-');
const exe = os === 'win32' ? 'fscale.exe' : 'fscale';
const binary = path.resolve(options.binary ?? path.join(root, 'target', 'release', exe));
const out = path.resolve(options.out ?? path.join(root, 'dist-npm'));

if (!(await exists(binary))) {
  fail(
    `the binary ${binary} does not exist. Build it (cargo build --release -p firescale-cli) or pass --binary <path>.`,
  );
}
for (const built of [
  path.join(root, 'packages', 'cli', 'dist', 'host.js'),
  path.join(root, 'packages', 'sdk', 'dist', 'index.js'),
]) {
  if (!(await exists(built))) fail(`${built} is missing. Run pnpm build first.`);
}

const version = await workspaceVersion();
const cliManifest = await readJson(path.join(root, 'packages', 'cli', 'package.json'));
const sdkManifest = await readJson(path.join(root, 'packages', 'sdk', 'package.json'));
const repository = (directory) => ({ ...cliManifest.repository, directory });
const publishConfig = { access: 'public' };

const stage = path.join(out, 'stage');
await rm(stage, { recursive: true, force: true, maxRetries: 5 });
await mkdir(stage, { recursive: true });

async function writeManifest(dir, manifest) {
  await writeFile(path.join(dir, 'package.json'), `${JSON.stringify(manifest, null, 2)}\n`);
}

/** A copy of `object` without the listed keys. */
const without = (object, keys) =>
  Object.fromEntries(Object.entries(object).filter(([key]) => !keys.includes(key)));

const notTest = (source) => !/(\.test\.|[\\/]test-helpers\.)/.test(source);

/**
 * Filter for a package's `dist` folder: skips tests, source maps (their sources
 * do not ship), and build output whose source file no longer exists. `tsc` never
 * deletes old output, so a long-lived checkout can hold files for deleted modules.
 */
function builtFrom(srcDir) {
  return (source) => {
    if (!notTest(source) || source.endsWith('.map')) return false;
    const match = /^(.*?)(\.d\.ts|\.js)$/.exec(path.basename(source));
    if (!match) return true; // a folder, or a file tsc did not produce
    return existsSync(path.join(srcDir, `${match[1]}.ts`));
  };
}

try {
  const license = path.join(root, 'LICENSE');

  // a. The platform package: only the binary.
  const platformName = `@firescale/cli-${platform}`;
  const platformDir = path.join(stage, 'platform');
  await mkdir(path.join(platformDir, 'bin'), { recursive: true });
  const platformManifest = {
    name: platformName,
    version,
    description: `The fscale binary for ${platform}. Install firescale, not this package.`,
    license: 'MIT',
    repository: repository(`packages/cli-${platform}`),
    os: [os],
    cpu: [cpu],
    ...(os === 'linux' ? { libc: ['glibc'] } : {}),
    files: ['bin'],
    preferUnplugged: true,
    publishConfig,
  };
  await writeManifest(platformDir, platformManifest);
  const stagedBinary = path.join(platformDir, 'bin', exe);
  await copyFile(binary, stagedBinary);
  await chmod(stagedBinary, 0o755);
  await writeFile(
    path.join(platformDir, 'README.md'),
    `# ${platformName}\n\nThis package is the \`fscale\` binary for ${platform}. Do not install it directly: install [\`firescale\`](https://www.npmjs.com/package/firescale), which picks the right binary for your machine.\n`,
  );
  await copyFile(license, path.join(platformDir, 'LICENSE'));

  // b. @firescale/sdk
  const sdkDir = path.join(stage, 'sdk');
  await mkdir(sdkDir, { recursive: true });
  const sdkPublished = without(sdkManifest, ['private', 'scripts', 'devDependencies']);
  await writeManifest(sdkDir, { ...sdkPublished, version, publishConfig });
  await cp(path.join(root, 'packages', 'sdk', 'dist'), path.join(sdkDir, 'dist'), {
    recursive: true,
    filter: builtFrom(path.join(root, 'packages', 'sdk', 'src')),
  });
  await cp(path.join(root, 'packages', 'sdk', 'templates'), path.join(sdkDir, 'templates'), {
    recursive: true,
  });
  await copyFile(license, path.join(sdkDir, 'LICENSE'));
  await writeFile(
    path.join(sdkDir, 'README.md'),
    '# @firescale/sdk\n\nThe rule authoring API for [firescale](https://www.npmjs.com/package/firescale): `defineRule`, types and test helpers.\n',
  );

  // c. firescale
  const cliDir = path.join(stage, 'firescale');
  await mkdir(cliDir, { recursive: true });
  const cliPublished = without(cliManifest, ['scripts', 'devDependencies']);
  await writeManifest(cliDir, {
    ...cliPublished,
    version,
    dependencies: { '@firescale/sdk': version },
    optionalDependencies: Object.fromEntries(
      PLATFORMS.map((key) => [`@firescale/cli-${key}`, version]),
    ),
  });
  for (const folder of ['bin', 'dist', 'schemas']) {
    await cp(path.join(root, 'packages', 'cli', folder), path.join(cliDir, folder), {
      recursive: true,
      filter: folder === 'dist' ? builtFrom(path.join(root, 'packages', 'cli', 'src')) : notTest,
    });
  }
  await copyFile(license, path.join(cliDir, 'LICENSE'));
  await copyFile(path.join(root, 'README.md'), path.join(cliDir, 'README.md'));

  await mkdir(out, { recursive: true });
  const tarballs = {
    firescale: npmPack(cliDir, out),
    sdk: npmPack(sdkDir, out),
    platform: npmPack(platformDir, out),
  };
  await writeFile(
    path.join(out, 'manifest.json'),
    `${JSON.stringify({ version, platform, tarballs }, null, 2)}\n`,
  );
  for (const [kind, file] of Object.entries(tarballs)) {
    process.stdout.write(`${kind}: ${path.join(out, file)}\n`);
  }
} finally {
  await rm(stage, { recursive: true, force: true, maxRetries: 5 });
}
