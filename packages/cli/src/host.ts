/**
 * The Node host: the TypeScript end of the bridge described in ADR 0008.
 *
 * The `fscale` binary starts this module as a child process when a run needs
 * TypeScript: a `firescale.config.ts`, or a custom rule pack. Requests arrive
 * as JSON lines on stdin; each gets exactly one JSON line on stdout.
 *
 *   -> {"id": 1, "method": "hello", "params": {}}
 *   <- {"id": 1, "result": {"protocolVersion": 1}}
 *
 * The process exits when stdin closes.
 */
import path from 'node:path';
import { createInterface } from 'node:readline';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { generatorSchema, listGenerators, runGenerator } from './generators/index.js';
import {
  checkRule,
  fixRuleWithChanges,
  FirescaleError,
  ignoreInheritedGitRepo,
  type Diagnostic,
  type Rule,
  type RulePack,
} from '@firescale/sdk';

/** Must match `PROTOCOL_VERSION` in crates/core/src/node.rs. */
export const PROTOCOL_VERSION = 1;

interface Request {
  id: number;
  method: string;
  params: Record<string, unknown>;
}

interface RuleDescription {
  id: string;
  severity: string;
  category: string;
  description: string;
  inputs: readonly string[];
  hasFix: boolean;
  after: readonly string[];
  skillFile: string;
}

