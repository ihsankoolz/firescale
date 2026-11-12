// The built-in generators, served to `fscale add` and `fscale schema`
// through the Node host (ADR 0028).
import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { FirescaleError } from '@firescale/sdk';
import { customRule } from './custom-rule.js';
import type { Generator, GeneratorContext, GeneratorOutput, InputSchema } from './types.js';
import { validateInput } from './validate.js';
import { workspacePackage } from './workspace-package.js';

export type { Generator, GeneratorContext, GeneratorOutput, InputSchema } from './types.js';
export { validateInput } from './validate.js';

export const GENERATORS: readonly Generator[] = [workspacePackage, customRule];

function find(name: string): Generator {
  const generator = GENERATORS.find((candidate) => candidate.name === name);
  if (!generator) {
    throw new FirescaleError(
      `Unknown generator "${name}". Generators: ${GENERATORS.map((g) => g.name).join(', ')}. Run \`fscale schema\` to list them.`,
    );
  }
  return generator;
}

/** Name and description of every generator. */
export function listGenerators(): { name: string; description: string }[] {
  return GENERATORS.map(({ name, description }) => ({ name, description }));
}

/** A generator's input schema. */
export function generatorSchema(name: string): InputSchema {
  return find(name).schema;
}

/** Validates the input, then runs the generator. Writes nothing. */
export function runGenerator(name: string, input: unknown, root: string): GeneratorOutput {
  const generator = find(name);
  const valid = validateInput(name, generator.schema, input);
  const ctx: GeneratorContext = {
    root,
    exists: (file) => existsSync(path.join(root, ...file.split('/'))),
    read: (file) => {
      try {
        return readFileSync(path.join(root, ...file.split('/')), 'utf8');
      } catch {
        return null;
      }
    },
  };
  return generator.generate(valid, ctx);
}
