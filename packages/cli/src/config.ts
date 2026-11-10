import { readFile, stat } from 'node:fs/promises';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import {
  FirescaleError,
  RULE_STATUSES,
  type PackEntry,
  type RuleOptions,
  type RulePack,
  type RuleStatus,
} from '@firescale/sdk';
import { BUILTIN_PACKS, DEFAULT_PACKS } from './packs.js';

/** Config file names, in the order they are looked up. */
export const CONFIG_FILES: readonly string[] = ['firescale.config.ts', 'firescale.config.json'];

/** Status of a rule that the config does not mention (SPEC §10). */
export const DEFAULT_STATUS: RuleStatus = 'evaluated';

const TOP_LEVEL_KEYS: readonly string[] = ['$schema', 'packs', 'rules', 'protectedPaths'];

/** A rule with its settings from the config applied. */
export interface ConfiguredRule extends PackEntry {
  status: RuleStatus;
  options: RuleOptions;
}

export interface ResolvedConfig {
  /** Repo-relative name of the config file, or null when defaults are used. */
  file: string | null;
  packs: readonly string[];
  /** Every rule of every enabled pack, including ones that are `off`. Sorted by id. */
  rules: readonly ConfiguredRule[];
  protectedPaths: readonly string[];
}

async function exists(file: string): Promise<boolean> {
  try {
    await stat(file);
    return true;
  } catch {
    return false;
  }
}

/**
 * Finds the repo root for `cwd`: the nearest folder, walking up, that holds a
 * firescale config; failing that, the nearest one that holds `.git`; failing
 * that, `cwd` itself.
 */
export async function findRoot(cwd: string): Promise<string> {
  const start = path.resolve(cwd);
  for (const markers of [CONFIG_FILES, ['.git']]) {
    let dir = start;
    for (;;) {
      for (const marker of markers) {
        if (await exists(path.join(dir, marker))) return dir;
      }
      const parent = path.dirname(dir);
      if (parent === dir) break;
      dir = parent;
    }
  }
  return start;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((item) => typeof item === 'string');
}

function isStatus(value: unknown): value is RuleStatus {
  return typeof value === 'string' && (RULE_STATUSES as readonly string[]).includes(value);
}

async function readRaw(root: string, file: string): Promise<unknown> {
  const absolute = path.join(root, file);
  if (file.endsWith('.json')) {
    const text = (await readFile(absolute, 'utf8')).replace(/^\uFEFF/, '');
    try {
      return JSON.parse(text) as unknown;
    } catch (error) {
      const reason = error instanceof Error ? error.message : String(error);
      throw new FirescaleError(
        `${file} is not valid JSON (${reason}). Fix the syntax and try again.`,
      );
    }
  }
  let loaded: unknown;
  try {
    loaded = await import(pathToFileURL(absolute).href);
  } catch (error) {
    const code = isRecord(error) ? error['code'] : undefined;
    if (code === 'ERR_UNKNOWN_FILE_EXTENSION') {
      throw new FirescaleError(
        `Node ${process.versions.node} cannot load ${file}. Upgrade to Node 22.18 or newer, or use firescale.config.json instead.`,
      );
    }
    const reason = error instanceof Error ? error.message : String(error);
    if (code === 'ERR_MODULE_NOT_FOUND' && reason.includes("'firescale'")) {
      throw new FirescaleError(
        `${file} imports "firescale", but it is not installed in this repo. Add firescale as a dev dependency, or export a plain object without the import.`,
      );
    }
    throw new FirescaleError(
      `Could not load ${file}: ${reason}. Fix the error in the file and try again.`,
    );
  }
  const exported = isRecord(loaded) ? (loaded['default'] ?? loaded['config']) : undefined;
  if (exported === undefined) {
    throw new FirescaleError(
      `${file} has no default export. Add \`export default defineConfig({ ... })\`.`,
    );
  }
  return exported;
}

