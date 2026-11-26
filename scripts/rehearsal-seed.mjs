#!/usr/bin/env node
// Starts the stand-in registry of a release rehearsal (ADR 0092): the real
// `latest` of the eight packages, read from npm (a public read), written into
// the stand-in's file. Only `latest`: the rehearsal publishes the candidate
// under next itself. Nothing is sent to npm.
//
// Usage: node scripts/rehearsal-seed.mjs --version <the version being rehearsed>
// FSCALE_FAKE_NPM_STATE names the stand-in's file; FSCALE_NPM_REGISTRY the
// registry read (the tests).
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';
import { PACKAGES, ReleaseError, VERSION, runScript } from './release-registry.mjs';

const FAKE = path.join(path.dirname(fileURLToPath(import.meta.url)), 'fake-npm.mjs');

await runScript('rehearsal-seed', async () => {
  const [flag, version, ...rest] = process.argv.slice(2);
  if (flag !== '--version' || !version || !VERSION.test(version) || rest.length > 0) {
    throw new ReleaseError('Usage: node scripts/rehearsal-seed.mjs --version <version>');
  }
  if (!process.env.FSCALE_FAKE_NPM_STATE) {
    throw new ReleaseError('FSCALE_FAKE_NPM_STATE must name the stand-in registry file.');
  }
  const registry = (process.env.FSCALE_NPM_REGISTRY || 'https://registry.npmjs.org').replace(
    /\/+$/,
    '',
  );
  for (const name of PACKAGES) {
    const url = `${registry}/-/package/${name.replace('/', '%2f')}/dist-tags`;
    const response = await globalThis.fetch(url, { headers: { accept: 'application/json' } });
    if (response.status !== 200) {
      throw new ReleaseError(
        `could not read the dist-tags of ${name} (HTTP ${response.status}). Run the job again.`,
      );
    }
    const latest = (await response.json()).latest;
    if (typeof latest !== 'string') throw new ReleaseError(`${name} has no latest on npm.`);
    if (latest === version) {
      throw new ReleaseError(
        `${version} is already latest of ${name} on npm, so a rehearsal of it cannot record a previous latest. Rehearse once main has the next version (the release pull request), before its tag.`,
      );
    }
    const seeded = spawnSync(process.execPath, [FAKE, 'seed', name, 'latest', latest], {
      encoding: 'utf8',
    });
    if (seeded.status !== 0)
      throw new ReleaseError(`the stand-in refused the seed: ${seeded.stderr}`);
    process.stdout.write(`seeded   ${name}: latest ${latest} (from npm)\n`);
  }
});
