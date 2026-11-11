// Measures `fscale verify` latency. verify runs inside agent stop hooks, so its
// own overhead (everything except the repo's lint, typecheck and test commands)
// has to stay small.
//
// Usage: node bench/verify-bench.mjs [--bin <path>] [--runs <n>] [--label <text>]
// Prints a Markdown table: one row per scenario with p50, p99 and max.
//
// Scenarios, each in a small throwaway git repo:
//   overhead   every step is turned off in the config: check, planning and output only
//   commands   lint, typecheck and test each start `node -e ""`: adds three shells and
//              three Node starts, the floor for any repo whose steps are Node tools
//   stop hook  `fscale hooks run --agent claude` around the "commands" scenario
import { spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { performance } from 'node:perf_hooks';
import { fileURLToPath } from 'node:url';

const args = process.argv.slice(2);
const option = (name, fallback) => {
  const index = args.indexOf(name);
  return index === -1 ? fallback : args[index + 1];
};
const repoRoot = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const exe = process.platform === 'win32' ? 'fscale.exe' : 'fscale';
const bin = path.resolve(option('--bin', path.join(repoRoot, 'target', 'release', exe)));
const runs = Number(option('--runs', '200'));
const label = option('--label', process.platform);

function makeRepo(config) {
  const root = mkdtempSync(path.join(tmpdir(), 'firescale-verify-bench-'));
  writeFileSync(path.join(root, 'firescale.config.json'), `${JSON.stringify(config)}\n`);
  writeFileSync(path.join(root, 'README.md'), '# bench\n');
  const git = spawnSync('git', ['init', '--quiet'], { cwd: root, windowsHide: true });
  if (git.status !== 0) {
    console.error('Could not run `git init`. Install git, then run this again.');
    process.exit(2);
  }
  spawnSync('git', ['add', '.'], { cwd: root, windowsHide: true });
  return root;
}

function measure(cwd, argv, input) {
  const once = () => {
    const start = performance.now();
    const result = spawnSync(bin, argv, {
      cwd,
      input,
      stdio: ['pipe', 'ignore', 'ignore'],
      windowsHide: true,
    });
    if (result.error) {
      console.error(
        `Could not run ${bin}: ${result.error.message}. Build it with \`cargo build --release -p firescale-cli\`, or pass --bin.`,
      );
      process.exit(2);
    }
    if (result.status !== 0) {
      console.error(
        `${bin} ${argv.join(' ')} exited ${result.status}; the benchmark expects verify to pass.`,
      );
      process.exit(2);
    }
    return performance.now() - start;
  };
  for (let i = 0; i < 3; i += 1) once();
  const samples = Array.from({ length: runs }, once).sort((a, b) => a - b);
  const at = (fraction) =>
    samples[Math.min(samples.length - 1, Math.floor(samples.length * fraction))];
  return { p50: at(0.5), p99: at(0.99), max: samples[samples.length - 1] };
}

const noop = 'node -e ""';
const overhead = makeRepo({ verify: { lint: false, typecheck: false, test: false } });
const commands = makeRepo({ verify: { lint: noop, typecheck: noop, test: noop } });
const hookInput = JSON.stringify({ session_id: 'bench', stop_hook_active: false });

const rows = [
  ['overhead: `fscale verify`, every step off', measure(overhead, ['verify'])],
  ['commands: `fscale verify`, three `node -e ""` steps', measure(commands, ['verify'])],
  [
    'stop hook: `fscale hooks run --agent claude`, same repo',
    measure(commands, ['hooks', 'run', '--agent', 'claude'], hookInput),
  ],
];
for (const root of [overhead, commands]) rmSync(root, { recursive: true, force: true });

const ms = (value) => `${(Math.round(value * 10) / 10).toFixed(1)} ms`;
console.log(`${label}, ${runs} runs each\n`);
console.log('| Scenario | p50 | p99 | max |');
console.log('| --- | ---: | ---: | ---: |');
for (const [name, result] of rows) {
  console.log(`| ${name} | ${ms(result.p50)} | ${ms(result.p99)} | ${ms(result.max)} |`);
}
