// Installs the packed tarballs into a fresh project with a real package manager
// and runs the installed `fscale` end to end.
//
// Usage: node scripts/smoke-npm.mjs --pm <npm|pnpm|yarn> [--tarballs <dir>] [--keep]
//
// Only the tarballs and the sample pack source in packages/cli/test-fixtures are
// read from this repository. Nothing here uses the repo's node_modules or target.
import { execFileSync } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { readdir, readFile, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

const IS_WIN = process.platform === 'win32';
const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, '..');
const FIXTURE = path.join(repoRoot, 'packages', 'cli', 'test-fixtures', 'custom-pack');

const OTHER_CORE_RULES = [
  'core/ci-exists',
  'core/env-example',
  'core/gitignore-basics',
  'core/no-secrets-in-source',
  'core/readme-setup',
  'core/scripts-present',
  'core/single-lockfile',
];

function die(message) {
  console.error(`smoke-npm: ${message}`);
  process.exit(2);
}

function parseArgs(argv) {
  const out = { pm: undefined, tarballs: 'dist-npm', keep: false };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--keep') out.keep = true;
    else if (arg === '--pm' || arg === '--tarballs') {
      const value = argv[i + 1];
      if (value === undefined) die(`${arg} needs a value. Example: --pm npm`);
      out[arg.slice(2)] = value;
      i += 1;
    } else
      die(
        `unknown argument "${arg}". Usage: node scripts/smoke-npm.mjs --pm <npm|pnpm|yarn> [--tarballs <dir>] [--keep]`,
      );
  }
  if (!['npm', 'pnpm', 'yarn'].includes(out.pm)) {
    die('--pm must be npm, pnpm or yarn. Example: node scripts/smoke-npm.mjs --pm npm');
  }
  return out;
}

/** Runs a command and returns { code, stdout, stderr } without throwing on a non-zero exit. */
function run(command, args, options = {}) {
  try {
    const stdout = execFileSync(command, args, {
      encoding: 'utf8',
      shell: IS_WIN,
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true,
      ...options,
    });
    return { code: 0, stdout, stderr: '' };
  } catch (error) {
    return {
      code: typeof error.status === 'number' ? error.status : 1,
      stdout: String(error.stdout ?? ''),
      stderr: String(error.stderr ?? '') || String(error.message),
    };
  }
}

function must(command, args, options) {
  const result = run(command, args, options);
  if (result.code !== 0) {
    console.error(result.stdout);
    console.error(result.stderr);
    die(
      `"${command} ${args.join(' ')}" failed with exit code ${result.code}. Read the output above and fix the cause.`,
    );
  }
  return result;
}

