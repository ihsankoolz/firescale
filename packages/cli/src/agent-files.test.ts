// Born agent-ready (M13, ADR 0027): the agent files that `init`, `adopt`
// and `sync` write, and what they never touch.
import { readFile, readdir, stat, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fixtureSet } from '@firescale/sdk/testing';
import { format } from 'prettier';
import { afterEach, describe, expect, it } from 'vitest';
import { EXIT_OK, EXIT_VIOLATIONS, runCli } from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

interface Diagnostic {
  ruleId: string;
  file: string;
  fixable: string;
}

const AGENT_READY_CONFIG = JSON.stringify({ packs: ['core', 'agent-ready'] });

async function diagnostics(root: string): Promise<Diagnostic[]> {
  const result = await runCli(['check', '--json', '--no-cache'], root);
  return (JSON.parse(result.stdout) as { diagnostics: Diagnostic[] }).diagnostics;
}

/** Every file under `root` but `.git`, by path with forward slashes. */
async function snapshot(root: string): Promise<Map<string, Buffer>> {
  const files = new Map<string, Buffer>();
  async function walk(dir: string): Promise<void> {
    for (const entry of await readdir(dir, { withFileTypes: true })) {
      if (entry.name === '.git') continue;
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) await walk(full);
      else files.set(path.relative(root, full).split(path.sep).join('/'), await readFile(full));
    }
  }
  await walk(root);
  return files;
}

/** True when every line of `before` is in `after`, in the same order. */
function everyLineSurvives(before: string, after: string): boolean {
  const got = after.split(/(?<=\n)/);
  let at = 0;
  for (const line of before.split(/(?<=\n)/)) {
    const accepted = line.endsWith('\n') ? [line] : [line, `${line}\n`, `${line}\r\n`];
    const found = got.findIndex((candidate, index) => index >= at && accepted.includes(candidate));
    if (found === -1) return false;
    at = found + 1;
  }
  return true;
}

/** The text with firescale's managed block (and the blank line before it) taken out. */
function withoutBlock(text: string): string {
  return text.replace(/\r?\n<!-- BEGIN firescale[\s\S]*?<!-- END firescale -->\r?\n/, '');
}

