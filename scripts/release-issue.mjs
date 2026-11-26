#!/usr/bin/env node
// The issue the Release workflow opens when it rolled a release back
// (ADR 0092, step 10). Prints the title on stdout and writes the body to --out.
//
// Usage: node scripts/release-issue.mjs --version <v> --run <url> --promote <result>
//          --smoke <result> --rollback <result> --out <file> [--rehearsal]
//   <result> is a GitHub job result: success, failure, cancelled or skipped.
import { writeFileSync } from 'node:fs';
import process from 'node:process';

const USAGE =
  'Usage: node scripts/release-issue.mjs --version <v> --run <url> --promote <result> --smoke <result> --rollback <result> --out <file> [--rehearsal]';

function fail(message) {
  process.stderr.write(`release-issue: ${message}\n`);
  process.exit(1);
}

const options = { rehearsal: false };
const argv = process.argv.slice(2);
for (let i = 0; i < argv.length; i += 1) {
  const key = argv[i] ?? '';
  if (key === '--rehearsal') {
    options.rehearsal = true;
    continue;
  }
  if (!['--version', '--run', '--promote', '--smoke', '--rollback', '--out'].includes(key))
    fail(USAGE);
  const value = argv[i + 1];
  if (value === undefined) fail(USAGE);
  options[key.slice(2)] = value;
  i += 1;
}
for (const key of ['version', 'run', 'promote', 'smoke', 'rollback', 'out']) {
  if (!options[key]) fail(USAGE);
}

const v = options.version;
const what =
  options.promote !== 'success'
    ? options.promote === 'cancelled'
      ? 'the promotion was cancelled'
      : 'the promotion failed'
    : options.smoke === 'cancelled'
      ? 'the production smoke was cancelled'
      : 'the production smoke failed';
const title = `${options.rehearsal ? '[rehearsal] ' : ''}Release ${v} rolled back: ${what}`;

const lines = [];
if (options.rehearsal) {
  lines.push(
    '> A rehearsal (ADR 0092): the workflow ran with a stand-in for npm, the production smoke failed on purpose, and nothing on npm was touched. This issue is closed at once.',
    '',
  );
}
lines.push(
  `The Release workflow for **${v}** moved \`latest\` back after ${what}: ${options.run}`,
  '',
  `- Promotion: ${options.promote}`,
  `- Production smoke (\`npx firescale@latest\` on Linux, macOS and Windows): ${options.smoke}`,
  `- Rollback: ${options.rollback}`,
  '',
);
if (options.rollback === 'success') {
  lines.push(
    "`latest` is back on the recorded version on all eight packages (the run's `promotion-record` artifact has the versions). `npx firescale` gives the previous release again.",
  );
} else {
  lines.push(
    `**The rollback failed.** \`latest\` may still be ${v} on some packages. Re-run the rollback job. If it keeps failing, an owner moves each package back by hand with \`npm dist-tag add <package>@<recorded version> latest\`, using the run's \`promotion-record\` artifact, or runs \`node scripts/promote-npm.mjs rollback --record <that file>\` while logged in to npm.`,
  );
}
lines.push(
  '',
  `Nothing was unpublished: ${v} stays on npm under \`next\` and by its number.`,
  '',
  '**Next:**',
  '',
  `1. Read the failed job's log in the run above and fix the cause.`,
  '2. Ship a patch: the next version, through a normal release. A version number is never reused.',
  `3. Consider deprecating the bad version, so that anyone who asks for it by number is warned: \`npm deprecate firescale@${v} "broken release, use <the patch>"\` (and the same for the seven other packages if they are affected).`,
  '',
);
writeFileSync(options.out, lines.join('\n'));
process.stdout.write(`${title}\n`);