const results = [];
function check(name, ok, detail) {
  results.push(ok);
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}`);
  if (!ok && detail) console.log(`      ${String(detail).trim().split('\n').join('\n      ')}`);
}

const options = parseArgs(process.argv.slice(2));
const tarballDir = path.resolve(options.tarballs);
const manifestPath = path.join(tarballDir, 'manifest.json');
if (!existsSync(manifestPath)) {
  die(
    `${manifestPath} does not exist. Run "node scripts/pack-npm.mjs" first, or pass --tarballs <dir>.`,
  );
}
const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'));
const tarballs = ['platform', 'sdk', 'firescale'].map((key) => {
  const file = manifest.tarballs?.[key];
  if (!file) die(`manifest.json has no tarballs.${key}. Re-run scripts/pack-npm.mjs.`);
  const full = path.join(tarballDir, file);
  if (!existsSync(full))
    die(`${full} is listed in manifest.json but missing. Re-run scripts/pack-npm.mjs.`);
  return full;
});

const project = mkdtempSync(path.join(os.tmpdir(), 'firescale-smoke-'));
console.log(`temp project: ${project}`);
console.log(
  `package manager: ${options.pm}, platform: ${manifest.platform}, version: ${manifest.version}`,
);

async function main() {
  writeFileSync(
    path.join(project, 'package.json'),
    `${JSON.stringify({ name: 'firescale-smoke', private: true, type: 'module' }, null, 2)}\n`,
  );

  // Install the three tarballs by absolute path.
  if (options.pm === 'npm') {
    must('npm', ['install', '--no-audit', '--no-fund', ...tarballs], { cwd: project });
  } else if (options.pm === 'pnpm') {
    // pnpm resolves firescale's dependency on @firescale/sdk (and the platform
    // package) from the registry, where they do not exist yet. Overrides point
    // those names at the tarballs.
    const [platformTgz, sdkTgz] = tarballs;
    const overrides = {
      '@firescale/sdk': `file:${sdkTgz}`,
      [`@firescale/cli-${manifest.platform}`]: `file:${platformTgz}`,
    };
    // pnpm 12 reads overrides from pnpm-workspace.yaml. JSON strings are valid YAML scalars.
    const lines = Object.entries(overrides).map(
      ([k, v]) => `  ${JSON.stringify(k)}: ${JSON.stringify(v)}`,
    );
    writeFileSync(path.join(project, 'pnpm-workspace.yaml'), `overrides:\n${lines.join('\n')}\n`);
    // --allow-build lets pnpm run the optional postinstall without an approval prompt.
    must('pnpm', ['add', '--allow-build=firescale', ...tarballs], { cwd: project });
  } else {
    // Yarn resolves every optional dependency, even for other platforms, and fails on
    // names missing from the registry. `resolutions` points the registry-only names
    // (sdk and all six platform packages) at the tarballs; Yarn then skips the
    // platform packages whose os/cpu do not match.
    const [platformTgz, sdkTgz] = tarballs;
    const resolutions = { '@firescale/sdk': `file:${sdkTgz}` };
    for (const key of [
      'linux-x64',
      'linux-arm64',
      'darwin-x64',
      'darwin-arm64',
      'win32-x64',
      'win32-arm64',
    ]) {
      resolutions[`@firescale/cli-${key}`] = `file:${platformTgz}`;
    }
    writeFileSync(
      path.join(project, 'package.json'),
      `${JSON.stringify({ name: 'firescale-smoke', private: true, type: 'module', resolutions }, null, 2)}\n`,
    );
    // Yarn Classic (1.22.22) on purpose: it uses a normal node_modules folder.
    // Yarn Berry would need PnP or nodeLinker settings, which is a different test.
    must(
      'npx',
      [
        '--yes',
        'yarn@1.22.22',
        'add',
        '--non-interactive',
        '--ignore-optional',
        ...tarballs.map((t) => `file:${t}`),
      ],
      {
        cwd: project,
      },
    );
  }

  const bin = (name) => path.join(project, 'node_modules', '.bin', IS_WIN ? `${name}.cmd` : name);
  const fscale = (args, cwd = project) => run(bin('fscale'), args, { cwd });

  // a, b: version through both bin names.
  const v1 = fscale(['--version']);
  check(
    `fscale --version prints ${manifest.version}`,
    v1.code === 0 && v1.stdout.trim() === manifest.version,
    `exit ${v1.code}\n${v1.stdout}\n${v1.stderr}`,
  );
  const v2 = run(bin('firescale'), ['--version'], { cwd: project });
  check(
    'firescale --version (second bin)',
    v2.code === 0 && v2.stdout.trim() === manifest.version,
    `exit ${v2.code}\n${v2.stdout}\n${v2.stderr}`,
  );

  // c: check in a git repo with a tracked .env.
  const repo = path.join(project, 'repo');
  mkdirSync(repo);
  must('git', ['init', '--quiet'], { cwd: repo });
  must('git', ['config', 'user.email', 'smoke@example.com'], { cwd: repo });
  must('git', ['config', 'user.name', 'Smoke'], { cwd: repo });
  await writeFile(path.join(repo, '.env'), 'EXAMPLE=1\n');
  const rules = Object.fromEntries(OTHER_CORE_RULES.map((id) => [id, 'off']));
  rules['core/no-committed-env'] = 'enforced';
  const config = { rules };
  const writeConfig = () =>
    writeFile(path.join(repo, 'firescale.config.json'), `${JSON.stringify(config, null, 2)}\n`);
  await writeConfig();
  must('git', ['add', '.'], { cwd: repo });
  must('git', ['commit', '--quiet', '-m', 'init'], { cwd: repo });

  const c1 = fscale(['check'], repo);
  check(
    'fscale check exits 1 and names .env',
    c1.code === 1 && c1.stdout.includes('.env'),
    `exit ${c1.code}\n${c1.stdout}\n${c1.stderr}`,
  );
  const c2 = fscale(['check', '--json'], repo);
  let json;
  try {
    json = JSON.parse(c2.stdout);
  } catch {
    json = undefined;
  }
  check(
    'fscale check --json is valid JSON with ok: false',
    c2.code === 1 && json?.ok === false,
    `exit ${c2.code}\n${c2.stdout}\n${c2.stderr}`,
  );

  // d: custom TypeScript rule, copied from the fixture with the SDK import pointed at `firescale`.
  const rulesDir = path.join(repo, 'tools', 'rules');
  cpSync(FIXTURE, rulesDir, { recursive: true });
  const rewrite = async (dir) => {
    for (const entry of await readdir(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) await rewrite(full);
      else if (entry.name.endsWith('.ts')) {
        const text = await readFile(full, 'utf8');
        await writeFile(full, text.replaceAll("'@firescale/sdk'", "'firescale'"));
      }
    }
  };
  await rewrite(rulesDir);
  await writeFile(path.join(repo, 'TODO.md'), '- something\n');
  config.packs = ['core', './tools/rules/pack.ts'];
  config.rules['sample/no-todo-file'] = 'enforced';
  await writeConfig();
  must('git', ['add', '.'], { cwd: repo });
  must('git', ['commit', '--quiet', '-m', 'custom rule'], { cwd: repo });
  const d = fscale(['check', '--json'], repo);
  let report;
  try {
    report = JSON.parse(d.stdout);
  } catch {
    report = undefined;
  }
  const hit = report?.diagnostics?.some(
    (x) => x.ruleId === 'sample/no-todo-file' && x.file === 'TODO.md',
  );
  check(
    'custom TypeScript rule reports sample/no-todo-file on TODO.md',
    Boolean(hit),
    `exit ${d.code}\n${d.stdout}\n${d.stderr}`,
  );

  // e: explain.
  const e = fscale(['explain', 'core/no-committed-env'], repo);
  check(
    'fscale explain core/no-committed-env exits 0',
    e.code === 0,
    `exit ${e.code}\n${e.stdout}\n${e.stderr}`,
  );
}

try {
  await main();
} finally {
  if (options.keep) console.log(`kept: ${project}`);
  else await rm(project, { recursive: true, force: true, maxRetries: 3 });
}

const failed = results.filter((ok) => !ok).length;
console.log(
  failed === 0
    ? `\nAll ${results.length} checks passed (${options.pm}).`
    : `\n${failed} of ${results.length} checks failed (${options.pm}). Re-run with --keep to inspect the project.`,
);
process.exit(failed === 0 ? 0 : 1);
