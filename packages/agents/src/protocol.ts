/**
 * The runner's side of the adapter protocol (ADR 0047): what `fscale fix
 * --agent` writes to the request file and reads back from the adapter's
 * last line of output. Field names match the Rust structs in
 * `crates/core/src/agentrun.rs`.
 */

export interface Policy {
  allowedTools: string[];
  deniedCommands: string[];
  deniedPaths: string[];
  maxTurns: number;
  maxCostUsd: number;
  timeoutSecs: number;
  loadProjectConfig: boolean;
}

export interface Request {
  cwd: string;
  rule: string;
  skill: string;
  diagnostics: unknown;
  prompt: string;
  policy: Policy;
}

export interface AgentResult {
  ok: boolean;
  costUsd?: number;
  turns?: number;
  error?: string;
}

/** How an adapter starts its agent for one request. */
export interface Invocation {
  args: string[];
  /** Written to the agent's standard input. */
  stdin: string;
}

/** One agent CLI behind the protocol. */
export interface Adapter {
  /** The `--agent` name. */
  name: string;
  /** The program on the PATH. */
  program: string;
  /** Arguments that print the agent's version. */
  versionArgs: string[];
  /** Arguments that exit 0 when the agent is signed in. */
  authArgs: string[];
  /** What was not confirmed from the agent's own docs or source. */
  unverified: string[];
  invocation(request: Request): Invocation;
  /** The result, from the agent's standard output and exit code. */
  parse(stdout: string, code: number): AgentResult;
}
