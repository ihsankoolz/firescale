import { type Change, type ContextInit, createContext } from './context.js';
import { FirescaleError } from './errors.js';
import { normalizeRelative } from './paths.js';
import { FIXABLE, type Diagnostic, type Finding, type FixResult, type Rule } from './types.js';

function toDiagnostic(rule: Rule, finding: Finding): Diagnostic {
  const where = `Rule "${rule.id}" returned an invalid finding`;
  if (typeof finding.message !== 'string' || finding.message === '') {
    throw new FirescaleError(`${where}: message is missing. Fix the rule's check().`);
  }
  if (typeof finding.file !== 'string' || finding.file === '') {
    throw new FirescaleError(`${where}: file is missing. Fix the rule's check().`);
  }
  if (!FIXABLE.includes(finding.fixable)) {
    throw new FirescaleError(`${where}: fixable must be one of ${FIXABLE.join(', ')}.`);
  }
  const diagnostic: Diagnostic = {
    ruleId: rule.id,
    message: finding.message,
    file: normalizeRelative(finding.file),
    fixable: finding.fixable,
  };
  if (finding.range) {
    const { startLine, endLine } = finding.range;
    if (
      !Number.isInteger(startLine) ||
      !Number.isInteger(endLine) ||
      startLine < 1 ||
      endLine < startLine
    ) {
      throw new FirescaleError(`${where}: range must be 1-based with endLine >= startLine.`);
    }
    diagnostic.range = { startLine, endLine };
  }
  if (finding.project !== undefined) diagnostic.project = finding.project;
  return diagnostic;
}

function compareDiagnostics(a: Diagnostic, b: Diagnostic): number {
  if (a.file !== b.file) return a.file < b.file ? -1 : 1;
  const lineA = a.range?.startLine ?? 0;
  const lineB = b.range?.startLine ?? 0;
  if (lineA !== lineB) return lineA - lineB;
  return a.message < b.message ? -1 : a.message > b.message ? 1 : 0;
}

/** Runs one rule's check and returns its diagnostics in a stable order. */
export async function checkRule(rule: Rule, init: ContextInit): Promise<Diagnostic[]> {
  const findings = await rule.check(createContext(init));
  return findings.map((finding) => toDiagnostic(rule, finding)).sort(compareDiagnostics);
}

/**
 * Runs one rule's deterministic fix on its current diagnostics. Returns an
 * empty result when the rule has no fix or nothing is deterministically fixable.
 */
export async function fixRule(rule: Rule, init: ContextInit): Promise<FixResult> {
  return (await fixRuleWithChanges(rule, init)).result;
}

/** The result of running one rule's fix: what it reported and what it changed. */
export interface FixOutcome {
  result: FixResult;
  changes: Change[];
  /** How many deterministic violations the fix was asked to repair. */
  attempted: number;
}

/**
 * Like `fixRule`, and also returns every change. With `dryRun`, the changes
 * are worked out but no file is written and the git index is not touched.
 */
export async function fixRuleWithChanges(
  rule: Rule,
  init: ContextInit,
  options: { dryRun?: boolean } = {},
): Promise<FixOutcome> {
  if (!rule.fix) return { result: { changedFiles: [] }, changes: [], attempted: 0 };
  const diagnostics = (await checkRule(rule, init)).filter((d) => d.fixable === 'deterministic');
  if (diagnostics.length === 0) {
    return { result: { changedFiles: [] }, changes: [], attempted: 0 };
  }
  const changes: Change[] = [];
  const context = createContext(init, {
    dryRun: options.dryRun ?? false,
    record: (change) => changes.push(change),
  });
  const result = await rule.fix(context, diagnostics);
  const changedFiles = [...new Set(result.changedFiles.map(normalizeRelative))].sort();
  return {
    result: result.notes ? { changedFiles, notes: result.notes } : { changedFiles },
    changes,
    attempted: diagnostics.length,
  };
}
