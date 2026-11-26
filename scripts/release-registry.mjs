// What the release scripts share (ADR 0021, ADR 0092): the eight packages and
// their tarballs, running npm, and reading the registry.
//
// Where the registry is read from:
//   FSCALE_NPM_REGISTRY set   that URL (the tests' stand-in registry)
//   FSCALE_NPM_COMMAND set    the stand-in npm (the tests and the rehearsal):
//                             `<it> view ...`, so a rehearsal never reads npm
//   neither                   https://registry.npmjs.org, public reads only
//
// The real npm is never started in a GitHub Actions run whose event is not a
// pushed tag: a run started by hand never publishes or moves a tag. A
// rehearsal sets FSCALE_NPM_COMMAND to the stand-in.
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { readdir, readFile } from 'node:fs/promises';
import path from 'node:path';
import process from 'node:process';
import { setTimeout as sleep } from 'node:timers/promises';

export const PLATFORMS = [
  'darwin-arm64',
  'darwin-x64',
  'linux-arm64',
  'linux-x64',
  'win32-arm64',
  'win32-x64',
];
/** The artifact whose copies of the two platform-independent tarballs are published. */
export const SHARED_FROM = 'linux-x64';
/** The eight packages, in the order they are published and promoted: `firescale` last. */
export const PACKAGES = [
  ...PLATFORMS.map((p) => `@firescale/cli-${p}`),
  '@firescale/sdk',
  'firescale',
];

/** An error the scripts print with their own prefix, and exit 1. */
export class ReleaseError extends Error {}

/** Runs `main`, and prints a ReleaseError as `<name>: <message>` with exit code 1. */
export async function runScript(name, main) {
  try {
    await main();
  } catch (error) {
    if (!(error instanceof ReleaseError)) throw error;
    process.stderr.write(`${name}: ${error.message}\n`);
    process.exit(1);
  }
}

/** A plain version such as 1.2.3 or 1.2.3-rc.1: it goes into command lines. */
export const VERSION = /^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/;
/** A dist-tag such as next or latest. */
export const TAG = /^[a-z][a-z0-9-]*$/;
export const isPrerelease = (version) => version.includes('-');

/** -1, 0 or 1, by semver precedence (a pre-release is lower than its release). */
export function compareVersions(a, b) {
  const split = (v) => {
    const [core, pre] = v.split(/-(.*)/s);
    return { core: core.split('.').map(Number), pre: pre === undefined ? [] : pre.split('.') };
  };
  const x = split(a);
  const y = split(b);
  for (let i = 0; i < 3; i += 1) {
    if (x.core[i] !== y.core[i]) return x.core[i] > y.core[i] ? 1 : -1;
  }
  if (x.pre.length === 0 || y.pre.length === 0) {
    return x.pre.length === y.pre.length ? 0 : x.pre.length === 0 ? 1 : -1;
  }
  for (let i = 0; i < Math.max(x.pre.length, y.pre.length); i += 1) {
    const p = x.pre[i];
    const q = y.pre[i];
    if (p === undefined) return -1;
    if (q === undefined) return 1;
    if (p === q) continue;
    const pn = /^\d+$/.test(p);
    const qn = /^\d+$/.test(q);
    if (pn && qn) return Number(p) > Number(q) ? 1 : -1;
    if (pn !== qn) return pn ? -1 : 1;
    return p > q ? 1 : -1;
  }
  return 0;
}

export function atLeast(version, minimum) {
  const parts = version.split('.').map((part) => Number.parseInt(part, 10));
  for (let i = 0; i < 3; i += 1) {
    if ((parts[i] ?? 0) !== minimum[i]) return (parts[i] ?? 0) > minimum[i];
  }
  return true;
}

/** How long to wait between reads while the registry catches up. FSCALE_RELEASE_WAIT_MS for tests. */
export function waitMs(normal) {
  const set = process.env.FSCALE_RELEASE_WAIT_MS;
  return set === undefined || set === '' ? normal : Number.parseInt(set, 10) || 0;
}

/** Every `.tgz` below `dir`, three levels deep. */
export async function tarballs(dir) {
  const found = [];
  const walk = async (current, depth) => {
    for (const entry of await readdir(current, { withFileTypes: true })) {
      const full = path.join(current, entry.name);
      if (entry.isDirectory() && depth < 3) await walk(full, depth + 1);
      else if (entry.isFile() && entry.name.endsWith('.tgz')) found.push(full);
    }
  };
  try {
    await walk(dir, 0);
  } catch (error) {
    throw new ReleaseError(
      `cannot read the folder ${dir} (${error.message}). Pass the folder the artifacts were downloaded to.`,
    );
  }
  return found.sort();
}

