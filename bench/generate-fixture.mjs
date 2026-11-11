// Generates a deterministic git repo fixture with many small files.
//
// Usage: node bench/generate-fixture.mjs <dir> [--files 50000]
import { execFileSync } from 'node:child_process';
import { existsSync, readdirSync } from 'node:fs';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';

const args = process.argv.slice(2);
const fail = (message) => {
  console.error(message);
  process.exit(2);
};
const filesIndex = args.indexOf('--files');
const fileCount = filesIndex === -1 ? 50000 : Number(args[filesIndex + 1]);
const positional = args.filter((arg, i) => !arg.startsWith('--') && args[i - 1] !== '--files');
const target = positional[0];
if (!target || !Number.isInteger(fileCount) || fileCount < 1) {
  fail('Usage: node bench/generate-fixture.mjs <dir> [--files 50000]');
}
const root = path.resolve(target);
if (existsSync(root) && readdirSync(root).length > 0) {
  fail(`Refusing to write into ${root}: it is not empty. Pass a new or empty directory.`);
}

const pad = (n, width) => String(n).padStart(width, '0');
const FILES_PER_MODULE = 20;
const MODULES_PER_PACKAGE = 10;
const started = Date.now();

const jobs = [];
for (let i = 0; i < fileCount; i += 1) {
  const file = i % FILES_PER_MODULE;
  const mod = Math.floor(i / FILES_PER_MODULE) % MODULES_PER_PACKAGE;
  const pkg = Math.floor(i / (FILES_PER_MODULE * MODULES_PER_PACKAGE));
  const dir = path.join(root, 'packages', `pkg-${pad(pkg, 3)}`, 'src', `mod-${pad(mod, 2)}`);
  jobs.push({
    dir,
    file: path.join(dir, `file-${pad(file, 3)}.ts`),
    content: `export const value${i} = ${i};\nexport const label${i} = 'file-${i}';\n`,
  });
}

const madeDirs = new Map();
const writeOne = async ({ dir, file, content }) => {
  if (!madeDirs.has(dir)) madeDirs.set(dir, mkdir(dir, { recursive: true }));
  await madeDirs.get(dir);
  await writeFile(file, content);
};

await mkdir(root, { recursive: true });
const CONCURRENCY = 64;
let next = 0;
const worker = async () => {
  while (next < jobs.length) {
    const job = jobs[next];
    next += 1;
    await writeOne(job);
  }
};
await Promise.all(Array.from({ length: CONCURRENCY }, worker));

await mkdir(path.join(root, 'apps', 'web'), { recursive: true });
await writeFile(path.join(root, 'README.md'), '# Benchmark fixture\n\nGenerated; do not edit.\n');
await writeFile(path.join(root, '.gitignore'), 'node_modules\ndist\n');
await writeFile(
  path.join(root, 'firescale.config.json'),
  `${JSON.stringify({ rules: { 'core/no-committed-env': 'enforced' } }, null, 2)}\n`,
);
await writeFile(path.join(root, '.env.example'), 'EXAMPLE=1\n');
await writeFile(path.join(root, 'apps', 'web', '.env.local'), 'EXAMPLE=1\n');

const git = (...gitArgs) =>
  execFileSync('git', ['-c', 'core.autocrlf=false', ...gitArgs], {
    cwd: root,
    stdio: 'ignore',
    maxBuffer: 1 << 28,
  });
git('init', '--quiet');
git('add', '--all', '--force');

const seconds = ((Date.now() - started) / 1000).toFixed(1);
console.log(`Generated ${fileCount + 5} files in ${root} in ${seconds}s`);
