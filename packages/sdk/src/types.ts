/** How bad a violation is. `blocker` is the most severe. */
export type Severity = 'blocker' | 'high' | 'medium' | 'low';
export const SEVERITIES: readonly Severity[] = ['blocker', 'high', 'medium', 'low'];

/** What kind of problem a rule guards against. */
export type Category = 'security' | 'reliability' | 'consistency' | 'maintainability';
export const CATEGORIES: readonly Category[] = [
  'security',
  'reliability',
  'consistency',
  'maintainability',
];

/**
 * Who can fix a violation:
 * - `deterministic`: the rule's own `fix()` handles it.
 * - `agent`: a coding agent can, following the rule's SKILL.md.
 * - `manual`: a human has to.
 */
export type Fixable = 'deterministic' | 'agent' | 'manual';
export const FIXABLE: readonly Fixable[] = ['deterministic', 'agent', 'manual'];

/** An inclusive, 1-based line range. */
export interface LineRange {
  startLine: number;
  endLine: number;
}

/** What a rule's `check()` returns for each violation. The engine adds the rule id. */
export interface Finding {
  message: string;
  /** Path relative to the repo root. Forward slashes. */
  file: string;
  range?: LineRange;
  /** Name of the project the file belongs to, when known. */
  project?: string;
  fixable: Fixable;
}

/** One violation of one rule. */
export interface Diagnostic extends Finding {
  /** Full rule id, `<pack>/<rule>`. */
  ruleId: string;
}

/** What a rule's `fix()` returns. */
export interface FixResult {
  /**
   * Repo-relative paths the fix changed, including files it removed from the
   * git index. Must be empty when there was nothing left to fix.
   */
  changedFiles: string[];
  /** Things the fix could not do, for a human or agent to follow up on. */
  notes?: string[];
}

/** A project inside the repo. Filled in by stack detection from M3. */
export interface Project {
  name: string;
  /** Repo-relative root of the project. Forward slashes. */
  root: string;
}

/** Detected facts about the repo. Minimal until M3 adds stack detection. */
export interface RepoFacts {
  packageManager: string | null;
  projects: readonly Project[];
}

export type RuleOptions = Readonly<Record<string, unknown>>;

/** Read access to files under the repo root. Paths are repo-relative. */
export interface FileReader {
  exists(path: string): Promise<boolean>;
  /** Reads a file as UTF-8 text. */
  read(path: string): Promise<string>;
}

/** Write access, only available to `fix()`. */
export interface FileWriter extends FileReader {
  /** Writes UTF-8 text, creating parent folders as needed. */
  write(path: string, content: string): Promise<void>;
}

/** Read access to git state. */
export interface GitReader {
  /** False when the root is not inside a git work tree. */
  isRepo(): Promise<boolean>;
  /** Repo-relative paths in the git index, sorted. Empty outside a git repo. */
  trackedFiles(): Promise<string[]>;
}

/** Git operations available to `fix()`. */
export interface GitWriter extends GitReader {
  /** Removes paths from the index and keeps the files on disk. */
  untrack(paths: readonly string[]): Promise<void>;
}

/** What `check()` receives. */
export interface RuleContext {
  /** Absolute path of the repo root. */
  readonly root: string;
  /** This rule's options from the config file. */
  readonly options: RuleOptions;
  readonly facts: RepoFacts;
  readonly files: FileReader;
  readonly git: GitReader;
}

/** What `fix()` receives. */
export interface FixContext extends RuleContext {
  readonly files: FileWriter;
  readonly git: GitWriter;
}

/** A rule definition, as passed to `defineRule`. */
export interface Rule {
  /** `<pack>/<rule>`, lowercase letters, digits and hyphens. */
  id: string;
  severity: Severity;
  category: Category;
  /** One sentence stating what must be true. */
  description: string;
  /** Globs of the files the check depends on. Used for cache hashing from M2. */
  inputs: readonly string[];
  /**
   * Optional. Ids of rules whose fixes must run before this rule's fix, for
   * example when this fix edits a file another rule's fix creates. It only
   * orders fixes. An id that no rule has, or a cycle, is an error.
   */
  after?: readonly string[];
  check: (ctx: RuleContext) => Promise<Finding[]> | Finding[];
  /** Optional. Must be deterministic and idempotent. */
  fix?: (ctx: FixContext, diagnostics: readonly Diagnostic[]) => Promise<FixResult> | FixResult;
}

