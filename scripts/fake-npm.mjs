#!/usr/bin/env node
// A stand-in for npm, for the tests and the release rehearsal (ADR 0092).
// It never talks to a registry: its registry is a JSON file.
//
//   FSCALE_FAKE_NPM_STATE    the registry file (required):
//                            { "packages": { "<name>": { "versions": { "<v>": "<integrity>" }, "tags": { "<tag>": "<v>" } } } }
//   FSCALE_FAKE_NPM_LOG      each call that changes something, appended (tarballs by file name)
//   FSCALE_FAKE_NPM_VERSION  what `--version` prints (default 11.21.3)
//   FSCALE_FAKE_NPM_FAIL     a call whose arguments contain this text fails as npm does
//                            when OIDC is refused (ENEEDAUTH), and changes nothing
//   FSCALE_FAKE_NPM_SILENT   a call whose arguments contain this text exits 0 and changes nothing
//   FSCALE_FAKE_NPM_CORRUPT  a publish whose arguments contain this text stores the wrong bytes
//
// What it understands:
//   --version
//   publish <file.tgz> [--access public] [--tag <tag>]   name and version from the file name
//   view <name>[@<version>] <dist-tags|dist.integrity|version> --json
//   dist-tag add <name>@<version> <tag>
//   dist-tag ls <name>
//   seed <name> <tag> <version>      a version that exists elsewhere (bytes unknown), with a tag
import { Buffer } from 'node:buffer';
import { createHash } from 'node:crypto';
import { appendFileSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import process from 'node:process';

const args = process.argv.slice(2);
const env = process.env;

function out(text) {
  process.stdout.write(`${text}\n`);
}
function npmError(code, message) {
  process.stderr.write(`npm error code ${code}\nnpm error ${message}\n`);
  process.exit(1);
}

if (args[0] === '--version') {
  out(env.FSCALE_FAKE_NPM_VERSION || '11.21.3');
  process.exit(0);
}

const stateFile = env.FSCALE_FAKE_NPM_STATE;
if (!stateFile) {
  process.stderr.write('fake-npm: FSCALE_FAKE_NPM_STATE must name the registry file.\n');
  process.exit(2);
}
const load = () => {
  try {
    return JSON.parse(readFileSync(stateFile, 'utf8'));
  } catch {
    return { packages: {} };
  }
};
const save = (state) => writeFileSync(stateFile, `${JSON.stringify(state, null, 2)}\n`);
const entry = (state, name) => (state.packages[name] ??= { versions: {}, tags: {} });

const shown = args.map((arg) => (arg.endsWith('.tgz') ? path.basename(arg) : arg)).join(' ');
const changes = args[0] === 'publish' || (args[0] === 'dist-tag' && args[1] !== 'ls');
const has = (text) => Boolean(text) && shown.includes(text);

if (changes) {
  if (env.FSCALE_FAKE_NPM_LOG) appendFileSync(env.FSCALE_FAKE_NPM_LOG, `${shown}\n`);
  if (has(env.FSCALE_FAKE_NPM_FAIL)) {
    npmError(
      'ENEEDAUTH',
      'This command requires you to be logged in. (stand-in: OIDC refused for this call)',
    );
  }
  if (has(env.FSCALE_FAKE_NPM_SILENT)) process.exit(0);
}

/** `name@version`, with a scope's own @ kept. */
function splitSpec(spec) {
  const at = spec.lastIndexOf('@');
  return at > 0 ? [spec.slice(0, at), spec.slice(at + 1)] : [spec, undefined];
}

/** The package name and version of a tarball, from its file name as `npm pack` makes it. */
function fromFileName(file) {
  const match = /^(.+?)-(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)\.tgz$/.exec(path.basename(file));
  if (!match) npmError('EBADTARBALL', `cannot read a name and version from ${path.basename(file)}`);
  const base = match[1];
  const name =
    base === 'firescale'
      ? 'firescale'
      : base.startsWith('firescale-')
        ? `@firescale/${base.slice(10)}`
        : base;
  return { name, version: match[2] };
}

const state = load();
const [command, sub] = args;

if (command === 'publish') {
  const file = args[1];
  const tagAt = args.indexOf('--tag');
  const tag = tagAt === -1 ? 'latest' : args[tagAt + 1];
  const { name, version } = fromFileName(file);
  const pkg = entry(state, name);
  if (version in pkg.versions) {
    npmError('E403', `You cannot publish over the previously published versions: ${version}.`);
  }
  const bytes = has(env.FSCALE_FAKE_NPM_CORRUPT)
    ? Buffer.from('not the bytes')
    : readFileSync(file);
  pkg.versions[version] = `sha512-${createHash('sha512').update(bytes).digest('base64')}`;
  pkg.tags[tag] = version;
  save(state);
  out(`+ ${name}@${version}`);
} else if (command === 'view') {
  const [name, version] = splitSpec(args[1] ?? '');
  const field = args[2];
  const pkg = state.packages[name];
  if (!pkg || (version !== undefined && !(version in pkg.versions))) {
    npmError('E404', `404 Not Found - GET ${name}${version ? `@${version}` : ''}`);
  }
  if (field === 'dist-tags') out(JSON.stringify(pkg.tags));
  else if (field === 'dist.integrity') out(JSON.stringify(pkg.versions[version] ?? null));
  else if (field === 'version') out(JSON.stringify(version ?? pkg.tags.latest));
  else npmError('EUSAGE', `the stand-in does not know the field ${field}`);
} else if (command === 'dist-tag' && sub === 'add') {
  const [name, version] = splitSpec(args[2] ?? '');
  const tag = args[3];
  const pkg = state.packages[name];
  if (!pkg || version === undefined || !(version in pkg.versions)) {
    npmError('E404', `404 Not Found - ${name}@${version}`);
  }
  pkg.tags[tag] = version;
  save(state);
  out(`+${tag}: ${name}@${version}`);
} else if (command === 'dist-tag' && sub === 'ls') {
  const pkg = state.packages[args[2]];
  if (!pkg) npmError('E404', `404 Not Found - ${args[2]}`);
  for (const [tag, version] of Object.entries(pkg.tags)) out(`${tag}: ${version}`);
} else if (command === 'seed') {
  const [, name, tag, version] = args;
  if (!name || !tag || !version) npmError('EUSAGE', 'seed <name> <tag> <version>');
  const pkg = entry(state, name);
  pkg.versions[version] ??= null;
  pkg.tags[tag] = version;
  save(state);
} else {
  npmError('EUSAGE', `the stand-in does not know \`npm ${shown}\``);
}
