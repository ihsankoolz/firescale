import { mkdir, readFile, stat, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { createGit } from './git.js';
import { normalizeRelative, resolveInRoot } from './paths.js';
import type { FileWriter, FixContext, RepoFacts, RuleOptions } from './types.js';

export interface ContextInit {
  /** Repo root. Made absolute. */
  root: string;
  options?: RuleOptions;
  facts?: RepoFacts;
}

/** One change a fix made, or in a dry run would make. */
export type Change =
  | { kind: 'write'; file: string; before: string | null; after: string }
  | { kind: 'untrack'; file: string };

/** How a fix context behaves. */
export interface FixMode {
  /** Record changes but do not write files or touch the git index. */
  dryRun?: boolean;
  /** Called once for every change, applied or not. */
  record?: (change: Change) => void;
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
    async write(file: string, content: string) {
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
export function createContext(init: ContextInit, mode: FixMode = {}): FixContext {
  const root = path.resolve(init.root);
  const files = createFiles(root);
  const git = createGit(root);
  return Object.freeze({
    root,
    options: Object.freeze({ ...init.options }),
    facts: init.facts ?? EMPTY_FACTS,
    files: {
      ...files,
      async write(file: string, content: string) {
        const before = (await files.exists(file)) ? await files.read(file) : null;
        // Writing what is already there is not a change.
        if (before === content) return;
        mode.record?.({ kind: 'write', file: normalizeRelative(file), before, after: content });
        if (!mode.dryRun) await files.write(file, content);
      },
    },
    git: {
      ...git,
      async untrack(paths: readonly string[]) {
        if (!mode.dryRun) await git.untrack(paths);
        for (const file of paths) mode.record?.({ kind: 'untrack', file: normalizeRelative(file) });
      },
    },
  });
}
