import { existsSync } from 'node:fs';
import { chmod, mkdir, readFile, symlink, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { runGit } from '@firescale/sdk';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, beforeAll, describe, expect, it } from 'vitest';
import { courseProjectFiles, shimEnv, writeToolShims } from './course-fixture.js';
import {
  commitAll,
  EXIT_OK,
  EXIT_USAGE,
  EXIT_VIOLATIONS,
  runCli,
  stepCalls,
} from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(fixtures.cleanup);

const WINDOWS = process.platform === 'win32';

/** The version of the fscale under test, as `fscale --version` prints it. */
let VERSION = '';
/**
 * The command a hook starts by default where the repo has no fscale of its
 * own: npx with this version of firescale, pinned (ADR 0064).
 */
let PINNED = '';
/** What `fscale doctor` starts for PINNED: npx without a download. */
let NO_DOWNLOAD = '';
beforeAll(async () => {
  VERSION = (await runCli(['--version'])).stdout.trim();
  PINNED = `npx --yes firescale@${VERSION}`;
  NO_DOWNLOAD = `npx --no -- firescale@${VERSION}`;
});

/**
 * What firescale answers to `--version --json`: a name no other program
 * gives, and its version. The check after installing asks for it (ADR 0064).
 */
const firescaleAnswer = (version: string): string => JSON.stringify({ name: 'firescale', version });

/**
 * The PATH of this machine without the folders that hold one of `programs`
 * (`npx`, `npx.cmd`, `fscale.exe` and the like), so a test can say what a
 * hook command finds. On Unix a folder with a link to `/bin/sh` comes
 * first: fscale starts the command through `sh`, as the agents do.
 */
async function pathWithout(programs: string[]): Promise<string[]> {
  const extensions = WINDOWS ? ['', '.cmd', '.exe', '.bat', '.ps1'] : [''];
  const current = (process.env['PATH'] ?? process.env['Path'] ?? '').split(path.delimiter);
  const kept = current.filter(
    (dir) =>
      dir !== '' &&
      // The folders pnpm put first to run these tests: an agent has none.
      !dir.split(/[\\/]/).includes('node_modules') &&
      !programs.some((program) =>
        extensions.some((extension) => existsSync(path.join(dir, `${program}${extension}`))),
      ),
  );
  if (WINDOWS) return kept;
  const shell = (await fixtures.create({ files: {}, track: 'no-git' })).root;
  await symlink('/bin/sh', path.join(shell, 'sh'));
  return [shell, ...kept];
}

/**
 * Stand-ins in a new folder (or in `inside` below it, such as
 * `node_modules/.bin`): each logs `<name> <arguments>` to `calls.log` in the
 * new folder and prints `answer`, then the lines of `extra` (`cmd` lines on
 * Windows, `sh` lines elsewhere) run.
 */
async function standIns(
  names: string[],
  answer: string,
  { inside = '', extra = [], at }: { inside?: string; extra?: string[]; at?: string } = {},
): Promise<{ dir: string; log: string; top: string }> {
  const top = at ?? (await fixtures.create({ files: {}, track: 'no-git' })).root;
  const dir = path.join(top, ...inside.split('/').filter(Boolean));
  await mkdir(dir, { recursive: true });
  const log = path.join(top, 'calls.log');
  const said = path.join(top, 'answer.txt');
  const lines = extra.map((line) => line.replaceAll('{log}', log));
  await writeFile(said, `${answer}\n`);
  for (const name of names) {
    if (WINDOWS) {
      await writeFile(
        path.join(dir, `${name}.cmd`),
        [
          '@echo off',
          `echo ${name} %*>> "${log}"`,
          `type "${said}"`,
          ...lines,
          'exit /b 0',
          '',
        ].join('\r\n'),
      );
    } else {
      const file = path.join(dir, name);
      await writeFile(
        file,
        [
          '#!/bin/sh',
          `echo "${name} $*" >> "${log}"`,
          `cat "${said}"`,
          ...lines,
          'exit 0',
          '',
        ].join('\n'),
      );
      await chmod(file, 0o755);
    }
  }
  return { dir, log, top };
}

/** A PATH with the stand-ins of `dir` first, and no other `npx` or `fscale`. */
async function onlyStandIns(dir: string): Promise<Record<string, string>> {
  return { PATH: [dir, ...(await pathWithout(['npx', 'fscale']))].join(path.delimiter) };
}

/**
 * The environment of a machine where `fscale` is not on the PATH and `npx`
 * is a stand-in that answers `--version --json` as this fscale does, so no
 * test downloads anything from npm.
 */
async function npxOnly(): Promise<Record<string, string>> {
  return onlyStandIns((await standIns(['npx'], firescaleAnswer(VERSION))).dir);
}

/** `fscale hooks install ...` on a machine with only a stand-in `npx` (see `npxOnly`). */
async function install(argv: string[], root: string) {
  return runCli(['hooks', 'install', ...argv], root, await npxOnly());
}

/** The exit code that tells Claude Code and Codex "do not stop yet". */
const EXIT_BLOCK = 2;

const HOOK_FILES = {
  claude: '.claude/settings.json',
  codex: '.codex/hooks.json',
  cursor: '.cursor/hooks.json',
} as const;

const read = (root: string, file: string): Promise<string> =>
  readFile(path.join(root, ...file.split('/')), 'utf8');

function verifyConfig(test: string): Record<string, string> {
  return {
    'firescale.config.json': JSON.stringify({
      verify: { lint: false, typecheck: false, test: 'node test.js' },
    }),
    'test.js': test,
  };
}

const FAILING = verifyConfig("console.error('1 test failed');\nprocess.exit(1);\n");
const PASSING = verifyConfig("console.log('ok');\n");

const stop = (root: string, agent: string, input: object, env: Record<string, string> = {}) =>
  runCli(['hooks', 'run', '--agent', agent], root, env, { input: JSON.stringify(input) });

