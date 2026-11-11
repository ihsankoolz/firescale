import { FirescaleError } from '@firescale/sdk';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import rule from './rule.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

const RULE_ID = 'core/no-committed-env';
const EXAMPLE = 'EXAMPLE=1\n';

describe('core/no-committed-env: passing repo', () => {
  it('reports nothing when only .env.example is tracked', async () => {
    const repo = await fixtures.create({
      files: { '.env.example': EXAMPLE, 'README.md': '# demo\n', 'apps/web/.env.example': EXAMPLE },
    });
    expect(await repo.check(rule)).toEqual([]);
  });

  it('reports nothing for an env file on disk that is not tracked', async () => {
    const repo = await fixtures.create({
      files: { '.env': EXAMPLE, '.gitignore': '.env\n', 'README.md': '# demo\n' },
      track: ['.gitignore', 'README.md'],
    });
    expect(await repo.check(rule)).toEqual([]);
  });

  it('does not flag .envrc or similarly named files', async () => {
    const repo = await fixtures.create({
      files: { '.envrc': 'EXAMPLE=1\n', '.environment': 'x\n', 'env.txt': 'x\n' },
    });
    expect(await repo.check(rule)).toEqual([]);
  });

  it('reports nothing outside a git repo', async () => {
    const repo = await fixtures.create({ files: { '.env': EXAMPLE }, track: 'no-git' });
    expect(await repo.check(rule)).toEqual([]);
  });

  it('honours the allow option', async () => {
    const repo = await fixtures.create({
      files: { '.env.template': EXAMPLE, '.env.example': EXAMPLE },
    });
    const options = { allow: ['.env.example', '.env.template'] };
    expect(await repo.check(rule, options)).toEqual([]);
    // With the default allow list only .env.example is exempt.
    const defaults = await repo.check(rule);
    expect(defaults.map((d) => d.file)).toEqual(['.env.template']);
  });

  it('rejects an invalid allow option', async () => {
    const repo = await fixtures.create({ files: { '.env': EXAMPLE } });
    await expect(repo.check(rule, { allow: 'nope' })).rejects.toThrow(FirescaleError);
    await expect(repo.check(rule, { allow: [1] })).rejects.toThrow(FirescaleError);
    await expect(repo.check(rule, { allow: ['package.json'] })).rejects.toThrow(FirescaleError);
  });
});

describe('core/no-committed-env: failing repo', () => {
  it('reports tracked env files, sorted, with forward-slash paths', async () => {
    const repo = await fixtures.create({
      files: {
        'apps/web/.env.local': EXAMPLE,
        '.env': EXAMPLE,
        '.env.example': EXAMPLE,
        'README.md': '# demo\n',
      },
    });
    const diagnostics = await repo.check(rule);
    expect(diagnostics.map((d) => d.file)).toEqual(['.env', 'apps/web/.env.local']);
    for (const d of diagnostics) {
      expect(d.ruleId).toBe(RULE_ID);
      expect(d.fixable).toBe('deterministic');
      expect(d.file).not.toContain('\\');
      expect(d.message).toContain(d.file);
    }
  });

  it('reports a tracked env file even when .gitignore lists it', async () => {
    const repo = await fixtures.create({ files: { '.env': EXAMPLE, '.gitignore': '.env\n' } });
    expect((await repo.check(rule)).map((d) => d.file)).toEqual(['.env']);
  });
});

describe('core/no-committed-env: fixed repo', () => {
  async function failing(gitignore?: string) {
    return fixtures.create({
      files: {
        '.env': 'EXAMPLE=1\n',
        'apps/web/.env.local': 'EXAMPLE=2\n',
        '.env.example': EXAMPLE,
        ...(gitignore === undefined ? {} : { '.gitignore': gitignore }),
      },
    });
  }

  it('untracks the files, keeps them on disk and ignores them', async () => {
    const repo = await failing();
    const result = await repo.fix(rule);

    expect(result.changedFiles).toEqual(['.env', '.gitignore', 'apps/web/.env.local']);
    expect(await repo.check(rule)).toEqual([]);
    expect(await repo.read('.env')).toBe('EXAMPLE=1\n');
    expect(await repo.read('apps/web/.env.local')).toBe('EXAMPLE=2\n');

    const tracked = await repo.trackedFiles();
    expect(tracked).not.toContain('.env');
    expect(tracked).not.toContain('apps/web/.env.local');
    expect(tracked).toContain('.env.example');

    const lines = ((await repo.read('.gitignore')) ?? '').split('\n');
    expect(lines).toEqual(expect.arrayContaining(['.env', '.env.*', '!.env.example']));
  });

  it('uses the allow option for the negation lines', async () => {
    const repo = await failing();
    await repo.fix(rule, { allow: ['.env.example', '.env.template'] });
    const lines = ((await repo.read('.gitignore')) ?? '').split('\n');
    expect(lines).toEqual(expect.arrayContaining(['!.env.example', '!.env.template']));
  });

  it('preserves existing .gitignore content and only adds missing lines', async () => {
    const repo = await failing('node_modules\n.env\n');
    await repo.fix(rule);
    const text = (await repo.read('.gitignore')) ?? '';
    expect(text.startsWith('node_modules\n.env\n')).toBe(true);
    const lines = text.split('\n');
    expect(lines.filter((l) => l === '.env')).toHaveLength(1);
    expect(lines).toEqual(expect.arrayContaining(['.env.*', '!.env.example']));
  });

  it('handles a .gitignore without a trailing newline', async () => {
    const repo = await failing('node_modules');
    await repo.fix(rule);
    const text = (await repo.read('.gitignore')) ?? '';
    expect(text.startsWith('node_modules\n')).toBe(true);
    expect(text.split('\n')).toContain('.env.*');
  });

  it('keeps CRLF line endings in an existing CRLF .gitignore', async () => {
    const repo = await failing('node_modules\r\ndist\r\n');
    await repo.fix(rule);
    const text = (await repo.read('.gitignore')) ?? '';
    expect(text.startsWith('node_modules\r\ndist\r\n')).toBe(true);
    expect(text.replaceAll('\r\n', '')).not.toContain('\n');
    expect(text.split('\r\n')).toEqual(expect.arrayContaining(['.env', '.env.*', '!.env.example']));
  });

  it('notes that history still contains the secrets', async () => {
    const repo = await failing();
    const result = await repo.fix(rule);
    expect(result.notes?.join(' ')).toContain('rotate');
  });
});

