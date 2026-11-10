import { describe, expect, it } from 'vitest';
import { FirescaleError } from './errors.js';
import { normalizeRelative, resolveInRoot, toPosix } from './paths.js';

describe('toPosix', () => {
  it('converts backslashes to forward slashes', () => {
    expect(toPosix('a\\b\\c.txt')).toBe('a/b/c.txt');
  });

  it('leaves forward slashes alone', () => {
    expect(toPosix('a/b')).toBe('a/b');
  });
});

describe('normalizeRelative', () => {
  it('normalises backslashes, ./ and redundant segments', () => {
    expect(normalizeRelative('apps\\web\\.env.local')).toBe('apps/web/.env.local');
    expect(normalizeRelative('./a/./b//c')).toBe('a/b/c');
    expect(normalizeRelative('a/b/../c')).toBe('a/c');
  });

  it.each(['../x', '..\\x', 'a/../../x', '..'])('rejects escaping path %j', (value) => {
    expect(() => normalizeRelative(value)).toThrow(FirescaleError);
  });

  it.each(['/etc/passwd', '\\share\\x'])('rejects absolute path %j', (value) => {
    expect(() => normalizeRelative(value)).toThrow(FirescaleError);
  });

  it('rejects a Windows drive path', () => {
    expect(() => normalizeRelative('C:\\x')).toThrow(/absolute/);
    expect(() => normalizeRelative('c:/x')).toThrow(FirescaleError);
  });
});

describe('resolveInRoot', () => {
  it('resolves under the root and refuses to escape', () => {
    const resolved = resolveInRoot(process.cwd(), 'a/b.txt');
    expect(toPosix(resolved).endsWith('/a/b.txt')).toBe(true);
    expect(() => resolveInRoot(process.cwd(), '../x')).toThrow(FirescaleError);
  });
});