/** A rule together with the location of its SKILL.md. */
export interface PackEntry {
  rule: Rule;
  skillFile: URL;
}

/** A named set of rules. */
export interface RulePack {
  /** Must equal the `<pack>` part of every rule id in it. */
  name: string;
  rules: readonly PackEntry[];
}

/** `off`: not run. `evaluated`: reported only. `enforced`: a violation fails the run. */
export type RuleStatus = 'off' | 'evaluated' | 'enforced';
export const RULE_STATUSES: readonly RuleStatus[] = ['off', 'evaluated', 'enforced'];

export interface RuleConfig {
  status?: RuleStatus;
  options?: Record<string, unknown>;
}

/** The shape of `firescale.config.ts` / `firescale.config.json`. */
export interface FirescaleConfig {
  /** Rule packs to turn on. Default: `['core']`. */
  packs?: string[];
  /** Per-rule status or `{ status, options }`, keyed by rule id. Default status: `evaluated`. */
  rules?: Record<string, RuleStatus | RuleConfig>;
  /** Globs an agent fix run may never change. Used by the diff guard from M5. */
  protectedPaths?: string[];
  /** Settings for `fscale verify`. See docs/verify.md. */
  verify?: VerifyConfig;
}

/**
 * A time limit: a number of seconds, a text such as `"90s"`, `"10m"`, `"1h"`
 * or `"1h 30m"`, or `false` for no limit. The default is 10 minutes a step.
 */
export type VerifyTimeout = number | string | false;

/** A verify step with a command and a time limit; either may be left out. */
export interface VerifyStepObject {
  command?: string;
  timeout?: VerifyTimeout;
}

/**
 * A verify step's setting: the command to run, `false` to turn the step off,
 * or an object with a `command` and a `timeout`.
 */
export type VerifyStepSetting = string | false | VerifyStepObject;

/** The steps `fscale verify` runs after `check`. */
export interface VerifySteps {
  lint?: VerifyStepSetting;
  typecheck?: VerifyStepSetting;
  test?: VerifyStepSetting;
}

/** A project's settings under `verify.projects`. */
export interface VerifyProjectSettings extends VerifySteps {
  /** The time limit of each of the project's steps. */
  timeout?: VerifyTimeout;
}

/** The `verify` section of the config. The top-level steps apply to the whole repo. */
export interface VerifyConfig extends VerifySteps {
  /** `true` makes a step that was skipped for lack of a command fail the run. Default `false`. */
  failOnSkipped?: boolean;
  /** The time limit of every step, unless a more specific one is set. Default `"10m"`. */
  timeout?: VerifyTimeout;
  /** `false`: do not set `CI=1` for the commands. Default `true`. */
  ci?: boolean;
  /**
   * `false`: a step whose output shows that a local service it needs is not
   * running runs on to its end. Default `true`: it is stopped as soon as that
   * is clear (ADR 0025).
   */
  stopWhenServiceDown?: boolean;
  /**
   * Errors a test step may report, with no test passed and none failed,
   * before it is stopped as likely setup; `false` never stops it. Default 10
   * (ADR 0035).
   */
  stopAfterErrors?: number | false;
  /**
   * How long the base run of blame-aware verify may take in all; `false` for
   * no limit. Default `"5m"` (ADR 0037).
   */
  baseBudget?: VerifyTimeout;
  /**
   * A command that installs dependencies in the base commit's worktree, run
   * at its root before the base steps. Default: none; the head's
   * `node_modules` and virtual environments are linked instead (ADR 0037).
   */
  baseInstall?: string | false;
  /** Per project, keyed by its folder: `false` turns it off, an object sets single steps. */
  projects?: Record<string, false | VerifyProjectSettings>;
}