describe('core/no-committed-env: fix run twice (idempotent)', () => {
  it('the second run changes nothing', async () => {
    const repo = await fixtures.create({
      files: { '.env': EXAMPLE, 'apps/web/.env.local': EXAMPLE, '.gitignore': 'dist\r\n' },
    });
    const first = await repo.fix(rule);
    expect(first.changedFiles.length).toBeGreaterThan(0);
    const gitignoreAfterFirst = await repo.read('.gitignore');
    const trackedAfterFirst = await repo.trackedFiles();

    const second = await repo.fix(rule);
    expect(second.changedFiles).toEqual([]);
    expect(await repo.read('.gitignore')).toBe(gitignoreAfterFirst);
    expect(await repo.trackedFiles()).toEqual(trackedAfterFirst);
    expect(await repo.check(rule)).toEqual([]);
  });

  it('does nothing on a repo that is already clean', async () => {
    const repo = await fixtures.create({ files: { '.env.example': EXAMPLE } });
    expect(await repo.fix(rule)).toEqual({ changedFiles: [] });
    expect(await repo.read('.gitignore')).toBeNull();
  });
});

describe('core/no-committed-env: review regressions', () => {
  it('keeps .env.example un-ignored when its negation sits before the ignore lines', async () => {
    const repo = await fixtures.create({
      files: { '.env': EXAMPLE, '.env.example': EXAMPLE, '.gitignore': '!.env.example\n.env\n' },
    });
    await repo.fix(rule);
    const lines = ((await repo.read('.gitignore')) ?? '').split('\n');
    expect(lines.lastIndexOf('!.env.example')).toBeGreaterThan(lines.lastIndexOf('.env.*'));
    expect(lines.lastIndexOf('!.env.example')).toBeGreaterThan(lines.lastIndexOf('.env'));

    // git itself must agree: without --force, the example is still addable.
    const { runGit } = await import('@firescale/sdk');
    const ignored = await runGit(repo.root, ['check-ignore', '--no-index', '-q', '.env.example']);
    expect(ignored.code).toBe(1);
    const envIgnored = await runGit(repo.root, ['check-ignore', '--no-index', '-q', '.env.local']);
    expect(envIgnored.code).toBe(0);
  });

  it('untracks files whose names contain spaces, unicode and glob characters', async () => {
    const names = ['my app/.env', 'café/.env.local', '[x]/.env', 'a b/[y] é/.env.prod'];
    const repo = await fixtures.create({
      files: Object.fromEntries(names.map((name) => [name, EXAMPLE])),
    });
    expect((await repo.check(rule)).map((d) => d.file).sort()).toEqual([...names].sort());
    const result = await repo.fix(rule);
    expect(result.changedFiles).toEqual(expect.arrayContaining(names));
    expect(await repo.trackedFiles()).toEqual([]);
    expect(await repo.check(rule)).toEqual([]);
    for (const name of names) expect(await repo.read(name)).toBe(EXAMPLE);
  });

  it('treats a file name as a literal, not as a glob over other files', async () => {
    const repo = await fixtures.create({
      files: { '[ab]/.env': EXAMPLE, 'a/keep.txt': 'x\n', 'b/keep.txt': 'x\n' },
    });
    await repo.fix(rule);
    expect(await repo.trackedFiles()).toEqual(['a/keep.txt', 'b/keep.txt']);
  });

  it('rejects allow entries that contain a folder', async () => {
    const repo = await fixtures.create({ files: { 'apps/web/.env.local': EXAMPLE } });
    await expect(repo.check(rule, { allow: ['apps/web/.env.local'] })).rejects.toThrow(
      'without folders',
    );
  });

  it('fix() itself is idempotent when called twice with the same diagnostics', async () => {
    const { createContext } = await import('@firescale/sdk');
    const repo = await fixtures.create({
      files: { '.env': EXAMPLE, '.gitignore': '.env\n' },
    });
    const diagnostics = await repo.check(rule);
    const ctx = createContext({ root: repo.root });
    if (!rule.fix) throw new Error('rule has no fix');

    await rule.fix(ctx, diagnostics);
    const afterFirst = await repo.read('.gitignore');
    expect(afterFirst).toBe(
      '.env\n# Env files hold secrets. Commit an example file instead.\n.env.*\n!.env.example\n',
    );
    // Second direct call: the file is already untracked and the lines exist.
    const second = await rule.fix(ctx, []);
    expect(second.changedFiles).toEqual([]);
    expect(await repo.read('.gitignore')).toBe(afterFirst);
  });

  it('does not duplicate lines in a .gitignore that starts with a BOM', async () => {
    const bom = String.fromCharCode(0xfeff);
    const repo = await fixtures.create({
      files: { '.env': EXAMPLE, '.gitignore': `${bom}.env\n.env.*\n!.env.example\n` },
      track: ['.env'],
    });
    const result = await repo.fix(rule);
    expect(result.changedFiles).toEqual(['.env']);
    expect(await repo.read('.gitignore')).toBe(`${bom}.env\n.env.*\n!.env.example\n`);
  });
});
