// A lint step never keeps the test step of its project from running, and a
// lint command fscale guessed, which the repo does not declare itself, only
// reports (ADR 0070). The fixtures are cut down from four public repos of
// `docs/reviews/real-repos-0.5.3.md`, with stand-ins that print and exit as
// the real tools did there.
import { chmod, mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fixtureSet } from '@firescale/sdk/testing';
import { afterEach, describe, expect, it } from 'vitest';
import { EXIT_OK, EXIT_VIOLATIONS, runCli } from './test-helpers.js';

const fixtures = fixtureSet();
afterEach(() => fixtures.cleanup());

/** What a stand-in does for a call that starts with these words. */
interface Answer {
  /** The first words of the call, after the tool's name: `format`, `format lint`. */
  args: string;
  code: number;
  out?: string;
}

/**
 * Writes stand-ins for the named tools into `dir`. Each call is appended to
 * `calls.log` there as "<tool> <arguments>"; a call no answer matches exits 0.
 */
async function writeTools(dir: string, tools: Record<string, Answer[]>): Promise<void> {
  await mkdir(dir, { recursive: true });
  const script = [
    "const fs = require('node:fs');",
    "const path = require('node:path');",
    'const [tool, ...args] = process.argv.slice(2);',
    "const call = args.join(' ');",
    "fs.appendFileSync(path.join(__dirname, 'calls.log'), tool + ' ' + call + '\\n');",
    "const answers = JSON.parse(fs.readFileSync(path.join(__dirname, 'answers.json'), 'utf8'))[tool] ?? [];",
    "if (args[0] === '--version' || args[0] === 'version') { console.log(tool + ' 1.0.0'); process.exit(0); }",
    "const answer = answers.find((one) => call === one.args || call.startsWith(one.args + ' '));",
    'if (answer?.out) process.stdout.write(answer.out);',
    'process.exit(answer?.code ?? 0);',
    '',
  ].join('\n');
  await writeFile(path.join(dir, 'tool.cjs'), script);
  await writeFile(path.join(dir, 'answers.json'), JSON.stringify(tools));
  await writeFile(path.join(dir, 'calls.log'), '');
  for (const tool of Object.keys(tools)) {
    if (process.platform === 'win32') {
      await writeFile(
        path.join(dir, `${tool}.cmd`),
        `@"${process.execPath}" "%~dp0tool.cjs" ${tool} %*\r\n`,
      );
    } else {
      const file = path.join(dir, tool);
      await writeFile(
        file,
        `#!/bin/sh\nexec "${process.execPath}" "$(dirname "$0")/tool.cjs" ${tool} "$@"\n`,
      );
      await chmod(file, 0o755);
    }
  }
}

function toolPath(dir: string): Record<string, string> {
  const current = process.env['PATH'] ?? process.env['Path'] ?? '';
  return { PATH: `${dir}${path.delimiter}${current}` };
}

async function calls(dir: string): Promise<string[]> {
  return (await readFile(path.join(dir, 'calls.log'), 'utf8'))
    .split('\n')
    .filter((line) => line !== '' && !line.endsWith(' --version') && !line.endsWith(' version'));
}

interface Step {
  name: string;
  project?: string;
  status: string;
  command?: string;
  reportOnly?: string;
}

interface Report {
  ok: boolean;
  result: string;
  steps: Step[];
}

/** Runs `fscale verify` twice in `root`: for people, and as JSON. */
async function verify(root: string, tools: string) {
  const env = { ...toolPath(tools), CI: undefined, GITHUB_ACTIONS: undefined };
  const human = await runCli(['verify', '--no-cache'], root, env);
  const json = await runCli(['verify', '--no-cache', '--json'], root, env);
  const report = JSON.parse(json.stdout) as Report;
  const step = (name: string, project?: string): Step => {
    const found = report.steps.find((one) => one.name === name && one.project === project);
    if (!found) throw new Error(`no ${name} step in ${json.stdout}`);
    return found;
  };
  return { human, json, report, step };
}

async function toolsFor(root: string, tools: Record<string, Answer[]>): Promise<string> {
  const dir = path.join(root, '..', `${path.basename(root)}-tools`);
  await writeTools(dir, tools);
  return dir;
}

// ardalis/GuardClauses and serilog/serilog: `dotnet format --verify-no-changes`
// exits 2 on whitespace, the repo's CI runs `dotnet test` and no formatter,
// and `dotnet test` passes.
const DOTNET_FILES = {
  'firescale.config.json': JSON.stringify({ packs: ['core'] }),
  'GuardClauses.sln': 'Microsoft Visual Studio Solution File, Format Version 12.00\n',
  '.editorconfig': 'root = true\n\n[*.cs]\nindent_size = 4\n',
  'src/GuardClauses/GuardClauses.csproj': '<Project Sdk="Microsoft.NET.Sdk"></Project>\n',
  'src/GuardClauses/Guard.cs': 'namespace GuardClauses;\n',
  '.github/workflows/ci.yml':
    'on: [push]\njobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      - run: dotnet test\n',
};
const DOTNET_LINT = 'dotnet format "GuardClauses.sln" --verify-no-changes';
const DOTNET_TOOLS = {
  dotnet: [
    {
      args: 'format',
      code: 2,
      out: 'src/GuardClauses/Guard.cs(1,1): error WHITESPACE: Fix whitespace formatting.\n',
    },
    { args: 'test', code: 0, out: 'Passed!  - Failed: 0, Passed: 12, Skipped: 0, Total: 12\n' },
  ],
};

