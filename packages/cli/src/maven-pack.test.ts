// The maven stack pack (M34, ADR 0054): init builds a starter, adopt
// proposes nothing on a project that is set up and exactly the gap on one
// that is not.
import { readFile, readdir } from 'node:fs/promises';
import path from 'node:path';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { EXIT_OK, runCli } from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

interface Diagnostic {
  ruleId: string;
  file: string;
  fixable: string;
}

async function diagnostics(root: string): Promise<Diagnostic[]> {
  const result = await runCli(['check', '--json', '--no-cache'], root);
  return (JSON.parse(result.stdout) as { diagnostics: Diagnostic[] }).diagnostics;
}

async function snapshot(root: string): Promise<Map<string, string>> {
  const files = new Map<string, string>();
  async function walk(dir: string): Promise<void> {
    for (const entry of await readdir(dir, { withFileTypes: true })) {
      if (entry.name === '.git') continue;
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) await walk(full);
      else
        files.set(
          path.relative(root, full).split(path.sep).join('/'),
          await readFile(full, 'utf8'),
        );
    }
  }
  await walk(root);
  return files;
}

const MAVEN_FILES = [
  'pom.xml',
  'src/main/java/com/example/app/App.java',
  'src/test/java/com/example/app/AppTest.java',
];

describe('fscale init --packs core,maven', () => {
  it('builds a starter where the maven pack reports no violations (snapshots)', async () => {
    const repo = await fixtures.create({ track: 'no-git' });
    const result = await runCli(['init', '--packs', 'core,maven'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    const left = await diagnostics(repo.root);
    expect(left.filter((d) => d.ruleId.startsWith('maven/'))).toEqual([]);
    for (const file of MAVEN_FILES) {
      expect(await readFile(path.join(repo.root, file), 'utf8')).toMatchSnapshot(file);
    }
    const before = await snapshot(repo.root);
    expect((await runCli(['sync'], repo.root)).code).toBe(EXIT_OK);
    expect(await snapshot(repo.root)).toEqual(before);
  });

  it('gives the starter the maven commands in facts, lint included', async () => {
    const repo = await fixtures.create({ track: 'no-git' });
    expect((await runCli(['init', '--packs', 'core,maven'], repo.root)).code).toBe(EXIT_OK);
    const facts = JSON.parse((await runCli(['facts', '--json'], repo.root)).stdout) as {
      projects: { kind: string; name: string; commands: Record<string, string> }[];
    };
    expect(facts.projects).toEqual([
      expect.objectContaining({
        kind: 'maven',
        name: 'app',
        commands: {
          build: 'mvn -B package -DskipTests',
          test: 'mvn -B test',
          lint: 'mvn -B checkstyle:check',
        },
      }),
    ]);
  });
});

describe('fscale adopt --dry-run with the maven pack', () => {
  async function setUpProject() {
    const starter = await fixtures.create({ track: 'no-git' });
    await runCli(['init', '--packs', 'maven'], starter.root);
    const files: Record<string, string> = {};
    for (const file of MAVEN_FILES) {
      files[file] = await readFile(path.join(starter.root, file), 'utf8');
    }
    files['firescale.config.json'] = JSON.stringify({ packs: ['maven'] });
    return files;
  }

  it('proposes no change on a project that is already set up', async () => {
    const repo = await fixtures.create({ files: await setUpProject() });
    const result = await runCli(['adopt', '--dry-run'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toContain('Dry run: `fscale adopt` would change nothing.');
  });

  it('proposes exactly the missing source encoding on a project that lacks it', async () => {
    const files = await setUpProject();
    files['pom.xml'] = (files['pom.xml'] ?? '').replace(
      '    <project.build.sourceEncoding>UTF-8</project.build.sourceEncoding>\n',
      '',
    );
    const repo = await fixtures.create({ files });
    const before = await snapshot(repo.root);
    const result = await runCli(['adopt', '--dry-run'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toContain('maven/properties\n  would edit pom.xml');
    expect(result.stdout).toContain('Dry run: 1 change.');
    expect(await snapshot(repo.root)).toEqual(before);
  });
});

describe('fscale check result cache with the maven pack', () => {
  const POM =
    '<project>\n  <modelVersion>4.0.0</modelVersion>\n  <groupId>com.example</groupId>\n  <artifactId>app</artifactId>\n  <version>1.0.0</version>\n</project>\n';

  async function ids(root: string, rule: string, ...args: string[]): Promise<string[]> {
    const result = await runCli(['check', '--json', '--rules', rule, ...args], root);
    const found = (JSON.parse(result.stdout) as { diagnostics: Diagnostic[] }).diagnostics;
    return found.map((d) => d.ruleId);
  }

  // The rules count Groovy files as code and tests, so their cached
  // results depend on them.
  it('stops reporting "no test" once a Spock test is added', async () => {
    const repo = await fixtures.create({
      files: {
        'firescale.config.json': JSON.stringify({ packs: ['maven'] }),
        'pom.xml': POM,
        'src/main/java/ex/App.java': 'package ex;\n\nclass App {}\n',
      },
    });
    // This run fills the cache.
    expect(await ids(repo.root, 'maven/tests')).toEqual(['maven/tests']);

    await repo.write(
      'src/test/groovy/ex/AppSpec.groovy',
      'package ex\n\nclass AppSpec extends spock.lang.Specification {}\n',
    );
    expect(await ids(repo.root, 'maven/tests', '--no-cache')).toEqual([]);
    expect(await ids(repo.root, 'maven/tests')).toEqual([]);
  });

  it('stops reporting "no code" once a Groovy class is added', async () => {
    const repo = await fixtures.create({
      files: {
        'firescale.config.json': JSON.stringify({ packs: ['maven'] }),
        'pom.xml': POM,
      },
    });
    expect(await ids(repo.root, 'maven/entry')).toEqual(['maven/entry']);

    await repo.write('src/main/groovy/ex/App.groovy', 'package ex\n\nclass App {}\n');
    expect(await ids(repo.root, 'maven/entry', '--no-cache')).toEqual([]);
    expect(await ids(repo.root, 'maven/entry')).toEqual([]);
  });
});
