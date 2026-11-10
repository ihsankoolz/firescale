import { readFile } from 'node:fs/promises';
import { FirescaleError, parseSkill } from '@firescale/sdk';
import type { ConfiguredRule, ResolvedConfig } from './config.js';

/** Finds a rule by full id, or by its short name when that is unambiguous. */
export function findRule(config: ResolvedConfig, query: string): ConfiguredRule {
  const exact = config.rules.find((entry) => entry.rule.id === query);
  if (exact) return exact;
  const byShortName = config.rules.filter((entry) => entry.rule.id.split('/')[1] === query);
  const only = byShortName[0];
  if (only && byShortName.length === 1) return only;
  const known = config.rules.map((entry) => entry.rule.id).join(', ') || '(none)';
  throw new FirescaleError(`Unknown rule "${query}". Available rules: ${known}.`);
}

/** Builds the text for `fscale explain <rule>`: the rule's metadata, then its SKILL.md body. */
export async function explainRule(entry: ConfiguredRule): Promise<string> {
  const { rule, status, skillFile } = entry;
  let text: string;
  try {
    text = await readFile(skillFile, 'utf8');
  } catch {
    throw new FirescaleError(
      `The SKILL.md for "${rule.id}" is missing from this install. Reinstall firescale and try again.`,
    );
  }
  const body = (parseSkill(text)?.body ?? text.replaceAll('\r\n', '\n')).trim();
  const header = [
    `Rule:        ${rule.id}`,
    `Severity:    ${rule.severity}`,
    `Category:    ${rule.category}`,
    `Status:      ${status}`,
    `Fix:         ${rule.fix ? 'deterministic fix available' : 'no deterministic fix'}`,
    `Description: ${rule.description}`,
  ];
  return `${header.join('\n')}\n\n${body}\n`;
}
