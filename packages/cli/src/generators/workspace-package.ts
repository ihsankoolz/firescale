// `fscale add package`: a new JavaScript package in the workspace.
import type { Generator, GeneratorContext } from './types.js';

/** True when a workspace pattern such as `packages/*` covers `folder`. */
function covers(pattern: string, folder: string): boolean {
  const clean = pattern.replace(/^\.\//, '').replace(/\/$/, '');
  if (clean === folder) return true;
  if (clean.endsWith('/**')) return folder.startsWith(`${clean.slice(0, -3)}/`);
  if (clean.endsWith('/*')) {
    const parent = clean.slice(0, -2);
    return folder.startsWith(`${parent}/`) && !folder.slice(parent.length + 1).includes('/');
  }
  return false;
}

/** The repo's workspace patterns and where they are listed, if it has any. */
function workspaces(ctx: GeneratorContext): { patterns: string[]; file: string } | null {
  const pnpm = ctx.read('pnpm-workspace.yaml');
  if (pnpm !== null) {
    const patterns = [...pnpm.matchAll(/^\s*-\s*['"]?([^'"#\n]+?)['"]?\s*$/gm)].map((m) =>
      (m[1] ?? '').trim(),
    );
    return { patterns, file: 'pnpm-workspace.yaml' };
  }
  const text = ctx.read('package.json');
  if (text === null) return null;
  try {
    const json = JSON.parse(text) as { workspaces?: string[] | { packages?: string[] } };
    const list = Array.isArray(json.workspaces) ? json.workspaces : json.workspaces?.packages;
    return list ? { patterns: list, file: 'package.json ("workspaces")' } : null;
  } catch {
    return null;
  }
}

export const workspacePackage: Generator = {
  name: 'package',
  description:
    'A new JavaScript package in the workspace: package.json, src/index.js, a node:test test and a README.',
  schema: {
    $schema: 'http://json-schema.org/draft-07/schema#',
    title: 'fscale add package',
    description: 'Input for the workspace package generator.',
    type: 'object',
    additionalProperties: false,
    required: ['name'],
    properties: {
      name: {
        type: 'string',
        description: 'The npm package name, for example "utils" or "@acme/utils".',
        pattern: '^(@[a-z0-9][a-z0-9._-]*/)?[a-z0-9][a-z0-9._-]*$',
        maxLength: 214,
      },
      folder: {
        type: 'string',
        description:
          'Where to put it, relative to the repo root with forward slashes. Default: packages/<name without its scope>.',
        pattern: '^(?!\\.\\.?(/|$))(?!.*/\\.\\.?(/|$))[A-Za-z0-9._-]+(/[A-Za-z0-9._-]+)*$',
      },
      description: {
        type: 'string',
        description: 'One sentence for package.json and the README.',
        maxLength: 200,
        default: '',
      },
    },
  },
  generate(input, ctx) {
    const name = String(input['name']);
    const short = name.includes('/') ? name.slice(name.indexOf('/') + 1) : name;
    const folder = typeof input['folder'] === 'string' ? input['folder'] : `packages/${short}`;
    const description = String(input['description'] ?? '');
    const manifest = {
      name,
      version: '0.0.0',
      private: true,
      description,
      type: 'module',
      exports: './src/index.js',
      scripts: { test: 'node --test' },
    };
    const notes: string[] = [];
    const listed = workspaces(ctx);
    if (listed === null) {
      notes.push(
        `This repo lists no workspaces, so its package manager will not install ${folder} with the root. Add a workspace list, or run the install in ${folder}.`,
      );
    } else if (!listed.patterns.some((pattern) => covers(pattern, folder))) {
      notes.push(`Add "${folder}" to ${listed.file}, so the workspace includes the new package.`);
    }
    return {
      files: [
        { path: `${folder}/package.json`, content: `${JSON.stringify(manifest, null, 2)}\n` },
        {
          path: `${folder}/src/index.js`,
          content: `/** The package's first export. Replace it with the real code. */\nexport function describe() {\n  return ${JSON.stringify(`${name}: ${description || 'a new package'}`)};\n}\n`,
        },
        {
          path: `${folder}/test/index.test.js`,
          content: `import assert from 'node:assert/strict';\nimport { test } from 'node:test';\nimport { describe } from '../src/index.js';\n\ntest('describes the package', () => {\n  assert.match(describe(), /^${name.replace(/[.*+?^${}()|[\]\\/]/g, '\\$&')}: /);\n});\n`,
        },
        {
          path: `${folder}/README.md`,
          content: `# ${name}\n\n${description ? `${description}\n\n` : ''}## Usage\n\n\`\`\`js\nimport { describe } from '${name}';\n\`\`\`\n\n## Test\n\n\`\`\`sh\nnode --test\n\`\`\`\n`,
        },
      ],
      notes,
    };
  },
};
