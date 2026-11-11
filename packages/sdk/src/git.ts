import { execFile } from 'node:child_process';
import { FirescaleError } from './errors.js';
import { normalizeRelative } from './paths.js';
import type { GitWriter } from './types.js';

/** How many paths go into one `git rm` call. */
const UNTRACK_BATCH = 100;

interface GitResult {
  code: number;
  stdout: string;
  stderr: string;
}

/**
 * Runs git in `cwd` without a shell. Resolves with the exit code instead of
 * rejecting, except when git itself cannot be started.
 */
export function runGit(cwd: string, args: readonly string[]): Promise<GitResult> {
  return new Promise((resolve, reject) => {
    execFile(
      'git',
      [...args],
      { cwd, encoding: 'utf8', maxBuffer: 256 * 1024 * 1024, windowsHide: true },
      (error, stdout, stderr) => {
        if (error && typeof error.code !== 'number') {
          const reason = error.code === 'ENOENT' ? 'git was not found on PATH' : error.message;
          reject(
            new FirescaleError(
              `Could not run git (${reason}). Install git and make sure it is on your PATH, then try again.`,
            ),
          );
          return;
        }
        const code = error && typeof error.code === 'number' ? error.code : 0;
        resolve({ code, stdout, stderr });
      },
    );
  });
}

/** Git access for a repo root. */
export function createGit(root: string): GitWriter {
  let isRepoCache: Promise<boolean> | undefined;

  const isRepo = (): Promise<boolean> => {
    isRepoCache ??= runGit(root, ['rev-parse', '--is-inside-work-tree']).then(
      (result) => result.code === 0 && result.stdout.trim() === 'true',
    );
    return isRepoCache;
  };

  return {
    isRepo,
    async trackedFiles() {
      if (!(await isRepo())) return [];
      // -z gives NUL-separated, unquoted paths, so odd file names survive.
      const result = await runGit(root, ['ls-files', '-z', '--cached']);
      if (result.code !== 0) {
        throw new FirescaleError(
          `git ls-files failed in ${root}: ${result.stderr.trim()}. Check that the repo is not corrupted (try \`git status\`).`,
        );
      }
      // Sort by code unit so the order is the same on every OS and locale.
      return result.stdout
        .split('\0')
        .filter((entry) => entry !== '')
        .sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
    },
    async untrack(paths) {
      if (paths.length === 0) return;
      const relative = paths.map(normalizeRelative);
      // Batches keep the command line under the Windows length limit.
      for (let start = 0; start < relative.length; start += UNTRACK_BATCH) {
        // --literal-pathspecs: file names are names, never globs or pathspec magic.
        const result = await runGit(root, [
          '--literal-pathspecs',
          'rm',
          '--cached',
          '--quiet',
          '--',
          ...relative.slice(start, start + UNTRACK_BATCH),
        ]);
        if (result.code !== 0) {
          throw new FirescaleError(
            `git rm --cached failed: ${result.stderr.trim()}. Fix the git error above, then run the fix again.`,
          );
        }
      }
    },
  };
}
