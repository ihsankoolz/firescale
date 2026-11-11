import {
  checkRule,
  FirescaleError,
  type Category,
  type Diagnostic,
  type RuleStatus,
  type Severity,
} from '@firescale/sdk';
import { type ConfiguredRule, type ResolvedConfig } from './config.js';

/** Version of the `fscale check --json` format. Bumped on any breaking change. */
export const CHECK_SCHEMA_VERSION = 1;

export interface RuleReport {
  id: string;
  status: Exclude<RuleStatus, 'off'>;
  severity: Severity;
  category: Category;
  description: string;
  /** Number of diagnostics this rule produced. */
  violations: number;
}

export interface ReportedDiagnostic extends Diagnostic {
  status: Exclude<RuleStatus, 'off'>;
  severity: Severity;
}

/** The `fscale check --json` document. Described by schemas/check-output.schema.json. */
export interface CheckReport {
  schemaVersion: typeof CHECK_SCHEMA_VERSION;
  /** firescale version that produced the report. */
  version: string;
  /** False when at least one enforced rule has a violation. */
  ok: boolean;
  summary: {
    rules: number;
    violations: number;
    enforcedViolations: number;
  };
  rules: RuleReport[];
  diagnostics: ReportedDiagnostic[];
}

function globToRegExp(glob: string): RegExp {
  const pattern = glob
    .split(/(\*\*|\*)/)
    .map((part) =>
      part === '**' ? '.*' : part === '*' ? '[^/]*' : part.replace(/[.+?^${}()|[\]\\]/g, '\\$&'),
    )
    .join('');
  return new RegExp(`^${pattern}$`);
}

/**
 * Picks the rules to run: not `off`, and matching the `--rules` globs when
 * given. `*` matches within one id segment, `**` matches across `/`. Several
 * globs can be separated by commas.
 */
export function selectRules(
  rules: readonly ConfiguredRule[],
  rulesGlob: string | undefined,
): ConfiguredRule[] {
  const active = rules.filter((entry) => entry.status !== 'off');
  if (rulesGlob === undefined) return active;
  const globs = rulesGlob
    .split(',')
    .map((glob) => glob.trim())
    .filter((glob) => glob !== '');
  if (globs.length === 0) {
    throw new FirescaleError('--rules needs a rule id or glob, for example --rules "core/*".');
  }
  const matchers = globs.map(globToRegExp);
  const matches = (entry: ConfiguredRule): boolean =>
    matchers.some((matcher) => matcher.test(entry.rule.id));
  const selected = active.filter(matches);
  if (selected.length === 0) {
    const off = rules.filter((entry) => entry.status === 'off' && matches(entry));
    if (off.length > 0) {
      throw new FirescaleError(
        `--rules "${rulesGlob}" only matches rules that are off: ${off.map((e) => e.rule.id).join(', ')}. Set them to "evaluated" or "enforced" in your firescale config.`,
      );
    }
    const known = rules.map((entry) => entry.rule.id).join(', ') || '(none)';
    throw new FirescaleError(`--rules "${rulesGlob}" matches no rule. Available rules: ${known}.`);
  }
  return selected;
}

export interface CheckInput {
  root: string;
  config: ResolvedConfig;
  version: string;
  rulesGlob?: string | undefined;
}

/** Runs the selected rules and builds the report. */
export async function runCheck(input: CheckInput): Promise<CheckReport> {
  const selected = selectRules(input.config.rules, input.rulesGlob);
  const rules: RuleReport[] = [];
  const diagnostics: ReportedDiagnostic[] = [];

  for (const { rule, status, options } of selected) {
    if (status === 'off') continue;
    let found: Diagnostic[];
    try {
      found = await checkRule(rule, { root: input.root, options });
    } catch (error) {
      if (error instanceof FirescaleError) throw error;
      const reason = error instanceof Error ? error.message : String(error);
      throw new FirescaleError(
        `Rule "${rule.id}" crashed: ${reason}. This is a bug in the rule. Set it to "off" in your firescale config to continue, and report it.`,
      );
    }
    rules.push({
      id: rule.id,
      status,
      severity: rule.severity,
      category: rule.category,
      description: rule.description,
      violations: found.length,
    });
    for (const diagnostic of found) {
      diagnostics.push({ ...diagnostic, status, severity: rule.severity });
    }
  }

  const enforcedViolations = diagnostics.filter((d) => d.status === 'enforced').length;
  return {
    schemaVersion: CHECK_SCHEMA_VERSION,
    version: input.version,
    ok: enforcedViolations === 0,
    summary: { rules: rules.length, violations: diagnostics.length, enforcedViolations },
    rules,
    diagnostics,
  };
}

function plural(count: number, word: string): string {
  return `${count} ${word}${count === 1 ? '' : 's'}`;
}

const FIX_HINT: Readonly<Record<Diagnostic['fixable'], string>> = {
  deterministic: 'deterministic fix available',
  agent: 'agent-fixable',
  manual: 'manual fix',
};

/** Renders a report for people. Plain text, no colour, forward-slash paths. */
export function formatHuman(report: CheckReport): string {
  const lines: string[] = [];
  for (const rule of report.rules) {
    if (rule.violations === 0) continue;
    lines.push(`${rule.id}  (${rule.status}, ${rule.severity})`);
    for (const diagnostic of report.diagnostics) {
      if (diagnostic.ruleId !== rule.id) continue;
      const line = diagnostic.range ? `:${diagnostic.range.startLine}` : '';
      lines.push(`  ${diagnostic.file}${line}`);
      lines.push(`    ${diagnostic.message} [${FIX_HINT[diagnostic.fixable]}]`);
    }
    lines.push(`  How to fix: fscale explain ${rule.id}`);
    lines.push('');
  }

  const { summary } = report;
  const checked = `${plural(summary.rules, 'rule')} checked`;
  if (summary.violations === 0) {
    lines.push(`OK: ${checked}, no violations.`);
  } else {
    const reportOnly = summary.violations - summary.enforcedViolations;
    const verdict = report.ok ? 'OK' : 'FAILED';
    lines.push(
      `${verdict}: ${checked}, ${plural(summary.violations, 'violation')} (${summary.enforcedViolations} enforced, ${reportOnly} report-only).`,
    );
    if (report.ok) {
      lines.push(
        'Report-only violations do not fail the run. Set a rule to "enforced" to make it fail.',
      );
    }
  }
  return `${lines.join('\n')}\n`;
}

/** Renders a report as the documented JSON format. */
export function formatJson(report: CheckReport): string {
  return `${JSON.stringify(report, null, 2)}\n`;
}
