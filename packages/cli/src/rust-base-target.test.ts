// Rust base runs keep their build folders (ADR 0081; build idea 10). A base
// run is made in a new worktree each time, so cargo built every crate from
// nothing in each one. Now each folder of the worktree with a Cargo.toml
// has its `target` linked to a folder of its own in the temp folder, kept
// from one base run to the next. Base steps never get CARGO_TARGET_DIR, so
// they never build into the head's folder.
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
  genTarget: string | null;
  marker: boolean;
}

/**
 * The test step: it records what it sees, leaves a marker in `target` when
 * there is one, and fails on the head only, so verify runs it on the base.
 */
function stepScript(log: string): string {
  return [
    "const fs = require('fs');",
    "const where = process.env.FSCALE_BASE_RUN === '1' ? 'base' : 'head';",
    'const cargoTarget = process.env.CARGO_TARGET_DIR ?? null;',
    'const real = (p) => { try { return fs.realpathSync(p); } catch { return null; } };',
    "const target = real('target');",
    "const genTarget = real('tools/gen/target');",
    "const marker = target !== null && fs.existsSync(target + '/marker');",
    `fs.appendFileSync(${JSON.stringify(log)}, JSON.stringify({ where, cargoTarget, target, genTarget, marker }) + '\\n');`,
    "if (where === 'base' && target !== null) fs.writeFileSync(target + '/marker', 'x');",
    "process.exit(where === 'head' ? 1 : 0);",
    '',
  ].join('\n');
}

const CARGO = '[package]\nname = "fixture"\nversion = "0.1.0"\nedition = "2021"\n';
const GEN = '[package]\nname = "gen"\nversion = "0.1.0"\nedition = "2021"\n\n[workspace]\n';

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
  const verify = async (env: Record<string, string | undefined> = {}) => {
    await rm(path.join(root, '.firescale'), { recursive: true, force: true });
    const result = await runCli(['verify'], root, { CARGO_TARGET_DIR: undefined, ...env });
    expect(result.code).toBe(EXIT_VIOLATIONS);
  };
  const base = async () => (await seen()).filter((entry) => entry.where === 'base');
  return { root, seen, base, verify };
}

/** Deletes the kept folders of a test's repo afterwards. */
function forget(target: string | null | undefined): void {
  if (target) made.push(path.dirname(target));
}

describe('a Rust base run keeps its build folder from one base run to the next', () => {
  it('links target to a folder in the temp folder, the same each run, with what the last run left there', async () => {
    const { root, base, verify } = await repo({ 'Cargo.toml': CARGO, 'src/lib.rs': '' });
    await verify();
    await verify();
    const runs = await base();
    expect(runs).toHaveLength(2);
    const [first, second] = runs;
    forget(first?.target);
    expect(first?.target).not.toBeNull();
    expect(second?.target).toBe(first?.target);
    expect([first?.marker, second?.marker]).toEqual([false, true]);
    const folder = first?.target ?? '';
    expect(folder.startsWith(realpathSync(os.tmpdir()) + path.sep)).toBe(true);
    expect(folder.startsWith(realpathSync(root) + path.sep)).toBe(false);
    // Base steps get no CARGO_TARGET_DIR: cargo builds into the link.
    expect([first?.cargoTarget, second?.cargoTarget]).toEqual([null, null]);
  });

  it("never uses the head's folder, also when the environment names it", async () => {
    const { root, seen, base, verify } = await repo({ 'Cargo.toml': CARGO, 'src/lib.rs': '' });
    const own = path.join(root, 'target');
    await mkdir(own);
    await writeFile(path.join(own, 'kept.txt'), 'head\n');
    await verify({ CARGO_TARGET_DIR: own });
    const [first] = await base();
    forget(first?.target);
    expect(first?.cargoTarget).toBeNull();
    expect(first?.target).not.toBe(realpathSync(own));
    // The head's steps keep what the environment gives them.
    const head = (await seen()).filter((entry) => entry.where === 'head');
    expect(head.length).toBeGreaterThan(0);
    for (const entry of head) expect(entry.cargoTarget).toBe(own);
    // Nothing was written into the head's target/.
    expect(await readdir(own)).toEqual(['kept.txt']);
    // The kept folder outlives the base worktree, which is gone.
    expect(existsSync(first?.target ?? '')).toBe(true);
  });

  it('gives a crate below the root that is a workspace of its own a folder of its own', async () => {
    const { base, verify } = await repo({
      'Cargo.toml': CARGO,
      'src/lib.rs': '',
      'tools/gen/Cargo.toml': GEN,
      'tools/gen/src/main.rs': 'fn main() {}\n',
    });
    await verify();
    const [first] = await base();
    forget(first?.target);
    expect(first?.target).not.toBeNull();
    expect(first?.genTarget).not.toBeNull();
    expect(first?.genTarget).not.toBe(first?.target);
    expect(path.dirname(first?.genTarget ?? '')).toBe(path.dirname(first?.target ?? ''));
  });

  it('two repos get two folders', async () => {
    const one = await repo({ 'Cargo.toml': CARGO, 'src/lib.rs': '' });
    const two = await repo({ 'Cargo.toml': CARGO, 'src/lib.rs': '' });
    await one.verify();
    await two.verify();
    const a = (await one.base())[0]?.target;
    const b = (await two.base())[0]?.target;
    forget(a);
    forget(b);
    expect(a).not.toBeNull();
    expect(b).not.toBeNull();
    expect(a).not.toBe(b);
  });

  it('changes nothing for a repo with no Rust', async () => {
    const { base, verify } = await repo({ 'package.json': '{"name":"x"}\n' });
    await verify();
    expect((await base()).map((entry) => [entry.cargoTarget, entry.target])).toEqual([
      [null, null],
    ]);
  });
});
