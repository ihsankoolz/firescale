// Benchmarks `fscale` startup and `check` (cold cache, warm cache, no cache) on a fixture.
//
// Usage: node bench/check-bench.mjs --bin <fscale> --fixture <dir> [--runs 10] [--label <text>] [--files <n>]
import { spawnSync } from 'node:child_process';
import { appendFileSync, existsSync, rmSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { performance } from 'node:perf_hooks';

const args = process.argv.slice(2);
const option = (name, fallback) => {
  const index = args.indexOf(name);
  return index === -1 ? fallback : args[index + 1];
};
const fail = (message) => {
  console.error(message);
  process.exit(2);
};

const binArg = option('--bin');
const fixtureArg = option('--fixture');
const runs = Number(option('--runs', '10'));
const label = option('--label', 'local');
const files = option('--files');
if (!binArg || !fixtureArg || !Number.isInteger(runs) || runs < 1) {
  fail(
    'Usage: node bench/check-bench.mjs --bin <fscale> --fixture <dir> [--runs 10] [--label <text>] [--files <n>]',
  );
}

let bin = path.resolve(binArg);
if (process.platform === 'win32' && path.extname(bin) === '' && existsSync(`${bin}.exe`)) {
  bin = `${bin}.exe`;
}
const fixture = path.resolve(fixtureArg);
if (!existsSync(bin))
  fail(`Binary not found: ${bin}. Build it with cargo build --release -p firescale-cli.`);
if (!existsSync(fixture))
  fail(`Fixture not found: ${fixture}. Create it with bench/generate-fixture.mjs.`);

const cacheDir = path.join(fixture, '.firescale');

const timeRun = (commandArgs) => {
  const start = performance.now();
  const result = spawnSync(bin, commandArgs, { cwd: fixture, stdio: 'ignore', windowsHide: true });
  const elapsed = performance.now() - start;
  if (result.error) {
    fail(`Could not run "fscale ${commandArgs.join(' ')}": ${result.error.message}`);
  }
  const accepted = commandArgs[0] === 'check' ? [0, 1] : [0];
  if (!accepted.includes(result.status)) {
    fail(
      `"fscale ${commandArgs.join(' ')}" exited with ${result.status ?? result.signal} (expected ${accepted.join(' or ')}). ` +
        'If this is an unknown-flag error, the binary does not support it yet; rebuild from current sources.',
    );
  }
  return elapsed;
};

const stats = (samples) => {
  const sorted = [...samples].sort((a, b) => a - b);
  const at = (fraction) =>
    sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * fraction))];
  const round = (value) => Math.round(value * 10) / 10;
  return {
    min: round(sorted[0]),
    p50: round(at(0.5)),
    p95: round(at(0.95)),
    mean: round(sorted.reduce((sum, value) => sum + value, 0) / sorted.length),
  };
};
const collect = (fn) => Array.from({ length: runs }, fn);

const results = {};
results['--version'] = stats(collect(() => timeRun(['--version'])));
results['check (cold cache)'] = stats(
  collect(() => {
    rmSync(cacheDir, { recursive: true, force: true });
    return timeRun(['check']);
  }),
);
rmSync(cacheDir, { recursive: true, force: true });
timeRun(['check']);
results['check (warm cache)'] = stats(collect(() => timeRun(['check'])));
results['check --no-cache'] = stats(collect(() => timeRun(['check', '--no-cache'])));

const cpus = os.cpus();
const heading =
  `### fscale bench: ${label} (${process.platform}/${process.arch}, ${cpus[0]?.model ?? 'unknown cpu'}, ` +
  `${cpus.length} cpus${files ? `, ${files} files` : ''}, ${runs} runs)`;
const rows = Object.entries(results).map(
  ([name, s]) =>
    `| ${name} | ${s.min.toFixed(1)} | ${s.p50.toFixed(1)} | ${s.p95.toFixed(1)} | ${s.mean.toFixed(1)} |`,
);
const markdown = [
  heading,
  '',
  '| Command | min (ms) | p50 (ms) | p95 (ms) | mean (ms) |',
  '| --- | ---: | ---: | ---: | ---: |',
  ...rows,
  '',
].join('\n');

console.log(markdown);
if (process.env.GITHUB_STEP_SUMMARY) {
  appendFileSync(process.env.GITHUB_STEP_SUMMARY, `${markdown}\n`);
}
console.log(
  `BENCH_JSON=${JSON.stringify({
    label,
    platform: process.platform,
    arch: process.arch,
    cpu: cpus[0]?.model ?? 'unknown',
    cpus: cpus.length,
    files: files ? Number(files) : null,
    runs,
    results,
  })}`,
);
