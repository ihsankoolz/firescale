import { FirescaleError } from './errors.js';
import {
  CATEGORIES,
  SEVERITIES,
  type FirescaleConfig,
  type PackEntry,
  type Rule,
  type RulePack,
} from './types.js';

const RULE_ID = /^[a-z0-9]+(?:-[a-z0-9]+)*\/[a-z0-9]+(?:-[a-z0-9]+)*$/;
const PACK_NAME = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;

/** Validates a rule definition and returns it frozen. */
export function defineRule(rule: Rule): Readonly<Rule> {
  const where = `defineRule(${JSON.stringify(rule.id)})`;
  if (typeof rule.id !== 'string' || !RULE_ID.test(rule.id)) {
    throw new FirescaleError(
      `${where}: invalid rule id. Use "<pack>/<rule>" with lowercase letters, digits and hyphens, for example "core/no-committed-env".`,
    );
  }
  if (!SEVERITIES.includes(rule.severity)) {
    throw new FirescaleError(`${where}: severity must be one of ${SEVERITIES.join(', ')}.`);
  }
  if (!CATEGORIES.includes(rule.category)) {
    throw new FirescaleError(`${where}: category must be one of ${CATEGORIES.join(', ')}.`);
  }
  if (typeof rule.description !== 'string' || rule.description.trim() === '') {
    throw new FirescaleError(`${where}: add a one-sentence description.`);
  }
  if (!Array.isArray(rule.inputs) || rule.inputs.some((glob) => typeof glob !== 'string')) {
    throw new FirescaleError(`${where}: inputs must be an array of glob strings.`);
  }
  if (rule.after !== undefined) {
    if (!Array.isArray(rule.after) || rule.after.some((id) => typeof id !== 'string')) {
      throw new FirescaleError(`${where}: after must be an array of rule ids.`);
    }
    const bad = rule.after.find((id) => !RULE_ID.test(id));
    if (bad !== undefined) {
      throw new FirescaleError(
        `${where}: after has an invalid rule id ${JSON.stringify(bad)}. Use "<pack>/<rule>", for example "core/gitignore-basics".`,
      );
    }
    if (rule.after.includes(rule.id)) {
      throw new FirescaleError(`${where}: after must not list the rule itself.`);
    }
  }
  if (typeof rule.check !== 'function') {
    throw new FirescaleError(`${where}: check must be a function.`);
  }
  if (rule.fix !== undefined && typeof rule.fix !== 'function') {
    throw new FirescaleError(`${where}: fix must be a function when present.`);
  }
  return Object.freeze({
    ...rule,
    inputs: Object.freeze([...rule.inputs]),
    ...(rule.after === undefined ? {} : { after: Object.freeze([...rule.after]) }),
  });
}

/** Validates a rule pack and returns it frozen. */
export function definePack(pack: RulePack): Readonly<RulePack> {
  if (typeof pack.name !== 'string' || !PACK_NAME.test(pack.name)) {
    throw new FirescaleError(
      `definePack(${JSON.stringify(pack.name)}): pack names use lowercase letters, digits and hyphens.`,
    );
  }
  const seen = new Set<string>();
  for (const { rule } of pack.rules) {
    if (!rule.id.startsWith(`${pack.name}/`)) {
      throw new FirescaleError(
        `definePack("${pack.name}"): rule "${rule.id}" belongs to another pack. Rename it to "${pack.name}/<rule>".`,
      );
    }
    if (seen.has(rule.id)) {
      throw new FirescaleError(`definePack("${pack.name}"): rule "${rule.id}" is listed twice.`);
    }
    seen.add(rule.id);
  }
  const rules: readonly PackEntry[] = Object.freeze(pack.rules.map((e) => Object.freeze({ ...e })));
  return Object.freeze({ name: pack.name, rules });
}

/** Identity helper that gives `firescale.config.ts` type checking and completion. */
export function defineConfig(config: FirescaleConfig): FirescaleConfig {
  return config;
}
