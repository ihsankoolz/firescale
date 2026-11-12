// The python stack pack (M16, ADR 0030): init builds a starter, and a repo
// of several services, each with its own requirements files, pytest.ini and
// virtual environment, is checked service by service and never pushed
// towards a root pyproject.toml.
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

const STARTER = [
  'requirements.txt',
  'requirements-dev.txt',
  'pytest.ini',
  'ruff.toml',
  'app/__init__.py',
  'app/main.py',
  'tests/test_main.py',
  '.gitignore',
];

/** One service of the course project's shape. */
function service(folder: string): Record<string, string> {
  return {
    [`${folder}/requirements.txt`]: 'fastapi==0.120.0\n',
    [`${folder}/requirements-dev.txt`]: '-r requirements.txt\npytest==9.2.0\nruff==0.17.5\n',
    [`${folder}/pytest.ini`]: '[pytest]\ntestpaths = tests\npythonpath = .\naddopts = -ra\n',
    [`${folder}/ruff.toml`]: 'line-length = 100\n',
    [`${folder}/main.py`]: 'def ping() -> str:\n    return "pong"\n',
    [`${folder}/tests/test_main.py`]:
      'from main import ping\n\n\ndef test_ping() -> None:\n    assert ping() == "pong"\n',
  };
}

describe('fscale init --packs core,python', () => {
  it('builds a starter where the python pack reports no violations (snapshots)', async () => {
    const repo = await fixtures.create({ track: 'no-git' });
    const result = await runCli(['init', '--packs', 'core,python'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect((await diagnostics(repo.root)).filter((d) => d.ruleId.startsWith('python/'))).toEqual(
      [],
    );
    for (const file of STARTER) {
      expect(await readFile(path.join(repo.root, file), 'utf8')).toMatchSnapshot(file);
    }
    expect(result.stdout).not.toContain('pyproject.toml');
    const before = await snapshot(repo.root);
    expect((await runCli(['sync'], repo.root)).code).toBe(EXIT_OK);
    expect(await snapshot(repo.root)).toEqual(before);
  });
});

describe('a repo of several services with no pyproject.toml', () => {
  const services = {
    ...service('backend/orders_service'),
    ...service('backend/payments_service'),
    '.gitignore': '__pycache__/\n.venv/\nvenv/\n.pytest_cache/\n.ruff_cache/\n',
    'firescale.config.json': JSON.stringify({ packs: ['python'] }),
  };

  it('adopt --dry-run proposes no change when every service is set up', async () => {
    const repo = await fixtures.create({ files: services });
    const result = await runCli(['adopt', '--dry-run'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toContain('Dry run: `fscale adopt` would change nothing.');
  });

  it('adopt --dry-run proposes exactly the missing pytest.ini, in that service, and nothing at the root', async () => {
    const files: Record<string, string> = { ...services };
    delete files['backend/payments_service/pytest.ini'];
    const repo = await fixtures.create({ files });
    const before = await snapshot(repo.root);
    const result = await runCli(['adopt', '--dry-run'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toContain(
      'python/pytest-config\n  would create backend/payments_service/pytest.ini',
    );
    expect(result.stdout).toContain('Dry run: 1 change.');
    expect(result.stdout).not.toContain('pyproject.toml');
    expect(await snapshot(repo.root)).toEqual(before);
  });
});
