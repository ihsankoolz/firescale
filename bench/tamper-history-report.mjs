#!/usr/bin/env node
// Summarises the tamper guard's findings on real history with their labels
// (ADR 0046): per language, commits, findings, findings labelled false, and
// the false-positive rate with a 95% Wilson interval.
//
// Usage: node bench/tamper-history-report.mjs [--data bench/tamper-history.json]
//
// The data file holds, per run of bench/tamper-history.mjs, the commits read
// and every finding with the label a fresh subagent gave it after reading
// the commit's diff (never the guard's reasoning).

import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const at = args.indexOf('--data');
const data = JSON.parse(
  readFileSync(at >= 0 ? args[at + 1] : path.join(here, 'tamper-history.json'), 'utf8'),
);

/** The 95% Wilson score interval of k successes in n trials, in percent. */
export function wilson(k, n) {
  if (n === 0) return [0, 0];
  const z = 1.959964;
  const p = k / n;
  const centre = p + (z * z) / (2 * n);
  const spread = z * Math.sqrt((p * (1 - p)) / n + (z * z) / (4 * n * n));
  const denominator = 1 + (z * z) / n;
  return [
    Math.max(0, (100 * (centre - spread)) / denominator),
    Math.min(100, (100 * (centre + spread)) / denominator),
  ];
}

const pct = (k, n) => (n === 0 ? '–' : `${((100 * k) / n).toFixed(1)}%`);
const interval = (k, n) => {
  const [low, high] = wilson(k, n);
  return n === 0 ? '–' : `${low.toFixed(1)}–${high.toFixed(1)}%`;
};

const NAMES = {
  js: 'JavaScript',
  ts: 'TypeScript',
  py: 'Python',
  rs: 'Rust',
  this: 'This repo (Rust, TypeScript)',
};
for (const [title, set] of Object.entries(data.sets)) {
  console.log(`### ${title}`);
  console.log('');
  console.log(
    '| Language | Repos | Commits | Commits with findings | Findings | False | False-positive rate (95% interval) | Commits with a false finding (95% interval) |',
  );
  console.log('| --- | --- | --- | --- | --- | --- | --- | --- |');
  const languages = [...new Set(data.runs.map((run) => run.language))];
  const rows = [
    ...languages.map((language) => [NAMES[language] ?? language, (x) => x.language === language]),
    ['All public repos', (x) => x.language !== 'this'],
  ];
  for (const [label, keep] of rows) {
    const runs = data.runs.filter(keep);
    const commits = runs.reduce((sum, run) => sum + run.commits, 0);
    const findings = set.findings.filter(keep);
    const falses = findings.filter((f) => f.label === false);
    const withFindings = new Set(findings.map((f) => f.commit)).size;
    const withFalse = new Set(falses.map((f) => f.commit)).size;
    console.log(
      `| ${label} | ${runs.map((run) => run.repo).join(', ')} | ${commits} | ${withFindings} | ${findings.length} | ${falses.length} | ${pct(falses.length, findings.length)} (${interval(falses.length, findings.length)}) | ${withFalse} of ${commits}, ${pct(withFalse, commits)} (${interval(withFalse, commits)}) |`,
    );
  }
  console.log('');
  console.log('| Kind | Findings | False | False-positive rate (95% interval) |');
  console.log('| --- | --- | --- | --- |');
  const kinds = [...new Set(set.findings.map((f) => f.kind))].sort();
  for (const kind of kinds) {
    const all = set.findings.filter((f) => f.kind === kind);
    const falses = all.filter((f) => f.label === false).length;
    console.log(
      `| ${kind} | ${all.length} | ${falses} | ${pct(falses, all.length)} (${interval(falses, all.length)}) |`,
    );
  }
  const unlabelled = set.findings.filter((f) => typeof f.label !== 'boolean').length;
  if (unlabelled > 0) console.log(`\n${unlabelled} findings have no label.`);
  console.log('');
}
