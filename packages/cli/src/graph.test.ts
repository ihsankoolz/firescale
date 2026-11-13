import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { runGit } from '@firescale/sdk';
import { fixtureSet, type FixtureRepo } from '@firescale/sdk/testing';
import { Ajv } from 'ajv';
import { afterEach, describe, expect, it } from 'vitest';
import {
  COURSE_SERVICES,
  COURSE_SHARED_USERS,
  courseProjectFiles,
  shimEnv,
  writeToolShims,
} from './course-fixture.js';
import { EXIT_OK, EXIT_USAGE, runCli } from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

const GRAPH_SCHEMA = new URL('../schemas/graph-output.schema.json', import.meta.url);
const VERIFY_SCHEMA = new URL('../schemas/verify-output.schema.json', import.meta.url);

async function validate(schemaUrl: URL, json: string): Promise<unknown> {
  const schema: object = JSON.parse(await readFile(schemaUrl, 'utf8'));
  const check = new Ajv({ strict: true }).compile(schema);
  const data: unknown = JSON.parse(json);
  const ok = check(data);
  expect(check.errors ?? []).toEqual([]);
  expect(ok).toBe(true);
  return data;
}

interface GraphReport {
  schemaVersion: number;
  nodes: { id: string; type: string; path: string; kind: string }[];
  edges: { from: string; to: string; sources: { source: string; file: string }[] }[];
  warnings: string[];
}

interface AffectedReport {
  scope: string;
  result: string;
  steps: { name: string; project?: string; status: string }[];
  affected?: {
    base: string | null;
    everything: boolean;
    reason?: string;
    projectsTotal: number;
    projectsRun: number;
    projects: { project?: string; affected: boolean; reason: string }[];
  };
}

/** Commits everything, with a throwaway identity. */
async function commitAll(repo: FixtureRepo, message: string): Promise<void> {
  for (const args of [
    ['add', '--all'],
    [
      '-c',
      'user.name=fixture',
      '-c',
      'user.email=fixture@example.invalid',
      '-c',
      'commit.gpgsign=false',
      'commit',
      '--quiet',
      '--no-verify',
      '-m',
      message,
    ],
  ]) {
    const result = await runGit(repo.root, args);
    expect(result.code).toBe(0);
  }
}

/** The course project, committed, with stand-ins for python, npx and npm. */
async function courseProject() {
  const repo = await fixtures.create({ files: courseProjectFiles() });
  await commitAll(repo, 'initial');
  const tools = await fixtures.create({ files: {}, track: 'no-git' });
  await writeToolShims(tools.root);
  const log = path.join(tools.root, 'calls.log');
  const calls = async (): Promise<string[]> =>
    (await readFile(log, 'utf8').catch(() => '')).split(/\r?\n/).filter(Boolean);
  return { repo, calls, env: shimEnv(tools.root, log) };
}

