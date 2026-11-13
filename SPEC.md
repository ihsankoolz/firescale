# firescale — Spec

> Fire scale is the rough crust that forms on hot metal in a forge. A smith knocks it off to show clean work.
> `firescale` knocks the scale off a repo, and proves it's clean.

CLI command: `fscale` (long form: `firescale`).

Sections 1 to 14 keep their numbers, because ADRs and code comments cite them. Sections 15 to 17 were added by ADR 0015.

## 1. What it is

A free toolkit for small teams where the rules are the scaffold, every repo is born agent-ready, and a referee keeps agents honest (ADR 0015).

- Every rule ships three things: a **check**, an optional **deterministic fix**, and a portable **SKILL.md**.
- **The rules are the scaffold.** `init` has no templates: it runs the fixes of the enabled rules until `check` passes. The same loop adopts an existing repo and repairs drift.
- Any coding agent (Claude Code, Codex, Gemini CLI, Cursor) can use the skill to fix what the deterministic fix can't.
- One `verify` command is the "definition of done" that humans, CI and agents all call.
- **A referee is built into verify.** It blocks only on failures a change introduced, and it catches attempts to pass by weakening the tests.

firescale is not a standalone agent referee and not a benchmark product. The referee exists to make the toolkit's own "done" trustworthy.

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
9. **One mechanism.** Scaffolding, adoption and drift repair are all "run the rules' fixes until check passes". There is no template engine beside it.
10. **Blame the change, not the repo.** A person or an agent is held to the failures their change introduced, measured against the base commit.

## 4. Commands

Every command is served by the native `fscale` binary. Commands marked **Node** start a Node process for part of their work; the rest never do, unless the repo uses a `firescale.config.ts` or custom TypeScript rules.

### Conform and verify

| Command | Purpose |
|---|---|
| `fscale check [--affected] [--rules <glob>] [--json\|--sarif]` | Run rules, report violations. Exit 1 if an enforced rule fails. A rule named exactly in `--rules` runs even when it is `off`, with a note. |
| `fscale fix [--dry-run] [--rules <glob>]` | Apply deterministic fixes, in dependency order, until nothing is left to fix (§15). |
| `fscale fix --agent <claude\|codex\|gemini> [--rule <id>]` | Hand one rule's remaining violations to an agent, with guardrails. **Node** (agent adapter). |
| `fscale explain <rule>` | Print what a rule checks, why, and how to fix it by hand. |
| `fscale verify [--affected] [--base <ref>] [--json]` | Conformance, then lint, typecheck and tests, judged by the referee (§8) against the base commit. `--affected` runs only the projects a change can affect. One compact failure summary. |
| `fscale hooks install [--agent <name>\|--all]` | Write agent hook config that calls `fscale verify` before the agent may stop. |
| `fscale facts [--json]` | Print detected repo facts: the projects, also those in subfolders, each with its package manager, tools and commands; and the project graph. |
| `fscale graph [--explain] [--json]` | Print the project graph: projects, shared folders and files, and the edges between them. `--explain` names the source of each edge. |

### Scaffold: init, adopt, sync, add

| Command | Purpose |
|---|---|
| `fscale init [--packs <list>]` | Build a new repo: write the config, then run every enabled rule's fix until `check` passes. No templates. Default packs: `core` and `agent-ready`. |
| `fscale adopt [--dry-run]` | The same loop on an existing repo. Never replaces content a person wrote. |
| `fscale sync [--check]` | The same loop again, to repair drift. `--check` changes nothing and fails when something would change, for CI. |
| `fscale add <generator> --json <input> [--dry-run]` | Scaffold an app, package or service from JSON input. **Node** (generators). |
| `fscale schema <generator>` | Print a generator's input JSON schema, so agents can call it correctly. **Node** (generators). |

### Team and ledger

| Command | Purpose |
|---|---|
| `fscale doctor [--json]` | Check a teammate's machine: tool versions, env variables from `.env.example`, installed dependencies. |
| `fscale ledger [--json]` | Summarise the agent fix runs recorded in `.firescale/ledger.jsonl`. |

