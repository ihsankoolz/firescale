// Validates generator input against its schema before anything is written
// (ADR 0028). Only the keywords generator schemas use are supported, which
// keeps the npm package free of a validator dependency.
import { FirescaleError } from '@firescale/sdk';
import type { InputSchema } from './types.js';

/** The input with defaults filled in, or a FirescaleError that names the field. */
export function validateInput(
  generator: string,
  schema: InputSchema,
  input: unknown,
): Record<string, string | boolean> {
  const fail = (field: string, problem: string): never => {
    throw new FirescaleError(
      `fscale add ${generator}: input.${field} ${problem}. Run \`fscale schema ${generator}\` to see the input it takes.`,
    );
  };
  if (typeof input !== 'object' || input === null || Array.isArray(input)) {
    throw new FirescaleError(
      `fscale add ${generator}: the input must be a JSON object, for example '{"name": "..."}'. Run \`fscale schema ${generator}\` to see the input it takes.`,
    );
  }
  const given = input as Record<string, unknown>;
  for (const key of Object.keys(given)) {
    if (!(key in schema.properties)) {
      fail(key, `is not a known field (known: ${Object.keys(schema.properties).join(', ')})`);
    }
  }
  const out: Record<string, string | boolean> = {};
  for (const [key, property] of Object.entries(schema.properties)) {
    const value = given[key];
    if (value === undefined) {
      if (schema.required.includes(key)) fail(key, 'is required');
      if (property.default !== undefined) out[key] = property.default;
      continue;
    }
    if (property.type === 'boolean') {
      if (typeof value !== 'boolean') fail(key, 'must be true or false');
      out[key] = value as boolean;
      continue;
    }
    if (typeof value !== 'string') fail(key, 'must be a string');
    const text = value as string;
    if (property.maxLength !== undefined && text.length > property.maxLength) {
      fail(key, `must be at most ${String(property.maxLength)} characters`);
    }
    if (property.enum && !property.enum.includes(text)) {
      fail(key, `must be one of ${property.enum.join(', ')}, got "${text}"`);
    }
    if (property.pattern && !new RegExp(property.pattern, 'u').test(text)) {
      fail(key, `must match ${property.pattern}, got "${text}"`);
    }
    out[key] = text;
  }
  return out;
}
