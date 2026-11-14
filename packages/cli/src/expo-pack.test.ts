// The expo stack pack (M30, ADR 0050): init builds a starter, adopt
// proposes nothing on an app that is set up and exactly the gap on one that
// is not.
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

const EXPO_FILES = [
  'package.json',
  'app.json',
  'App.tsx',
  'index.ts',
  'tsconfig.json',
  'eslint.config.js',
  '.gitignore',
];

describe('fscale init --packs core,expo', () => {
  it('builds a starter where the expo pack reports no violations (snapshots)', async () => {
    const repo = await fixtures.create({ track: 'no-git' });
    const result = await runCli(['init', '--packs', 'core,expo'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    const left = await diagnostics(repo.root);
    expect(left.filter((d) => d.ruleId.startsWith('expo/'))).toEqual([]);
    for (const file of EXPO_FILES) {
      expect(await readFile(path.join(repo.root, file), 'utf8')).toMatchSnapshot(file);
    }
    const before = await snapshot(repo.root);
    expect((await runCli(['sync'], repo.root)).code).toBe(EXIT_OK);
    expect(await snapshot(repo.root)).toEqual(before);
  });

  it('builds a starter with the supabase pack too', async () => {
    const repo = await fixtures.create({ track: 'no-git' });
    const result = await runCli(['init', '--packs', 'core,expo,supabase'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    const left = await diagnostics(repo.root);
    expect(
      left.filter((d) => d.ruleId.startsWith('expo/') || d.ruleId.startsWith('supabase/')),
    ).toEqual([]);
    const pkg = JSON.parse(await readFile(path.join(repo.root, 'package.json'), 'utf8')) as {
      dependencies: Record<string, string>;
      devDependencies: Record<string, string>;
    };
    expect(pkg.dependencies['expo']).toBe('~58.0.25');
    expect(pkg.devDependencies['supabase']).toBe('3.1.0');
  });
});

describe('fscale adopt --dry-run with the expo pack', () => {
  async function setUpApp() {
    const starter = await fixtures.create({ track: 'no-git' });
    await runCli(['init', '--packs', 'expo'], starter.root);
    const files: Record<string, string> = {};
    for (const file of EXPO_FILES) {
      files[file] = await readFile(path.join(starter.root, file), 'utf8');
    }
    files['firescale.config.json'] = JSON.stringify({ packs: ['expo'] });
    return files;
  }

  it('proposes no change on an app that is already set up', async () => {
    const repo = await fixtures.create({ files: await setUpApp() });
    const result = await runCli(['adopt', '--dry-run'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toContain('Dry run: `fscale adopt` would change nothing.');
  });

  it('proposes exactly the missing tsconfig.json on an app that lacks it', async () => {
    const files = await setUpApp();
    delete files['tsconfig.json'];
    const repo = await fixtures.create({ files });
    const before = await snapshot(repo.root);
    const result = await runCli(['adopt', '--dry-run'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toContain('expo/tsconfig\n  would create tsconfig.json');
    expect(result.stdout).toContain('Dry run: 1 change.');
    expect(await snapshot(repo.root)).toEqual(before);
  });
});