describe('fscale graph', () => {
  it('prints the projects, the shared folder and the edges of the course project', async () => {
    const repo = await fixtures.create({ files: courseProjectFiles(), track: 'no-git' });
    const plain = await runCli(['graph'], repo.root);
    expect(plain.code).toBe(EXIT_OK);
    expect(plain.stderr).toBe('');
    expect(plain.stdout).toMatchSnapshot();
    for (const service of COURSE_SHARED_USERS) {
      expect(plain.stdout).toContain(`backend/${service} -> backend/shared (Python import)\n`);
    }
    expect(plain.stdout).toContain('  backend/shared (folder)\n');

    const explained = await runCli(['graph', '--explain'], repo.root);
    expect(explained.code).toBe(EXIT_OK);
    expect(explained.stdout).toMatchSnapshot();
    expect(explained.stdout).toContain(
      '    Python import: backend/auth_service/main.py:1: from shared.db import URL\n',
    );
  });

  it('prints schema-valid JSON whose edges name their source', async () => {
    const repo = await fixtures.create({ files: courseProjectFiles(), track: 'no-git' });
    const result = await runCli(['graph', '--json'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    const graph = (await validate(GRAPH_SCHEMA, result.stdout)) as GraphReport;
    expect(graph.schemaVersion).toBe(1);
    expect(result.stdout).toMatchSnapshot();
    expect(result.stdout).not.toContain('\\');
    const toShared = graph.edges.filter((edge) => edge.to === 'shared:backend/shared');
    expect(toShared.map((edge) => edge.from)).toEqual(
      COURSE_SHARED_USERS.map((service) => `python:backend/${service}`),
    );
    expect(toShared.every((edge) => edge.sources[0]?.source === 'python-import')).toBe(true);
  });

  it('shows the same edges in fscale facts', async () => {
    const repo = await fixtures.create({ files: courseProjectFiles(), track: 'no-git' });
    const graph = JSON.parse((await runCli(['graph', '--json'], repo.root)).stdout) as GraphReport;
    const facts = JSON.parse((await runCli(['facts', '--json'], repo.root)).stdout) as {
      graph: Pick<GraphReport, 'nodes' | 'edges'>;
    };
    expect(facts.graph.edges).toEqual(graph.edges);
    expect(facts.graph.nodes).toEqual(graph.nodes);
    const human = await runCli(['facts'], repo.root);
    expect(human.stdout).toContain('  backend/auth_service -> backend/shared (Python import)\n');
  });

  it('warns about a dynamic import and widens its edges', async () => {
    const files = courseProjectFiles();
    files['backend/audit_service/main.py'] =
      'import importlib\n\nplugin = importlib.import_module(NAME)\n';
    const repo = await fixtures.create({ files, track: 'no-git' });
    const graph = (await validate(
      GRAPH_SCHEMA,
      (await runCli(['graph', '--json'], repo.root)).stdout,
    )) as GraphReport;
    expect(graph.warnings).toHaveLength(1);
    expect(graph.warnings[0]).toContain('backend/audit_service/main.py:3: importlib.import_module');
    const wide = graph.edges.filter((edge) =>
      edge.sources.some((source) => source.source === 'dynamic-import'),
    );
    expect(wide.map((edge) => edge.to).sort()).toEqual(
      [
        ...COURSE_SERVICES.filter((s) => s !== 'audit_service').map((s) => `python:backend/${s}`),
        'shared:backend/shared',
      ].sort(),
    );
    const human = await runCli(['graph'], repo.root);
    expect(human.stdout).toContain('Warnings:\n  backend/audit_service/main.py:3:');
  });

  it('rejects --explain with --json', async () => {
    const repo = await fixtures.create({ files: courseProjectFiles(), track: 'no-git' });
    const result = await runCli(['graph', '--json', '--explain'], repo.root);
    expect(result.code).toBe(EXIT_USAGE);
  });
});

describe('fscale verify --affected', () => {
  it('runs only the service that changed, and says how many projects ran', async () => {
    const { repo, calls, env } = await courseProject();
    await repo.write('backend/audit_service/main.py', 'app = 1\n');
    const result = await runCli(['verify', '--affected'], repo.root, env);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toMatchSnapshot();
    expect(result.stdout.split('\n')[0]).toBe(
      'verify: PARTIAL (affected only) - 1 of 6 projects ran, 5 not affected; 2 of 4 steps ran, 2 skipped',
    );
    expect(await calls()).toEqual(['python -m pytest in audit_service']);
  });

  it('runs every service that imports a changed shared folder, and nothing else', async () => {
    const { repo, calls, env } = await courseProject();
    await repo.write('backend/shared/db.py', 'URL = "x"\n');
    const json = await runCli(['verify', '--affected', '--json'], repo.root, env);
    const report = (await validate(VERIFY_SCHEMA, json.stdout)) as AffectedReport;
    expect(report.scope).toBe('affected');
    expect(report.affected?.projectsRun).toBe(3);
    expect(report.affected?.projectsTotal).toBe(6);
    const ran = report.affected?.projects.filter((p) => p.affected).map((p) => p.project);
    expect(ran).toEqual(COURSE_SHARED_USERS.map((service) => `backend/${service}`));
    for (const step of report.steps.filter((s) => s.project === 'frontend')) {
      expect(step.status).toBe('not-affected');
    }
    const reasons = report.affected?.projects.filter((p) => p.affected).map((p) => p.reason);
    expect(reasons?.every((reason) => reason.startsWith('depends on backend/shared'))).toBe(true);
    const ranCommands = await calls();
    expect(ranCommands.some((call) => call.includes('frontend'))).toBe(false);
    expect(ranCommands.some((call) => call.includes('audit_service'))).toBe(false);
  });

  it('runs only the frontend for a frontend change, and everything for the root Compose file', async () => {
    const { repo, calls, env } = await courseProject();
    await repo.write('frontend/src/main.ts', 'export const app = 2;\n');
    const frontend = await runCli(['verify', '--affected'], repo.root, env);
    expect(frontend.stdout.split('\n')[0]).toBe(
      'verify: OK (affected only) - 1 of 6 projects ran, 5 not affected',
    );
    expect((await calls()).every((call) => call.endsWith('in frontend'))).toBe(true);

    await repo.write(
      'docker-compose.yml',
      'services:\n  auth:\n    build: ./backend/auth_service\n  db:\n    image: postgres:16\n',
    );
    const all = await runCli(['verify', '--affected', '--json'], repo.root, env);
    const report = (await validate(VERIFY_SCHEMA, all.stdout)) as AffectedReport;
    expect(report.affected?.everything).toBe(true);
    expect(report.affected?.reason).toBe('the Compose file docker-compose.yml changed');
    expect(report.affected?.projectsRun).toBe(6);
  });

  it('runs everything, with a note, when there is no base and the tree is clean', async () => {
    const { repo, env } = await courseProject();
    const result = await runCli(['verify', '--affected'], repo.root, env);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout.split('\n')[0]).toBe(
      'verify: PARTIAL (affected only) - 6 of 6 projects ran; 10 of 19 steps ran, 9 skipped',
    );
    expect(result.stdout).toContain(
      'note: --affected found no uncommitted change and no --base was given',
    );
  });

  it('runs nothing that changed nothing when a base is given', async () => {
    const { repo, calls, env } = await courseProject();
    const result = await runCli(['verify', '--affected', '--base', 'HEAD'], repo.root, env);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout.split('\n')[0]).toBe(
      'verify: OK (affected only) - 0 of 6 projects ran, 6 not affected',
    );
    expect(await calls()).toEqual([]);
  });

  it('plans with --dry-run and runs nothing, not even a script', async () => {
    const { repo, calls, env } = await courseProject();
    await repo.write('backend/shared/db.py', 'URL = "y"\n');
    const result = await runCli(['verify', '--affected', '--dry-run'], repo.root, env);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toMatchSnapshot();
    expect(result.stdout.split('\n')[0]).toBe(
      'verify --affected (dry run): 3 of 6 projects would run, 3 not affected. Nothing was run.',
    );
    expect(await calls()).toEqual([]);
    const json = await runCli(['verify', '--dry-run', '--json'], repo.root, env);
    expect(json.code).toBe(EXIT_USAGE);
  });
});
