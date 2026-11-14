import { readFile } from 'node:fs/promises';
import { fixtureSet } from '@firescale/sdk/testing';
import { Ajv } from 'ajv';
import { afterEach, describe, expect, it } from 'vitest';
import { EXIT_OK, EXIT_USAGE, runCli } from './test-helpers.js';

// `fscale ledger` (M28, ADR 0048): the agent fix runs summed, per rule and
// per agent, from a fixture ledger with damaged lines.

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

const SCHEMA = new URL('../schemas/ledger-output.schema.json', import.meta.url);

const line = (fields: Record<string, unknown>) =>
  JSON.stringify({
    schema: 1,
    time: '2026-11-14T01:02:03Z',
    rule: 'core/env-example',
    agent: 'claude',
    result: 'pass',
    filesChanged: 1,
    linesChanged: 2,
    durationMs: 4200,
    ...fields,
  });

const LEDGER = [
  line({ costUsd: 0.12, turns: 3 }),
  line({ costUsd: 0.3, turns: 7, result: 'fail', reason: 'tamper' }),
  line({ agent: 'codex', turns: 2 }),
  'this line is not JSON',
  line({ agent: 'codex', rule: 'core/readme-setup', result: 'fail', reason: 'timeout' }),
  '',
  line({ result: 'maybe' }),
  line({ costUsd: -1 }),
  line({ rule: 'core/readme-setup', costUsd: 0.05, turns: 1 }),
  // A run cut short while it was written.
  line({}).slice(0, 30),
].join('\n');

async function repo(ledger: string | undefined) {
  const fixture = await fixtures.create({
    files: {
      'firescale.config.json': '{ "packs": ["core"] }\n',
      ...(ledger === undefined ? {} : { '.firescale/ledger.jsonl': ledger }),
    },
  });
  return fixture.root;
}

describe('fscale ledger', () => {
  it('sums runs, pass rate and cost per rule and per agent, and names damaged lines', async () => {
    const root = await repo(LEDGER);
    const result = await runCli(['ledger'], root);
    expect(result.code, result.stderr).toBe(EXIT_OK);
    expect(result.stdout).toMatchSnapshot();
  });

  it('prints JSON that matches its schema', async () => {
    const root = await repo(LEDGER);
    const result = await runCli(['ledger', '--json'], root);
    expect(result.code, result.stderr).toBe(EXIT_OK);
    const json = JSON.parse(result.stdout) as Record<string, unknown>;
    const ajv = new Ajv({ allErrors: true });
    const validate = ajv.compile(JSON.parse(await readFile(SCHEMA, 'utf8')) as object);
    expect(validate(json), JSON.stringify(validate.errors)).toBe(true);
    expect(json).toMatchSnapshot();
    expect(json['runs']).toBe(5);
    expect(json['passed']).toBe(3);
    expect(json['costUsd']).toBe(0.47);
    expect(json['damagedLines']).toEqual([4, 7, 8, 10]);
  });

  it('says there are no runs yet when the repo has no ledger', async () => {
    const root = await repo(undefined);
    const human = await runCli(['ledger'], root);
    expect(human.code).toBe(EXIT_OK);
    expect(human.stdout).toContain('No agent runs yet');
    const json = JSON.parse((await runCli(['ledger', '--json'], root)).stdout) as Record<
      string,
      unknown
    >;
    expect(json['found']).toBe(false);
    expect(json['runs']).toBe(0);
  });

  it('reads a ledger with CRLF line endings', async () => {
    const root = await repo(LEDGER.replaceAll('\n', '\r\n'));
    const json = JSON.parse((await runCli(['ledger', '--json'], root)).stdout) as Record<
      string,
      unknown
    >;
    expect(json['runs']).toBe(5);
    expect(json['damagedLines']).toEqual([4, 7, 8, 10]);
  });

  it('takes no arguments', async () => {
    const root = await repo(LEDGER);
    const result = await runCli(['ledger', 'extra'], root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain('ledger takes no arguments');
  });
});
