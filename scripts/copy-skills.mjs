// Copies every SKILL.md from a source tree into the build output, keeping the
// folder layout, so a built rule pack can find its skills next to its rules.
// Usage: node scripts/copy-skills.mjs <srcDir> <outDir>
import { copyFile, mkdir, readdir } from 'node:fs/promises';
import path from 'node:path';

const [srcDir, outDir] = process.argv.slice(2);
if (!srcDir || !outDir) {
  console.error('Usage: node scripts/copy-skills.mjs <srcDir> <outDir>');
  process.exit(2);
}

const entries = await readdir(srcDir, { recursive: true, withFileTypes: true });
let copied = 0;
for (const entry of entries) {
  if (!entry.isFile() || entry.name !== 'SKILL.md') continue;
  const from = path.join(entry.parentPath, entry.name);
  const to = path.join(outDir, path.relative(srcDir, from));
  await mkdir(path.dirname(to), { recursive: true });
  await copyFile(from, to);
  copied += 1;
}
console.log(`Copied ${copied} SKILL.md file(s) to ${outDir}`);
