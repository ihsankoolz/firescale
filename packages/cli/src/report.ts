import type { Category, Diagnostic, RuleStatus, Severity } from '@firescale/sdk';

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

/**
 * The `fscale check --json` document, as produced by the `fscale` binary.
 * Described by schemas/check-output.schema.json.
 */
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
