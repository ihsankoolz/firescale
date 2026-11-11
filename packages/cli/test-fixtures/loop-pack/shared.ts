// Helpers for the loop-pack test fixtures: tiny rules that exercise the fix loop.
import { defineRule, type Rule } from '@firescale/sdk';

export const skillFile = new URL('./SKILL.md', import.meta.url);

interface FileRule {
  id: string;
  after?: string[];
  file: string;
  /** The content the file must have, worked out from the repo when the fix runs. */
  wanted: (read: (file: string) => Promise<string | null>) => Promise<string | null>;
}

/**
 * A rule that wants one file to have one content. Its fix writes that
 * content. When `wanted` resolves to null the rule cannot be satisfied yet
 * and reports a manual finding instead.
 */
export function fileRule(spec: FileRule): Readonly<Rule> {
  const reader =
    (ctx: { files: { exists(f: string): Promise<boolean>; read(f: string): Promise<string> } }) =>
    async (file: string) => ((await ctx.files.exists(file)) ? ctx.files.read(file) : null);
  return defineRule({
    id: spec.id,
    severity: 'low',
    category: 'consistency',
    description: `${spec.file} has the expected content.`,
    inputs: ['**'],
    ...(spec.after ? { after: spec.after } : {}),
    async check(ctx) {
      const read = reader(ctx);
      const wanted = await spec.wanted(read);
      if (wanted === null) {
        return [
          { file: spec.file, message: `${spec.file} cannot be written yet.`, fixable: 'manual' },
        ];
      }
      if ((await read(spec.file)) === wanted) return [];
      return [
        {
          file: spec.file,
          message: `${spec.file} is missing or out of date.`,
          fixable: 'deterministic',
        },
      ];
    },
    async fix(ctx) {
      const wanted = await spec.wanted(reader(ctx));
      if (wanted === null) return { changedFiles: [] };
      await ctx.files.write(spec.file, wanted);
      return { changedFiles: [spec.file] };
    },
  });
}
