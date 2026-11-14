// The supabase stack pack (M29, ADR 0049): init builds a starter, adopt
// proposes nothing on a project that is set up and exactly the gap on one
// that is not.
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

const SUPABASE_FILES = [
  'package.json',
  'supabase/config.toml',
  'supabase/.gitignore',
  'supabase/tests/database/rls_test.sql',
];

describe('fscale init --packs core,supabase', () => {
  it('builds a starter where the supabase pack reports no violations (snapshots)', async () => {
    const repo = await fixtures.create({ track: 'no-git' });
    const result = await runCli(['init', '--packs', 'core,supabase'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    const left = await diagnostics(repo.root);
    expect(left.filter((d) => d.ruleId.startsWith('supabase/'))).toEqual([]);
    for (const file of SUPABASE_FILES) {
      expect(await readFile(path.join(repo.root, file), 'utf8')).toMatchSnapshot(file);
    }
    // A second run changes nothing.
    const before = await snapshot(repo.root);
    expect((await runCli(['sync'], repo.root)).code).toBe(EXIT_OK);
    expect(await snapshot(repo.root)).toEqual(before);
  });

  it('builds the same starter next to the nextjs pack', async () => {
    const repo = await fixtures.create({ track: 'no-git' });
    const result = await runCli(['init', '--packs', 'core,nextjs,supabase'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    const left = await diagnostics(repo.root);
    expect(
      left.filter((d) => d.ruleId.startsWith('supabase/') || d.ruleId.startsWith('nextjs/')),
    ).toEqual([]);
    const pkg = JSON.parse(await readFile(path.join(repo.root, 'package.json'), 'utf8')) as {
      dependencies: Record<string, string>;
      devDependencies: Record<string, string>;
    };
    expect(pkg.dependencies['next']).toBeDefined();
    expect(pkg.devDependencies['supabase']).toBe('3.1.0');
  });
});

describe('fscale adopt --dry-run with the supabase pack', () => {
  async function setUpProject() {
    const starter = await fixtures.create({ track: 'no-git' });
    await runCli(['init', '--packs', 'supabase'], starter.root);
    const files: Record<string, string> = {};
    for (const file of SUPABASE_FILES) {
      files[file] = await readFile(path.join(starter.root, file), 'utf8');
    }
    files['supabase/migrations/20261114000000_notes.sql'] =
      'create table public.notes (id bigint primary key);\nalter table public.notes enable row level security;\n';
    files['firescale.config.json'] = JSON.stringify({ packs: ['supabase'] });
    return files;
  }

  it('proposes no change on a project that is already set up', async () => {
    const repo = await fixtures.create({ files: await setUpProject() });
    const result = await runCli(['adopt', '--dry-run'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toContain('Dry run: `fscale adopt` would change nothing.');
    expect((await diagnostics(repo.root)).filter((d) => d.ruleId.startsWith('supabase/'))).toEqual(
      [],
    );
  });

  it('proposes exactly the [inbucket] rename on a config from an older CLI', async () => {
    const files = await setUpProject();
    files['supabase/config.toml'] =
      'project_id = "supabase-app"\n\n[inbucket]\nenabled = true\nport = 54324\n';
    const repo = await fixtures.create({ files });
    const before = await snapshot(repo.root);
    const result = await runCli(['adopt', '--dry-run'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toContain('supabase/local-smtp\n  would edit supabase/config.toml');
    expect(result.stdout).toContain('-[inbucket]');
    expect(result.stdout).toContain('+[local_smtp]');
    expect(result.stdout).toContain('Dry run: 1 change.');
    expect(await snapshot(repo.root)).toEqual(before);
  });

  it('reports a table without row level security for an agent, and writes nothing for it', async () => {
    const files = await setUpProject();
    files['supabase/migrations/20261114010000_todos.sql'] = 'create table todos (id int);\n';
    const repo = await fixtures.create({ files });
    const found = (await diagnostics(repo.root)).filter((d) => d.ruleId === 'supabase/rls');
    expect(found).toEqual([
      expect.objectContaining({
        file: 'supabase/migrations/20261114010000_todos.sql',
        fixable: 'agent',
      }),
    ]);
    const result = await runCli(['adopt', '--dry-run'], repo.root);
    expect(result.stdout).toContain('Dry run: `fscale adopt` would change nothing.');
  });
});
