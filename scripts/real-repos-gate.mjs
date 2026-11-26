#!/usr/bin/env node
// The gate of one job of real-repos.yml (ADR 0092). The job's steps only
// record what fscale did on a real repo; this fails the job when fscale itself
// broke: one of its commands exited with a code fscale never uses (0, 1, 2 and
// 130 are its codes; anything else is a panic or a program that is missing),
// one of its steps left no exit code (it hung and the step was stopped), or
// `fscale --version` did not print a version, or not the version under test (a
// launcher that finds no binary exits 2, a code fscale uses, and is caught
// here). The repo's own test command (2-own-test) is not judged.
//
// Usage: node scripts/real-repos-gate.mjs --summary <out/summary.txt> --printed <what fscale --version printed> [--version <expected>]
import { readFileSync } from 'node:fs';
import process from 'node:process';

const FSCALE_CODES = new Set(['0', '1', '2', '130']);
/** Every fscale step of a real-repos job, as its summary names them. */
const STEPS = [
  '1-doctor',
  '1-facts',
  '1-graph',
  '1-check',
  '1-verify-dry-run',
  '1-verify-budget',
  '3-verify-again',
];
const USAGE =
  'Usage: node scripts/real-repos-gate.mjs --summary <file> --printed <text> [--version <expected>]';

function fail(message) {
  process.stderr.write(`real-repos-gate: ${message}\n`);
  process.exit(1);
}

const options = {};
const argv = process.argv.slice(2);
for (let i = 0; i < argv.length; i += 2) {
  const key = argv[i];
  if (!['--summary', '--version', '--printed'].includes(key ?? '') || argv[i + 1] === undefined) {
    fail(USAGE);
  }
  options[key.slice(2)] = argv[i + 1];
}
if (!options.summary || options.printed === undefined) fail(USAGE);

let text = '';
try {
  text = readFileSync(options.summary, 'utf8');
} catch {
  fail(
    `${options.summary} is missing: fscale never ran in this job. Read the job's earlier steps.`,
  );
}

const problems = [];
const printed = options.printed.trim();
if (options.version) {
  if (printed !== options.version) {
    problems.push(`the installed fscale printed "${printed}", not ${options.version}`);
  }
} else if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(printed)) {
  problems.push(`fscale --version printed "${printed}", not a version`);
}
const seen = new Set();
let commands = 0;
for (const line of text.split(/\r?\n/)) {
  // Lines the job writes: "<step>: exit <code> after <n> s". Steps 1 and 3 are fscale's.
  const match = /^([13]-[\w-]+): exit (\S+) after /.exec(line);
  if (!match) continue;
  commands += 1;
  seen.add(match[1]);
  if (!FSCALE_CODES.has(match[2] ?? '')) problems.push(`${match[1]} exited ${match[2]}`);
}
if (commands === 0) {
  problems.push('fscale never ran: the summary has no fscale command');
} else {
  for (const step of STEPS) {
    if (!seen.has(step))
      problems.push(`${step} has no exit code: it hung and was stopped, or never started`);
  }
}
if (problems.length > 0) {
  fail(
    `${problems.join('; ')}. fscale exits only with 0, 1, 2 or 130; read that command's stderr in the job's uploaded output.`,
  );
}
process.stdout.write(
  `${commands} fscale commands, none crashed${options.version ? `, version ${options.version}` : ''}.\n`,
);
