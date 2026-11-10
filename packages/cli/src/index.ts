/**
 * Public API of the `firescale` package, for config files and rule authors:
 *
 * ```ts
 * import { defineConfig } from 'firescale';
 * export default defineConfig({ rules: { 'core/no-committed-env': 'enforced' } });
 * ```
 */
export {
  defineConfig,
  definePack,
  defineRule,
  type Diagnostic,
  type Finding,
  type FirescaleConfig,
  type FixContext,
  type FixResult,
  type Rule,
  type RuleConfig,
  type RuleContext,
  type RulePack,
  type RuleStatus,
} from '@firescale/sdk';
export {
  CHECK_SCHEMA_VERSION,
  type CheckReport,
  type ReportedDiagnostic,
  type RuleReport,
} from './check.js';
