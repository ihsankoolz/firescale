// The rules that write the repo's commands into agent files
// (agent-ready/agents-md, agent-ready/agent-permissions) take the commands
// from `fscale facts`. Every file facts reads to decide a command must be
// among their cache inputs, or a cached pass hides a stale AGENTS.md.
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { EXIT_OK, EXIT_VIOLATIONS, runCli } from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

const AGENTS_MD = 'agent-ready/agents-md';
const PERMISSIONS = 'agent-ready/agent-permissions';

const CONFIG = `${JSON.stringify(
  {
    packs: ['agent-ready'],
    rules: {
      'agent-ready/agent-skills': 'off',
      'agent-ready/commands-documented': 'off',
      'agent-ready/single-test-command': 'off',
      'agent-ready/verify-hook': 'off',
    },
  },
  null,
  2,
)}\n`;

const POM = '<project>\n  <artifactId>demo</artifactId>\n</project>\n';
const GRADLE = "plugins { id 'java' }\n";
/** A Node project in a subfolder, with no scripts: its commands come from tool config files. */
const WEB = `${JSON.stringify({ name: 'web', private: true }, null, 2)}\n`;
const PYPROJECT = '[project]\nname = "demo"\nversion = "0.1.0"\n';

interface Case {
  /** The file that appears, and what it is. */
  adds: string;
  content?: string;
  /** The repo before the file appears. */
  files: Record<string, string>;
  /** The command that changes, as AGENTS.md must then list it. */
  command: string;
  /** The rules whose cached pass is then out of date. */
  stale: string[];
}

const CASES: Case[] = [
  {
    adds: 'mvnw',
    files: { 'pom.xml': POM },
    command: './mvnw -B test',
    stale: [PERMISSIONS, AGENTS_MD],
  },
  {
    adds: 'mvnw.cmd',
    files: { 'pom.xml': POM },
    command: './mvnw -B test',
    stale: [PERMISSIONS, AGENTS_MD],
  },
  {
    adds: 'gradlew',
    files: { 'build.gradle': GRADLE },
    command: './gradlew test',
    stale: [PERMISSIONS, AGENTS_MD],
  },
  {
    adds: 'gradlew.bat',
    files: { 'build.gradle': GRADLE },
    command: './gradlew test',
    stale: [PERMISSIONS, AGENTS_MD],
  },
  // A wrapper in a project below the root.
  {
    adds: 'service/mvnw',
    files: { 'service/pom.xml': POM },
    command: './mvnw -B test',
    stale: [AGENTS_MD],
  },
  {
    adds: 'web/.oxlintrc.json',
    content: '{}\n',
    files: { 'web/package.json': WEB },
    command: 'npx --no oxlint',
    stale: [AGENTS_MD],
  },
  {
    adds: 'web/oxlint.config.ts',
    content: 'export default {};\n',
    files: { 'web/package.json': WEB },
    command: 'npx --no oxlint',
    stale: [AGENTS_MD],
  },
  {
    adds: 'web/.eslintrc.json',
    content: '{}\n',
    files: { 'web/package.json': WEB },
    command: 'npx --no eslint .',
    stale: [AGENTS_MD],
  },
  {
    adds: 'web/.eslintrc',
    content: '{}\n',
    files: { 'web/package.json': WEB },
    command: 'npx --no eslint .',
    stale: [AGENTS_MD],
  },
  {
    adds: 'web/biome.json',
    content: '{}\n',
    files: { 'web/package.json': WEB },
    command: 'npx --no biome check .',
    stale: [AGENTS_MD],
  },
  {
    adds: 'web/biome.jsonc',
    content: '{}\n',
    files: { 'web/package.json': WEB },
    command: 'npx --no biome check .',
    stale: [AGENTS_MD],
  },
  {
    adds: 'web/vitest.workspace.ts',
    content: 'export default [];\n',
    files: { 'web/package.json': WEB },
    command: 'npx --no vitest run',
    stale: [AGENTS_MD],
  },
  // A lockfile below the root names that project's package manager.
  {
    adds: 'web/yarn.lock',
    files: { 'web/package.json': WEB },
    command: 'yarn install',
    stale: [AGENTS_MD],
  },
  {
    adds: 'web/pnpm-lock.yaml',
    files: { 'web/package.json': WEB },
    command: 'pnpm install',
    stale: [AGENTS_MD],
  },
  {
    adds: 'web/bun.lock',
    files: { 'web/package.json': WEB },
    command: 'bun install',
    stale: [AGENTS_MD],
  },
  {
    adds: 'conftest.py',
    files: { 'pyproject.toml': PYPROJECT },
    command: 'python -m pytest',
    stale: [PERMISSIONS, AGENTS_MD],
  },
  // Beside a requirements.txt, these make the folder a Python project.
  {
    adds: 'manage.py',
    files: { 'requirements.txt': 'pytest\n' },
    command: 'python -m pytest',
    stale: [PERMISSIONS, AGENTS_MD],
  },
  {
    adds: 'api/alembic.ini',
    files: { 'api/requirements.txt': 'pytest\n' },
    command: 'python -m pytest',
    stale: [AGENTS_MD],
  },
  {
    adds: '.ruff.toml',
    files: { 'pyproject.toml': PYPROJECT },
    command: 'python -m ruff check .',
    stale: [PERMISSIONS, AGENTS_MD],
  },
  {
    adds: '.mypy.ini',
    files: { 'pyproject.toml': PYPROJECT },
    command: 'python -m mypy .',
    stale: [PERMISSIONS, AGENTS_MD],
  },
  // A development list whose name does not start with "requirements".
  {
    adds: 'dev-requirements.txt',
    content: 'pytest\n',
    files: { 'pyproject.toml': PYPROJECT },
    command: 'python -m pip install -r dev-requirements.txt',
    stale: [PERMISSIONS, AGENTS_MD],
  },
];

