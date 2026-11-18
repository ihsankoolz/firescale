#!/usr/bin/env node
// Renders every Mermaid block of the files that have diagrams, and fails
// when one does not parse.
//
//   node scripts/check-mermaid.mjs [file ...]
//
// It needs `mmdc`, the command of @mermaid-js/mermaid-cli, on the PATH
// (`npm install --global @mermaid-js/mermaid-cli`), or MMDC set to it. The
// package downloads a browser, so it is not a dependency of this repo.
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const files = process.argv.slice(2);
if (files.length === 0) files.push('README.md', 'docs/architecture.md');

/** The Mermaid blocks of a Markdown text, each with the line it starts on. */
function blocks(text) {
  const found = [];
  const lines = text.split(/\r?\n/);
  let start = -1;
  for (const [index, line] of lines.entries()) {
    if (start < 0 && line.trim() === '```mermaid') {
      start = index + 1;
    } else if (start >= 0 && line.trim() === '```') {
      found.push({ line: start, code: lines.slice(start, index).join('\n') });
      start = -1;
    }
  }
  return found;
}

const mmdc = process.env.MMDC ?? 'mmdc';
const work = mkdtempSync(path.join(tmpdir(), 'firescale-mermaid-'));
let failed = 0;
let count = 0;
try {
  for (const file of files) {
    const text = readFileSync(path.resolve(root, file), 'utf8');
    for (const block of blocks(text)) {
      count += 1;
      const input = path.join(work, `diagram-${count}.mmd`);
      const output = path.join(work, `diagram-${count}.svg`);
      writeFileSync(input, `${block.code}\n`);
      const result = spawnSync(mmdc, ['--quiet', '--input', input, '--output', output], {
        encoding: 'utf8',
        shell: process.platform === 'win32',
      });
      if (result.error?.code === 'ENOENT') {
        console.error(
          `check-mermaid: ${mmdc} was not found. Install it with: npm install --global @mermaid-js/mermaid-cli`,
        );
        process.exit(2);
      }
      let size = 0;
      try {
        size = statSync(output).size;
      } catch {
        // No file: the diagram did not render.
      }
      if (result.status !== 0 || size === 0) {
        failed += 1;
        console.error(`FAILED  ${file}:${block.line}`);
        console.error(`${result.stderr ?? ''}${result.stdout ?? ''}`.trim());
      } else {
        console.log(`ok      ${file}:${block.line}`);
      }
    }
  }
} finally {
  rmSync(work, { recursive: true, force: true });
}
console.log(`${count} diagrams, ${failed} failed`);
process.exit(failed === 0 ? 0 : 1);