/** The eight packages in publish order, each with the tarball that is published. */
export function plan(files, version) {
  const pick = (name, tarball, artifact) => {
    const matches = files.filter(
      (file) =>
        path.basename(file) === tarball &&
        (artifact === undefined || file.split(path.sep).includes(`npm-${artifact}`)),
    );
    if (matches.length === 0) {
      throw new ReleaseError(
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

/** npm's integrity string for a file: `sha512-<base64>`. */
export async function integrity(file) {
  return `sha512-${createHash('sha512')
    .update(await readFile(file))
    .digest('base64')}`;
}

const standIn = () => process.env.FSCALE_NPM_COMMAND || '';

/** Throws unless the real npm may run here: anywhere but a GitHub Actions run that is not a tag push. */
export function assertRealNpmAllowed() {
  if (standIn()) return;
  const env = process.env;
  if (env.GITHUB_ACTIONS !== 'true') return;
  if (env.GITHUB_EVENT_NAME === 'push' && env.GITHUB_REF_TYPE === 'tag') return;
  throw new ReleaseError(
    `refusing to run the real npm in a run that is not a tag push (event ${env.GITHUB_EVENT_NAME || 'unknown'}, ref type ${env.GITHUB_REF_TYPE || 'unknown'}). Only a pushed tag v<version> publishes or moves a tag; a rehearsal sets FSCALE_NPM_COMMAND to scripts/fake-npm.mjs.`,
  );
}

/** Runs npm, or the stand-in named by FSCALE_NPM_COMMAND (a Node script given the same arguments). */
export function npm(args, options = {}) {
  const override = standIn();
  if (override) {
    return spawnSync(process.execPath, [override, ...args], { encoding: 'utf8', ...options });
  }
  assertRealNpmAllowed();
  // On Windows npm is a .cmd file and needs a shell, so an argument with a
  // space in it must be quoted by hand.
  const windows = process.platform === 'win32';
  const quoted = windows ? args.map((arg) => (/\s/.test(arg) ? `"${arg}"` : arg)) : args;
  return spawnSync('npm', quoted, { encoding: 'utf8', shell: windows, ...options });
}

function registryUrl() {
  const set = process.env.FSCALE_NPM_REGISTRY;
  if (set) return set.replace(/\/+$/, '');
  return standIn() ? undefined : 'https://registry.npmjs.org';
}

const encoded = (name) => name.replace('/', '%2f');

/** GET a JSON document; undefined for 404; a ReleaseError for anything else after three tries. */
async function getJson(url, what) {
  let lastError = '';
  for (let attempt = 1; attempt <= 3; attempt += 1) {
    try {
      const response = await globalThis.fetch(url, { headers: { accept: 'application/json' } });
      if (response.status === 200) return await response.json();
      if (response.status === 404) return undefined;
      lastError = `HTTP ${response.status}`;
    } catch (error) {
      lastError = error instanceof Error ? error.message : String(error);
    }
    if (attempt < 3) await sleep(attempt * waitMs(2000));
  }
  throw new ReleaseError(
    `could not find out ${what} (${lastError}). Nothing more was done. Run the job again.`,
  );
}

/** `<stand-in> view <spec> <field> --json`, parsed; undefined for E404. */
function viewStandIn(spec, field, what) {
  const result = npm(['view', spec, field, '--json']);
  if (result.status === 0) {
    const text = (result.stdout ?? '').trim();
    return text === '' ? undefined : JSON.parse(text);
  }
  if (/E404/.test(result.stderr ?? '')) return undefined;
  throw new ReleaseError(
    `could not find out ${what} (${(result.stderr ?? '').trim()}). Nothing more was done. Run the job again.`,
  );
}

/** The registry's version document of name@version, or undefined. */
export async function versionDocument(name, version) {
  const url = registryUrl();
  if (url === undefined) {
    const found = viewStandIn(
      `${name}@${version}`,
      'dist.integrity',
      `whether ${name}@${version} is on the registry`,
    );
    return found === undefined ? undefined : { dist: { integrity: found } };
  }
  return getJson(
    `${url}/${encoded(name)}/${version}`,
    `whether ${name}@${version} is on the registry`,
  );
}

/** True when name@version is on the registry. */
export async function isPublished(name, version) {
  return (await versionDocument(name, version)) !== undefined;
}

/** The dist-tags of a package ({} when it has none or is not there). Public: no authentication. */
export async function distTags(name) {
  const url = registryUrl();
  if (url === undefined) return viewStandIn(name, 'dist-tags', `the dist-tags of ${name}`) ?? {};
  return (
    (await getJson(`${url}/-/package/${encoded(name)}/dist-tags`, `the dist-tags of ${name}`)) ?? {}
  );
}

/**
 * Checks that the registry holds the bytes of each tarball, and that `tag`
 * (when given) points at the version. Reads again for a while: a new version
 * or tag can take a moment to show.
 */
export async function checkOnRegistry(steps, version, { tag, attempts = 10, wait = 6000 } = {}) {
  for (const step of steps) {
    const label = `${step.name}@${version}`;
    const want = await integrity(step.file);
    let problem = '';
    for (let attempt = 1; attempt <= attempts; attempt += 1) {
      const document = await versionDocument(step.name, version);
      const got = document?.dist?.integrity;
      if (document === undefined) {
        problem = `${label} is not on the registry`;
      } else if (got !== want) {
        // Bytes do not change once published: no point in reading again.
        throw new ReleaseError(
          `${label} on the registry is not the bytes of ${path.basename(step.file)} (registry ${got ?? 'none'}, tarball ${want}). Do not promote ${version}: ship a new patch version.`,
        );
      } else if (tag !== undefined && (await distTags(step.name))[tag] !== version) {
        problem = `${tag} of ${step.name} is ${(await distTags(step.name))[tag] ?? 'not set'}, not ${version}`;
      } else {
        problem = '';
        break;
      }
      if (attempt < attempts) await sleep(waitMs(wait));
    }
    if (problem) {
      throw new ReleaseError(
        `${problem} after ${attempts} reads. Check the package on npmjs.com; then run the job again.`,
      );
    }
    process.stdout.write(
      `checked  ${label}: the registry has the bytes of ${path.basename(step.file)}${tag ? `, ${tag} is ${version}` : ''}\n`,
    );
  }
}