describe('fscale hooks install', () => {
  it('writes a hook file for each of the three agents', async () => {
    const repo = await fixtures.create();
    const result = await install(['--all'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toMatchSnapshot('stdout');
    expect(result.stderr).toMatchSnapshot('stderr');
    for (const [agent, file] of Object.entries(HOOK_FILES)) {
      expect(await read(repo.root, file)).toMatchSnapshot(agent);
    }
  });

  it('prints no note that the Cursor format is unchecked: ADR 0027 confirmed it', async () => {
    const repo = await fixtures.create();
    const result = await install(['--agent', 'cursor'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toBe(
      `installed         .cursor/hooks.json\n\nEach agent now runs \`${PINNED} hooks run --agent <name>\` before it stops, and keeps working while \`fscale verify\` fails.\nThe hook starts on this machine: \`${PINNED} --version --json\` answered firescale ${VERSION}.\n`,
    );
    // Only the line that says what the check runs (ADR 0064).
    expect(result.stderr).toBe(
      `fscale: checking that the hook can start: running \`${PINNED} --version --json\` in the repo root, as an agent would. npx may download firescale from the npm registry first.\n`,
    );
  });

  it('changes nothing when run again', async () => {
    const repo = await fixtures.create();
    await install(['--all'], repo.root);
    const before = await Promise.all(Object.values(HOOK_FILES).map((f) => read(repo.root, f)));
    const again = await install(['--all'], repo.root);
    expect(again.code).toBe(EXIT_OK);
    expect(again.stdout).toContain('already installed .claude/settings.json');
    const after = await Promise.all(Object.values(HOOK_FILES).map((f) => read(repo.root, f)));
    expect(after).toEqual(before);
  });

  it('installs for one agent, keeps existing settings, and honours --bin', async () => {
    const repo = await fixtures.create({
      files: { '.claude/settings.json': '{\n"permissions":{"deny":["Read(./.env)"]}}\n' },
    });
    const result = await install(['--agent', 'claude', '--bin', 'npx firescale'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    const settings = JSON.parse(await read(repo.root, HOOK_FILES.claude)) as {
      permissions: unknown;
      hooks: { Stop: { hooks: { command: string }[] }[] };
    };
    expect(settings.permissions).toEqual({ deny: ['Read(./.env)'] });
    expect(settings.hooks.Stop[0]?.hooks[0]?.command).toBe(
      'npx firescale hooks run --agent claude',
    );
    await expect(read(repo.root, HOOK_FILES.codex)).rejects.toThrow();
  });

  it('adds the stop hook when the same command only appears somewhere else', async () => {
    const repo = await fixtures.create({
      files: {
        '.claude/settings.json': JSON.stringify(
          { permissions: { allow: ['Bash(fscale hooks run --agent claude)'] } },
          null,
          2,
        ),
      },
    });
    const result = await install(['--agent', 'claude'], repo.root);
    expect(result.stdout).toContain('installed         .claude/settings.json');
    const settings = JSON.parse(await read(repo.root, HOOK_FILES.claude)) as {
      hooks: { Stop: { hooks: { command: string }[] }[] };
    };
    expect(settings.hooks.Stop[0]?.hooks[0]?.command).toBe(`${PINNED} hooks run --agent claude`);
  });

  it("keeps every line of a person's file and adds only the hook's lines", async () => {
    const files = {
      // Four spaces, a number written 1e3, odd spacing and key order.
      [HOOK_FILES.claude]:
        '{\n    "zeta" :   1e3,\n    "alpha":[ 1,2 ],\n\n    "permissions": {\n        "deny": [\n            "Read(./.env)"\n        ]\n    }\n}\n',
      // Tabs and CRLF, no newline at the end, a stop hook of their own.
      [HOOK_FILES.codex]:
        '{\r\n\t"hooks": {\r\n\t\t"Stop": [\r\n\t\t\t{ "hooks": [ { "type": "command", "command": "echo bye" } ] }\r\n\t\t]\r\n\t}\r\n}',
      // No version, another event's hook.
      [HOOK_FILES.cursor]:
        '{\n   "hooks": {\n      "afterFileEdit": [ { "command": "fmt" } ]\n   }\n}\n',
    };
    const repo = await fixtures.create({ files });
    const result = await install(['--all'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    const written: string[] = [];
    for (const [file, before] of Object.entries(files)) {
      const after = await read(repo.root, file);
      // Each line a person wrote is there, with its own line ending, in order.
      const afterLines = after.split(/(?<=\n)/);
      let at = 0;
      for (const line of before.split(/(?<=\n)/)) {
        at = afterLines.indexOf(line, at) + 1;
        expect(at, `${file}: ${JSON.stringify(line)} in ${JSON.stringify(after)}`).toBeGreaterThan(
          0,
        );
      }
      expect(after).toContain(`${PINNED} hooks run --agent`);
      expect(after.includes('\r\n')).toBe(before.includes('\r\n'));
      expect(after.replaceAll('\r\n', '').includes('\r')).toBe(false);
      written.push(after);
    }
    expect(written[0]).toContain('"zeta" :   1e3,');
    const again = await install(['--all'], repo.root);
    expect(again.code).toBe(EXIT_OK);
    expect(await Promise.all(Object.keys(files).map((f) => read(repo.root, f)))).toEqual(written);
  });

  it('refuses a file written on one line, and says what to add by hand', async () => {
    const minified = '{"permissions":{"deny":["Read(./.env)"]}}\n';
    const repo = await fixtures.create({ files: { '.claude/settings.json': minified } });
    const result = await install(['--agent', 'claude'], repo.root);
    expect(result.code).not.toBe(EXIT_OK);
    expect(result.stdout).toBe('');
    expect(result.stderr).toMatchSnapshot('stderr');
    expect(await read(repo.root, HOOK_FILES.claude)).toBe(minified);
  });

  it('--all tries every file: it writes the ones it can, and says which it refused', async () => {
    const cases = [
      // The first file is refused: the two after it must still be written.
      { refused: ['claude'], content: '{ nope' },
      // The middle file is refused, for the other reason.
      { refused: ['codex'], content: '{"hooks":{}}\n' },
      // Two refusals: each gets its own reason.
      { refused: ['claude', 'cursor'], content: '{"a":1}\n' },
    ] as const;
    for (const { refused, content } of cases) {
      const label = refused.join(' and ');
      const files = Object.fromEntries(refused.map((agent) => [HOOK_FILES[agent], content]));
      const repo = await fixtures.create({ files });
      const result = await install(['--all'], repo.root);
      expect(result.code, label).toBe(EXIT_USAGE);
      expect(result.stdout).toMatchSnapshot(`${label}: stdout`);
      expect(result.stderr).toMatchSnapshot(`${label}: stderr`);
      for (const [agent, file] of Object.entries(HOOK_FILES)) {
        const text = await read(repo.root, file);
        if ((refused as readonly string[]).includes(agent)) {
          expect(text, `${label}: ${file}`).toBe(content);
        } else {
          expect(text, `${label}: ${file}`).toContain(`${PINNED} hooks run --agent ${agent}`);
        }
      }
    }
  });

  it('--all with every file refused says so, and claims no hook', async () => {
    const files = Object.fromEntries(Object.values(HOOK_FILES).map((file) => [file, '{"a":1}\n']));
    const repo = await fixtures.create({ files });
    const result = await install(['--all'], repo.root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stdout).toBe(
      'refused           .claude/settings.json\nrefused           .codex/hooks.json\nrefused           .cursor/hooks.json\n\nNot every hook is installed. Refused and left as it was: .claude/settings.json, .codex/hooks.json, .cursor/hooks.json. The reason, and what to add by hand, is printed above this list (on stderr). Do that, then run this again.\n',
    );
    expect(result.stdout).not.toContain('The hook is in');
    expect(result.stderr.match(/^fscale: /gm)).toHaveLength(3);
    for (const file of Object.values(HOOK_FILES)) {
      expect(await read(repo.root, file)).toBe('{"a":1}\n');
    }
  });

  it('explains bad usage', async () => {
    const repo = await fixtures.create();
    const none = await runCli(['hooks', 'install'], repo.root);
    expect(none.code).toBe(EXIT_USAGE);
    expect(none.stderr).toContain('needs --agent <claude|codex|cursor> or --all');
    const unknown = await install(['--agent', 'vim'], repo.root);
    expect(unknown.code).toBe(EXIT_USAGE);
    expect(unknown.stderr).toContain('Supported agents: claude, codex, cursor.');
    const both = await install(['--agent', 'claude', '--all'], repo.root);
    expect(both.code).toBe(EXIT_USAGE);
    const action = await runCli(['hooks'], repo.root);
    expect(action.code).toBe(EXIT_USAGE);
  });

  it('refuses to overwrite a settings file that is not valid JSON', async () => {
    const repo = await fixtures.create({ files: { '.claude/settings.json': '{ nope' } });
    const result = await install(['--agent', 'claude'], repo.root);
    expect(result.code).toBe(EXIT_USAGE);
    expect(result.stderr).toContain('.claude/settings.json is not valid JSON');
    expect(await read(repo.root, HOOK_FILES.claude)).toBe('{ nope');
  });
});

/** The command of each agent's stop hook in the repo. */
async function hookCommands(root: string): Promise<Record<string, string | undefined>> {
  const claude = JSON.parse(await read(root, HOOK_FILES.claude)) as {
    hooks: { Stop: { hooks: { command: string }[] }[] };
  };
  const codex = JSON.parse(await read(root, HOOK_FILES.codex)) as {
    hooks: { Stop: { hooks: { command: string }[] }[] };
  };
  const cursor = JSON.parse(await read(root, HOOK_FILES.cursor)) as {
    hooks: { stop: { command: string }[] };
  };
  return {
    claude: claude.hooks.Stop[0]?.hooks[0]?.command,
    codex: codex.hooks.Stop[0]?.hooks[0]?.command,
    cursor: cursor.hooks.stop[0]?.command,
  };
}

// Finding 6 of docs/reviews/cs464-0.5.1.md: in a clone of the course project
// (no root package.json, `fscale` not on the PATH), `hooks install --all`
// wrote `fscale hooks run --agent <name>`, which cannot start there, and said
// nothing about it (ADR 0064).
describe('the command the hook starts (ADR 0064)', () => {
  it('writes a command that starts where fscale is not on the PATH: the course project', async () => {
    const repo = await fixtures.create({ files: courseProjectFiles() });
    const { dir, log } = await standIns(['npx'], firescaleAnswer(VERSION));
    const env = await onlyStandIns(dir);
    const result = await runCli(['hooks', 'install', '--all'], repo.root, env);
    expect(result.code).toBe(EXIT_OK);
    // This version of firescale, pinned: a release does not change the
    // hook of every teammate (review of ADR 0064).
    expect(await hookCommands(repo.root)).toEqual({
      claude: `${PINNED} hooks run --agent claude`,
      codex: `${PINNED} hooks run --agent codex`,
      cursor: `${PINNED} hooks run --agent cursor`,
    });
    // It started the command once, the way an agent would, and says so
    // before it runs it: npx may download firescale.
    expect((await readFile(log, 'utf8')).trim().split(/\r?\n/)).toEqual([
      `npx --yes firescale@${VERSION} --version --json`,
    ]);
    expect(result.stderr).toBe(
      `fscale: checking that the hook can start: running \`${PINNED} --version --json\` in the repo root, as an agent would. npx may download firescale from the npm registry first.\n`,
    );
    expect(result.stdout).toContain(
      `The hook starts on this machine: \`${PINNED} --version --json\` answered firescale ${VERSION}.\n`,
    );
  });

  it('says plainly when the command it wrote cannot start, and exits 1', async () => {
    const repo = await fixtures.create({ files: courseProjectFiles() });
    // Neither fscale nor npx on the PATH.
    const env = { PATH: (await pathWithout(['npx', 'fscale'])).join(path.delimiter) };
    const result = await runCli(['hooks', 'install', '--all'], repo.root, env);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    expect(result.stdout).toContain(
      `The hook cannot start on this machine: \`${PINNED} --version --json\` exited with code`,
    );
    expect(result.stdout).toContain(
      'An agent whose stop hook cannot start stops without verify having run',
    );
    expect(result.stdout).not.toContain('The hook starts on this machine');
    // The files are written: the command may start on a teammate's machine.
    expect((await hookCommands(repo.root)).claude).toBe(`${PINNED} hooks run --agent claude`);
  });

  it('keeps a command given with --bin as it is, and checks that one', async () => {
    const repo = await fixtures.create({ files: courseProjectFiles() });
    const { dir, log } = await standIns(['my-fscale'], firescaleAnswer(VERSION));
    const env = await onlyStandIns(dir);
    const result = await runCli(
      ['hooks', 'install', '--agent', 'codex', '--bin', 'my-fscale --cwd .'],
      repo.root,
      env,
    );
    expect(result.code).toBe(EXIT_OK);
    const codex = JSON.parse(await read(repo.root, HOOK_FILES.codex)) as {
      hooks: { Stop: { hooks: { command: string }[] }[] };
    };
    expect(codex.hooks.Stop[0]?.hooks[0]?.command).toBe(
      'my-fscale --cwd . hooks run --agent codex',
    );
    expect((await readFile(log, 'utf8')).trim()).toBe('my-fscale --cwd . --version --json');
    expect(result.stdout).toContain(
      `The hook starts on this machine: \`my-fscale --cwd . --version --json\` answered firescale ${VERSION}.\n`,
    );
  });

  it("uses the repo's own fscale script when it has one", async () => {
    const repo = await fixtures.create({
      files: {
        'package.json': JSON.stringify({ name: 'app', scripts: { fscale: 'fscale' } }),
        'pnpm-lock.yaml': "lockfileVersion: '9.0'\n",
      },
    });
    const { dir } = await standIns(['pnpm'], firescaleAnswer(VERSION));
    const env = await onlyStandIns(dir);
    const result = await runCli(['hooks', 'install', '--agent', 'claude'], repo.root, env);
    expect(result.code).toBe(EXIT_OK);
    const settings = JSON.parse(await read(repo.root, HOOK_FILES.claude)) as {
      hooks: { Stop: { hooks: { command: string }[] }[] };
    };
    expect(settings.hooks.Stop[0]?.hooks[0]?.command).toBe(
      'pnpm --silent fscale hooks run --agent claude',
    );
    expect(result.stderr).toContain("It runs this repo's own `fscale` package script.");
  });

  it('uses the firescale the repo installs, when it is one of its dependencies', async () => {
    const repo = await fixtures.create({
      files: {
        'package.json': JSON.stringify({ name: 'app', devDependencies: { firescale: '^0.5.0' } }),
        'pnpm-lock.yaml': "lockfileVersion: '9.0'\n",
      },
    });
    const { dir, log } = await standIns(['pnpm'], firescaleAnswer(VERSION));
    const env = await onlyStandIns(dir);
    const result = await runCli(['hooks', 'install', '--agent', 'claude'], repo.root, env);
    expect(result.code).toBe(EXIT_OK);
    expect(
      (
        JSON.parse(await read(repo.root, HOOK_FILES.claude)) as {
          hooks: { Stop: { hooks: { command: string }[] }[] };
        }
      ).hooks.Stop[0]?.hooks[0]?.command,
    ).toBe('pnpm exec fscale hooks run --agent claude');
    expect((await readFile(log, 'utf8')).trim()).toBe('pnpm exec fscale --version --json');
    expect(result.stderr).toContain("It runs the fscale installed in this repo's node_modules.");
  });

  it('says which firescale the hook starts when it is not this one', async () => {
    const repo = await fixtures.create({ files: courseProjectFiles() });
    const { dir } = await standIns(['npx'], firescaleAnswer('0.0.1'));
    const env = await onlyStandIns(dir);
    const result = await runCli(['hooks', 'install', '--agent', 'cursor'], repo.root, env);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toContain(
      `The hook starts on this machine: \`${PINNED} --version --json\` answered firescale 0.0.1. That is not this fscale (${VERSION}): the agent will run firescale 0.0.1.\n`,
    );
  });

  it('checks the command already in a file, and says when it is not firescale', async () => {
    // A hook installed earlier with the command this run would write, where
    // npx answers something that is not firescale's answer.
    const repo = await fixtures.create({
      files: {
        [HOOK_FILES.cursor]: `{\n  "version": 1,\n  "hooks": {\n    "stop": [\n      { "command": "${PINNED} hooks run --agent cursor" }\n    ]\n  }\n}\n`,
      },
    });
    const { dir } = await standIns(['npx'], 'Usage: fscale [options]');
    const env = await onlyStandIns(dir);
    const result = await runCli(['hooks', 'install', '--agent', 'cursor'], repo.root, env);
    expect(result.stdout).toContain('already installed .cursor/hooks.json');
    expect(result.code).toBe(EXIT_VIOLATIONS);
    expect(result.stdout).toContain(
      `The hook cannot start on this machine: \`${PINNED} --version --json\` answered "Usage: fscale [options]", which is not firescale's answer`,
    );
  });

  // Review of ADR 0064, finding 5: `--bin npx` passed the check with npm's
  // version, since any one word that starts with a digit was taken.
  it("takes only firescale's answer: a program that prints a version is not firescale", async () => {
    const repo = await fixtures.create({ files: courseProjectFiles() });
    const { dir } = await standIns(['npx'], '10.9.2');
    const env = await onlyStandIns(dir);
    const result = await runCli(
      ['hooks', 'install', '--agent', 'claude', '--bin', 'npx'],
      repo.root,
      env,
    );
    expect(result.code).toBe(EXIT_VIOLATIONS);
    expect(result.stdout).toContain(
      'The hook cannot start on this machine: `npx --version --json` answered "10.9.2", which is not firescale\'s answer',
    );
    expect(result.stdout).not.toContain('The hook starts on this machine');
  });

  it('says which commands may download firescale, and calls only a package script one', async () => {
    const cases = [
      ['pnpm dlx firescale', 'pnpm'],
      ['yarn dlx firescale', 'yarn'],
      ['bunx firescale', 'bunx'],
      ['bun x firescale', 'bun'],
      ['npm exec firescale', 'npm'],
    ] as const;
    for (const [bin, program] of cases) {
      const repo = await fixtures.create({ files: courseProjectFiles() });
      const { dir } = await standIns([program], firescaleAnswer(VERSION));
      const env = await onlyStandIns(dir);
      const result = await runCli(
        ['hooks', 'install', '--agent', 'claude', '--bin', bin],
        repo.root,
        env,
      );
      expect(result.code, bin).toBe(EXIT_OK);
      expect(result.stderr, bin).toContain('may download firescale from the npm registry first.');
      expect(result.stderr, bin).not.toContain('package script');
    }
  });

  it("sets CLAUDE_PROJECT_DIR to the repo root for the Claude file's command", async () => {
    const repo = await fixtures.create({ files: courseProjectFiles() });
    const { dir, log } = await standIns(['my-fscale'], firescaleAnswer(VERSION), {
      extra: [
        WINDOWS
          ? 'echo dir=%CLAUDE_PROJECT_DIR%>> "{log}"'
          : 'echo "dir=$CLAUDE_PROJECT_DIR" >> "{log}"',
      ],
    });
    const env = { ...(await onlyStandIns(dir)), CLAUDE_PROJECT_DIR: 'somewhere-else' };
    const result = await runCli(
      ['hooks', 'install', '--agent', 'claude', '--bin', 'my-fscale'],
      repo.root,
      env,
    );
    expect(result.code).toBe(EXIT_OK);
    const lines = (await readFile(log, 'utf8')).trim().split(/\r?\n/);
    expect(lines[0]).toBe('my-fscale --version --json');
    expect(path.resolve((lines[1] ?? '').replace(/^dir=/, '').trim())).toBe(
      path.resolve(repo.root),
    );
  });

  // Review of ADR 0064, finding 3 (fixture fixD): run through npx or a
  // package script, fscale had `node_modules/.bin` first on its PATH, so
  // `--bin fscale` "started" there and nowhere else.
  it('leaves out of the PATH the folders a package manager put first for its own run', async () => {
    const repo = await fixtures.create({ files: courseProjectFiles() });
    // The folder `pnpm fscale` or `npm run` puts first: the repo's own.
    const { dir, log } = await standIns(['fscale'], firescaleAnswer(VERSION), {
      inside: 'node_modules/.bin',
      at: repo.root,
    });
    const env = await onlyStandIns(dir);
    const result = await runCli(
      ['hooks', 'install', '--agent', 'claude', '--bin', 'fscale'],
      repo.root,
      env,
    );
    expect(result.code).toBe(EXIT_VIOLATIONS);
    expect(existsSync(log)).toBe(false);
    expect(result.stdout).toContain(
      'The hook cannot start on this machine: `fscale --version --json` exited with code',
    );
    expect(result.stderr).toContain(
      `It leaves out of the PATH the folder a package manager put there for its own run (${dir}), which an agent does not have.`,
    );
  });

  // CI of b071811: pnpm/action-setup puts pnpm in
  // `~/setup-pnpm/node_modules/.bin`, a folder of the PATH an agent has
  // too. Leaving out every folder inside a `node_modules` left out pnpm.
  it('keeps a node_modules folder of the PATH that no package manager put there for this run', async () => {
    const repo = await fixtures.create({ files: courseProjectFiles() });
    const { dir, log } = await standIns(['fscale'], firescaleAnswer(VERSION), {
      inside: 'setup-pnpm/node_modules/.bin',
    });
    const env = await onlyStandIns(dir);
    const result = await runCli(
      ['hooks', 'install', '--agent', 'claude', '--bin', 'fscale'],
      repo.root,
      env,
    );
    expect(result.code).toBe(EXIT_OK);
    expect((await readFile(log, 'utf8')).trim()).toBe('fscale --version --json');
    expect(result.stderr).not.toContain('It leaves out of the PATH');
  });

  // Second review of ADR 0064, finding A: npm 10 takes the option after
  // `--no` for its own, so `npx --no fscale --version` printed npm's version.
  it('ends npx options with -- in the command of a repo that installs firescale with npm', async () => {
    const repo = await fixtures.create({
      files: {
        'package.json': JSON.stringify({ name: 'app', devDependencies: { firescale: '^0.5.0' } }),
      },
    });
    const { dir, log } = await standIns(['npx'], firescaleAnswer(VERSION));
    const result = await runCli(
      ['hooks', 'install', '--agent', 'claude'],
      repo.root,
      await onlyStandIns(dir),
    );
    expect(result.code).toBe(EXIT_OK);
    expect(
      (
        JSON.parse(await read(repo.root, HOOK_FILES.claude)) as {
          hooks: { Stop: { hooks: { command: string }[] }[] };
        }
      ).hooks.Stop[0]?.hooks[0]?.command,
    ).toBe('npx --no -- fscale hooks run --agent claude');
    expect((await readFile(log, 'utf8')).trim()).toBe('npx --no -- fscale --version --json');
  });

  // Second review of ADR 0064, finding D (fixture fixInit): a hook that an
  // older firescale wrote, pinned to its version, was "not started" as a
  // command fscale does not write, and the run claimed that the agents now
  // run this version.
  it('starts an older pin firescale wrote, takes its plain version, and says which version the agents run', async () => {
    const old = 'npx --yes firescale@0.5.0';
    const repo = await fixtures.create({
      files: {
        [HOOK_FILES.claude]: `${JSON.stringify({ hooks: { Stop: [{ hooks: [{ type: 'command', command: `${old} hooks run --agent claude` }] }] } }, null, 2)}\n`,
      },
    });
    // firescale 0.5.1 and older answer `--version --json` with the version alone.
    const { dir, log } = await standIns(['npx'], '0.5.0');
    const result = await runCli(
      ['hooks', 'install', '--agent', 'claude'],
      repo.root,
      await onlyStandIns(dir),
    );
    // The hook was there, and no step has a kept time: nothing is added,
    // and its command is kept (ADR 0083).
    expect(result.stdout).toContain('already installed .claude/settings.json');
    expect(await read(repo.root, HOOK_FILES.claude)).toContain(
      `"command": "${old} hooks run --agent claude"`,
    );
    expect(result.code).toBe(EXIT_OK);
    expect((await readFile(log, 'utf8')).trim()).toBe(`${old} --version --json`);
    expect(result.stdout).toContain(
      `The hook starts on this machine: \`${old} --version --json\` answered firescale 0.5.0. That is not this fscale (${VERSION}): the agent will run firescale 0.5.0.\n`,
    );
    expect(result.stdout).not.toContain(`now runs \`${PINNED} hooks run`);
    expect(result.stdout).toContain(`\`${old} hooks run --agent claude\``);
  });

  // Review of the final diff, finding 3: firescale 0.5.1 wrote
  // `fscale hooks run --agent <name>`, which the course project still has.
  // Nothing rewrites it (sync moves only a pin), so the output must say how
  // to replace it.
  it('says how to replace the command firescale 0.5.1 and older wrote', async () => {
    const repo = await fixtures.create({
      files: {
        [HOOK_FILES.claude]: `${JSON.stringify({ hooks: { Stop: [{ hooks: [{ type: 'command', command: 'fscale hooks run --agent claude' }] }] } }, null, 2)}\n`,
      },
    });
    const why = `it runs \`fscale hooks run --agent claude\`, the command firescale 0.5.1 and older wrote. It starts only where \`fscale\` is on the PATH, and fscale does not start it to check. To get \`${PINNED} hooks run --agent claude\`, remove that stop hook entry from .claude/settings.json and run \`fscale hooks install --agent claude\`.`;
    const result = await install(['--agent', 'claude'], repo.root);
    expect(result.stdout).toContain(`The hook in .claude/settings.json was not started: ${why}`);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    const doctor = await runCli(['doctor'], repo.root, await npxOnly());
    expect(doctor.stdout).toContain(
      `  warning  .claude/settings.json: the stop hook was not started: ${why}`,
    );
  });

  // Review of ADR 0064, finding 4 (fixture fixD): the check ran the text
  // before `hooks run --agent <name>` from a hook file already in the repo.
  it('never starts a command from a hook file that it would not write itself', async () => {
    const command = 'echo ran-from-repo-file > pwned.txt & npx firescale hooks run --agent cursor';
    const repo = await fixtures.create({
      files: {
        [HOOK_FILES.cursor]: `${JSON.stringify({ version: 1, hooks: { stop: [{ command }] } }, null, 2)}\n`,
      },
    });
    const result = await install(['--agent', 'cursor'], repo.root);
    expect(result.stdout).toContain('already installed .cursor/hooks.json');
    expect(existsSync(path.join(repo.root, 'pwned.txt'))).toBe(false);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    expect(result.stdout).toContain(
      `The hook in .cursor/hooks.json was not started: it runs \`${command}\`, which is not the command fscale writes here (\`${PINNED}\`), and fscale starts no other.`,
    );
    expect(result.stdout).not.toContain('The hook starts on this machine');
  });
});

// The owner's decision on ADR 0064: an installed pin is not left behind on
// an upgrade. `sync` moves the version of a stop hook command firescale
// wrote (`npx --yes firescale@<old> hooks run --agent <name>`, exactly),
// says so, and changes no other command.
describe('fscale sync and the pin of a stop hook firescale wrote (ADR 0064)', () => {
  it('moves an older pin firescale wrote to this version, says so, and changes no other command', async () => {
    const old = 'npx --yes firescale@0.5.0';
    const claude = `${JSON.stringify(
      {
        env: { TZ: 'UTC' },
        hooks: {
          Stop: [
            {
              hooks: [
                { type: 'command', command: `${old} hooks run --agent claude` },
                { type: 'command', command: `${old} hooks run --agent claude --budget 60` },
                { type: 'command', command: 'echo stopped' },
              ],
            },
          ],
        },
      },
      null,
      4,
    )}\n`;
    const codex = `${JSON.stringify({ hooks: { Stop: [{ hooks: [{ type: 'command', command: 'npx firescale hooks run --agent codex' }] }] } }, null, 2)}\n`;
    const cursor = `${JSON.stringify({ version: 1, hooks: { stop: [{ command: 'npx --yes firescale@99.0.0 hooks run --agent cursor' }] } }, null, 2)}\n`;
    const repo = await fixtures.create({
      files: {
        'firescale.config.json': JSON.stringify({ packs: [] }),
        [HOOK_FILES.claude]: claude,
        [HOOK_FILES.codex]: codex,
        [HOOK_FILES.cursor]: cursor,
      },
    });
    const note = `.claude/settings.json: the stop hook firescale wrote ran firescale 0.5.0 (\`${old}\`); sync moves its pin to firescale ${VERSION}. sync changes no other command.`;

    const check = await runCli(['sync', '--check'], repo.root);
    expect(check.stdout).toContain(note);
    expect(check.stdout).toContain('would edit .claude/settings.json');
    expect(check.code).toBe(EXIT_VIOLATIONS);
    expect(await read(repo.root, HOOK_FILES.claude)).toBe(claude);

    const sync = await runCli(['sync'], repo.root);
    expect(sync.stdout).toContain(note);
    expect(sync.stdout).toContain('edited .claude/settings.json');
    expect(sync.code).toBe(EXIT_OK);
    expect(await read(repo.root, HOOK_FILES.claude)).toBe(
      claude.replace(
        `"${old} hooks run --agent claude"`,
        `"npx --yes firescale@${VERSION} hooks run --agent claude"`,
      ),
    );
    expect(await read(repo.root, HOOK_FILES.codex)).toBe(codex);
    expect(await read(repo.root, HOOK_FILES.cursor)).toBe(cursor);

    const again = await runCli(['sync', '--check'], repo.root);
    expect(again.stdout).not.toContain('sync moves its pin');
    expect(again.code).toBe(EXIT_OK);
  });
});

// Review of ADR 0064, finding 7: the check after installing runs on the
// installer's machine only. `fscale doctor` starts each hook the same way,
// on whichever machine it runs.
describe('fscale doctor and the agent hooks (ADR 0064)', () => {
  // Second review of ADR 0064, finding C: doctor downloads nothing. It asks
  // npx for the pinned firescale with --no, which runs it only when it is
  // installed or already in npx's cache.
  it('starts the stop hook of each hook file once, without downloading, and says that it starts', async () => {
    const repo = await fixtures.create();
    expect((await install(['--all'], repo.root)).code).toBe(EXIT_OK);
    const { dir, log } = await standIns(['npx'], firescaleAnswer(VERSION));
    const env = await onlyStandIns(dir);
    const result = await runCli(['doctor'], repo.root, env);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toContain('Agent hooks\n');
    for (const file of Object.values(HOOK_FILES)) {
      expect(result.stdout).toContain(
        `  ok       ${file}: the stop hook starts: \`${NO_DOWNLOAD} --version --json\` answered firescale ${VERSION}.\n`,
      );
    }
    // One command in three files: started once.
    expect((await readFile(log, 'utf8')).trim().split(/\r?\n/)).toEqual([
      `${NO_DOWNLOAD} --version --json`,
    ]);
    expect(result.stderr).toContain(
      `fscale: checking that the hook can start: running \`${NO_DOWNLOAD} --version --json\` in the repo root, as an agent would. It downloads nothing: npx runs firescale only when it is installed or already in npx's cache.`,
    );
    const json = JSON.parse((await runCli(['doctor', '--json'], repo.root, env)).stdout) as {
      schemaVersion: number;
      checks: { section: string; subject: string; status: string }[];
    };
    expect(json.schemaVersion).toBe(2);
    expect(json.checks.filter((check) => check.section === 'hooks')).toEqual(
      Object.values(HOOK_FILES).map((subject) =>
        expect.objectContaining({ subject, status: 'ok' }),
      ),
    );
  });

  it('is a problem when the stop hook cannot start on this machine', async () => {
    const repo = await fixtures.create();
    expect((await install(['--agent', 'claude'], repo.root)).code).toBe(EXIT_OK);
    const env = { PATH: (await pathWithout(['npx', 'fscale'])).join(path.delimiter) };
    const result = await runCli(['doctor'], repo.root, env);
    expect(result.code).toBe(EXIT_VIOLATIONS);
    expect(result.stdout).toContain(
      `  PROBLEM  .claude/settings.json: the stop hook cannot start: \`${NO_DOWNLOAD} --version --json\` exited with code`,
    );
  });

  it('says when the pinned firescale is not downloaded yet, as a warning, and downloads nothing', async () => {
    const repo = await fixtures.create();
    expect((await install(['--agent', 'claude'], repo.root)).code).toBe(EXIT_OK);
    const { dir, log } = await standIns(
      ['npx'],
      `npm error npx canceled due to missing packages and no YES option: ["firescale@${VERSION}"]`,
      { extra: [WINDOWS ? 'exit /b 1' : 'exit 1'] },
    );
    const result = await runCli(['doctor'], repo.root, await onlyStandIns(dir));
    expect(result.code).toBe(EXIT_OK);
    expect((await readFile(log, 'utf8')).trim()).toBe(`${NO_DOWNLOAD} --version --json`);
    expect(result.stdout).toContain(
      `  warning  .claude/settings.json: the stop hook was not started: firescale ${VERSION} is not downloaded yet, and doctor downloads nothing. The agent's first stop downloads it through \`${PINNED}\`, which needs the network.`,
    );
  });

  // Second review of ADR 0064, finding C (fixture fixG): doctor started the
  // repo's own `fscale` package script.
  it("never runs the repo's own fscale package script", async () => {
    const repo = await fixtures.create({
      files: {
        'package.json': JSON.stringify({
          name: 'app',
          private: true,
          scripts: { fscale: 'echo ran> pwned-doctor.txt' },
        }),
        [HOOK_FILES.claude]: `${JSON.stringify({ hooks: { Stop: [{ hooks: [{ type: 'command', command: 'npm run --silent fscale -- hooks run --agent claude' }] }] } }, null, 2)}\n`,
      },
    });
    // npm and node stay on the PATH: the script would run if doctor started it.
    const env = { PATH: (await pathWithout(['fscale'])).join(path.delimiter) };
    const result = await runCli(['doctor'], repo.root, env);
    expect(existsSync(path.join(repo.root, 'pwned-doctor.txt'))).toBe(false);
    expect(result.stdout).toContain(
      "  warning  .claude/settings.json: the stop hook was not started: it runs this repo's own `fscale` package script, and doctor runs no script of the repo. Check it yourself: `npm run --silent fscale -- --version --json`.",
    );
  });

  // Review of the final diff, finding 1: on Windows, `cmd` looks for a
  // program in the current folder before the PATH, so a repo that commits
  // `npx.cmd` at its root ran it from doctor and from the install check.
  it.skipIf(!WINDOWS)(
    'never runs a program file the repo holds at its root, from doctor or the install check',
    async () => {
      const repo = await fixtures.create({
        files: {
          'npx.cmd': '@echo ran> pwned-npx.txt\r\n',
          [HOOK_FILES.claude]: `${JSON.stringify({ hooks: { Stop: [{ hooks: [{ type: 'command', command: `${PINNED} hooks run --agent claude` }] }] } }, null, 2)}\n`,
        },
      });
      const { dir, log } = await standIns(['npx'], firescaleAnswer(VERSION));
      // Some shells set this for their own children; a person's usually
      // does not, and then cmd looks in the current folder first.
      const env = { ...(await onlyStandIns(dir)), NoDefaultCurrentDirectoryInExePath: undefined };
      const doctor = await runCli(['doctor'], repo.root, env);
      expect(existsSync(path.join(repo.root, 'pwned-npx.txt'))).toBe(false);
      expect(doctor.stdout).toContain('  ok       .claude/settings.json: the stop hook starts');
      const installed = await runCli(['hooks', 'install', '--agent', 'claude'], repo.root, env);
      expect(existsSync(path.join(repo.root, 'pwned-npx.txt'))).toBe(false);
      expect(installed.code).toBe(EXIT_OK);
      expect((await readFile(log, 'utf8')).trim().split(/\r?\n/)).toEqual([
        `${NO_DOWNLOAD} --version --json`,
        `${PINNED} --version --json`,
      ]);
    },
  );

  it('never starts a command it would not write, and says so', async () => {
    const command = 'echo ran-from-repo-file > pwned.txt & npx firescale hooks run --agent cursor';
    const repo = await fixtures.create({
      files: {
        [HOOK_FILES.cursor]: `${JSON.stringify({ version: 1, hooks: { stop: [{ command }] } }, null, 2)}\n`,
      },
    });
    const result = await runCli(['doctor'], repo.root, await npxOnly());
    expect(existsSync(path.join(repo.root, 'pwned.txt'))).toBe(false);
    expect(result.stdout).toContain(
      `  warning  .cursor/hooks.json: the stop hook was not started: it runs \`${command}\`, which is not the command fscale writes here (\`${PINNED}\`), and fscale starts no other.`,
    );
  });
});

describe('fscale hooks run (the stop hook)', () => {
  it('lets the agent stop, silently, when verify passes', async () => {
    const repo = await fixtures.create({ files: PASSING });
    const result = await stop(repo.root, 'claude', { session_id: 's1', stop_hook_active: false });
    expect(result).toEqual({ code: EXIT_OK, stdout: '', stderr: '' });
  });

  it('blocks Claude Code with exit code 2 and the verify summary on stderr', async () => {
    const repo = await fixtures.create({ files: FAILING });
    const result = await stop(repo.root, 'claude', { session_id: 's1', stop_hook_active: false });
    expect(result.code).toBe(EXIT_BLOCK);
    expect(result.stdout).toBe('');
    expect(result.stderr).toMatchSnapshot();
  });

  it('loop guard: blocks three times in a row, then lets the agent stop', async () => {
    const repo = await fixtures.create({ files: FAILING });
    const first = await stop(repo.root, 'codex', { session_id: 'loop', stop_hook_active: false });
    expect(first.code).toBe(EXIT_BLOCK);
    expect(first.stderr).toContain('attempt 1 of 3');
    for (const attempt of [2, 3]) {
      const next = await stop(repo.root, 'codex', { session_id: 'loop', stop_hook_active: true });
      expect(next.code).toBe(EXIT_BLOCK);
      expect(next.stderr).toContain(`attempt ${String(attempt)} of 3`);
    }
    const released = await stop(repo.root, 'codex', { session_id: 'loop', stop_hook_active: true });
    expect(released.code).toBe(EXIT_OK);
    expect(released.stderr).toContain('verify still fails after 3 attempts');
    expect(released.stderr).toContain('allowed to stop');
    // The counter lives in .firescale/, which ignores itself, so nothing shows up in git.
    expect(await read(repo.root, '.firescale/.gitignore')).toContain('*');
  });

  it('loop guard: when it lets the agent go, the person sees the agent’s last message (ADR 0087)', async () => {
    const repo = await fixtures.create({ files: FAILING });
    const env = { FSCALE_HOOK_MAX_BLOCKS: '1' };
    const explanation =
      'I believe test_total in tests/test_cart.py is wrong: it expects 11 but the spec says 10. Stopping.';
    for (const agent of ['claude', 'codex'] as const) {
      const session = `explain-${agent}`;
      const blocked = await stop(
        repo.root,
        agent,
        { session_id: session, stop_hook_active: false },
        env,
      );
      expect(blocked.code).toBe(EXIT_BLOCK);
      const released = await stop(
        repo.root,
        agent,
        { session_id: session, stop_hook_active: true, last_assistant_message: explanation },
        env,
      );
      expect(released.code).toBe(EXIT_OK);
      const shown = JSON.parse(released.stdout) as { systemMessage: string };
      expect(shown.systemMessage).toContain('verify still fails after 1 attempts');
      expect(shown.systemMessage).toContain(`The agent's last message:\n${explanation}`);
      expect(released.stderr).toContain('allowed to stop');
    }
    // No message from the agent: the person still sees why it stopped.
    await stop(repo.root, 'claude', { session_id: 'quiet', stop_hook_active: false }, env);
    const quiet = await stop(
      repo.root,
      'claude',
      { session_id: 'quiet', stop_hook_active: true },
      env,
    );
    const shown = JSON.parse(quiet.stdout) as { systemMessage: string };
    expect(shown.systemMessage).toContain('allowed to stop');
    expect(shown.systemMessage).not.toContain("The agent's last message");
  });

  it('loop guard: FSCALE_HOOK_MAX_BLOCKS changes the cap, and sessions do not share a count', async () => {
    const repo = await fixtures.create({ files: FAILING });
    const env = { FSCALE_HOOK_MAX_BLOCKS: '1' };
    const a1 = await stop(repo.root, 'claude', { session_id: 'a', stop_hook_active: false }, env);
    expect(a1.code).toBe(EXIT_BLOCK);
    expect(a1.stderr).toContain('attempt 1 of 1');
    const b1 = await stop(repo.root, 'claude', { session_id: 'b', stop_hook_active: false }, env);
    expect(b1.code).toBe(EXIT_BLOCK);
    const a2 = await stop(repo.root, 'claude', { session_id: 'a', stop_hook_active: true }, env);
    expect(a2.code).toBe(EXIT_OK);
    expect(a2.stderr).toContain('allowed to stop');
  });

  it('answers Cursor with a follow-up message on stdout', async () => {
    const repo = await fixtures.create({ files: FAILING });
    const blocked = await stop(repo.root, 'cursor', { conversation_id: 'c1', loop_count: 0 });
    expect(blocked.code).toBe(EXIT_OK);
    const reply = JSON.parse(blocked.stdout) as { followup_message: string };
    expect(reply.followup_message).toContain('verify: FAILED at test');
    const capped = await stop(repo.root, 'cursor', { conversation_id: 'c1', loop_count: 3 });
    expect(capped.code).toBe(EXIT_OK);
    expect(capped.stdout).toBe('{}\n');
  });

  it('does not trap the agent when verify itself cannot run', async () => {
    const repo = await fixtures.create({ files: { 'firescale.config.json': '{ nope' } });
    const result = await stop(repo.root, 'claude', { session_id: 's1' });
    expect(result.code).toBe(1);
    expect(result.stderr).toContain('the stop hook could not run verify');
  });

  it('survives input that is not JSON', async () => {
    const repo = await fixtures.create({ files: PASSING });
    const odd = await runCli(['hooks', 'run', '--agent', 'claude'], repo.root, {}, { input: '??' });
    expect(odd.code).toBe(EXIT_OK);
  });

  it.each([
    ['a payload without stop_hook_active', 'claude', '{"session_id":"quiet"}'],
    ['input that is not JSON', 'codex', '??'],
    ['no input at all', 'claude', ''],
  ])('loop guard: still caps with %s', async (_name, agent, input) => {
    const repo = await fixtures.create({ files: FAILING });
    const codes: number[] = [];
    for (let i = 0; i < 4; i += 1) {
      const result = await runCli(['hooks', 'run', '--agent', agent], repo.root, {}, { input });
      codes.push(result.code);
    }
    expect(codes).toEqual([EXIT_BLOCK, EXIT_BLOCK, EXIT_BLOCK, EXIT_OK]);
  });

  it('loop guard: caps Cursor on its own count when Cursor sends no loop_count', async () => {
    const repo = await fixtures.create({ files: FAILING });
    const replies: string[] = [];
    for (let i = 0; i < 4; i += 1) {
      replies.push((await stop(repo.root, 'cursor', { conversation_id: 'c2' })).stdout);
    }
    expect(replies.slice(0, 3).every((reply) => reply.includes('followup_message'))).toBe(true);
    expect(replies[3]).toBe('{}\n');
  });

  it('never answers its own errors with exit code 2, which would block the agent', async () => {
    const repo = await fixtures.create({ files: FAILING });
    const typo = await runCli(['hooks', 'run', '--agent', 'claud'], repo.root);
    expect(typo.code).toBe(1);
    expect(typo.stderr).toContain('unknown agent "claud"');
    const missing = await runCli(['hooks', 'run'], repo.root);
    expect(missing.code).toBe(1);
    const flag = await runCli(['hooks', 'run', '--agent', 'claude', '--nope'], repo.root);
    expect(flag.code).toBe(1);
  });

  it('is the stop hook wherever its options are written, so its own errors never exit 2', async () => {
    const repo = await fixtures.create({ files: FAILING });
    const orders = [
      ['--agent', 'claud', 'hooks', 'run'],
      ['hooks', '--agent', 'claud', 'run'],
      ['--cwd', repo.root, 'hooks', 'run', '--agent', 'claud'],
      ['--agent=claud', '--cwd', repo.root, 'hooks', '--bin', 'x', 'run'],
    ];
    for (const args of orders) {
      const typo = await runCli(args, repo.root);
      expect(typo.stderr, args.join(' ')).toContain('unknown agent "claud"');
      expect(typo.code, args.join(' ')).toBe(1);
    }
    const flag = await runCli(['--agent', 'claude', 'hooks', 'run', '--nope'], repo.root);
    expect(flag.stderr).toContain('unknown option "--nope"');
    expect(flag.code).toBe(1);
    // --budget is refused in the stop hook with 1, wherever it is written:
    // its value is not taken for one of the command's words.
    for (const args of [
      ['hooks', 'run', '--agent', 'claude', '--budget', '30m'],
      ['hooks', '--budget', '30m', 'run', '--agent', 'claude'],
      ['--budget', '30m', 'hooks', 'run', '--agent', 'claude'],
    ]) {
      const budget = await runCli(args, repo.root, { CI: undefined, GITHUB_ACTIONS: undefined });
      expect(budget.stderr, args.join(' ')).toContain('--budget cannot be used in the stop hook');
      expect(budget.code, args.join(' ')).toBe(1);
    }
    const noValue = await runCli(['--cwd', repo.root, 'hooks', 'run', '--agent'], repo.root);
    expect(noValue.stderr).toContain('option --agent needs a value');
    expect(noValue.code).toBe(1);
  });

  it("does not take an option's value for the command: other commands keep exit code 2", async () => {
    const repo = await fixtures.create({ files: FAILING });
    // "hooks" and "run" here are values of options, not the command.
    const cases: { args: string[]; says: string }[] = [
      { args: ['--rules', 'hooks', 'run'], says: 'unknown command "run"' },
      { args: ['--agent', 'hooks', 'run', '--nope'], says: 'unknown option "--nope"' },
      { args: ['hooks', '--bin', 'run'], says: 'hooks needs an action' },
      { args: ['check', '--rules', 'hooks', 'run'], says: 'check takes no arguments, got "run"' },
      { args: ['--bin', 'hooks', '--agent', 'run', 'verify', 'extra'], says: 'verify takes no' },
    ];
    for (const { args, says } of cases) {
      const result = await runCli(args, repo.root);
      expect(result.stderr, args.join(' ')).toContain(says);
      expect(result.code, args.join(' ')).toBe(EXIT_USAGE);
    }
  });

  it('lets the agent stop on a partial run, and says that it was not a clean pass', async () => {
    // No command for lint or typecheck. The default (ADR 0020): do not block.
    const files = {
      'package.json': JSON.stringify({ name: 'demo', scripts: { test: 'node test.js' } }),
      'test.js': "console.log('ok');\n",
    };
    const repo = await fixtures.create({ files });
    const result = await stop(repo.root, 'claude', { session_id: 'p1', stop_hook_active: false });
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toBe('');
    expect(result.stderr).toBe(
      'fscale: verify was only partial, so this is not a clean pass. 2 of 4 steps ran; skipped: lint, typecheck. Run `fscale verify` for the details, or set verify.failOnSkipped to true to make skipped steps block.\n',
    );
    // Cursor reads JSON from stdout, so the note must stay off it.
    const cursor = await stop(repo.root, 'cursor', { conversation_id: 'p1', loop_count: 0 });
    expect(cursor.code).toBe(EXIT_OK);
    expect(cursor.stdout).toBe('');
    expect(cursor.stderr).toContain('verify was only partial');
  });

  it('blocks on a skipped step when verify.failOnSkipped is on', async () => {
    const repo = await fixtures.create({
      files: {
        'package.json': JSON.stringify({ name: 'demo', scripts: { test: 'node test.js' } }),
        'test.js': "console.log('ok');\n",
        'firescale.config.json': JSON.stringify({ verify: { failOnSkipped: true } }),
      },
    });
    const result = await stop(repo.root, 'claude', { session_id: 'p2', stop_hook_active: false });
    expect(result.code).toBe(EXIT_BLOCK);
    expect(result.stderr).toContain(
      'verify: FAILED - 2 of 4 steps skipped, and verify.failOnSkipped is on',
    );
  });
});

// Finding 5 of docs/reviews/cs464-0.5.1.md: in a copy of the course project,
// after `hooks install --all`, `verify --affected --json` gave
// `"reason": ".claude/settings.json belongs to no project"` for all six
// projects. An agent's hook files are read by the agent and by firescale's
// check, not by a project's build or tests (ADR 0064).
describe('the hook files and verify --affected (ADR 0064)', () => {
  const EVERY_PROJECT = [
    'audit_service',
    'auth_service',
    'frontend',
    'ledger_service',
    'market_service',
    'realtime_service',
  ];

  /** The projects whose steps ran, from the stand-ins' log. */
  const projectsIn = (calls: string[]): string[] =>
    [...new Set(calls.map((call) => call.replace(/^.* in /, '')))].sort();

  /** The course project, committed, with stand-ins for python, npx and npm. */
  async function courseProject(files: Record<string, string> = {}) {
    const repo = await fixtures.create({ files: { ...courseProjectFiles(), ...files } });
    await commitAll(repo.root, 'initial');
    const tools = await fixtures.create({ files: {}, track: 'no-git' });
    await writeToolShims(tools.root, { versions: true });
    const log = path.join(tools.root, 'calls.log');
    const env = shimEnv(tools.root, log);
    const verify = async (...flags: string[]) => {
      await writeFile(log, '');
      const result = await runCli(['verify', ...flags], repo.root, env);
      return { ...result, calls: await stepCalls(log), headline: result.stdout.split('\n')[0] };
    };
    return { repo, verify };
  }

  it('runs no project for the three files hooks install writes: the course project', async () => {
    const { repo, verify } = await courseProject();
    const first = await verify();
    expect(first.code).toBe(EXIT_OK);
    expect(projectsIn(first.calls)).toEqual(EVERY_PROJECT);

    expect((await install(['--all'], repo.root)).code).toBe(EXIT_OK);
    const affected = await verify('--affected');
    expect(affected.code).toBe(EXIT_OK);
    expect(affected.stdout).not.toContain('belongs to no project');
    expect(affected.headline).toBe(
      'verify: OK (affected only) - 0 of 6 projects ran, 6 not affected',
    );
    expect(affected.calls).toEqual([]);
    expect(affected.stdout).toContain(
      'note: 3 changed files (.claude/settings.json and 2 more) are agent hook files, which only a step that can read every file of the repo could read, and no such step runs here, so they run nothing; the check still covers them.',
    );
    const json = JSON.parse((await verify('--affected', '--json')).stdout) as {
      affected?: { everything: boolean; projectsRun: number };
    };
    expect(json.affected).toMatchObject({ everything: false, projectsRun: 0 });

    // Not in a project's cache key either: a full run takes every pass
    // from the cache.
    const full = await verify();
    expect(full.code).toBe(EXIT_OK);
    expect(full.calls).toEqual([]);
  });

  it("runs the repo root's own steps for them: those can read every file", async () => {
    // `prettier --check .` at the root reads .claude/settings.json.
    const { repo, verify } = await courseProject({
      'firescale.config.json': JSON.stringify({ verify: { lint: 'npx prettier --check .' } }),
    });
    await verify();
    expect((await install(['--all'], repo.root)).code).toBe(EXIT_OK);
    const root = path.basename(repo.root);
    const affected = await verify('--affected');
    expect(affected.calls).toEqual([`npx prettier --check . in ${root}`]);
    expect(affected.stdout).toContain(
      "  the repo root's own steps: .claude/settings.json and 2 more changed, and these steps can read every file of the repo, the agent hook files among them",
    );
    expect(affected.stdout).not.toContain('are agent hook files');
    // In the root's key too: after another change to a hook file, a full
    // run lints again, and every project's steps come from the cache.
    await repo.write('.cursor/hooks.json', `${await read(repo.root, '.cursor/hooks.json')}\n`);
    const full = await verify();
    expect(full.calls).toEqual([`npx prettier --check . in ${root}`]);
  });
});

// Review of ADR 0064, findings 1 and 2 (fixtures fixA and fixC). A test can
// read a hook file by its path (`require('../.claude/settings.json')`), and
// Claude Code gives the `env` of `.claude/settings.json` to every command it
// starts, the stop hook's verify among them. Only the hook entries are set
// aside; a project whose files hold the hook file reads all of it.
describe('a hook file a project reads, and what is outside the hook entries (ADR 0064)', () => {
  /**
   * Projects `web` and `api`, no root package.json, `.claude/settings.json`
   * committed as `{}` on `main`. `web`'s test reads the file and fails when
   * it says BROKEN anywhere; with `readsEnv` it fails when `FSCALE_EXAMPLE_API_MODE` is
   * `strict` instead.
   */
  async function twoProjects(readsEnv = false) {
    const test = readsEnv
      ? [
          "if (process.env.FSCALE_EXAMPLE_API_MODE === 'strict') { console.error('strict mode breaks 3 tests'); process.exit(1); }",
          "console.log('1 passed');",
          '',
        ]
      : [
          "const settings = require('../.claude/settings.json');",
          "if (JSON.stringify(settings).includes('BROKEN')) { console.error('settings say BROKEN'); process.exit(1); }",
          "console.log('1 passed');",
          '',
        ];
    const repo = await fixtures.create({
      files: {
        '.claude/settings.json': '{}\n',
        'api/package.json': JSON.stringify({
          name: 'api',
          version: '1.0.0',
          scripts: { test: 'node -e "console.log(1)"' },
        }),
        'web/package.json': JSON.stringify({
          name: 'web',
          version: '1.0.0',
          scripts: { test: 'node test.js' },
        }),
        'web/test.js': test.join('\n'),
      },
      track: 'no-git',
    });
    expect((await runGit(repo.root, ['init', '--quiet', '--initial-branch=main'])).code).toBe(0);
    await commitAll(repo.root, 'initial');
    return repo;
  }

  const headline = (stdout: string): string => stdout.split('\n')[0] ?? '';

  it('runs a project that reads the hook file again when only its hook entries change', async () => {
    const repo = await twoProjects();
    expect((await runCli(['verify'], repo.root)).code).toBe(EXIT_OK);
    await repo.write(
      '.claude/settings.json',
      // firescale's own entry, the only one set aside (second review of
      // ADR 0064, finding B), with a pin that says BROKEN.
      `${JSON.stringify({ hooks: { Stop: [{ hooks: [{ type: 'command', command: 'npx --yes firescale@0.0.0-BROKEN hooks run --agent claude' }] }] } }, null, 2)}\n`,
    );
    const affected = await runCli(['verify', '--affected'], repo.root);
    expect(affected.code).toBe(EXIT_VIOLATIONS);
    expect(headline(affected.stdout)).toContain('FAILED at test in web');
    expect(headline(affected.stdout)).toContain('1 of 2 projects ran');
    // Not served from the cache either.
    const full = await runCli(['verify'], repo.root);
    expect(full.code).toBe(EXIT_VIOLATIONS);
    expect(headline(full.stdout)).toContain('FAILED at test in web');
  }, 300_000);

  it('fixture fixA: a change outside the hook entries is not served from the cache, and affects every project', async () => {
    const repo = await twoProjects();
    expect((await runCli(['verify'], repo.root)).code).toBe(EXIT_OK);
    await repo.write('.claude/settings.json', '{ "env": { "BROKEN": "1" } }\n');
    const full = await runCli(['verify'], repo.root);
    expect(full.code).toBe(EXIT_VIOLATIONS);
    expect(headline(full.stdout)).toContain('FAILED at test in web');
    const affected = await runCli(['verify', '--affected'], repo.root);
    expect(affected.code).toBe(EXIT_VIOLATIONS);
    expect(headline(affected.stdout)).not.toContain('0 of 2 projects ran');
    expect(headline(affected.stdout)).toContain('2 of 2 projects ran');
  }, 300_000);

  it('fixture fixA: never calls a failure pre-existing when a hook file changed', async () => {
    const repo = await twoProjects();
    expect((await runGit(repo.root, ['checkout', '--quiet', '-b', 'feature'])).code).toBe(0);
    await repo.write('.claude/settings.json', '{ "env": { "BROKEN": "1" } }\n');
    await commitAll(repo.root, 'break settings');
    const result = await runCli(['verify', '--base', 'main'], repo.root);
    expect(result.stdout).not.toContain('pre-existing');
    expect(headline(result.stdout)).toContain('FAILED at test in web (new)');
    expect(result.code).toBe(EXIT_VIOLATIONS);
  }, 300_000);

  // Second review of ADR 0064, finding B (fixture fixC2): the whole
  // `hooks` object was set aside, so a SessionStart hook that sets the env
  // of the session's commands changed what verify sees and nothing ran.
  it('fixture fixC2: a hook entry firescale did not write reaches every step, so a change of it runs every project', async () => {
    const repo = await twoProjects(true);
    expect((await runCli(['verify'], repo.root)).code).toBe(EXIT_OK);
    const command = 'echo "export FSCALE_EXAMPLE_API_MODE=strict" >> "$CLAUDE_ENV_FILE"';
    await repo.write(
      '.claude/settings.json',
      `${JSON.stringify({ hooks: { SessionStart: [{ hooks: [{ type: 'command', command }] }] } }, null, 2)}\n`,
    );
    // What the agent's session then gives the stop hook's verify.
    const env = { FSCALE_EXAMPLE_API_MODE: 'strict' };
    const affected = await runCli(['verify', '--affected'], repo.root, env);
    expect(affected.code).toBe(EXIT_VIOLATIONS);
    expect(headline(affected.stdout)).toContain('2 of 2 projects ran');
    const full = await runCli(['verify'], repo.root, env);
    expect(full.code).toBe(EXIT_VIOLATIONS);
    expect(headline(full.stdout)).toContain('FAILED at test in web');
  }, 300_000);

  it('fixture fixC: the env of .claude/settings.json reaches every step, so a change of it runs every project', async () => {
    const repo = await twoProjects(true);
    expect((await runCli(['verify'], repo.root)).code).toBe(EXIT_OK);
    await repo.write(
      '.claude/settings.json',
      '{ "env": { "FSCALE_EXAMPLE_API_MODE": "strict" } }\n',
    );
    // What Claude Code gives the commands it starts, its stop hook among them.
    const env = { FSCALE_EXAMPLE_API_MODE: 'strict' };
    const affected = await runCli(['verify', '--affected'], repo.root, env);
    expect(affected.code).toBe(EXIT_VIOLATIONS);
    expect(headline(affected.stdout)).toContain('2 of 2 projects ran');
    const full = await runCli(['verify'], repo.root, env);
    expect(full.code).toBe(EXIT_VIOLATIONS);
    expect(headline(full.stdout)).toContain('FAILED at test in web');
  }, 300_000);
});

describe("the stop hook's time limit (ADR 0083)", () => {
  /** A repo whose only planned step is `node test.js`, and its kept time. */
  async function timedRepo(minutes: number | null, files: Record<string, string> = {}) {
    const keptTimes =
      minutes === null
        ? {}
        : {
            '.firescale/step-times.json': JSON.stringify({
              format: 1,
              steps: [
                {
                  step: 'test',
                  command: 'node test.js',
                  durationMs: minutes * 60_000,
                  finished: true,
                  ranAt: Math.floor(Date.now() / 1000),
                },
              ],
            }),
          };
    return fixtures.create({ files: { ...PASSING, ...keptTimes, ...files } });
  }

  interface Entry {
    type?: string;
    command: string;
    timeout?: number;
  }
  async function claudeEntry(root: string): Promise<Entry | undefined> {
    const settings = JSON.parse(await read(root, HOOK_FILES.claude)) as {
      hooks: { Stop: { hooks: Entry[] }[] };
    };
    return settings.hooks.Stop[0]?.hooks[0];
  }

  it('writes no limit while no step has a kept time, and one when install runs again after a verify', async () => {
    const repo = await timedRepo(null);
    const result = await install(['--all'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    // Nothing to set it from: Claude Code's default stays, and a later
    // install can still write one (review of ADR 0083, finding 1).
    expect(await claudeEntry(repo.root)).toEqual({
      type: 'command',
      command: `${PINNED} hooks run --agent claude`,
    });
    expect(await read(repo.root, HOOK_FILES.codex)).not.toContain('timeout');
    expect(await read(repo.root, HOOK_FILES.cursor)).not.toContain('timeout');
    expect(result.stdout).toContain(
      "Claude Code's stop hook has no `timeout` in .claude/settings.json, so Claude Code's default of 10m applies: no step has a kept time yet. After a fscale verify, run this again to set it from the steps' times, or pass --timeout <time>.\n",
    );
    const verify = await runCli(['verify'], repo.root);
    expect(verify.code).toBe(EXIT_OK);
    const again = await install(['--agent', 'claude'], repo.root);
    expect(again.stdout).toContain('timeout added     .claude/settings.json');
    expect((await claudeEntry(repo.root))?.timeout).toBe(600);
  });

  it('sets the limit from the kept step times: twice their sum, and the base run, in whole minutes', async () => {
    const repo = await timedRepo(20);
    const result = await install(['--agent', 'claude'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    // 2 x 20m + 5m (verify.baseBudget's default) = 45m.
    expect((await claudeEntry(repo.root))?.timeout).toBe(2700);
    expect(result.stdout).toContain(
      "Claude Code's stop hook may run for 45m (`timeout` in .claude/settings.json): twice the 20m the planned steps took when they last ran, and 5m for the base run.\n",
    );
  });

  it('never writes less than the default 10m', async () => {
    const repo = await timedRepo(1);
    expect((await install(['--agent', 'claude'], repo.root)).code).toBe(EXIT_OK);
    expect((await claudeEntry(repo.root))?.timeout).toBe(600);
  });

  it('takes the limit from --timeout, and refuses it for an agent that has no such field', async () => {
    const repo = await timedRepo(20);
    const result = await install(['--agent', 'claude', '--timeout', '1h 30m'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect((await claudeEntry(repo.root))?.timeout).toBe(5400);
    expect(result.stdout).toContain(
      "Claude Code's stop hook may run for 1h 30m (`timeout` in .claude/settings.json), as --timeout says.\n",
    );
    const other = await fixtures.create({ files: PASSING });
    const codex = await install(['--agent', 'codex', '--timeout', '30m'], other.root);
    expect(codex.code).toBe(EXIT_USAGE);
    expect(codex.stderr).toContain(
      '--timeout is written only on the Claude Code hook: Codex and Cursor have no time limit field firescale writes.',
    );
    const bad = await install(['--agent', 'claude', '--timeout', 'soon'], other.root);
    expect(bad.code).toBe(EXIT_USAGE);
    expect(bad.stderr).toContain('--timeout needs a time longer than zero');
  });

  it('adds the limit as a new line to a stop hook installed before, and keeps every line', async () => {
    const before = `{\n  "permissions": {\n    "allow": ["Bash(ls)"]\n  },\n  "hooks": {\n    "Stop": [\n      {\n        "hooks": [\n          {\n            "type": "command",\n            "command": "${PINNED} hooks run --agent claude"\n          }\n        ]\n      }\n    ]\n  }\n}\n`;
    const repo = await timedRepo(20, { '.claude/settings.json': before });
    const result = await install(['--agent', 'claude'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(result.stdout).toContain('timeout added     .claude/settings.json');
    const after = await read(repo.root, HOOK_FILES.claude);
    const afterLines = after.split('\n');
    for (const line of before.split('\n')) expect(afterLines).toContain(line);
    expect(afterLines.length).toBe(before.split('\n').length + 1);
    expect((await claudeEntry(repo.root))?.timeout).toBe(2700);
    // Run again: nothing changes.
    const again = await install(['--agent', 'claude'], repo.root);
    expect(again.stdout).toContain('already installed .claude/settings.json');
    expect(await read(repo.root, HOOK_FILES.claude)).toBe(after);
  });

  it('keeps a limit that is in the file already, and says what it would be', async () => {
    const before = `{\n  "hooks": {\n    "Stop": [\n      {\n        "hooks": [\n          {\n            "type": "command",\n            "command": "${PINNED} hooks run --agent claude",\n            "timeout": 300\n          }\n        ]\n      }\n    ]\n  }\n}\n`;
    const repo = await timedRepo(20, { '.claude/settings.json': before });
    const result = await install(['--agent', 'claude'], repo.root);
    expect(result.code).toBe(EXIT_OK);
    expect(await read(repo.root, HOOK_FILES.claude)).toBe(before);
    expect(result.stdout).toContain(
      "Claude Code's stop hook keeps the `timeout` of 5m in .claude/settings.json: firescale does not change a value in the file. To give it 45m, edit that value.\n",
    );
  });

  it('doctor warns when the kept times of the steps the hook would run exceed its limit', async () => {
    const repo = await timedRepo(20);
    expect((await install(['--agent', 'claude', '--timeout', '10m'], repo.root)).code).toBe(
      EXIT_OK,
    );
    const result = await runCli(['doctor'], repo.root, await npxOnly());
    expect(result.stdout).toContain(
      '  warning  .claude/settings.json: the steps the stop hook would run took 20m in all when they last ran, more than its limit of 10m (`timeout`). Claude Code ends a stop hook that runs past its limit, and the agent stops without verify having finished.\n',
    );
    expect(result.stdout).toContain(
      'work on a branch: the stop hook then runs only the projects a change reaches, as fscale verify --affected does; or give the hook a smaller scope (a faster command for the slow steps under verify in the config); or raise the limit: edit `timeout` in .claude/settings.json.',
    );
    // A hook with no limit has Claude Code's default, 10m: the same warning.
    const entry = { type: 'command', command: `${PINNED} hooks run --agent claude` };
    const none = await timedRepo(20, {
      '.claude/settings.json': `${JSON.stringify({ hooks: { Stop: [{ hooks: [entry] }] } }, null, 2)}\n`,
    });
    const noLimit = await runCli(['doctor'], none.root, await npxOnly());
    expect(noLimit.stdout).toContain("more than its limit of 10m (Claude Code's default)");
  });

  it('keeps a limit in the file that is not a whole number, and doctor reads a number of seconds with a point', async () => {
    const entry = (timeout: unknown) =>
      `${JSON.stringify({ hooks: { Stop: [{ hooks: [{ type: 'command', command: `${PINNED} hooks run --agent claude`, timeout }] }] } }, null, 2)}
`;
    const point = await timedRepo(20, { '.claude/settings.json': entry(900.5) });
    const before = await read(point.root, HOOK_FILES.claude);
    const result = await install(['--agent', 'claude'], point.root);
    expect(result.code).toBe(EXIT_OK);
    expect(await read(point.root, HOOK_FILES.claude)).toBe(before);
    expect(result.stdout).toContain(
      "Claude Code's stop hook keeps the `timeout` of 900.5 in .claude/settings.json: firescale does not change a value in the file. To give it 45m, edit that value.\n",
    );
    const doctor = await runCli(['doctor'], point.root, await npxOnly());
    expect(doctor.stdout).toContain('more than its limit of 15m (`timeout`)');
    const text = await timedRepo(20, { '.claude/settings.json': entry('900') });
    const odd = await runCli(['doctor'], text.root, await npxOnly());
    expect(odd.stdout).toContain(
      'more than its limit of 10m (Claude Code\'s default: the `timeout` in the file, "900", is not a number of seconds)',
    );
  });

  it('refuses a limit longer than a timer can wait, and --timeout outside hooks install', async () => {
    const repo = await timedRepo(20);
    const long = await install(['--agent', 'claude', '--timeout', '3000000'], repo.root);
    expect(long.code).toBe(EXIT_USAGE);
    expect(long.stderr).toContain(
      '--timeout needs a time longer than zero and at most 2147483 seconds',
    );
    const verify = await runCli(['verify', '--timeout', '5m'], repo.root);
    expect(verify.code).toBe(EXIT_USAGE);
    expect(verify.stderr).toContain('--timeout is for `fscale hooks install`');
  });

  it('doctor says nothing about the limit when the kept times fit in it', async () => {
    const repo = await timedRepo(20);
    expect((await install(['--agent', 'claude'], repo.root)).code).toBe(EXIT_OK);
    const result = await runCli(['doctor'], repo.root, await npxOnly());
    expect(result.stdout).not.toContain('the steps the stop hook would run');
  });
});

describe('a way out for an agent that believes a test is wrong (ADR 0084)', () => {
  const WAY_OUT =
    'If you believe a test or check is wrong, do not edit it: say which and why, and stop.';

  it('is in the block message for each agent', async () => {
    const repo = await fixtures.create({ files: FAILING });
    for (const agent of ['claude', 'codex'] as const) {
      const result = await stop(repo.root, agent, { session_id: `w-${agent}` });
      expect(result.code, agent).toBe(EXIT_BLOCK);
      expect(result.stderr.split('\n').slice(0, 2), agent).toEqual([
        'fscale verify failed, so the work is not done yet (attempt 1 of 3).',
        WAY_OUT,
      ]);
    }
    const cursor = await stop(repo.root, 'cursor', { conversation_id: 'w-cursor', loop_count: 0 });
    const reply = JSON.parse(cursor.stdout) as { followup_message: string };
    expect(reply.followup_message).toContain(`(attempt 1 of 3).\n${WAY_OUT}\n\nverify: FAILED`);
  });
});
