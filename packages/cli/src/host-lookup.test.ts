import { chmod, copyFile, cp, mkdir, mkdtemp, rm, symlink } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fixtureSet, type FixtureRepo } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import type { CheckReport } from './report.js';
import { BIN, EXIT_USAGE, EXIT_VIOLATIONS, REPO, runCli } from './test-helpers.js';

const fixtures = fixtureSet();
const tempDirs: string[] = [];
afterEach(async () => {
  await fixtures.cleanup();
  for (const dir of tempDirs.splice(0)) {
    await rm(dir, { recursive: true, force: true, maxRetries: 5 });
  }
});

const CUSTOM_PACK = path.join(REPO, 'packages', 'cli', 'test-fixtures', 'custom-pack');
const SAMPLE = 'sample/no-todo-file';
/** FSCALE_NODE_HOST and FSCALE_NODE must not leak into these tests. */
const AUTO = { FSCALE_NODE_HOST: undefined, FSCALE_NODE: undefined };
const EXE = process.platform === 'win32' ? 'fscale.exe' : 'fscale';

async function tempDir(prefix: string): Promise<string> {
  const dir = await mkdtemp(path.join(tmpdir(), prefix));
  tempDirs.push(dir);
  return dir;
}

/** Copies the fscale binary into `dir` and returns its new path. */
async function copyBinary(dir: string): Promise<string> {
  await mkdir(dir, { recursive: true });
  const target = path.join(dir, EXE);
  await copyFile(BIN, target);
  await chmod(target, 0o755);
  return target;
}

/** A repo with the sample pack; `firescale` is linked into node_modules only on request. */
async function packRepo(options: { linkFirescale: boolean }): Promise<FixtureRepo> {
  const repo = await fixtures.create({
    files: {
      'TODO.md': '- item\n',
      'firescale.config.json': JSON.stringify({
        packs: ['core', './tools/rules/pack.ts'],
        rules: { [SAMPLE]: 'enforced' },
      }),
    },
  });
  await cp(CUSTOM_PACK, path.join(repo.root, 'tools', 'rules'), { recursive: true });
  const scope = path.join(repo.root, 'node_modules', '@firescale');
  await mkdir(scope, { recursive: true });
  // 'junction' works on Windows without admin rights; other platforms ignore the type.
  await symlink(path.join(REPO, 'packages', 'sdk'), path.join(scope, 'sdk'), 'junction');
  if (options.linkFirescale) {
    await symlink(
      path.join(REPO, 'packages', 'cli'),
      path.join(repo.root, 'node_modules', 'firescale'),
      'junction',
    );
  }
  return repo;
}

function expectCustomRuleRan(result: { code: number; stdout: string; stderr: string }): void {
  expect(result.code, result.stderr).toBe(EXIT_VIOLATIONS);
  const report = JSON.parse(result.stdout) as CheckReport;
  expect(report.diagnostics.map((d) => d.file)).toContain('TODO.md');
}

describe('automatic lookup of the Node host', () => {
  it('finds it next to the binary in an npm layout', async () => {
    const layout = await tempDir('firescale-npm-');
    const bin = await copyBinary(
      path.join(layout, 'node_modules', '@firescale', 'cli-test', 'bin'),
    );
    await symlink(
      path.join(REPO, 'packages', 'cli'),
      path.join(layout, 'node_modules', 'firescale'),
      'junction',
    );
    const repo = await packRepo({ linkFirescale: false });
    const result = await runCli(['check', '--json'], repo.root, AUTO, { bin });
    expectCustomRuleRan(result);
  });

  it('resolves it from the repo when nothing is next to the binary', async () => {
    const bin = await copyBinary(await tempDir('firescale-alone-'));
    const repo = await packRepo({ linkFirescale: true });
    const result = await runCli(['check', '--json'], repo.root, AUTO, { bin });
    expectCustomRuleRan(result);
  });

  it('tells the user to install firescale when the host cannot be found', async () => {
    const bin = await copyBinary(await tempDir('firescale-alone-'));
    const repo = await packRepo({ linkFirescale: false });
    const result = await runCli(['check'], repo.root, AUTO, { bin });
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain('firescale npm package is installed');
  });

  it('finds the host in the dev layout of this checkout', async () => {
    const repo = await packRepo({ linkFirescale: false });
    const result = await runCli(['check', '--json'], repo.root, AUTO);
    expectCustomRuleRan(result);
  });
});
