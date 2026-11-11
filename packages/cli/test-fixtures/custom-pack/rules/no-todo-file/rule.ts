import { defineRule, FirescaleError } from '@firescale/sdk';

const DEFAULT_NAMES: readonly string[] = ['TODO.md'];

function baseName(file: string): string {
  return file.slice(file.lastIndexOf('/') + 1);
}

/** Flags tracked files with a forbidden name. The `names` option replaces the default list. */
export default defineRule({
  id: 'sample/no-todo-file',
  severity: 'low',
  category: 'maintainability',
  description: 'No TODO.md files are tracked by git.',
  inputs: ['**/TODO.md'],
  async check(ctx) {
    const names = ctx.options['names'] ?? DEFAULT_NAMES;
    if (!Array.isArray(names) || !names.every((name) => typeof name === 'string')) {
      throw new FirescaleError(
        'sample/no-todo-file: option "names" must be a list of file names. Fix it in your firescale config.',
      );
    }
    // Logging must not corrupt the bridge: the host sends it to stderr.
    console.log('sample/no-todo-file: checking');
    const tracked = await ctx.git.trackedFiles();
    return tracked
      .filter((file) => names.includes(baseName(file)))
      .map((file) => ({
        file,
        message: `"${file}" is tracked. Move its items to the issue tracker and delete it.`,
        fixable: 'manual' as const,
      }));
  },
});
