import { defineRule } from '@firescale/sdk';

const IGNORE_LINE = '*.log';

/** Flags tracked `.log` files. The fix untracks them and ignores `*.log`. */
export default defineRule({
  id: 'fixable/no-tracked-log',
  severity: 'low',
  category: 'maintainability',
  description: 'No .log files are tracked by git.',
  inputs: ['**/*.log', '.gitignore'],
  async check(ctx) {
    const tracked = await ctx.git.trackedFiles();
    return tracked
      .filter((file) => file.endsWith('.log'))
      .map((file) => ({
        file,
        message: `"${file}" is a log file tracked by git.`,
        fixable: 'deterministic' as const,
      }));
  },
  async fix(ctx, diagnostics) {
    const tracked = new Set(await ctx.git.trackedFiles());
    const files = diagnostics.map((d) => d.file).filter((file) => tracked.has(file));
    await ctx.git.untrack(files);
    const changedFiles = [...files];

    const current = (await ctx.files.exists('.gitignore'))
      ? await ctx.files.read('.gitignore')
      : '';
    if (!current.split(/\r?\n/).includes(IGNORE_LINE)) {
      const separator = current === '' || current.endsWith('\n') ? '' : '\n';
      await ctx.files.write('.gitignore', `${current}${separator}${IGNORE_LINE}\n`);
      changedFiles.push('.gitignore');
    }
    return {
      changedFiles,
      notes: files.length > 0 ? [`Untracked ${files.length} log file(s).`] : [],
    };
  },
});
