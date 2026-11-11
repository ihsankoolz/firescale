# firescale — Spec

> Fire scale is the rough crust that forms on hot metal in a forge. A smith knocks it off to show clean work.
> `firescale` knocks the scale off a repo, and proves it's clean.

CLI command: `fscale` (long form: `firescale`).

## 1. What it is

A free, stack-agnostic conformance and verify engine.

- Every rule ships three things: a **check**, an optional **deterministic fix**, and a portable **SKILL.md**.
- Any coding agent (Claude Code, Codex, Gemini CLI, Cursor) can use the skill to fix what the deterministic fix can't.
- One `verify` command is the "definition of done" that humans, CI and agents all call.

## 2. Who it is for

- Small teams and student group projects with mixed experience.
- Solo developers who start many repos and want the same standards in each.
- Teams that use AI coding agents and want them to finish properly.

## 3. Principles

1. **Works on any repo.** Nx, Turborepo or neither. No adoption needed from teammates to run a check.
2. **Deterministic first.** An agent only handles what code can't fix safely.
3. **Agent-agnostic.** Everything is reachable through CLI JSON output. Agents differ only in thin adapters.
4. **Adopt standards, don't invent them.** AGENTS.md, SKILL.md (agentskills.io), MCP, SARIF.
5. **Safe by default.** Agents run with allow-lists, deny-lists, budgets, and a diff guard. Nothing is trusted on the agent's word.
6. **Measured.** Speed and fix success are benchmarked and reported.
7. **Cross-platform.** Windows, macOS and Linux are all first-class.
8. **Native front door.** `fscale` is one Rust binary. Built-in rules run with no Node process, so a check starts in milliseconds. Node is started only when a TypeScript rule, a `.ts` config, a generator or an agent adapter is needed.

## 4. Commands

Every command is served by the native `fscale` binary. Commands marked **Node** start a Node process for part of their work; the rest never do, unless the repo uses a `firescale.config.ts` or custom TypeScript rules.

### Stage 1 — conform and verify

| Command | Purpose |
|---|---|
| `fscale check [--affected] [--rules <glob>] [--json\|--sarif]` | Run rules, report violations. Exit 1 if an enforced rule fails. A rule named exactly in `--rules` runs even when it is `off`, with a note. |
| `fscale fix [--dry-run] [--rules <glob>]` | Apply deterministic fixes. |
| `fscale fix --agent <claude\|codex\|gemini> [--rule <id>]` | Hand one rule's remaining violations to an agent, with guardrails. **Node** (agent adapter). |
| `fscale explain <rule>` | Print what a rule checks, why, and how to fix it by hand. |
| `fscale verify [--affected] [--json]` | Conformance, then lint, typecheck and tests. One compact failure summary. |
| `fscale hooks install [--agent <name>\|--all]` | Write agent hook config that calls `fscale verify` before the agent may stop. |

### Stage 2 — init, add, sync

| Command | Purpose |
|---|---|
| `fscale init [--agents]` | Detect the stack, write config, optionally set up agent skills, hooks and permissions. **Node** (generators). |
| `fscale add <generator> [--json <input>] [--dry-run]` | Scaffold an app, package or service. **Node** (generators). |
| `fscale schema <generator>` | Print a generator's input JSON schema (so agents can call it correctly). **Node** (generators). |
| `fscale sync [--check]` | Regenerate CI workflow and the managed AGENTS.md block. `--check` fails on drift. **Node** (generators). |
| `fscale facts [--json]` | Print detected repo facts: projects, package manager, commands, project graph. |

### Stage 3 — later

- `fscale mcp`: a thin MCP server over the same JSON.
- Nx plugins for gaps only (EAS build/submit, Supabase migrations).

## 5. Rule format

There are two kinds of rule. Both produce the same diagnostics, appear in the same report and ship the same SKILL.md.

### Built-in rules (Rust)

Core rule packs are written in Rust and compiled into the `fscale` binary. A rule is a folder:

```
crates/rules/src/<pack>/<rule-id>/
  mod.rs           # metadata, check(), optional fix()
  SKILL.md         # agentskills.io format, embedded in the binary
  tests.rs         # fixtures: passing repo, failing repo, fixed repo
```

### Custom rules (TypeScript)

Teams write their own rules with the TypeScript SDK. A rule is a folder inside a pack:

```
<pack>/rules/<rule-id>/
  rule.ts          # metadata, check(), optional fix()
  SKILL.md         # agentskills.io format
  rule.test.ts     # fixtures: passing repo, failing repo, fixed repo
```

`rule.ts` shape (API fixed in Milestone 1, see ADR 0007):

```ts
export default defineRule({
  id: 'team/no-todo-in-main',
  severity: 'medium',             // blocker | high | medium | low
  category: 'maintainability',    // security | reliability | consistency | maintainability
  description: 'No TODO comments in src/main.',
  inputs: ['src/main/**'],        // globs used for cache hashing
  check: async (ctx) => Finding[],
  fix:   async (ctx, diagnostics) => FixResult,   // optional, deterministic, idempotent
});
```

A custom pack is listed in the config by path (`packs: ['core', './tools/rules/pack.ts']`). The binary starts Node once per run to load and run custom rules.

### Shared contract

- `ctx` gives read access to files, git state, and `facts` (detected stack and project graph).
- A `Diagnostic` has: rule id, message, file, optional line range, optional project, and `fixable: 'deterministic' | 'agent' | 'manual'`.
- `fix()` must be idempotent. Running it twice changes nothing the second time.

`SKILL.md` sections:

1. Frontmatter per the agentskills.io spec (name, description). No custom fields.
2. **What this rule enforces and why** (for humans).
3. **Manual fix** (for humans).
4. **Agent fix instructions**, split into:
   - `DETERMINISTIC` steps: the agent applies these.
   - `REPORT` items: the agent only reports these; it must not act.
