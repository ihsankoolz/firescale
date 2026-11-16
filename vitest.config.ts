import { defineConfig } from 'vitest/config';
import { BaseSequencer, type TestSpecification } from 'vitest/node';

// The files that take longest, longest first (ADR 0066). Vitest starts a
// file it has no earlier result for by its size, and in CI it never has one.
// tamper.test.ts is small but runs 479 corpus cases, about 870 s on the
// Windows runner, so it started last and ran alone for 8 to 12 minutes.
// Started first, it runs beside the others. Order only: every file still runs.
const LONGEST_FIRST = [
  'packages/cli/src/tamper.test.ts',
  'packages/cli/src/config-affects.test.ts',
  'packages/cli/src/blame.test.ts',
];

const rank = (spec: TestSpecification): number => {
  const file = spec.moduleId.replaceAll('\\', '/');
  const index = LONGEST_FIRST.findIndex((name) => file.endsWith(`/${name}`));
  return index === -1 ? LONGEST_FIRST.length : index;
};

class LongestFirst extends BaseSequencer {
  override async sort(files: TestSpecification[]): Promise<TestSpecification[]> {
    // Array sort is stable: the other files keep Vitest's own order.
    return (await super.sort(files)).sort((a, b) => rank(a) - rank(b));
  }
}

export default defineConfig({
  test: {
    include: ['packages/*/src/**/*.test.ts'],
    testTimeout: 30_000,
    sequence: { sequencer: LongestFirst },
  },
});
