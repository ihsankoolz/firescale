// Base runs of a Rust repo get an empty target, every step of them (ADR 0081,
// addendum 1). A build kept between base runs could be read without cargo
// rebuilding it (a sibling binary of `cargo test -p`, a program after
// `cargo build &&`, `cargo xtask`) and make a head failure read
// pre-existing. A base step never gets CARGO_TARGET_DIR, so it never
// builds into the head's folder either.
import { chmod, mkdir, readdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { runGit } from '@firescale/sdk';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { commitAll, EXIT_VIOLATIONS, runCli } from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(async () => {
  await fixtures.cleanup();
});

interface Seen {
  step: string;
  where: 'base' | 'head';
  cargoTarget: string | null;
  target: string | null;
  marker: boolean;
}

/**
 * A step that records what it sees, leaves a marker in CARGO_TARGET_DIR on
 * the base when it has one, and fails on the head only, so verify runs it
 * on the base. `--version` answers for the step cache's key.
 */
function recorder(log: string): string {
  return [
    "const fs = require('fs');",
    "const path = require('path');",
    "if (process.argv.includes('--version')) { console.log('cargo 1.0.0'); process.exit(0); }",
    "const step = process.argv[2] === 'node-step' ? 'node' : 'cargo';",
    "const where = process.env.FSCALE_BASE_RUN === '1' ? 'base' : 'head';",
    'const cargoTarget = process.env.CARGO_TARGET_DIR ?? null;',
    "let target = null; try { target = fs.realpathSync('target'); } catch {}",
    "const marker = cargoTarget !== null && fs.existsSync(path.join(cargoTarget, 'marker'));",
    `fs.appendFileSync(${JSON.stringify(log)}, JSON.stringify({ step, where, cargoTarget, target, marker }) + '\\n');`,
    "if (where === 'base' && cargoTarget !== null) { fs.mkdirSync(cargoTarget, { recursive: true }); fs.writeFileSync(path.join(cargoTarget, 'marker'), 'x'); }",
    "process.exit(where === 'head' ? 1 : 0);",
    '',
  ].join('\n');
}

/** A `cargo` on the PATH that runs the recorder. */
async function fakeCargo(dir: string, script: string): Promise<void> {
  await mkdir(dir, { recursive: true });
  if (process.platform === 'win32') {
    await writeFile(
      path.join(dir, 'cargo.cmd'),
      `@echo off\r\n"${process.execPath}" "${script}" %*\r\n`,
    );
  } else {
    const file = path.join(dir, 'cargo');
    await writeFile(file, `#!/bin/sh\nexec "${process.execPath}" "${script}" "$@"\n`);
    await chmod(file, 0o755);
  }
}

const CARGO = '[package]\nname = "fixture"\nversion = "0.1.0"\nedition = "2021"\n';

/**
 * A repo on a branch of `main` with a cargo step (lint) and a node step
 * (test), both failing on the head only.
 */
async function repo(files: Record<string, string>) {
  const outside = await fixtures.create({ files: {}, track: 'no-git' });
  const log = path.join(outside.root, 'seen.log');
  const script = path.join(outside.root, 'recorder.js');
  await writeFile(script, recorder(log));
  const bin = path.join(outside.root, 'bin');
  await fakeCargo(bin, script);
  const fixture = await fixtures.create({
    files: {
      'firescale.config.json': JSON.stringify({
        packs: ['core'],
        verify: {
          lint: 'cargo test',
          typecheck: false,
          test: `node ${JSON.stringify(script)} node-step`,
        },
      }),
      '.gitignore': '.firescale/\ntarget/\n',
      ...files,
    },
    track: 'no-git',
  });
  const root = fixture.root;
  expect((await runGit(root, ['init', '--quiet', '--initial-branch=main'])).code).toBe(0);
  await commitAll(root, 'base');
  expect((await runGit(root, ['checkout', '--quiet', '-b', 'feature'])).code).toBe(0);
  await writeFile(path.join(root, 'change.txt'), 'a change\n');
  const seen = async (): Promise<Seen[]> =>
    (await readFile(log, 'utf8').catch(() => ''))
      .split(/\r?\n/)
      .filter(Boolean)
      .map((line) => JSON.parse(line) as Seen);
  const pathVar = `${bin}${path.delimiter}${process.env['PATH'] ?? process.env['Path'] ?? ''}`;
  /** One verify run whose base runs again: nothing kept from the last. */
  const verify = async (env: Record<string, string | undefined> = {}) => {
    await rm(path.join(root, '.firescale'), { recursive: true, force: true });
    const result = await runCli(['verify'], root, {
      PATH: pathVar,
      CARGO_TARGET_DIR: undefined,
      ...env,
    });
    expect(result.code).toBe(EXIT_VIOLATIONS);
  };
  const base = async (step: 'cargo' | 'node') =>
    (await seen()).filter((entry) => entry.where === 'base' && entry.step === step);
  return { root, seen, base, verify };
}

describe('every base step of a Rust repo gets an empty target', () => {
  it('a cargo step and any other: no CARGO_TARGET_DIR, no target, nothing kept from the last base run', async () => {
    const { base, verify } = await repo({ 'Cargo.toml': CARGO, 'src/lib.rs': '' });
    await verify();
    await verify();
    for (const step of ['cargo', 'node'] as const) {
      const runs = await base(step);
      expect(runs).toHaveLength(2);
      for (const entry of runs) {
        expect([entry.cargoTarget, entry.target, entry.marker]).toEqual([null, null, false]);
      }
    }
  });

  it("never uses the head's folder, also when the environment names it", async () => {
    const { root, seen, base, verify } = await repo({ 'Cargo.toml': CARGO, 'src/lib.rs': '' });
    const own = path.join(root, 'target');
    await mkdir(own);
    await writeFile(path.join(own, 'kept.txt'), 'head\n');
    await verify({ CARGO_TARGET_DIR: own });
    for (const step of ['cargo', 'node'] as const) {
      const [entry] = await base(step);
      expect([entry?.cargoTarget, entry?.target]).toEqual([null, null]);
    }
    // The head's steps keep what the environment gives them.
    const head = (await seen()).filter((entry) => entry.where === 'head');
    expect(head.length).toBeGreaterThan(0);
    for (const entry of head) expect(entry.cargoTarget).toBe(own);
    expect(await readdir(own)).toEqual(['kept.txt']);
  });

  it('changes nothing for a repo with no Rust', async () => {
    const { base, verify } = await repo({ 'package.json': '{"name":"x"}\n' });
    await verify();
    for (const step of ['cargo', 'node'] as const) {
      expect((await base(step)).map((entry) => [entry.cargoTarget, entry.target])).toEqual([
        [null, null],
      ]);
    }
  });
});
