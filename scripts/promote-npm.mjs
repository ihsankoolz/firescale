#!/usr/bin/env node
// Production for the eight firescale packages (ADR 0092, steps 8 and 10):
// only dist-tags move. Nothing is ever unpublished.
//
//   record   --version <v> --out <file>
//            Reads the current `latest` of all eight from the registry and
//            writes it to <file>. Refuses a pre-release, a `latest` that is
//            already <v> (a record never holds the candidate), and a <v> lower
//            than the latest of firescale. Moves nothing.
//   promote  --version <v> --record <file> --dir <artifacts>
//            Checks that the registry holds the built bytes of all eight, and
//            that each `latest` is still the recorded one (or already <v>);
//            then moves `latest` to <v>, the platform packages first and
//            firescale last, and reads each back.
//   rollback --record <file>
//            Moves `latest` back to the recorded version on every package
//            where it differs, firescale first, and reads each back. Tries
//            every package, then fails if any could not be moved.
//
// Moving a dist-tag through trusted publishing (OIDC) needs npm 11.21.0 or
// later and "Allow npm dist-tag" on the package's trusted publisher. A call
// that fails stops the job; a call that exits 0 is not trusted: every tag is
// read back from the registry.
//
// FSCALE_NPM_COMMAND replaces `npm` and the registry (the tests and the
// rehearsal); the real npm is refused in a run that is not a tag push.
import { readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import process from 'node:process';
import { setTimeout as sleep } from 'node:timers/promises';
import {
  PACKAGES,
  ReleaseError,
  VERSION,
  assertRealNpmAllowed,
  atLeast,
  checkOnRegistry,
  compareVersions,
  distTags,
  isPrerelease,
  npm,
  plan,
  runScript,
  tarballs,
  waitMs,
} from './release-registry.mjs';

const MIN_NPM = [11, 21, 0];
const USAGE = [
  'Usage: node scripts/promote-npm.mjs record --version <v> --out <file>',
  '       node scripts/promote-npm.mjs promote --version <v> --record <file> --dir <artifacts>',
  '       node scripts/promote-npm.mjs rollback --record <file>',
].join('\n');
const READS = 10;
const READ_WAIT = 6000;

function parseArgs(argv) {
  const [command, ...rest] = argv;
  const options = { command };
  for (let i = 0; i < rest.length; i += 2) {
    const key = rest[i] ?? '';
    const value = rest[i + 1];
    if (!['--version', '--out', '--record', '--dir'].includes(key) || value === undefined) {
      throw new ReleaseError(`unknown argument "${key}".\n${USAGE}`);
    }
    options[key.slice(2)] = value;
  }
  const need = {
    record: ['version', 'out'],
    promote: ['version', 'record', 'dir'],
    rollback: ['record'],
  }[command ?? ''];
  if (!need) throw new ReleaseError(`unknown command "${command ?? ''}".\n${USAGE}`);
  for (const key of need) {
    if (!options[key]) throw new ReleaseError(`${command} needs --${key}.\n${USAGE}`);
  }
  if (options.version !== undefined && !VERSION.test(options.version)) {
    throw new ReleaseError(`"${options.version}" is not a version such as 1.2.3.`);
  }
  return options;
}

function refusePrerelease(version) {
  if (isPrerelease(version)) {
    throw new ReleaseError(
      `${version} is a pre-release: it stays under next and is never moved to latest (ADR 0092).`,
    );
  }
}

function checkNpm() {
  // Before any read: a run that may not move a tag stops here.
  assertRealNpmAllowed();
  const version = npm(['--version']).stdout?.trim() ?? '';
  if (!atLeast(version, MIN_NPM)) {
    throw new ReleaseError(
      `moving dist-tags with trusted publishing needs npm ${MIN_NPM.join('.')} or later, and this is ${version || 'unknown'}. The job installs npm@11.21.3; check that step.`,
    );
  }
}

async function readRecord(file) {
  let text;
  try {
    text = await readFile(file, 'utf8');
  } catch {
    throw new ReleaseError(
      `no promotion record at ${file}. The record job writes it before promotion; without it there is nothing to roll back to.`,
    );
  }
  const record = JSON.parse(text);
  for (const name of PACKAGES) {
    if (typeof record.latest?.[name] !== 'string') {
      throw new ReleaseError(`the promotion record ${file} has no latest for ${name}.`);
    }
  }
  return record;
}

/** Moves `tag` of name to version and reads it back. Returns an error text, or undefined. */
async function moveTag(name, version, tag) {
  const result = npm(['dist-tag', 'add', `${name}@${version}`, tag], { timeout: 120_000 });
  if (result.status !== 0) {
    const said = `${result.stdout ?? ''}\n${result.stderr ?? ''}`
      .trim()
      .split(/\r?\n/)
      .slice(-6)
      .join(' | ');
    return {
      call: `npm dist-tag add ${name}@${version} ${tag} failed (exit ${result.status ?? 'none: timed out'}): ${said}`,
    };
  }
  let seen;
  for (let attempt = 1; attempt <= READS; attempt += 1) {
    seen = (await distTags(name))[tag];
    if (seen === version) return undefined;
    if (attempt < READS) await sleep(waitMs(READ_WAIT));
  }
  return { readBack: seen ?? 'not set' };
}

async function record(options) {
  refusePrerelease(options.version);
  const latest = {};
  for (const name of PACKAGES) {
    const current = (await distTags(name)).latest;
    if (current === undefined) {
      throw new ReleaseError(
        `${name} has no latest on the registry; there would be nothing to roll back to.`,
      );
    }
    if (current === options.version) {
      throw new ReleaseError(
        `latest of ${name} is already ${options.version}. A record never holds the candidate: someone moved latest by hand, or a promotion ran without this record. Check the dist-tags on npmjs.com.`,
      );
    }
    latest[name] = current;
  }
  if (compareVersions(options.version, latest.firescale) < 0) {
    throw new ReleaseError(
      `${options.version} is lower than the latest of firescale, ${latest.firescale}. Moving latest down is a rollback, not a release.`,
    );
  }
  await writeFile(
    options.out,
    `${JSON.stringify({ version: options.version, latest }, null, 2)}\n`,
  );
  process.stdout.write(
    `recorded latest of ${PACKAGES.length} packages: firescale is ${latest.firescale}, candidate ${options.version}\n`,
  );
}

async function promote(options) {
  refusePrerelease(options.version);
  const saved = await readRecord(options.record);
  if (saved.version !== options.version) {
    throw new ReleaseError(`the record is for ${saved.version}, not ${options.version}.`);
  }
  checkNpm();
  const steps = plan(await tarballs(path.resolve(options.dir)), options.version);
  // The bytes once more, after the approval: what moves to latest is what was built.
  await checkOnRegistry(steps, options.version, { attempts: 3 });
  // Nothing moves unless every latest is where the record left it, or already moved by this run.
  const current = {};
  for (const name of PACKAGES) {
    current[name] = (await distTags(name)).latest;
    if (current[name] !== saved.latest[name] && current[name] !== options.version) {
      throw new ReleaseError(
        `latest of ${name} is ${current[name] ?? 'not set'}; the record says ${saved.latest[name]}. latest moved since this run recorded it, so nothing was moved. Find out who moved it before releasing.`,
      );
    }
  }
  let moved = 0;
  for (const name of PACKAGES) {
    if (current[name] === options.version) {
      process.stdout.write(`already  ${name} is ${options.version}\n`);
      continue;
    }
    const problem = await moveTag(name, options.version, 'latest');
    if (problem?.call) {
      throw new ReleaseError(
        `${problem.call}. If npm says ENEEDAUTH, or that the trusted publisher cannot manage dist-tags, turn on "Allow npm dist-tag" in that package's trusted publisher on npmjs.com. ${moved} package(s) were moved before it; the rollback job moves them back.`,
      );
    }
    if (problem) {
      throw new ReleaseError(
        `npm reported success, but latest of ${name} is ${problem.readBack}, not ${options.version}, after ${READS} reads. ${moved} package(s) were moved before it; the rollback job moves them back.`,
      );
    }
    moved += 1;
    process.stdout.write(`latest   ${name} is ${options.version} (was ${saved.latest[name]})\n`);
  }
  process.stdout.write(
    `Promoted: latest is ${options.version} on all ${PACKAGES.length} packages.\n`,
  );
}

async function rollback(options) {
  const saved = await readRecord(options.record);
  checkNpm();
  const problems = [];
  let moved = 0;
  // firescale first: the launcher goes back before the packages it depends on.
  for (const name of [...PACKAGES].reverse()) {
    const want = saved.latest[name];
    const now = (await distTags(name)).latest;
    if (now === want) {
      process.stdout.write(`already  ${name} is ${want}\n`);
      continue;
    }
    const problem = await moveTag(name, want, 'latest');
    if (problem?.call) {
      problems.push(`latest of ${name} is still ${now ?? 'not set'} (${problem.call})`);
    } else if (problem) {
      problems.push(`latest of ${name} is still ${problem.readBack}: npm reported success`);
    } else {
      moved += 1;
      process.stdout.write(`latest   ${name} is back on ${want} (was ${now ?? 'not set'})\n`);
    }
  }
  if (problems.length > 0) {
    throw new ReleaseError(
      `could not move latest back:\n  ${problems.join('\n  ')}\nRun the rollback job again. If it keeps failing, an owner moves each by hand: npm dist-tag add <package>@<recorded version> latest (the record: ${options.record}).`,
    );
  }
  process.stdout.write(
    `Rolled back: latest is the recorded version on all ${PACKAGES.length} packages (${moved} moved).\n`,
  );
}

await runScript('promote-npm', async () => {
  const options = parseArgs(process.argv.slice(2));
  if (options.command === 'record') await record(options);
  else if (options.command === 'promote') await promote(options);
  else await rollback(options);
});