/** Validates a raw config value and applies it to the available packs. */
export function resolveConfig(
  raw: unknown,
  file: string | null,
  available: ReadonlyMap<string, RulePack> = BUILTIN_PACKS,
): ResolvedConfig {
  const name = file ?? 'config';
  if (!isRecord(raw)) {
    throw new FirescaleError(`${name} must contain an object with "packs" and "rules".`);
  }
  for (const key of Object.keys(raw)) {
    if (!TOP_LEVEL_KEYS.includes(key)) {
      throw new FirescaleError(
        `${name}: unknown key "${key}". Allowed keys: packs, rules, protectedPaths.`,
      );
    }
  }

  const packNames = raw['packs'] ?? DEFAULT_PACKS;
  if (!isStringArray(packNames)) {
    throw new FirescaleError(
      `${name}: "packs" must be a list of pack names, for example ["core"].`,
    );
  }
  const entries = new Map<string, PackEntry>();
  for (const packName of packNames) {
    const pack = available.get(packName);
    if (!pack) {
      throw new FirescaleError(
        `${name}: unknown rule pack "${packName}". Available packs: ${[...available.keys()].join(', ')}.`,
      );
    }
    for (const entry of pack.rules) entries.set(entry.rule.id, entry);
  }

  const ruleSettings = raw['rules'] ?? {};
  if (!isRecord(ruleSettings)) {
    throw new FirescaleError(`${name}: "rules" must be an object keyed by rule id.`);
  }
  const settings = new Map<string, { status: RuleStatus; options: RuleOptions }>();
  for (const [id, value] of Object.entries(ruleSettings)) {
    if (!entries.has(id)) {
      const known = [...entries.keys()].sort().join(', ') || '(none)';
      throw new FirescaleError(
        `${name}: unknown rule "${id}". Rules in the enabled packs: ${known}.`,
      );
    }
    const expected = `"off", "evaluated", "enforced" or { status, options }`;
    if (isStatus(value)) {
      settings.set(id, { status: value, options: {} });
    } else if (isRecord(value)) {
      const extra = Object.keys(value).find((key) => key !== 'status' && key !== 'options');
      if (extra !== undefined) {
        throw new FirescaleError(
          `${name}: rule "${id}" has unknown key "${extra}". Use ${expected}.`,
        );
      }
      const status = value['status'] ?? DEFAULT_STATUS;
      const options = value['options'] ?? {};
      if (!isStatus(status)) {
        throw new FirescaleError(
          `${name}: rule "${id}" has an invalid status. Use "off", "evaluated" or "enforced".`,
        );
      }
      if (!isRecord(options)) {
        throw new FirescaleError(`${name}: rule "${id}" options must be an object.`);
      }
      settings.set(id, { status, options });
    } else {
      throw new FirescaleError(`${name}: rule "${id}" must be ${expected}.`);
    }
  }

  const protectedPaths = raw['protectedPaths'] ?? [];
  if (!isStringArray(protectedPaths)) {
    throw new FirescaleError(`${name}: "protectedPaths" must be a list of globs.`);
  }

  const rules = [...entries.values()]
    .map((entry) => ({
      ...entry,
      status: settings.get(entry.rule.id)?.status ?? DEFAULT_STATUS,
      options: settings.get(entry.rule.id)?.options ?? {},
    }))
    .sort((a, b) => (a.rule.id < b.rule.id ? -1 : a.rule.id > b.rule.id ? 1 : 0));

  return { file, packs: [...packNames], rules, protectedPaths: [...protectedPaths] };
}

/** Loads `firescale.config.ts` or `.json` from the repo root. Defaults apply when neither exists. */
export async function loadConfig(root: string): Promise<ResolvedConfig> {
  const found: string[] = [];
  for (const file of CONFIG_FILES) {
    if (await exists(path.join(root, file))) found.push(file);
  }
  if (found.length > 1) {
    throw new FirescaleError(
      `Found both ${found.join(' and ')}. Keep one config file and delete the other.`,
    );
  }
  const file = found[0];
  if (file === undefined) return resolveConfig({}, null);
  return resolveConfig(await readRaw(root, file), file);
}