describe('a lint command fscale guessed, in a repo that does not declare it', () => {
  it('reports the failing dotnet format, runs dotnet test, and does not fail the run', async () => {
    const repo = await fixtures.create({ files: DOTNET_FILES });
    const tools = await toolsFor(repo.root, DOTNET_TOOLS);
    const { human, json, report, step } = await verify(repo.root, tools);

    expect(step('lint')).toMatchObject({
      status: 'report-only',
      command: DOTNET_LINT,
      reportOnly:
        'fscale guessed this command, and nothing in this repo asks for it (looked for `dotnet format` in a CI file)',
    });
    expect(step('test')).toMatchObject({
      status: 'passed',
      command: 'dotnet test "GuardClauses.sln"',
    });
    expect(report.ok).toBe(true);
    expect(report.result).toBe('ok');
    expect(json.code).toBe(EXIT_OK);
    expect(human.code).toBe(EXIT_OK);
    // Both commands ran, each time.
    expect(await calls(tools)).toEqual([
      'dotnet format GuardClauses.sln --verify-no-changes',
      'dotnet test GuardClauses.sln',
      'dotnet format GuardClauses.sln --verify-no-changes',
      'dotnet test GuardClauses.sln',
    ]);

    // The summary says it plainly, with the config line that makes it block.
    const lines = human.stdout.split('\n');
    expect(lines[0]).toBe('verify: OK - report-only: lint failed (a lint command fscale guessed)');
    expect(human.stdout).toContain(`  lint       failed   report-only: ${DOTNET_LINT} (exit 2)\n`);
    expect(human.stdout).toContain('  test       passed   dotnet test "GuardClauses.sln"\n');
    expect(human.stdout).toContain(
      '\nReport-only: lint failed, and that does not fail this run. fscale guessed this command, and nothing in this repo asks for it (looked for `dotnet format` in a CI file).\n' +
        'To make it block, add this to your firescale config: "verify": { "lint": "dotnet format \\"GuardClauses.sln\\" --verify-no-changes" }\n',
    );
    expect(human.stdout).toContain('error WHITESPACE: Fix whitespace formatting.');
  });

  it('blocks once the config names the command, and the tests still run', async () => {
    const repo = await fixtures.create({
      files: {
        ...DOTNET_FILES,
        'firescale.config.json': JSON.stringify({ packs: ['core'], verify: { lint: DOTNET_LINT } }),
      },
    });
    const tools = await toolsFor(repo.root, DOTNET_TOOLS);
    const { human, report, step } = await verify(repo.root, tools);
    expect(step('lint').status).toBe('failed');
    expect(step('lint').reportOnly).toBeUndefined();
    expect(step('test').status).toBe('passed');
    expect(report.result).toBe('failed');
    expect(human.code).toBe(EXIT_VIOLATIONS);
    expect(human.stdout.split('\n')[0]).toBe('verify: FAILED at lint');
    expect(human.stdout).not.toContain('Report-only');
  });

  it('blocks when a CI file of the repo runs the command', async () => {
    const repo = await fixtures.create({
      files: {
        ...DOTNET_FILES,
        '.github/workflows/ci.yml':
          'on: [push]\njobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      - run: dotnet format --verify-no-changes\n      - run: dotnet test\n',
      },
    });
    const tools = await toolsFor(repo.root, DOTNET_TOOLS);
    const { human, step } = await verify(repo.root, tools);
    expect(step('lint').status).toBe('failed');
    expect(step('lint').reportOnly).toBeUndefined();
    expect(step('test').status).toBe('passed');
    expect(human.code).toBe(EXIT_VIOLATIONS);
  });

  it('names the project in the config line for a project below the root', async () => {
    // rrousselGit/provider, without its analysis_options.yaml: `flutter
    // analyze` exits 1 on `info` lines, and `flutter test` passes.
    const repo = await fixtures.create({
      files: {
        'firescale.config.json': JSON.stringify({ packs: ['core'] }),
        'packages/provider/pubspec.yaml':
          'name: provider\ndependencies:\n  flutter:\n    sdk: flutter\n',
        'packages/provider/.dart_tool/package_config.json': '{"configVersion":2,"packages":[]}\n',
        'packages/provider/lib/provider.dart': 'library provider;\n',
        'packages/provider/test/provider_test.dart': 'void main() {}\n',
      },
    });
    const tools = await toolsFor(repo.root, {
      flutter: [
        {
          args: 'analyze',
          code: 1,
          out: 'Analyzing provider...\n\n   info • Unnecessary use of multiple underscores • lib/provider.dart:1:1 • unnecessary_underscores\n\n7 issues found. (ran in 1.2s)\n',
        },
        { args: 'test', code: 0, out: 'All tests passed!\n' },
      ],
    });
    const { human, step } = await verify(repo.root, tools);
    expect(step('lint', 'packages/provider').status).toBe('report-only');
    expect(step('test', 'packages/provider').status).toBe('passed');
    expect(human.code).toBe(EXIT_OK);
    expect(human.stdout.split('\n')[0]).toBe(
      'verify: OK - report-only: lint in packages/provider failed (a lint command fscale guessed)',
    );
    expect(human.stdout).toContain(
      '\nReport-only: lint in packages/provider failed, and that does not fail this run. fscale guessed this command, and nothing in this repo asks for it (looked for analysis_options.yaml, and `flutter analyze` or `dart analyze` in a CI file).\n' +
        'To make it block, add this to your firescale config: "verify": { "projects": { "packages/provider": { "lint": "flutter analyze" } } }\n',
    );
  });

  it('says nothing more when the guessed command passes, but marks the step in the JSON', async () => {
    // spf13/cobra: `go vet ./...` and `go test ./...` pass.
    const repo = await fixtures.create({
      files: {
        'firescale.config.json': JSON.stringify({ packs: ['core'] }),
        'go.mod': 'module example.com/cobra\n\ngo 1.22\n',
        'cobra.go': 'package cobra\n',
      },
    });
    const tools = await toolsFor(repo.root, { go: [] });
    const { human, step } = await verify(repo.root, tools);
    expect(step('lint')).toMatchObject({
      status: 'passed',
      command: 'go vet ./...',
      reportOnly:
        'fscale guessed this command, and nothing in this repo asks for it (looked for `go vet` in a CI file)',
    });
    expect(human.stdout.split('\n')[0]).toBe('verify: OK');
    expect(human.stdout).not.toContain('eport-only');
    expect(human.code).toBe(EXIT_OK);
  });
});