interface PackDescription {
  spec: string;
  name: string;
  rules: RuleDescription[];
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function stringParam(params: Record<string, unknown>, name: string): string {
  const value = params[name];
  if (typeof value !== 'string') {
    throw new Error(`bridge request is missing the string parameter "${name}"`);
  }
  return value;
}

/**
 * True for the SDK's FirescaleError. Checked by name, not `instanceof`: a pack
 * can load its own copy of the SDK, whose error class is a different object.
 */
function isUserError(error: unknown): error is Error {
  return error instanceof Error && error.name === 'FirescaleError';
}

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Imports a module by absolute path and turns Node's errors into next steps. */
async function importFile(absolute: string, shown: string): Promise<Record<string, unknown>> {
  try {
    const loaded: unknown = await import(pathToFileURL(absolute).href);
    return isRecord(loaded) ? loaded : {};
  } catch (error) {
    const code = isRecord(error) ? error['code'] : undefined;
    const reason = reasonOf(error);
    if (code === 'ERR_UNKNOWN_FILE_EXTENSION') {
      throw new FirescaleError(
        `Node ${process.versions.node} cannot load ${shown}. Upgrade to Node 22.18 or newer, or use firescale.config.json instead.`,
      );
    }
    if (code === 'ERR_MODULE_NOT_FOUND' && reason.includes(absolute)) {
      throw new FirescaleError(`${shown} does not exist. Check the path in your firescale config.`);
    }
    if (code === 'ERR_MODULE_NOT_FOUND' && /'(firescale|@firescale\/sdk)'/.test(reason)) {
      throw new FirescaleError(
        `${shown} imports the firescale package, but it is not installed in this repo. Add firescale as a dev dependency, or export a plain object without the import.`,
      );
    }
    if (/Unexpected token 'export'|Cannot use import statement/.test(reason)) {
      throw new FirescaleError(
        `${shown} was loaded as CommonJS, so its import/export syntax failed. Set "type": "module" in the nearest package.json, or use firescale.config.json instead.`,
      );
    }
    throw new FirescaleError(
      `Could not load ${shown}: ${reason}. Fix the error in the file and try again.`,
    );
  }
}

async function loadConfig(params: Record<string, unknown>): Promise<unknown> {
  const file = stringParam(params, 'file');
  const loaded = await importFile(path.join(stringParam(params, 'root'), file), file);
  const exported = loaded['default'] ?? loaded['config'];
  if (exported === undefined) {
    throw new FirescaleError(
      `${file} has no default export. Add \`export default defineConfig({ ... })\`.`,
    );
  }
  return exported;
}

/** Packs stay loaded for the life of the process, keyed by their path in the config. */
const packs = new Map<string, RulePack>();

function isRulePack(value: unknown): value is RulePack {
  return (
    isRecord(value) &&
    typeof value['name'] === 'string' &&
    Array.isArray(value['rules']) &&
    value['rules'].every(
      (entry) =>
        isRecord(entry) &&
        isRecord(entry['rule']) &&
        typeof entry['rule']['id'] === 'string' &&
        typeof entry['rule']['check'] === 'function' &&
        entry['skillFile'] instanceof URL,
    )
  );
}

async function loadPack(root: string, spec: string): Promise<RulePack> {
  const cached = packs.get(spec);
  if (cached) return cached;
  const loaded = await importFile(path.resolve(root, spec), spec);
  const pack = loaded['default'] ?? loaded['pack'];
  if (!isRulePack(pack)) {
    throw new FirescaleError(
      `${spec} does not export a rule pack. Add \`export default definePack({ name, rules })\`, where each rule entry is { rule, skillFile }.`,
    );
  }
  packs.set(spec, pack);
  return pack;
}

async function describePacks(params: Record<string, unknown>): Promise<PackDescription[]> {
  const root = stringParam(params, 'root');
  const specs = params['packs'];
  if (!Array.isArray(specs) || !specs.every((spec) => typeof spec === 'string')) {
    throw new Error('bridge request is missing the string list parameter "packs"');
  }
  const described: PackDescription[] = [];
  for (const spec of specs) {
    const pack = await loadPack(root, spec);
    described.push({
      spec,
      name: pack.name,
      rules: pack.rules.map(({ rule, skillFile }) => ({
        id: rule.id,
        severity: rule.severity,
        category: rule.category,
        description: rule.description,
        inputs: rule.inputs,
        hasFix: rule.fix !== undefined,
        after: rule.after ?? [],
        skillFile: fileURLToPath(skillFile),
      })),
    });
  }
  return described;
}

async function findRule(root: string, spec: string, ruleId: string): Promise<Rule> {
  const pack = await loadPack(root, spec);
  const entry = pack.rules.find(({ rule }) => rule.id === ruleId);
  if (!entry) throw new Error(`rule "${ruleId}" is not in pack ${spec}`);
  return entry.rule;
}

async function check(params: Record<string, unknown>): Promise<Diagnostic[]> {
  const root = stringParam(params, 'root');
  const ruleId = stringParam(params, 'ruleId');
  const rule = await findRule(root, stringParam(params, 'pack'), ruleId);
  const options = isRecord(params['options']) ? params['options'] : {};
  try {
    return await checkRule(rule, { root, options });
  } catch (error) {
    if (isUserError(error)) throw error;
    throw new FirescaleError(
      `Rule "${ruleId}" crashed: ${reasonOf(error)}. This is a bug in the rule. Set it to "off" in your firescale config to continue, and report it to the rule's author.`,
    );
  }
}

async function fix(params: Record<string, unknown>): Promise<unknown> {
  // `root` is where the fix works. In a dry run of the fix loop that is a
  // scratch copy of the repo, while the pack is still loaded from the repo.
  const root = stringParam(params, 'root');
  const packRoot = typeof params['packRoot'] === 'string' ? params['packRoot'] : root;
  if (path.resolve(packRoot) !== path.resolve(root)) ignoreInheritedGitRepo();
  const ruleId = stringParam(params, 'ruleId');
  const rule = await findRule(packRoot, stringParam(params, 'pack'), ruleId);
  const options = isRecord(params['options']) ? params['options'] : {};
  try {
    const outcome = await fixRuleWithChanges(
      rule,
      { root, options },
      { dryRun: params['dryRun'] === true },
    );
    return {
      changedFiles: outcome.result.changedFiles,
      notes: outcome.result.notes ?? [],
      changes: outcome.changes,
      attempted: outcome.attempted,
    };
  } catch (error) {
    if (isUserError(error)) throw error;
    throw new FirescaleError(
      `The fix of rule "${ruleId}" crashed: ${reasonOf(error)}. This is a bug in the rule. The fix may have changed some files before it failed: check git status and git diff, and report the bug to the rule's author.`,
    );
  }
}

async function handle(request: Request): Promise<unknown> {
  switch (request.method) {
    case 'hello':
      return { protocolVersion: PROTOCOL_VERSION };
    case 'loadConfig':
      return loadConfig(request.params);
    case 'describePacks':
      return describePacks(request.params);
    case 'checkRule':
      return check(request.params);
    case 'fixRule':
      return fix(request.params);
    case 'listGenerators':
      return listGenerators();
    case 'generatorSchema':
      return generatorSchema(stringParam(request.params, 'name'));
    case 'runGenerator':
      return runGenerator(
        stringParam(request.params, 'name'),
        request.params['input'],
        stringParam(request.params, 'root'),
      );
    default:
      throw new Error(`unknown bridge method "${request.method}"`);
  }
}

/** Serves requests from stdin until it closes. */
export async function serve(): Promise<void> {
  // stdout carries the protocol, so anything a config or rule logs goes to stderr.
  console.log = console.error;
  console.info = console.error;
  console.debug = console.error;

  const lines = createInterface({ input: process.stdin, crlfDelay: Infinity });
  for await (const line of lines) {
    if (line.trim() === '') continue;
    let id: number | null = null;
    let response: Record<string, unknown>;
    try {
      const request = JSON.parse(line) as Request;
      id = request.id;
      response = { id, result: (await handle(request)) ?? null };
    } catch (error) {
      response = { id, error: { message: reasonOf(error) } };
    }
    process.stdout.write(`${JSON.stringify(response)}\n`);
  }
}

await serve();
// Stdin closed: the run is over. Exit even if a rule left a timer or handle open.
process.exit(0);
