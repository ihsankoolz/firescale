// The README that the `firescale` npm package ships has no diagram: npm does
// not draw Mermaid. `scripts/npm-readme.mjs` makes it from the repo's README.
import { execFile } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { promisify } from 'node:util';
import { describe, expect, it } from 'vitest';
import { REPO } from './test-helpers.js';

const run = promisify(execFile);
const FENCE = '```mermaid';

describe('the README of the npm package', () => {
  it('has no diagram, and links to the README on GitHub in their place', async () => {
    const readme = await readFile(path.join(REPO, 'README.md'), 'utf8');
    // The repo's README has the diagrams, at most five.
    const diagrams = readme.split(FENCE).length - 1;
    expect(diagrams).toBeGreaterThan(0);
    expect(diagrams).toBeLessThanOrEqual(5);

    const { stdout } = await run(process.execPath, [path.join(REPO, 'scripts', 'npm-readme.mjs')], {
      maxBuffer: 16 * 1024 * 1024,
    });
    expect(stdout).not.toContain(FENCE);
    expect(stdout).not.toContain('diagrams:start');
    expect(stdout).toContain(
      'read them in [the README on GitHub](https://github.com/ihsankoolz/firescale#how-it-works).',
    );
    // Everything else is the repo's README.
    expect(stdout.startsWith(readme.slice(0, readme.indexOf('<!-- diagrams:start -->')))).toBe(
      true,
    );
    expect(stdout.endsWith(readme.slice(readme.indexOf('## One real project')))).toBe(true);
  });
});
