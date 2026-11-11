import { readFile } from 'node:fs/promises';
import { fixtureSet } from '@firescale/sdk/testing';
import { Ajv } from 'ajv';
import { afterEach, describe, expect, it } from 'vitest';
import {
  ENFORCED_CONFIG,
  EXIT_OK,
  EXIT_USAGE,
  EXIT_VIOLATIONS,
  FAKE_ENV,
  runCli,
} from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

const SCHEMA = new URL('../test-fixtures/sarif-schema-2.1.0.json', import.meta.url);

interface Sarif {
  version: string;
  runs: {
    tool: { driver: { rules: { id: string }[] } };
    results: {
      level: string;
      locations: { physicalLocation: { artifactLocation: { uri: string } } }[];
    }[];
  }[];
}

/** Compiles the official SARIF 2.1.0 schema (JSON Schema draft-04 wording, no draft-04-only keywords). */
async function validator() {
  const schema = JSON.parse(await readFile(SCHEMA, 'utf8')) as Record<string, unknown>;
  delete schema['$schema'];
  delete schema['id'];
  return new Ajv({ allErrors: true, strict: false, validateFormats: false }).compile(schema);
}

function stableVersion(text: string): string {
  return text.replace(/"version": "(?!2\.1\.0")[^"]+"/, '"version": "<version>"');
}

function failingRepo(config?: string, extra: Record<string, string> = {}) {
  return fixtures.create({
    files: {
      '.env': FAKE_ENV,
      '.env.example': 'EXAMPLE=\n',
      'apps/web/.env.local': FAKE_ENV,
      'README.md': '# fixture\n',
      ...(config === undefined ? {} : { 'firescale.config.json': config }),
      ...extra,
    },
  });
}

async function sarif(root: string) {
  const result = await runCli(['check', '--sarif'], root);
  return { result, doc: JSON.parse(result.stdout) as Sarif };
}

describe('fscale check --sarif', () => {
  it('validates against the SARIF 2.1.0 schema for a failing repo and exits 1', async () => {
    const validate = await validator();
    const repo = await failingRepo(ENFORCED_CONFIG);
    const { result, doc } = await sarif(repo.root);
    expect(validate(doc), JSON.stringify(validate.errors)).toBe(true);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    expect(doc.runs[0]?.results.length).toBeGreaterThan(0);
    expect(doc.runs[0]?.results.every((r) => r.level === 'error')).toBe(true);
  });

  it('validates for a clean repo and exits 0', async () => {
    const validate = await validator();
    const repo = await fixtures.create({ files: { 'README.md': '# fixture\n' } });
    const { result, doc } = await sarif(repo.root);
    expect(validate(doc), JSON.stringify(validate.errors)).toBe(true);
    expect(result.code).toBe(EXIT_OK);
    expect(doc.runs[0]?.results).toEqual([]);
  });

  it('validates for a report-only run, uses level note and exits 0', async () => {
    const validate = await validator();
    const repo = await failingRepo();
    const { result, doc } = await sarif(repo.root);
    expect(validate(doc), JSON.stringify(validate.errors)).toBe(true);
    expect(result.code).toBe(EXIT_OK);
    expect(doc.runs[0]?.results.length).toBeGreaterThan(0);
    expect(doc.runs[0]?.results.every((r) => r.level === 'note')).toBe(true);
  });

  it('matches the snapshot', async () => {
    const repo = await failingRepo(ENFORCED_CONFIG);
    const result = await runCli(['check', '--sarif'], repo.root);
    expect(stableVersion(result.stdout)).toMatchSnapshot();
  });

  it('rejects --json together with --sarif with exit 2', async () => {
    const repo = await failingRepo();
    const result = await runCli(['check', '--json', '--sarif'], repo.root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain('--sarif');
    expect(result.stdout).toBe('');
  });

  it('percent-encodes file names and uses forward slashes', async () => {
    const repo = await failingRepo(ENFORCED_CONFIG, { 'my dir/café/.env': FAKE_ENV });
    const { doc } = await sarif(repo.root);
    const uris = (doc.runs[0]?.results ?? []).map(
      (r) => r.locations[0]?.physicalLocation.artifactLocation.uri,
    );
    expect(uris).toContain('my%20dir/caf%C3%A9/.env');
    expect(uris).toContain('apps/web/.env.local');
    expect(uris.some((uri) => uri?.includes('\\'))).toBe(false);
  });
});