interface Report {
  diagnostics: { ruleId: string; file: string }[];
}

function rules(stdout: string): string[] {
  const ids = (JSON.parse(stdout) as Report).diagnostics.map((d) => d.ruleId);
  return [...new Set(ids)].sort();
}

/** Writes the agent files with `fscale fix`, then stores the passes in the cache. */
async function agentReady(files: Record<string, string>, track?: readonly string[]) {
  const repo = await fixtures.create({
    files: { 'firescale.config.json': CONFIG, ...files },
    ...(track ? { track } : {}),
  });
  expect((await runCli(['fix'], repo.root)).code).toBe(EXIT_OK);
  const passed = await runCli(['check'], repo.root, { FSCALE_LOG: 'cache' });
  expect(passed.stdout).toBe('OK: 2 rules checked, no violations.\n');
  expect(passed.stderr).toBe(`cache: miss ${PERMISSIONS}\ncache: miss ${AGENTS_MD}\n`);
  return repo;
}

describe('the cache of the rules that list the commands of fscale facts', () => {
  it.each(CASES)(
    'runs them again when $adds appears',
    async ({ adds, content, files, command, stale }) => {
      const repo = await agentReady(files);
      expect(await repo.read('AGENTS.md')).not.toContain(command);

      await repo.write(adds, content ?? '');
      const fresh = await runCli(['check', '--json', '--no-cache'], repo.root);
      expect(rules(fresh.stdout)).toEqual([...stale].sort());
      const cached = await runCli(['check', '--json'], repo.root);
      expect(rules(cached.stdout)).toEqual([...stale].sort());
      expect(cached.code).toBe(EXIT_VIOLATIONS);

      // The fix writes the new command, which is what made the pass stale.
      await runCli(['fix'], repo.root);
      expect(await repo.read('AGENTS.md')).toContain(command);
    },
  );

  // git still lists a tracked file that .gitignore names, so the files the
  // cache sees stay the same, while facts stops looking in the folder.
  it.each(['.gitignore', 'apps/.gitignore'])(
    'runs agents-md again when %s starts to ignore a tracked project',
    async (ignore) => {
      const web = `${JSON.stringify({ name: 'web', scripts: { test: 'node --test' } }, null, 2)}\n`;
      const repo = await agentReady({ 'apps/web/package.json': web }, [
        'firescale.config.json',
        'apps/web/package.json',
      ]);
      expect(await repo.read('AGENTS.md')).toContain('npm run test');

      await repo.write(ignore, 'web/\n');
      const fresh = await runCli(['check', '--json', '--no-cache'], repo.root);
      expect(rules(fresh.stdout)).toEqual([AGENTS_MD]);
      const cached = await runCli(['check', '--json'], repo.root);
      expect(rules(cached.stdout)).toEqual([AGENTS_MD]);
      expect(cached.code).toBe(EXIT_VIOLATIONS);
    },
  );
});
