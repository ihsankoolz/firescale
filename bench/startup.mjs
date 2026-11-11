// Measures wall-clock startup time of a command by running it many times.
//
// Usage: node bench/startup.mjs [--runs <n>] [--cwd <dir>] -- <command> [args...]
// Prints one JSON line: { command, runs, minMs, p50Ms, p95Ms, meanMs }.
import { spawnSync } from 'node:child_process';
import { performance } from 'node:perf_hooks';

const args = process.argv.slice(2);
const split = args.indexOf('--');
if (split === -1 || split === args.length - 1) {
  console.error('Usage: node bench/startup.mjs [--runs <n>] [--cwd <dir>] -- <command> [args...]');
  process.exit(2);
}
const options = args.slice(0, split);
const [command, ...commandArgs] = args.slice(split + 1);
const optionValue = (name, fallback) => {
  const index = options.indexOf(name);
  return index === -1 ? fallback : options[index + 1];
};
const runs = Number(optionValue('--runs', '50'));
const cwd = optionValue('--cwd', process.cwd());

const run = () => {
  const start = performance.now();
  const result = spawnSync(command, commandArgs, { cwd, stdio: 'ignore', windowsHide: true });
  if (result.error) {
    console.error(`Could not run ${command}: ${result.error.message}`);
    process.exit(2);
  }
  return performance.now() - start;
};

// Warm the file cache so the first sample is not an outlier.
for (let i = 0; i < 3; i += 1) run();
const samples = Array.from({ length: runs }, run).sort((a, b) => a - b);
const at = (fraction) => samples[Math.min(samples.length - 1, Math.floor(samples.length * fraction))];
const round = (value) => Math.round(value * 10) / 10;
console.log(
  JSON.stringify({
    command: [command, ...commandArgs].join(' '),
    runs,
    minMs: round(samples[0]),
    p50Ms: round(at(0.5)),
    p95Ms: round(at(0.95)),
    meanMs: round(samples.reduce((sum, value) => sum + value, 0) / samples.length),
  }),
);
