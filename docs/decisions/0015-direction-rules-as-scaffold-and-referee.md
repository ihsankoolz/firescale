# 0015 — Direction: a toolkit for small teams, where the rules are the scaffold and a referee keeps agents honest

Status: accepted (2026-11-11, after M7). Changes SPEC §1, §3, §4, §5, §6, §7, §8, §9, §11 and §12, adds §15 to §17, and replaces PLAN.md from M8 onward. Supersedes the "Later" list of the old plan.

## Context

After M7, firescale can check a repo, fix what is safe to fix, and make an agent pass `fscale verify` before it stops. Research on 2026-11-11 looked at where to take it next, and at one tempting pivot: a standalone "agent referee" or benchmark product.

What the research found:

- Generic repo hygiene rules are already covered well by alint. Writing more of them by hand adds little.
- Stack-specific setup (Expo, Supabase, Next.js, Python, a student group project) is where small teams lose time, and nothing covers it as rules.
- Agents that are told "make verify pass" sometimes do it by skipping a test, deleting an assertion or adding a lint-disable. Existing tools in this space (trustbut, goodhart, skylos) show the idea works. None of the three is built into the tool that also defines "done".
- Keeping a history of flaky tests is a product of its own and not worth building here.

## Decision

**firescale stays a conformance and scaffolding toolkit for small teams.** Its commands are `init`, `add`, `sync`, `check`, `fix` and `verify`. It does not become a standalone referee or a benchmark product. The research is folded in as five changes.

Pitch: _a toolkit for small teams where the rules are the scaffold, every repo is born agent-ready, and a referee keeps agents honest._

### 1. Rules are the scaffold

- `fscale init` has no templates. It writes a config that turns on the chosen packs, then runs every enabled rule's deterministic fix, again and again, until `fscale check` passes.
- The same loop serves three commands: `init` builds a new repo, `adopt` brings an existing repo up to the rules, and `sync` repairs drift. `sync --check` changes nothing and fails when a fix would change something, for CI.
- A **stack pack** is a set of rules whose fixes create files. There is no second mechanism for scaffolding.
- This puts three requirements on fixes:
  - **Idempotent.** Already required (SPEC §5). A second run changes nothing.
  - **Ordered.** A rule may declare the rules that must be fixed before it. The loop runs fixes in that order and rejects a cycle.
  - **Convergent.** The loop has a pass limit. If violations with deterministic fixes remain after it, or two fixes keep undoing each other, the command stops with an error that names the rules and files involved. It never loops forever and never reports success on a repo that does not pass.
- Violations that only an agent or a person can fix do not block `init`, `adopt` or `sync`. They are listed at the end.

### 2. Born agent-ready

- `init` also writes what a coding agent needs: an AGENTS.md with a managed block, the rule skills, the verify hooks, and safe permission settings, for Claude Code, Codex and Cursor. These are fixes of the `agent-ready` rules, so they follow change 1.
- Text outside the managed block in AGENTS.md belongs to the team and is never rewritten.
- Generators stay: `fscale add <generator>` takes its input as JSON and supports `--dry-run`, and `fscale schema <generator>` prints the input schema, so an agent can call a generator correctly without reading its source.

### 3. A built-in referee

**a. Blame-aware verify.** Every failure is labelled:

| Label          | Meaning                                         | Blocks |
| -------------- | ----------------------------------------------- | ------ |
| `new`          | Fails on the head, passed on the base commit.   | yes    |
| `pre-existing` | Fails on both.                                  | no     |
| `fixed`        | Failed on the base, passes on the head.         | no     |
| `flaky`        | Failed on the head, then passed when run again. | no     |

- The base commit is run in a git worktree, never in the user's working tree.
- Base results are cached by base SHA + toolchain versions + lockfile hash, so the base is run once per base commit.
- A `new` failure is run a second time before it blocks. If it passes then, it is `flaky`. That is the whole of flaky handling: **no flaky-test history is kept.**

**b. Tamper guard.** Detects attempts to pass verify by weakening it:

- tests skipped or deleted, and a net loss of assertions,
- new lint-disable comments or `allow` attributes,
- lowered coverage thresholds,
- CI steps removed, or made unable to fail,
- `--no-verify`.

How: source is parsed with tree-sitter and compared between base and head by **net counts** per file, so moving or renaming a test is not a finding. The policy (what is protected, what is allowed) is read **from the base commit**, so a change cannot loosen its own policy. Clear cheats fail hard. Changed expected values and changed snapshots are not always cheats, so they need a person's approval instead.

Prior art to study before designing it: trustbut, goodhart, skylos. What firescale aims to do differently: syntax-tree based (not regular expressions over diffs), anchored to the base commit, and with a false-positive rate that is measured and published.

### 4. Group-project mode

- `fscale doctor` checks a teammate's machine: tool versions, the environment variables listed in `.env.example`, and installed dependencies. It prints what is wrong and the command that fixes it.
- A `group-project` pack adds a pull request template, a branch naming rule, a contributing guide, and a CI workflow that runs `fscale verify`.

### 5. Ledger

- Every agent fix run is appended to `.firescale/ledger.jsonl`: rule, agent, pass or fail, diff size, duration and cost.
- `fscale ledger` prints a summary.
- A public benchmark built from ledgers is possible later. It is not planned.

### What stops

- **No more hand-written generic hygiene rules** beyond the eight in the `core` pack. If alint is installed, firescale may ingest its output and report it as diagnostics; that is optional and not a dependency.
- Future rule effort goes into stack packs: Expo, Supabase, Next.js, Python.

### Order of work

The suggested order was: rules-as-scaffold, group-project pack and doctor, born agent-ready, blame-aware verify, tamper guard, guarded agent runner and ledger, stack packs. PLAN.md keeps that order. Each theme is split so that one milestone fits one session. Two adjustments, both because of dependencies:

1. **The fix loop is its own milestone, before `init`.** Ordering by declared dependencies and the convergence error are engine work that `init`, `adopt` and `sync` all stand on, and they can be proven with the fixes that exist today.
2. **Prior-art study opens each referee theme** instead of being a milestone of its own, so the design ADR and the first working slice land together.

One thing the order cannot fix: group-project mode is for teammates, and teammates need to install firescale. Publishing to npm is a manual step that only the owner can take (DECISIONS.md, "Publishing to npm"). It is not a milestone, but `doctor` and the group-project pack are only useful to a real team once it is done.

## Consequences

- `init` no longer needs Node. It becomes a Rust command that runs the fix loop. Generators (`add`, `schema`) stay in TypeScript behind the bridge.
- The rule contract grows: a rule can declare which rules come before it, and fixes that create files become normal. Custom TypeScript rules get the same field.
- The `agent-ready` rules gain fixes. ADR 0014 left them without fixes because generating those files "belongs to `init`"; with this ADR, `init` is those fixes.
- tree-sitter and its grammars become dependencies of the binary when the tamper guard is built. That needs its own ADR, with the size and build-time cost measured.
- The old M8 (guarded agent runner) moves after the referee. The referee is what makes an unattended agent run trustworthy, so the runner is built on top of it.
- The old "Later" items (publish to npm and a docs site, an MCP server, Nx plugins) stay later and unordered.
- The project name's origin (SPEC, first lines) is unchanged. AGENTS.md's originality rule still applies: nothing is copied from any private codebase, and the product is described in its own words.
