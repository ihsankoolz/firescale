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
  FirescaleError,
  type Diagnostic,
  type Finding,
  type FirescaleConfig,
  type VerifyConfig,
  type VerifySteps,
  type VerifyStepSetting,
  type VerifyStepObject,
  type VerifyProjectSettings,
  type VerifyTimeout,
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
} from './report.js';