5. **Verify**: the exact command to confirm the fix (`fscale check --rules <id>`).

## 6. Starter rule packs

**core** (Milestone 3):

1. `no-committed-env` — no `.env*` files tracked (except `.env.example`).
2. `gitignore-basics` — `.gitignore` exists and covers the detected stack.
3. `single-lockfile` — exactly one lockfile, matching the declared package manager.
4. `ci-exists` — a CI workflow exists and runs on pull requests.
5. `readme-setup` — README has install and run steps.
6. `env-example` — if code reads env vars, `.env.example` lists them.
7. `scripts-present` — standard scripts exist: lint, test, build (per detected stack).
8. `no-secrets-in-source` — no obvious keys or tokens in tracked files.

**agent-ready** (Milestone 4): AGENTS.md exists, commands are documented, tests run with one command, verify hook installed.

Later: `expo`, `supabase`, `nextjs`, `python`, `group-project`.

## 7. Agent runner guardrails

`fscale fix --agent` must do all of this:

1. **Pre-flight.** Agent CLI is installed and authenticated. Working tree is clean. Refuse to start if secrets are tracked.
2. **Isolation.** Work on a fresh branch or git worktree. Never on the user's current branch.
3. **Scoped prompt.** Point the agent at one rule's SKILL.md and that rule's diagnostics only.
4. **Tool limits.** Strict allow-list of tools. Deny-list for secrets: `.env*`, `*.pem`, `*.key`, `secrets/**`, and similar.
5. **Budget limits.** Cap turns and spend on every run.
6. **Controlled config.** Do not load the target repo's own agent hooks or MCP servers during a fix run.
7. **Post-run diff guard.** Reject the run if protected paths changed, or files outside the rule's allowed globs changed.
8. **Re-check.** Run the rule's check again. Pass means commit; fail means roll back.
9. **One rule per commit**, with a commit trailer recording rule id, agent and outcome.
10. **Ledger.** Append every run to `.firescale/ledger.jsonl`: rule, agent, pass/fail, diff size, duration.

Each agent sits behind an adapter interface. Exact CLI flags must be confirmed against that agent's current docs when the adapter is written, and pinned in tests.

## 8. Verify and hooks

- `fscale verify` runs, in order: `check`, lint, typecheck, test.
- It uses `nx affected` or `turbo` when present. Otherwise it falls back to package scripts.
- Output is one compact summary written for an agent to act on: what failed, where, and the command to re-run.
- `fscale hooks install` writes hook config for each supported agent so the agent can't stop while verify fails.
- Hooks must guard against loops: detect a repeated stop attempt and cap retries.

## 9. Architecture

```
firescale/
  crates/
    core/            # Rust: engine library. Diagnostics model, config, git and file access,
                     #       rule trait, reports, Node bridge; later walker, hashing, cache, scheduler
    rules/           # Rust: built-in rule packs (core, agent-ready)
    cli/             # Rust: the fscale binary
  packages/
    cli/             # TypeScript: the `firescale` npm package. Node host for the bridge,
                     #             defineConfig re-exports, npm launcher
    sdk/             # TypeScript: defineRule, types, test helpers for custom rules
    agents/          # TypeScript: agent adapters
    generators/      # TypeScript: init/add/sync (Stage 2)
  bench/             # benchmark harness and fixture repos
  docs/
```

**Rust owns:**

- The `fscale` command: argument parsing, config, rule selection, output and exit codes.
- Built-in rules and their deterministic fixes.
- Parallel, gitignore-aware file walking.
- Glob matching and content hashing.
- Rule-result cache, keyed by rule version + hash of the rule's input files + config.
- Parallel scheduling of rule checks.
- The diagnostics data model, with JSON and SARIF serialisation.
- `verify`, hooks, and the agent runner's guardrails.

**TypeScript owns:**

- The rule SDK for custom rules, generators, agent adapters, Nx plugins.

**Bridge:** when a run needs TypeScript, the binary starts one Node child process and exchanges JSON lines over its stdin and stdout (ADR 0008). A repo with only built-in rules and a JSON config never starts Node.

**Distribution:** installed through npm. The `firescale` package depends on one prebuilt binary package per platform, so `npx firescale` works with no Rust toolchain on the user's machine.

**Project graph:** read from Nx when present. Otherwise build a simple one from workspace files (pnpm/npm/yarn workspaces, Cargo workspace, uv workspace).

## 10. Config

One file at the repo root: `firescale.config.ts` (or `.json`).

- Which rule packs are on.
- Per-rule status: `off`, `evaluated` (report only), `enforced` (fails CI).
- Per-rule options.
- Protected paths for the agent diff guard.

Default status is `evaluated`, so adopting the tool never breaks a build on day one.

## 11. Metrics to track from day one

- Cold vs warm `fscale check` time on a large fixture repo.
- Cache hit rate.
- p50 and p99 `fscale verify` latency (it runs inside agent hooks, so it must be fast).
- Agent fix success: share of agent runs that pass the re-check.

## 12. Out of scope

- Not a task runner. Nx and Turborepo do that.
- Not a multi-agent rule-file syncer. rulesync and ruler do that; we can emit input for them.
- Not a skill installer. `npx skills` does that.
- Not a standalone readiness score. It's just the `agent-ready` rule pack.
- No agent jobs on fork pull requests, ever.

## 13. Originality rule

- This project is inspired by publicly documented ideas (Nx plugins, Nx Conformance, Agent Skills).
- No code, rule text, names, or docs may be copied from any private or employer codebase.
- Everything is written fresh from public documentation.

## 14. Licence

MIT.
