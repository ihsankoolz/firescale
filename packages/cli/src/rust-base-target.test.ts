// Rust base runs keep their build folder (ADR 0081; build idea 10). A base
// run is made in a new worktree each time, so cargo built every crate from
// nothing in each one. Base steps of a repo with Rust now get a build
// folder of their own in the temp folder, kept from one base run to the
// next: CARGO_TARGET_DIR, and the worktree's `target` linked to it. Never
// the head's `target/`.
import { existsSync, realpathSync } from 'node:fs';
import { mkdir, readdir, readFile, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { runGit } from '@firescale/sdk';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { commitAll, EXIT_VIOLATIONS, runCli } from './test-helpers.js';

const fixtures = fixtureSet();
const made: string[] = [];
afterEach(async () => {
  for (const folder of made.splice(0)) await rm(folder, { recursive: true, force: true });
  await fixtures.cleanup();
});

interface Seen {
  where: 'base' | 'head';
  cargoTarget: string | null;
  target: string | null;
  marker: boolean;
}

/**
 * The test step: it records what it sees, leaves a marker in its build
 * folder, and fails on the head only, so verify runs it on the base.
 */
function stepScript(log: string): string {
  return [
    "const fs = require('fs');",
    "const where = process.env.FSCALE_BASE_RUN === '1' ? 'base' : 'head';",
    'const cargoTarget = process.env.CARGO_TARGET_DIR ?? null;',
    "let target = null; try { target = fs.realpathSync('target'); } catch {}",
    "const marker = cargoTarget !== null && fs.existsSync(require('path').join(cargoTarget, 'marker'));",
    `fs.appendFileSync(${JSON.stringify(log)}, JSON.stringify({ where, cargoTarget, target, marker }) + '\\n');`,
    "if (cargoTarget !== null) { fs.mkdirSync(cargoTarget, { recursive: true }); fs.writeFileSync(require('path').join(cargoTarget, 'marker'), 'x'); }",
    "process.exit(where === 'head' ? 1 : 0);",
    '',
  ].join('\n');
}

const CARGO = '[package]\nname = "fixture"\nversion = "0.1.0"\nedition = "2021"\n';

/** A repo on a branch of `main`, whose test fails on the head only. */
async function repo(files: Record<string, string>) {
  const outside = await fixtures.create({ files: {}, track: 'no-git' });
  const log = path.join(outside.root, 'seen.log');
  const fixture = await fixtures.create({
    files: {
      'firescale.config.json': JSON.stringify({
        packs: ['core'],
        verify: { lint: false, typecheck: false, test: 'node test.js' },
      }),
      '.gitignore': '.firescale/\ntarget/\n',
      'test.js': stepScript(log),
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
  /** One verify run whose base runs again: nothing kept from the last. */
  const verify = async () => {
    await rm(path.join(root, '.firescale'), { recursive: true, force: true });
    const result = await runCli(['verify'], root, { CARGO_TARGET_DIR: undefined });
    expect(result.code).toBe(EXIT_VIOLATIONS);
  };
  return { root, seen, verify };
}

describe('a Rust base run keeps its build folder from one base run to the next', () => {
  it('gives base steps one build folder in the temp folder, the same each run, and links target to it', async () => {
    const { root, seen, verify } = await repo({ 'Cargo.toml': CARGO, 'src/lib.rs': '' });
    await verify();
    await verify();
    const base = (await seen()).filter((entry) => entry.where === 'base');
    expect(base).toHaveLength(2);
    const [first, second] = base;
    expect(first?.cargoTarget).not.toBeNull();
    made.push(first?.cargoTarget ?? '');
    // The same folder both times, and what the first run left there is
    // there in the second.
    expect(second?.cargoTarget).toBe(first?.cargoTarget);
    expect([first?.marker, second?.marker]).toEqual([false, true]);
    // In the temp folder, and nowhere in the repo.
    const folder = realpathSync(first?.cargoTarget ?? '');
    expect(folder.startsWith(realpathSync(os.tmpdir()) + path.sep)).toBe(true);
    expect(folder.startsWith(realpathSync(root) + path.sep)).toBe(false);
    // The worktree's `target` is that folder, so a test that reads
    // target/debug/<binary> finds what the base built.
    expect([first?.target, second?.target]).toEqual([folder, folder]);
  });

  it('never touches the head: its steps get no CARGO_TARGET_DIR, and its target/ is as it was', async () => {
    const { root, seen, verify } = await repo({ 'Cargo.toml': CARGO, 'src/lib.rs': '' });
    await mkdir(path.join(root, 'target'));
    await writeFile(path.join(root, 'target', 'kept.txt'), 'head\n');
    await verify();
    const entries = await seen();
    const base = entries.find((entry) => entry.where === 'base');
    made.push(base?.cargoTarget ?? '');
    const head = entries.filter((entry) => entry.where === 'head');
    // The head's failure runs once more (the flaky check): each run is the head's.
    expect(head.length).toBeGreaterThan(0);
    for (const entry of head) expect([entry.cargoTarget, entry.marker]).toEqual([null, false]);
    expect(await readdir(path.join(root, 'target'))).toEqual(['kept.txt']);
    expect(base?.cargoTarget).not.toBe(path.join(root, 'target'));
    // The folder outlives the base worktree, which is gone.
    expect(existsSync(base?.cargoTarget ?? '')).toBe(true);
  });

  it('two repos get two folders', async () => {
    const one = await repo({ 'Cargo.toml': CARGO, 'src/lib.rs': '' });
    const two = await repo({ 'Cargo.toml': CARGO, 'src/lib.rs': '' });
    await one.verify();
    await two.verify();
    const a = (await one.seen()).find((entry) => entry.where === 'base')?.cargoTarget;
    const b = (await two.seen()).find((entry) => entry.where === 'base')?.cargoTarget;
    made.push(a ?? '', b ?? '');
    expect(a).not.toBeNull();
    expect(b).not.toBeNull();
    expect(a).not.toBe(b);
  });

  it('changes nothing for a repo with no Rust', async () => {
    const { seen, verify } = await repo({ 'package.json': '{"name":"x"}\n' });
    await verify();
    const base = (await seen()).filter((entry) => entry.where === 'base');
    expect(base.map((entry) => [entry.cargoTarget, entry.target])).toEqual([[null, null]]);
  });
});