### Later

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
  after: ['core/gitignore-basics'], // optional: rules whose fixes must run first
  check: async (ctx) => Finding[],
  fix:   async (ctx, diagnostics) => FixResult,   // optional, deterministic, idempotent
});
```

A custom pack is listed in the config by path (`packs: ['core', './tools/rules/pack.ts']`). The binary starts Node once per run to load and run custom rules.

### Shared contract

- `ctx` gives read access to files, git state, and `facts` (detected stack and project graph).
- A `Diagnostic` has: rule id, message, file, optional line range, optional project, and `fixable: 'deterministic' | 'agent' | 'manual'`. A built-in rule may give one diagnostic its own severity, lower or higher than the rule's.
- `fix()` must be idempotent. Running it twice changes nothing the second time.
- A fix may create files. A rule may declare, with `after`, the rules whose fixes must run before its own. A cycle is an error.

`SKILL.md` sections:

1. Frontmatter per the agentskills.io spec (name, description). No custom fields.
2. **What this rule enforces and why** (for humans).
3. **Manual fix** (for humans).
4. **Agent fix instructions**, split into:
   - `DETERMINISTIC` steps: the agent applies these.
   - `REPORT` items: the agent only reports these; it must not act.
5. **Verify**: the exact command to confirm the fix (`fscale check --rules <id>`).

## 6. Rule packs

**core** (built, on by default):

1. `no-committed-env` — no `.env*` files tracked (except `.env.example`).
2. `gitignore-basics` — `.gitignore` exists and covers the detected stack.
3. `single-lockfile` — exactly one lockfile, matching the declared package manager.
4. `ci-exists` — a CI workflow exists and runs on pull requests.
5. `readme-setup` — README has install and run steps.
6. `env-example` — if code reads env vars, the nearest `.env.example` lists them. Variables only tests read are low severity.
7. `scripts-present` — standard scripts exist: lint, test, build (per detected stack).
8. `no-secrets-in-source` — no obvious keys or tokens in tracked files.

**No more hand-written generic hygiene rules will be added beyond these eight.** alint already covers that ground. If alint is installed, firescale may ingest its output and report it as diagnostics; this is optional and never a dependency.

**agent-ready** (built; opt-in for `check` in an existing repo, on by default in `fscale init`): `agents-md` (AGENTS.md with a managed block), `commands-documented`, `single-test-command`, `verify-hook`, `agent-permissions`, `agent-skills`. Their fixes write the agent files of §16 (ADR 0027).

**group-project** (built, opt-in): a pull request template, a branch naming rule, a contributing guide, and a CI workflow that runs `fscale verify` (ADR 0023).

**Stack packs** (planned): `python`, `nextjs`, `supabase`, `expo`. This is where future rule effort goes. A stack pack is a set of rules whose fixes create and repair the files that stack needs, so `fscale init --packs core,<stack>` builds a working starter with no templates.

## 7. Agent runner guardrails and the ledger

`fscale fix --agent` must do all of this:

1. **Pre-flight.** Agent CLI is installed and authenticated. Working tree is clean. Refuse to start if secrets are tracked.
2. **Isolation.** Work on a fresh branch or git worktree. Never on the user's current branch.
3. **Scoped prompt.** Point the agent at one rule's SKILL.md and that rule's diagnostics only.
4. **Tool limits.** Strict allow-list of tools. Deny-list for secrets: `.env*`, `*.pem`, `*.key`, `secrets/**`, and similar.
5. **Budget limits.** Cap turns and spend on every run.
6. **Controlled config.** Do not load the target repo's own agent hooks or MCP servers during a fix run.
7. **Post-run diff guard.** Reject the run if protected paths changed, or files outside the rule's allowed globs changed.
8. **Re-check.** Run the rule's check again, and the referee (§8) over the diff. Pass means commit; fail means roll back.
9. **One rule per commit**, with a commit trailer recording rule id, agent and outcome.
10. **Ledger.** Append every run to `.firescale/ledger.jsonl`: rule, agent, pass/fail, diff size, duration and cost.

`fscale ledger` prints a summary of the ledger: runs, pass rate and cost, per rule and per agent. A public benchmark built from ledgers may come later; it is not planned.

Each agent sits behind an adapter interface. Exact CLI flags must be confirmed against that agent's current docs when the adapter is written, and pinned in tests.

## 8. Verify, hooks and the referee

### Verify and hooks (built, M7)

- `fscale verify` runs, in order: `check`, lint, typecheck, test.
- It uses `nx affected` or `turbo` when present. Otherwise it falls back to package scripts.
- Output is one compact summary written for an agent to act on: what failed, where, and the command to re-run.
- `fscale hooks install` writes hook config for each supported agent so the agent can't stop while verify fails.
- With no workspace file at the root, verify runs each project that `fscale facts` finds in a subfolder: that project's lint, typecheck and test, in its folder (ADR 0020).
- A step with no command is skipped, and verify prints a visible warning that names it. A skipped step is not a failure unless `verify.failOnSkipped` is on, but a run with a skipped step is never reported as a clean pass: the headline says `PARTIAL` and how many steps ran.
- Hooks must guard against loops: detect a repeated stop attempt and cap retries. The hook releases an agent after 3 blocks in a row.

### Affected-only verify (M17)

- `fscale verify --affected [--base <ref>]` runs lint, typecheck and test only for the projects a change can affect: the owners of the changed files and every project that depends on them in the project graph (§9). The others are reported as `not affected`, which is not the same as skipped.
- The headline says how many projects ran out of how many. An affected run never reads as a plain OK for the whole repo.
- When unsure, more is affected, never less. A changed file that no project owns, a root lockfile, a CI workflow, a Compose file or the firescale config affects every project. A dynamic Python import counts as depending on every other Python project, with a warning. With no base and a clean tree there is nothing to compare, so everything is affected, with a note.
- The stop hook runs a full verify until blame-aware verify changes it (M21).

### The step cache (M18, M22)

- A verify step that **passed** is cached on the machine, keyed by the firescale version, the command, the hashes of the project's files and of the projects and shared folders it depends on, the lockfile, the tool versions, the OS and CPU architecture, and an allowlist of environment variables. A failure always runs again.
- A step that needs a network service that is not in the repo is never cached.
- The CI that `init` generates keeps the cache in the GitHub Actions cache. Only pushes to the default branch write it; pull requests and laptops only read it, so a pull request cannot plant a result that the default branch trusts (CVE-2025-36852).

### Blame-aware verify

Each failure gets a label, and only `new` blocks. The labels are on by default whenever there is a base commit:

| Label          | Meaning                                         |
| -------------- | ----------------------------------------------- |
| `new`          | Fails on the head, passed on the base commit.   |
| `pre-existing` | Fails on both.                                  |
| `fixed`        | Failed on the base, passes on the head.         |
| `flaky`        | Failed on the head, passed when run again.      |

- The **head** is the working tree, uncommitted changes included. The **base** is the merge base with `--base <ref>`, or with the default branch.
- The base commit is run in a git worktree. The user's working tree and index are never touched.
- With a base, every step runs, so an old failure in one step cannot hide a new failure in a later one.
- The base run uses the step cache: a project whose inputs are equal on the base and the head needs no base run, and only the projects whose inputs differ are run in the worktree.
- With no base that differs from the head (a clean tree on the default branch, or outside git), every failure blocks, as before.
- A `new` failure is run again before it blocks. **No flaky-test history is kept**: the rerun is all there is.
- The stop hook uses the same labels, so an agent is only sent back for what it broke.

### Tamper guard

Detects a change that passes verify by weakening it:

- tests skipped or deleted, and a net loss of assertions,
- new lint-disable comments or `allow` attributes,
- lowered coverage thresholds,
- CI steps removed or made unable to fail,
- `--no-verify`.

How it works:

- Source is parsed with tree-sitter. Base and head are compared by **net counts**, so moving or renaming a test is not a finding.
- Policy is read from the **base commit**, so a change cannot loosen its own policy.
- Clear cheats fail hard. Changed expected values and changed snapshots need a person's approval instead, and an approval made by the change under review does not count.

Prior art to study before building it: trustbut, goodhart, skylos. firescale's aim is to be syntax-tree based, anchored to the base commit, and to publish a measured false-positive rate.

## 9. Architecture

```
firescale/
  crates/
    core/            # Rust: engine library. Diagnostics model, config, git and file access,
                     #       rule trait, reports, Node bridge; later walker, hashing, cache, scheduler
    rules/           # Rust: built-in rule packs (core, agent-ready, group-project, stack packs)
    cli/             # Rust: the fscale binary
  packages/
    cli/             # TypeScript: the `firescale` npm package. Node host for the bridge,
                     #             defineConfig re-exports, npm launcher
    sdk/             # TypeScript: defineRule, types, test helpers for custom rules
    agents/          # TypeScript: agent adapters
    generators/      # TypeScript: generators for `fscale add` and `fscale schema`
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
- The fix loop behind `init`, `adopt` and `sync`.
- The referee: the base run in a worktree, its cache, and the tamper guard (tree-sitter).
- `doctor` and `ledger`.

**TypeScript owns:**

- The rule SDK for custom rules, generators (`add`, `schema`), agent adapters, Nx plugins.

**Bridge:** when a run needs TypeScript, the binary starts one Node child process and exchanges JSON lines over its stdin and stdout (ADR 0008). A repo with only built-in rules and a JSON config never starts Node.

**Distribution:** installed through npm. The `firescale` package depends on one prebuilt binary package per platform, so `npx firescale` works with no Rust toolchain on the user's machine.

**Project graph:** read from Nx when present. Otherwise build one from workspace files (pnpm/npm/yarn workspaces, Cargo workspace, uv workspace). Projects in subfolders that no workspace file lists are found by their marker files, four folder levels deep (ADR 0020). Edges between projects come, most trusted first, from local paths in manifests, from Dockerfile `COPY` and Compose `build` and `depends_on`, and from Python and JavaScript/TypeScript imports read with tree-sitter. A folder that projects use but that is not a project is a shared-source node; root files such as `.env.example`, Compose files and OpenAPI specs are inputs of the projects that read them (ADR 0032).

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
- Agent fix success: share of agent runs that pass the re-check (from the ledger).
- Tamper guard: false-positive rate and miss rate on a labelled set of changes.
- Blame-aware verify: time added by the base run, with a cold and a warm cache.

## 12. Out of scope

- Not a task runner. Nx and Turborepo do that.
- Not a standalone agent referee or benchmark product. The referee serves `verify`.
- Not a generic hygiene linter beyond the core pack. alint does that.
- No flaky-test history or flaky-test management.
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

## 15. Rules as the scaffold

`init`, `adopt`, `sync` and `fix` are one loop:

1. Run `check`.
2. Run the deterministic fix of every enabled rule that has violations, in the order given by the rules' `after` declarations.
3. Repeat until no deterministic violation is left. A pass that changes nothing while such violations remain is an error (see "Convergent").

Requirements:

- **Idempotent.** Every fix, run twice, changes nothing the second time.
- **Ordered.** Fixes run after the fixes they declare a dependency on. A cycle or an unknown rule id is an error before anything is written.
- **Convergent.** The loop has a pass limit (10, ADR 0018). If it is reached, or two fixes keep undoing each other, the command stops with a clear error that names the rules and files, and exits non-zero. It never reports success on a repo that does not pass.
- **Careful with existing work.** A fix never replaces content a person wrote. It creates a missing file, merges into a structured file, appends, or rewrites inside a managed block.

What differs between the commands:

| Command        | Starts from             | Writes                               |
| -------------- | ----------------------- | ------------------------------------ |
| `init`         | an empty or new folder  | the config, then everything          |
| `adopt`        | an existing repo        | what is missing; `--dry-run` first   |
| `sync`         | a repo that has drifted | what drifted                         |
| `sync --check` | CI                      | nothing; exit 1 if `sync` would write |

Violations that only an agent or a person can fix do not block the loop. They are listed at the end.

## 16. Born agent-ready

- `fscale init` turns on the `agent-ready` pack by default, so a new repo gets, for Claude Code, Codex and Cursor: an AGENTS.md with a **managed block**, the skills of the enabled rules, the verify hooks, and safe permission settings (no reading of env files or secrets; the repo's own check commands allowed).
- These are fixes of `agent-ready` rules, so they follow §15. `fscale sync` repairs them.
- Text outside the managed block of AGENTS.md belongs to the team and is never rewritten.
- Generators (`fscale add`) take JSON input and support `--dry-run`. `fscale schema <generator>` prints the input schema. A skill tells agents to call `fscale add` instead of hand-writing boilerplate.
- Each agent's file formats must be confirmed from its current official docs when the fix is written, and pinned in tests.

## 17. Group-project mode

- `fscale doctor` checks a teammate's machine: tool versions against what the repo declares, every variable named in `.env.example` (names only; values are never printed), and installed dependencies. Each problem comes with the command that fixes it.
- The `group-project` pack (§6) gives a team a pull request template, branch naming, a contributing guide and CI that runs `fscale verify`.
