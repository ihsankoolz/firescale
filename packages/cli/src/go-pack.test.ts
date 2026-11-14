// The go stack pack (M33, ADR 0053): init builds a starter, adopt proposes
// nothing on a module that is set up and exactly the gap on one that is not.
import { readFile, readdir } from 'node:fs/promises';
import path from 'node:path';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { EXIT_OK, runCli } from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

interface Diagnostic {
  ruleId: string;
  file: string;
  fixable: string;
}

async function diagnostics(root: string): Promise<Diagnostic[]> {
  const result = await runCli(['check', '--json', '--no-cache'], root);
  return (JSON.parse(result.stdout) as { diagnostics: Diagnostic[] }).diagnostics;
}

async function snapshot(root: string): Promise<Map<string, string>> {
  const files = new Map<string, string>();
  async function walk(dir: string): Promise<void> {
    for (const entry of await readdir(dir, { withFileTypes: true })) {
      if (entry.name === '.git') continue;
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) await walk(full);
      else
        files.set(
          path.relative(root, full).split(path.sep).join('/'),
          await readFile(full, 'utf8'),
        );
    }
  }
  await walk(root);
  return files;
}

const GO_FILES = ['go.mod', 'main.go', 'main_test.go', '.gitignore'];

describe('fscale init --packs core,go', () => {
  it('builds a starter where the go pack reports no violations (snapshots)', async () => {
    const repo = await fixtures.create({ track: 'no-git' });
    const result = await runCli(['init', '--packs', 'core,go'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    const left = await diagnostics(repo.root);
    expect(left.filter((d) => d.ruleId.startsWith('go/'))).toEqual([]);
    for (const file of GO_FILES) {
      expect(await readFile(path.join(repo.root, file), 'utf8')).toMatchSnapshot(file);
    }
    const before = await snapshot(repo.root);
    expect((await runCli(['sync'], repo.root)).code).toBe(EXIT_OK);
    expect(await snapshot(repo.root)).toEqual(before);
  });

  it('gives the starter the go commands in facts', async () => {
    const repo = await fixtures.create({ track: 'no-git' });
    expect((await runCli(['init', '--packs', 'core,go'], repo.root)).code).toBe(EXIT_OK);
    const facts = JSON.parse((await runCli(['facts', '--json'], repo.root)).stdout) as {
      projects: { kind: string; name: string; commands: Record<string, string> }[];
    };
    expect(facts.projects).toEqual([
      expect.objectContaining({
        kind: 'go',
        name: 'app',
        commands: { build: 'go build ./...', test: 'go test ./...', lint: 'go vet ./...' },
      }),
    ]);
  });
});

describe('fscale adopt --dry-run with the go pack', () => {
  async function setUpModule() {
    const starter = await fixtures.create({ track: 'no-git' });
    await runCli(['init', '--packs', 'go'], starter.root);
    const files: Record<string, string> = {};
    for (const file of GO_FILES) {
      files[file] = await readFile(path.join(starter.root, file), 'utf8');
    }
    files['firescale.config.json'] = JSON.stringify({ packs: ['go'] });
    return files;
  }

  it('proposes no change on a module that is already set up', async () => {
    const repo = await fixtures.create({ files: await setUpModule() });
    const result = await runCli(['adopt', '--dry-run'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toContain('Dry run: `fscale adopt` would change nothing.');
  });

  it('proposes exactly the missing go line on a module that lacks it', async () => {
    const files = await setUpModule();
    files['go.mod'] = 'module example.com/app\n';
    const repo = await fixtures.create({ files });
    const before = await snapshot(repo.root);
    const result = await runCli(['adopt', '--dry-run'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toContain('go/go-mod\n  would edit go.mod');
    expect(result.stdout).toContain('Dry run: 1 change.');
    expect(await snapshot(repo.root)).toEqual(before);
  });
});
