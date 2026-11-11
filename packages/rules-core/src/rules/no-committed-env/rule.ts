import path from 'node:path';
import { defineRule, FirescaleError, type RuleOptions } from '@firescale/sdk';

/** Example files that are meant to be committed. Override with the `allow` option. */
const DEFAULT_ALLOW: readonly string[] = ['.env.example'];

const GITIGNORE = '.gitignore';

/** True for `.env` and `.env.<anything>`. Not for `.envrc` or `.environment`. */
function isEnvFile(file: string): boolean {
  return /^\.env(?:\..+)?$/.test(path.posix.basename(file));
}

function allowList(options: RuleOptions): readonly string[] {
  const allow = options['allow'];
  if (allow === undefined) return DEFAULT_ALLOW;
  if (
    !Array.isArray(allow) ||
    !allow.every(
      (name): name is string => typeof name === 'string' && !/[\\/]/.test(name) && isEnvFile(name),
    )
  ) {
    throw new FirescaleError(
      'core/no-committed-env: option "allow" must be a list of env file names without folders, for example [".env.example", ".env.template"]. Fix it in your firescale config.',
    );
  }
  return allow;
}

/** Ignore every env file at any depth. */
const IGNORE_LINES: readonly string[] = ['.env', '.env.*'];

/**
 * Works out which lines to append to `.gitignore` so that env files are
 * ignored and the allowed examples are not. In a gitignore the last matching
 * line wins, so each `!<example>` line must come after the ignore lines: one
 * that sits before them is added again at the end.
 */
function linesToAppend(gitignore: string, allow: readonly string[]): string[] {
  const lines = gitignore
    .replace(new RegExp(`^${String.fromCharCode(0xfeff)}`), '')
    .split(/\r?\n/)
    .map((line) => line.trimEnd());
  const missing = IGNORE_LINES.filter((line) => !lines.includes(line));
  const lastIgnore =
    missing.length > 0
      ? Number.POSITIVE_INFINITY
      : Math.max(...IGNORE_LINES.map((line) => lines.lastIndexOf(line)));
  const negations = allow
    .map((name) => `!${name}`)
    .filter((line) => lines.lastIndexOf(line) < lastIgnore);
  return [...missing, ...negations];
}

export default defineRule({
  id: 'core/no-committed-env',
  severity: 'blocker',
  category: 'security',
  description: 'No .env files are tracked by git, except example files such as .env.example.',
  inputs: ['**/.env', '**/.env.*', '.gitignore'],

  async check(ctx) {
    const allow = allowList(ctx.options);
    const tracked = await ctx.git.trackedFiles();
    return tracked
      .filter((file) => isEnvFile(file) && !allow.includes(path.posix.basename(file)))
      .map((file) => ({
        file,
        message: `"${file}" is tracked by git. Env files usually hold secrets and must not be committed.`,
        fixable: 'deterministic' as const,
      }));
  },

  async fix(ctx, diagnostics) {
    const allow = allowList(ctx.options);
    const changedFiles: string[] = [];

    const files = diagnostics.map((diagnostic) => diagnostic.file);
    await ctx.git.untrack(files);
    changedFiles.push(...files);

    const current = (await ctx.files.exists(GITIGNORE)) ? await ctx.files.read(GITIGNORE) : '';
    const eol = current.includes('\r\n') ? '\r\n' : '\n';
    const missing = linesToAppend(current, allow);
    if (missing.length > 0) {
      const separator = current === '' || current.endsWith('\n') ? '' : eol;
      const block = ['# Env files hold secrets. Commit an example file instead.', ...missing];
      await ctx.files.write(GITIGNORE, current + separator + block.join(eol) + eol);
      changedFiles.push(GITIGNORE);
    }

    if (files.length === 0) return { changedFiles };
    return {
      changedFiles,
      notes: [
        `Untracked ${files.length} env file(s). They are still on disk and still in git history: rotate any secret they contained.`,
      ],
    };
  },
});