describe('fscale init in an empty folder', () => {
  it('yields a repo where the whole agent-ready pack passes, with snapshot-tested agent files', async () => {
    const repo = await fixtures.create({ track: 'no-git' });
    const result = await runCli(['init'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    const left = await diagnostics(repo.root);
    expect(left.filter((d) => d.ruleId.startsWith('agent-ready/'))).toEqual([]);
    for (const file of [
      'AGENTS.md',
      '.claude/settings.json',
      '.codex/hooks.json',
      '.codex/config.toml',
      '.cursor/hooks.json',
      '.cursor/cli.json',
      '.claude/skills/firescale-core-no-committed-env/SKILL.md',
    ]) {
      expect(await readFile(path.join(repo.root, file), 'utf8')).toMatchSnapshot(file);
    }
    // The same skill for each agent, where each one reads skills.
    const claude = await readFile(
      path.join(repo.root, '.claude/skills/firescale-agent-ready-agents-md/SKILL.md'),
      'utf8',
    );
    for (const dir of ['.agents/skills', '.cursor/skills']) {
      expect(
        await readFile(
          path.join(repo.root, dir, 'firescale-agent-ready-agents-md/SKILL.md'),
          'utf8',
        ),
      ).toBe(claude);
    }
  });
});

describe('generated Markdown', () => {
  it('is already formatted as Prettier would, so a repo that runs Prettier never drifts', async () => {
    const repo = await fixtures.create({ track: 'no-git' });
    await runCli(['init'], repo.root);
    for (const file of [
      'AGENTS.md',
      '.claude/skills/firescale-core-ci-exists/SKILL.md',
      '.agents/skills/firescale-agent-ready-agents-md/SKILL.md',
    ]) {
      const text = await readFile(path.join(repo.root, file), 'utf8');
      expect(await format(text, { parser: 'markdown' }), file).toBe(text);
    }
    const claude = await fixtures.create({ files: { 'CLAUDE.md': '# Notes\n' } });
    await writeFile(path.join(claude.root, 'firescale.config.json'), AGENT_READY_CONFIG);
    await runCli(['sync'], claude.root);
    const text = await readFile(path.join(claude.root, 'CLAUDE.md'), 'utf8');
    expect(await format(text, { parser: 'markdown' })).toBe(text);
  });
});

describe('fscale sync and the AGENTS.md managed block', () => {
  const ours =
    '﻿# Agents\r\n\r\nNever touch `legacy/`.\r\n<!-- firescale:managed -->\r\nlooks managed, is not\r\n<!-- /firescale:managed -->\r\n\tTabs and trailing spaces   \r\nlast line, no newline';

  it('leaves text outside the block unchanged, also after a hand edit inside it', async () => {
    const repo = await fixtures.create({
      files: {
        'firescale.config.json': AGENT_READY_CONFIG,
        'AGENTS.md': ours,
        'package.json': '{ "name": "x", "scripts": { "test": "node --test" } }\n',
      },
    });
    expect((await runCli(['sync'], repo.root)).code).toBe(EXIT_OK);
    const synced = await readFile(path.join(repo.root, 'AGENTS.md'), 'utf8');
    expect(synced.startsWith(ours)).toBe(true);
    expect(withoutBlock(synced.slice(ours.length - 'last line, no newline'.length))).toBe(
      'last line, no newline\r\n',
    );

    // A hand edit inside the block: sync --check fails, sync repairs only the block.
    const edited = synced.replace('`npm run test`', '`npm run test -- --watch`');
    expect(edited).not.toBe(synced);
    await writeFile(path.join(repo.root, 'AGENTS.md'), edited);
    const check = await runCli(['sync', '--check'], repo.root);
    expect(check.code).toBe(EXIT_VIOLATIONS);
    expect(check.stdout).toContain('agent-ready/agents-md');
    expect(await readFile(path.join(repo.root, 'AGENTS.md'), 'utf8')).toBe(edited);
    expect((await runCli(['sync'], repo.root)).code).toBe(EXIT_OK);
    expect(await readFile(path.join(repo.root, 'AGENTS.md'), 'utf8')).toBe(synced);
    expect((await runCli(['sync', '--check'], repo.root)).code).toBe(EXIT_OK);
  });

  it('keeps every other setting of an existing agent settings file', async () => {
    const settings =
      '{\n    "model": "opus",\n    "env": {\n        "TZ": "UTC"\n    },\n    "permissions": {\n        "allow": [\n            "Bash(make test)"\n        ],\n        "defaultMode": "acceptEdits"\n    },\n    "hooks": {\n        "PostToolUse": [\n            { "matcher": "Edit", "hooks": [ { "type": "command", "command": "prettier -w" } ] }\n        ]\n    }\n}\n';
    const repo = await fixtures.create({
      files: { 'firescale.config.json': AGENT_READY_CONFIG, '.claude/settings.json': settings },
    });
    expect((await runCli(['sync'], repo.root)).code).toBe(EXIT_OK);
    const after = await readFile(path.join(repo.root, '.claude/settings.json'), 'utf8');
    expect(everyLineSurvives(settings, after)).toBe(true);
    const json = JSON.parse(after) as {
      model: string;
      env: Record<string, string>;
      permissions: { allow: string[]; deny: string[]; defaultMode: string };
      hooks: Record<string, unknown[]>;
    };
    expect(json.model).toBe('opus');
    expect(json.env).toEqual({ TZ: 'UTC' });
    expect(json.permissions.defaultMode).toBe('acceptEdits');
    expect(json.permissions.allow).toContain('Bash(make test)');
    expect(json.permissions.deny).toContain('Read(**/.env)');
    expect(json.hooks['PostToolUse']).toHaveLength(1);
    expect(JSON.stringify(json.hooks['Stop'])).toContain('hooks run --agent claude');
  });
});

describe('a repo shaped like the course project', () => {
  // A 41 KB CLAUDE.md at the root, a CLAUDE.md in backend/ and frontend/,
  // and a .claude folder with the team's own settings and commands.
  const bigClaude =
    '# Course project\n\n' +
    Array.from(
      { length: 700 },
      (_, n) => `- Note ${String(n)}: keep the services independent; see backend/README.md.`,
    ).join('\n') +
    '\n```\n@AGENTS.md inside a fence does not count\n```\nNo final newline';
  const files: Record<string, string> = {
    'CLAUDE.md': bigClaude,
    'backend/CLAUDE.md': '# Backend\n\nRun pytest in each service.\n',
    'frontend/CLAUDE.md': '# Frontend\r\n\r\nUse npm run test:run.\r\n',
    '.claude/settings.json':
      '{\n  "permissions": {\n    "allow": [\n      "Bash(npm run test:run)"\n    ]\n  }\n}\n',
    '.claude/settings.local.json': '{ "permissions": { "allow": ["Bash(ls)"] } }\n',
    '.claude/commands/review.md': '# Review\n\nReview the diff.\n',
    'backend/audit_service/requirements.txt': 'pytest\n',
    'backend/audit_service/pytest.ini': '[pytest]\n',
    'backend/audit_service/main.py': '',
    'frontend/package.json': '{ "name": "frontend", "scripts": { "test:run": "vitest run" } }\n',
    'firescale.config.json': AGENT_READY_CONFIG,
  };

  it('adopt and sync keep every existing line, and write only inside managed blocks or in files they create', async () => {
    expect(Buffer.byteLength(bigClaude)).toBeGreaterThan(40_000);
    const repo = await fixtures.create({ files });
    const before = await snapshot(repo.root);

    for (const argv of [['adopt'], ['sync'], ['sync']]) {
      const result = await runCli(argv, repo.root);
      expect(result.code, `${argv.join(' ')}: ${result.stderr}`).toBe(EXIT_OK);
    }
    const after = await snapshot(repo.root);
    for (const [file, content] of before) {
      const now = after.get(file);
      expect(now, `${file} was deleted`).toBeDefined();
      const was = content.toString('utf8');
      const is = (now ?? Buffer.alloc(0)).toString('utf8');
      expect(everyLineSurvives(was, is), `${file} lost a line`).toBe(true);
      if (file === 'CLAUDE.md') {
        // Only a managed block, added at the end.
        expect(is.startsWith(was)).toBe(true);
        expect(withoutBlock(is.slice(was.length - 'No final newline'.length))).toBe(
          'No final newline\n',
        );
        expect(is).toContain('\n@AGENTS.md\n\n<!-- END firescale -->\n');
      } else if (file === '.claude/settings.json') {
        // Valid JSON with the team's entry and firescale's added as new lines.
        const json = JSON.parse(is) as { permissions: { allow: string[] } };
        expect(json.permissions.allow).toContain('Bash(npm run test:run)');
      } else {
        expect(is, file).toBe(was);
      }
    }
    // Everything else firescale wrote is a file it created.
    const created = [...after.keys()].filter((file) => !before.has(file));
    for (const file of created) {
      const text = (after.get(file) ?? Buffer.alloc(0)).toString('utf8');
      const ours =
        /^\.(claude|agents|cursor)\/skills\/firescale-[a-z-]+\/SKILL\.md$/.test(file) ||
        [
          'AGENTS.md',
          '.codex/hooks.json',
          '.codex/config.toml',
          '.cursor/hooks.json',
          '.cursor/cli.json',
          '.gitignore',
          '.env.example',
          'README.md',
          '.github/workflows/ci.yml',
        ].includes(file) ||
        file.startsWith('.firescale/');
      expect(ours, `${file} was created`).toBe(true);
      if (file.endsWith('SKILL.md')) expect(text).toContain('<!-- Generated by firescale from ');
    }
    // The nested CLAUDE.md files and the team's own .claude files are untouched.
    for (const file of [
      'backend/CLAUDE.md',
      'frontend/CLAUDE.md',
      '.claude/settings.local.json',
      '.claude/commands/review.md',
    ]) {
      expect(after.get(file), file).toEqual(before.get(file));
    }
    // AGENTS.md lists each project with commands that work on every machine.
    const agents = (after.get('AGENTS.md') ?? Buffer.alloc(0)).toString('utf8');
    expect(agents).toContain('### `backend/audit_service`');
    expect(agents).toContain('- Test: `python -m pytest`');
    expect(agents).toContain('### `frontend`');
    expect(
      (await diagnostics(repo.root)).filter((d) => d.ruleId.startsWith('agent-ready/')),
    ).toEqual([]);
    expect((await stat(path.join(repo.root, 'AGENTS.md'))).isFile()).toBe(true);
  });
});
