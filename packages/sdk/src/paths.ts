import path from 'node:path';
import { FirescaleError } from './errors.js';

/** Converts a path to forward slashes, the only form firescale ever outputs. */
export function toPosix(value: string): string {
  return value.replaceAll('\\', '/');
}

/**
 * Normalises a repo-relative path: forward slashes, no leading `./`, no `..`
 * segments that leave the root.
 */
export function normalizeRelative(value: string): string {
  const normalized = path.posix.normalize(toPosix(value));
  if (normalized === '..' || normalized.startsWith('../') || path.posix.isAbsolute(normalized)) {
    throw new FirescaleError(
      `Path "${value}" is outside the repo root. Use a path relative to the repo root.`,
    );
  }
  if (/^[A-Za-z]:/.test(normalized)) {
    throw new FirescaleError(`Path "${value}" is absolute. Use a path relative to the repo root.`);
  }
  return normalized;
}

/** Resolves a repo-relative path to an absolute one, refusing to leave the root. */
export function resolveInRoot(root: string, relative: string): string {
  return path.resolve(root, ...normalizeRelative(relative).split('/'));
}
