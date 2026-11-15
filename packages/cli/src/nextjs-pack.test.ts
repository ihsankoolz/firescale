// The nextjs stack pack (M15, ADR 0029): init builds a starter, adopt
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

const NEXT_FILES = [
  'package.json',
  'app/layout.tsx',
  'app/page.tsx',
  'next.config.ts',
  'tsconfig.json',
  'eslint.config.mjs',
  '.gitignore',
];

describe('fscale init --packs core,nextjs', () => {
  it('builds a starter where the nextjs pack reports no violations (snapshots)', async () => {
    const repo = await fixtures.create({ track: 'no-git' });
    const result = await runCli(['init', '--packs', 'core,nextjs'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    const left = await diagnostics(repo.root);
    expect(left.filter((d) => d.ruleId.startsWith('nextjs/'))).toEqual([]);
    for (const file of NEXT_FILES) {
      expect(await readFile(path.join(repo.root, file), 'utf8')).toMatchSnapshot(file);
    }
    // A second run changes nothing.
    const before = await snapshot(repo.root);
    expect((await runCli(['sync'], repo.root)).code).toBe(EXIT_OK);
    expect(await snapshot(repo.root)).toEqual(before);
  });
});

describe('fscale adopt --dry-run with the nextjs pack', () => {
  async function setUpApp() {
    const starter = await fixtures.create({ track: 'no-git' });
    await runCli(['init', '--packs', 'nextjs'], starter.root);
    const files: Record<string, string> = {};
    for (const file of NEXT_FILES) {
      files[file] = await readFile(path.join(starter.root, file), 'utf8');
    }
    files['firescale.config.json'] = JSON.stringify({ packs: ['nextjs'] });
    return files;
  }

  it('proposes no change on an app that is already set up', async () => {
    const repo = await fixtures.create({ files: await setUpApp() });
    const result = await runCli(['adopt', '--dry-run'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toContain('Dry run: `fscale adopt` would change nothing.');
  });

  it('proposes exactly the missing root layout on an app that lacks it', async () => {
    const files = await setUpApp();
    delete files['app/layout.tsx'];
    const repo = await fixtures.create({ files });
    const before = await snapshot(repo.root);
    const result = await runCli(['adopt', '--dry-run'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toContain('nextjs/app-router\n  would create app/layout.tsx');
    expect(result.stdout).toContain('Dry run: 1 change.');
    expect(await snapshot(repo.root)).toEqual(before);
  });
});

describe('fscale check result cache with the nextjs pack', () => {
  // The pack applies by what package.json says, so every rule's cached
  // result depends on it (as the supabase pack's does since M29).
  it('reports the app once package.json depends on next, not the cached "no finding"', async () => {
    const repo = await fixtures.create({
      files: {
        'firescale.config.json': JSON.stringify({ packs: ['nextjs'] }),
        'package.json': JSON.stringify({ name: 'shop', private: true }),
      },
    });
    const ids = async (...args: string[]) => {
      const result = await runCli(['check', '--json', ...args], repo.root);
      const found = (JSON.parse(result.stdout) as { diagnostics: Diagnostic[] }).diagnostics;
      return found.map((d) => d.ruleId).sort();
    };
    // Not a Next.js app yet: only nextjs/package-json speaks. This run
    // fills the cache.
    expect(await ids()).toEqual(['nextjs/package-json']);

    await repo.write(
      'package.json',
      JSON.stringify({
        name: 'shop',
        private: true,
        dependencies: { next: '16.4.6', react: '19.3.1', 'react-dom': '19.3.1' },
      }),
    );
    const cached = await ids();
    expect(cached).toEqual(
      expect.arrayContaining(['nextjs/app-router', 'nextjs/gitignore', 'nextjs/next-config']),
    );
    expect(cached).toEqual(await ids('--no-cache'));
  });
});
