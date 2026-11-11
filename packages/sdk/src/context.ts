import { mkdir, readFile, stat, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { createGit } from './git.js';
import { resolveInRoot } from './paths.js';
import type { FileWriter, FixContext, RepoFacts, RuleOptions } from './types.js';

export interface ContextInit {
  /** Repo root. Made absolute. */
  root: string;
  options?: RuleOptions;
  facts?: RepoFacts;
}

const EMPTY_FACTS: RepoFacts = Object.freeze({ packageManager: null, projects: Object.freeze([]) });

function createFiles(root: string): FileWriter {
  return {
    async exists(file) {
      try {
        await stat(resolveInRoot(root, file));
        return true;
      } catch {
        return false;
      }
    },
    read(file) {
      return readFile(resolveInRoot(root, file), 'utf8');
    },
    async write(file, content) {
      const target = resolveInRoot(root, file);
      await mkdir(path.dirname(target), { recursive: true });
      await writeFile(target, content, 'utf8');
    },
  };
}

/**
 * Builds the context handed to a rule. The same object serves `check()` and
 * `fix()`; the narrower `RuleContext` type is what keeps `check()` read-only.
 */
export function createContext(init: ContextInit): FixContext {
  const root = path.resolve(init.root);
  return Object.freeze({
    root,
    options: Object.freeze({ ...init.options }),
    facts: init.facts ?? EMPTY_FACTS,
    files: createFiles(root),
    git: createGit(root),
  });
}
