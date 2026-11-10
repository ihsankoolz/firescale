/**
 * Test helpers for rule authors. Import from `@firescale/sdk/testing`.
 *
 * Fixture repos are built in a temp folder at test time rather than checked
 * in: a fixture needs its own git index, and a nested `.git` folder cannot be
 * committed (see docs/decisions/0006-fixture-repos.md).
 */
import { mkdir, mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { createGit, runGit } from './git.js';
import { FirescaleError } from './errors.js';
import { resolveInRoot } from './paths.js';
import { checkRule, fixRule } from './run.js';
import type { Diagnostic, FixResult, Rule, RuleOptions } from './types.js';

export interface FixtureSpec {
  /** File contents keyed by repo-relative path (forward slashes). */
  files?: Record<string, string>;
  /**
   * Which files to add to the git index.
   * - `'all'` (default): every file, even ones a `.gitignore` would skip.
   * - a list: only those paths.
   * - `'none'`: a git repo with an empty index.
   * - `'no-git'`: a plain folder that is not a git repo.
   */
  track?: 'all' | 'none' | 'no-git' | readonly string[];
}

export interface FixtureRepo {
  /** Absolute, real path of the fixture root. */
  readonly root: string;
  /** Runs the rule's check against the fixture. */
  check(rule: Rule, options?: RuleOptions): Promise<Diagnostic[]>;
  /** Runs the rule's deterministic fix against the fixture. */
  fix(rule: Rule, options?: RuleOptions): Promise<FixResult>;
  /** Repo-relative paths currently in the git index, sorted. */
  trackedFiles(): Promise<string[]>;
  /** Reads a file, or returns null when it does not exist. */
  read(file: string): Promise<string | null>;
  write(file: string, content: string): Promise<void>;
  /** Adds paths to the git index, including ignored ones. */
  track(paths: readonly string[]): Promise<void>;
  /** Deletes the fixture. Safe to call twice. */
  cleanup(): Promise<void>;
}

async function git(root: string, args: readonly string[]): Promise<void> {
  // Fixtures must not depend on the machine's git config or line-ending settings.
  const result = await runGit(root, [
    '-c',
    'core.autocrlf=false',
    '-c',
    'core.safecrlf=false',
    ...args,
  ]);
  if (result.code !== 0) {
    throw new FirescaleError(
      `Fixture setup failed: git ${args.join(' ')}: ${result.stderr.trim()}. Check that git is installed and working.`,
    );
  }
}

/** Creates a throwaway repo in the OS temp folder. Call `cleanup()` when done. */
export async function createFixtureRepo(spec: FixtureSpec = {}): Promise<FixtureRepo> {
  // realpath: macOS temp folders are symlinks and Windows may hand out 8.3 short names.
  const root = await realpath(await mkdtemp(path.join(tmpdir(), 'firescale-fixture-')));
  const track = spec.track ?? 'all';

  const write = async (file: string, content: string): Promise<void> => {
    const target = resolveInRoot(root, file);
    await mkdir(path.dirname(target), { recursive: true });
    await writeFile(target, content, 'utf8');
  };
  const trackPaths = async (paths: readonly string[]): Promise<void> => {
    if (paths.length > 0) await git(root, ['add', '--force', '--', ...paths]);
  };

  const files = Object.entries(spec.files ?? {});
  for (const [file, content] of files) await write(file, content);

  if (track !== 'no-git') {
    await git(root, ['init', '--quiet']);
    if (track === 'all') await trackPaths(files.map(([file]) => file));
    else if (track !== 'none') await trackPaths(track);
  }

  return {
    root,
    check: (rule, options) => checkRule(rule, options ? { root, options } : { root }),
    fix: (rule, options) => fixRule(rule, options ? { root, options } : { root }),
    trackedFiles: () => createGit(root).trackedFiles(),
    async read(file) {
      try {
        return await readFile(resolveInRoot(root, file), 'utf8');
      } catch {
        return null;
      }
    },
    write,
    track: trackPaths,
    cleanup: () => rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }),
  };
}

/**
 * Tracks fixtures so one `afterEach(fixtures.cleanup)` removes them all.
 *
 * ```ts
 * const fixtures = fixtureSet();
 * afterEach(fixtures.cleanup);
 * const repo = await fixtures.create({ files: { '.env': 'A=1\n' } });
 * ```
 */
export function fixtureSet(): {
  create(spec?: FixtureSpec): Promise<FixtureRepo>;
  cleanup(): Promise<void>;
} {
  const open: FixtureRepo[] = [];
  return {
    async create(spec) {
      const repo = await createFixtureRepo(spec);
      open.push(repo);
      return repo;
    },
    async cleanup() {
      await Promise.all(open.splice(0).map((repo) => repo.cleanup()));
    },
  };
}