describe('a failing lint step and the tests of the same project', () => {
  it('runs swift test after a failing swift format lint, and fails the run for the lint', async () => {
    // apple/swift-argument-parser: the repo has a `.swift-format` file,
    // `swift format lint --strict` exits 1, and `swift test` passes.
    const repo = await fixtures.create({
      files: {
        'firescale.config.json': JSON.stringify({ packs: ['core'] }),
        'Package.swift':
          '// swift-tools-version:5.9\nimport PackageDescription\nlet package = Package(name: "swift-argument-parser")\n',
        '.swift-format': '{ "version": 1 }\n',
        'Sources/ArgumentParser/Parser.swift': 'struct Parser {}\n',
        'Tests/ArgumentParserTests/ParserTests.swift': 'import XCTest\n',
      },
    });
    const tools = await toolsFor(repo.root, {
      swift: [
        {
          args: 'format lint',
          code: 1,
          out: 'Sources/ArgumentParser/Parser.swift:1:1: error: [AllPublicDeclarationsHaveDocumentation] add a documentation comment\n',
        },
        { args: 'test', code: 0, out: "Test Suite 'All tests' passed.\n" },
      ],
    });
    const { human, report, step } = await verify(repo.root, tools);
    // A config file of the linter is in the repo: the lint blocks.
    expect(step('lint')).toMatchObject({
      status: 'failed',
      command: 'swift format lint --recursive --strict .',
    });
    expect(step('lint').reportOnly).toBeUndefined();
    // And the tests ran all the same.
    expect(step('test')).toMatchObject({ status: 'passed', command: 'swift test' });
    expect(await calls(tools)).toEqual([
      'swift format lint --recursive --strict .',
      'swift test',
      'swift format lint --recursive --strict .',
      'swift test',
    ]);
    expect(report.result).toBe('failed');
    expect(human.code).toBe(EXIT_VIOLATIONS);
    expect(human.stdout.split('\n')[0]).toBe('verify: FAILED at lint');
    expect(human.stdout).toContain('  test       passed   swift test\n');
  });

  it('reports a failing lint and a failing test together', async () => {
    // rrousselGit/provider as it is: an analysis_options.yaml, so the
    // analyzer blocks; here the tests fail too, and both are named.
    const repo = await fixtures.create({
      files: {
        'firescale.config.json': JSON.stringify({ packs: ['core'] }),
        'pubspec.yaml': 'name: provider\ndependencies:\n  flutter:\n    sdk: flutter\n',
        'analysis_options.yaml': 'linter:\n  rules:\n    - unnecessary_underscores\n',
        '.dart_tool/package_config.json': '{"configVersion":2,"packages":[]}\n',
        'lib/provider.dart': 'library provider;\n',
        'test/provider_test.dart': 'void main() {}\n',
      },
    });
    const tools = await toolsFor(repo.root, {
      flutter: [
        { args: 'analyze', code: 1, out: '   info • Unnecessary use of multiple underscores\n' },
        { args: 'test', code: 1, out: 'Some tests failed.\n' },
      ],
    });
    const { human, step } = await verify(repo.root, tools);
    expect(step('lint').status).toBe('failed');
    expect(step('test').status).toBe('failed');
    expect(human.stdout.split('\n')[0]).toBe('verify: FAILED at lint, test');
    expect(human.code).toBe(EXIT_VIOLATIONS);
  });
});
