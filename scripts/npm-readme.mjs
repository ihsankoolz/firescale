#!/usr/bin/env node
// The README of the `firescale` npm package: the repo's README without its
// diagrams. npmjs.com does not draw Mermaid, so the section between the two
// `diagrams` marks gives way to a link to the README on GitHub.
//
//   node scripts/npm-readme.mjs    prints it
import { readFileSync, realpathSync } from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

const START = '<!-- diagrams:start -->';
const END = '<!-- diagrams:end -->';
const GITHUB = 'https://github.com/ihsankoolz/firescale#how-it-works';

/** The text of README.md as the npm package ships it. */
export function npmReadme(text) {
  const start = text.indexOf(START);
  const end = text.indexOf(END);
  if (start < 0 || end < start) {
    throw new Error(`README.md must hold "${START}" and then "${END}".`);
  }
  const link = `## How it works\n\nDiagrams show how the parts fit together. npm does not draw them: read them in [the README on GitHub](${GITHUB}).`;
  const result = `${text.slice(0, start)}${link}${text.slice(end + END.length)}`;
  if (result.includes('```mermaid')) {
    throw new Error('README.md has a Mermaid diagram outside the diagrams marks.');
  }
  return result;
}

// Node gives this file's real path, so the path it was started by is made
// real too: a checkout may be reached through a link.
if (process.argv[1] && realpathSync(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
  process.stdout.write(npmReadme(readFileSync(path.join(root, 'README.md'), 'utf8')));
}
