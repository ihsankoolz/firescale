#!/usr/bin/env node
// Publishes the eight firescale packages from the tarballs the Release
// workflow built, in the order that never leaves `firescale` pointing at a
// package that is not there: the six platform packages, then @firescale/sdk,
// then firescale.
//
// Everything is published under the dist-tag "next", a normal version too:
// staging (ADR 0092). `latest` is never touched here; only the promotion,
// after the owner's approval, moves it.
//
// After publishing, it checks that the registry holds the bytes of each
// tarball (its `dist.integrity` against the SHA-512 of the file) and that
// `next` is the version.
//
// Safe to run again: a package version that is already on the registry is
// skipped, so a release that stopped halfway can be finished.
//
// Authentication is npm trusted publishing (OIDC). There is no token: the
// job needs `id-token: write`, npm 11.5.1 or later and Node 22.14.0 or later.
//
// Usage: node scripts/publish-npm.mjs --dir <folder with the artifacts> --version <version> [--dry-run]
//   --dry-run   look everything up and print the plan, publish nothing
//   <folder> holds one subfolder per artifact, named npm-<platform>.
// FSCALE_NPM_COMMAND replaces `npm` (the tests and the rehearsal), and then
// the registry is read through it too. FSCALE_NPM_REGISTRY changes the
// registry that is read (the tests).
import path from 'node:path';
import process from 'node:process';
import {
  ReleaseError,
  VERSION,
  assertRealNpmAllowed,
  atLeast,
  checkOnRegistry,
  isPublished,
  npm,
  plan,
  runScript,
  tarballs,
} from './release-registry.mjs';

const MIN_NPM = [11, 5, 1];
const MIN_NODE = [22, 14, 0];
const USAGE = 'Usage: node scripts/publish-npm.mjs --dir <folder> --version <version> [--dry-run]';
const STAGING_TAG = 'next';

function parseArgs(argv) {
  const options = { dryRun: false };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--dry-run') {
      options.dryRun = true;
    } else if (arg === '--dir' || arg === '--version') {
      const value = argv[i + 1];
      if (value === undefined || value.startsWith('--')) {
        throw new ReleaseError(`${arg} needs a value. ${USAGE}`);
      }
      options[arg.slice(2)] = value;
      i += 1;
    } else {
      throw new ReleaseError(`unknown argument "${arg}". ${USAGE}`);
    }
  }
  if (!options.dir || !options.version) {
    throw new ReleaseError(`--dir and --version are both needed. ${USAGE}`);
  }
  if (!VERSION.test(options.version)) {
    throw new ReleaseError(`"${options.version}" is not a version such as 1.2.3. ${USAGE}`);
  }
  return options;
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  const steps = plan(await tarballs(path.resolve(options.dir)), options.version);

  if (!options.dryRun) {
    // Before anything is read or published: a run that may not publish stops here.
    assertRealNpmAllowed();
    const node = process.versions.node;
    if (!atLeast(node, MIN_NODE)) {
      throw new ReleaseError(
        `npm trusted publishing needs Node ${MIN_NODE.join('.')} or later, and this is ${node}. Use a newer Node in the publish job.`,
      );
    }
    const npmVersion = npm(['--version']).stdout?.trim() ?? '';
    if (!atLeast(npmVersion, MIN_NPM)) {
      throw new ReleaseError(
        `npm trusted publishing needs npm ${MIN_NPM.join('.')} or later, and this is ${npmVersion || 'unknown'}. Use a Node version that ships it, or run \`npm install -g npm@latest\` first.`,
      );
    }
  }

  let published = 0;
  let skipped = 0;
  for (const step of steps) {
    const label = `${step.name}@${options.version}`;
    if (await isPublished(step.name, options.version)) {
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
    const result = npm(['publish', step.file, '--access', 'public', '--tag', STAGING_TAG], {
      stdio: 'inherit',
      timeout: 600_000,
    });
    if (result.status !== 0) {
      // A second run of this job may have published it in the meantime.
      if (await isPublished(step.name, options.version)) {
        process.stdout.write(`skip     ${label} (it is on the registry now)\n`);
        skipped += 1;
        continue;
      }
      throw new ReleaseError(
        `npm publish failed for ${label}. ${published} package(s) were published before it in this run. Fix the cause (RELEASING.md lists the usual ones: a missing or expired trusted publisher shows as ENEEDAUTH) and run the job again: packages that are already published are skipped.`,
      );
    }
    published += 1;
  }
  if (options.dryRun) {
    process.stdout.write(
      `Dry run: ${steps.length - skipped} to publish, ${skipped} already on the registry.\n`,
    );
    return;
  }
  process.stdout.write(`Done: ${published} published, ${skipped} already on the registry.\n`);
  await checkOnRegistry(steps, options.version, { tag: STAGING_TAG });
  process.stdout.write(
    `Checked: all ${steps.length} on the registry with the built bytes, under ${STAGING_TAG}.\n`,
  );
}

await runScript('publish-npm', main);
