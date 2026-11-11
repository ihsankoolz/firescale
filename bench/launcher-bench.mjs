// Measures `--version` wall time for the native binary and for the same binary
// started through the npm launcher (one extra Node process).
//
// Usage: node bench/launcher-bench.mjs [--binary <path>] [--launcher <path>]
// Prints a markdown table, appends it to $GITHUB_STEP_SUMMARY when set, and ends
// with one line: LAUNCHER_BENCH_JSON=<json>.
import { spawnSync } from 'node:child_process';
import { appendFileSync, existsSync } from 'node:fs';
import path from 'node:path';
import { performance } from 'node:perf_hooks';
import process from 'node:process';

const WARMUPS = 3;
const RUNS = 20;

const args = process.argv.slice(2);
const option = (name, fallback) => {
  const index = args.indexOf(name);
  return index === -1 ? fallback : args[index + 1];
};
const binary = path.resolve(
  option(
    '--binary',
    path.join('target', 'release', process.platform === 'win32' ? 'fscale.exe' : 'fscale'),
  ),
);
const launcher = path.resolve(
  option('--launcher', path.join('packages', 'cli', 'bin', 'fscale.js')),
);
for (const [label, file] of [
  ['binary', binary],
  ['launcher', launcher],
]) {
  if (!existsSync(file)) {
    console.error(
      `launcher-bench: the ${label} ${file} does not exist. Build it first or pass --${label} <path>.`,
    );
    process.exit(2);
  }
}

function measure(command, commandArgs, env) {
  const once = () => {
    const start = performance.now();
    const result = spawnSync(command, commandArgs, { env, stdio: 'ignore', windowsHide: true });
    if (result.error || result.status !== 0) {
      console.error(
        `launcher-bench: "${command} ${commandArgs.join(' ')}" failed. Run it by hand to see why.`,
      );
      process.exit(2);
    }
    return performance.now() - start;
  };
  for (let i = 0; i < WARMUPS; i += 1) once();
  const samples = Array.from({ length: RUNS }, once).sort((a, b) => a - b);
  const at = (fraction) =>
    samples[Math.min(samples.length - 1, Math.floor(samples.length * fraction))];
  const round = (value) => Math.round(value * 10) / 10;
  return {
    minMs: round(samples[0]),
    p50Ms: round(at(0.5)),
    p95Ms: round(at(0.95)),
    meanMs: round(samples.reduce((sum, value) => sum + value, 0) / samples.length),
  };
}

const direct = measure(binary, ['--version'], process.env);
const viaLauncher = measure(process.execPath, [launcher, '--version'], {
  ...process.env,
  FSCALE_BINARY: binary,
});

const row = (name, r) => `| ${name} | ${r.minMs} | ${r.p50Ms} | ${r.p95Ms} | ${r.meanMs} |`;
const table = [
  `### Launcher startup (${process.platform}-${process.arch}, ${RUNS} runs)`,
  '',
  '| Path | min ms | p50 ms | p95 ms | mean ms |',
  '| --- | ---: | ---: | ---: | ---: |',
  row('native binary', direct),
  row('npm launcher + binary', viaLauncher),
  '',
].join('\n');
console.log(table);
if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, `${table}\n`);
console.log(
  `LAUNCHER_BENCH_JSON=${JSON.stringify({ platform: `${process.platform}-${process.arch}`, runs: RUNS, direct, launcher: viaLauncher })}`,
);
