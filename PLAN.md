# firescale — Build Plan

One milestone per session and per pull request. A milestone is done only when every box under "Done when" is true.

Status key: ☐ not started · ◐ in progress · ☑ done

---

## M0 — Skeleton ☑

**Build**

- pnpm workspace + Cargo workspace, laid out as in SPEC §9.
- TypeScript strict config, ESLint, Prettier, Vitest.
- Rust: `cargo fmt`, `clippy` with warnings denied, `cargo test`.
- GitHub Actions CI on Windows, macOS and Linux: install, lint, typecheck, test, build.
- A `fscale --version` command that calls one trivial function in the Rust core through the Node binding. This proves the bridge works on all three OSes.
- README with a one-paragraph pitch and dev setup.
- `docs/decisions/` folder with an ADR for each real choice made (binding approach, test runner, and so on).

**Done when**

- ☑ CI is green on all three OSes.
- ☑ `pnpm build && pnpm test` passes from a clean clone.
- ☑ `fscale --version` prints a version that came through the Rust binding.

---

## M1 — Rule SDK and the first rule ☑

**Build**

- `packages/sdk`: `defineRule`, `Diagnostic`, `FixResult`, rule context, and a fixture-based test helper.
- Config loader for `firescale.config.ts` / `.json` (SPEC §10).
- `fscale check` with human output and `--json`.
- `fscale explain <rule>`.
- One real rule end to end: `core/no-committed-env`, with check, fix, SKILL.md and tests.
- A SKILL.md template and a test that validates every SKILL.md has the required sections.

**Done when**

- ☑ `fscale check` finds a tracked `.env` in a fixture repo and exits 1 when the rule is `enforced`.
- ☑ `fscale check --json` output matches a documented schema, with a snapshot test.
- ☑ The rule test covers: passing repo, failing repo, fixed repo, fix run twice (idempotent).

---

## M2 — Rust front door ☑

Design change, see ADR 0008: `fscale` is a native Rust binary, core rules are built in, and Node is started only for TypeScript rules, `.ts` config, generators and agent adapters.

**Build**

- `crates/cli`: the `fscale` binary with `check`, `explain` and `--version`. Same behaviour, same exit codes and the same `--json` schema as M1.
- Config loading for `firescale.config.json` in Rust. `firescale.config.ts` is loaded by starting Node.
- `core/no-committed-env` ported to Rust as a built-in rule, with the same tests.
- The Node bridge: one child process, JSON lines. Runs a custom rule defined with the M1 SDK.
- Remove the napi binding and the TypeScript CLI and rule pack it replaces.
- Small fixes: flag `.ENV` case variants; the SKILL.md validator ignores headings inside code fences; `--rules <id>` runs a rule that is `off`, with a note; Node 22 in the CI matrix; GitHub Actions pinned to commit SHAs.
- `bench/startup.mjs` and startup numbers before and after in `bench/RESULTS.md`.

**Done when**

- ☑ `fscale` is a Rust binary, and the M1 CLI snapshot tests for `check`, `explain` and `--version` pass against it unchanged.
- ☑ `fscale check --json` output validates against the unchanged JSON schema.
- ☑ A repo with only built-in rules runs with no Node process, proven by a test.
- ☑ `core/no-committed-env` is built in, with tests for: passing repo, failing repo, fixed repo, fix run twice.
- ☑ A sample custom TypeScript rule runs through the bridge, and a `.ts` config loads through Node, each proven by a test.
- ☑ Startup time for `fscale --version` and `fscale check`, before and after, is recorded in `bench/RESULTS.md`.
- ☑ CI is green on all three OSes, on Node 22 and Node 24.

---

## M3 — Engine: walk, hash, cache ☑

**Build** (all Rust, in `crates/core`)

- Parallel, gitignore-aware file walker.
- Glob matching and content hashing.
- Rule-result cache keyed by rule version + input-file hashes + config hash. Stored under `.firescale/cache/`.
- Parallel scheduling of rule checks. Custom TypeScript rules are batched through the one Node child.
- `bench/`: a generator for large fixture repos and a benchmark script.
- Loose ends from M2: an airtight "no Node process" test, tests for the automatic lookup of the Node host, the `core/no-committed-env` SKILL.md mentions case variants, and benchmarks recorded from Windows and macOS CI runners.

**Done when**

- ☑ A second `fscale check` with no file changes is served from cache, proven by a test.
- ☑ Changing a file inside a rule's `inputs` invalidates only that rule's result, proven by a test.
- ☑ Benchmarks for cold and warm runs on a 50k-file fixture are recorded in `bench/RESULTS.md`.
- ☑ All Rust code passes clippy with warnings denied.

---

## M4 — `fix`, SARIF and facts ☑

**Build**

- `fscale fix` and `fscale fix --dry-run` for built-in rules, and for custom rules through the bridge.
- SARIF output (`--sarif`).
- Stack detection (`fscale facts`, `--json`) from marker files, and the simple project graph.
- `--affected` support using git diff plus the project graph.

**Done when**

- ☑ `fscale fix` fixes `core/no-committed-env` in a fixture repo, and a second run changes nothing.
- ☑ `fscale fix --dry-run` prints a diff and changes nothing.
- ☑ `--sarif` output validates against the SARIF 2.1.0 schema, with a snapshot test.
- ☑ `fscale facts --json` has a documented schema and a snapshot test.

---

## M5 — Core rule pack ☐

**Build**

- The remaining seven `core` rules from SPEC §6, in Rust, each with check, SKILL.md, tests, and a deterministic fix where one is safe.

**Done when**

- ☐ All 8 core rules pass their fixture tests on all three OSes.
- ☐ Every fix is idempotent, proven by tests.
- ☐ Every SKILL.md passes validation.
- ☐ Running `fscale check` on this repo itself reports zero violations (dogfooding).

---

## M6 — npm packaging ☐

**Build**

- Per-platform binary packages and the `firescale` launcher package, as designed in ADR 0008 §6.
- A release workflow that builds the binary for each platform and assembles the packages. Publishing itself stays a manual, approved step.
- Install smoke tests: `npm`, `pnpm` and `yarn`, on all three OSes, from locally packed tarballs.

**Done when**

- ☐ `npx firescale --version` works from packed tarballs on all three OSes with no Rust toolchain.
- ☐ A custom TypeScript rule works from an npm install, proven by a smoke test.
- ☐ The launcher's overhead is measured and recorded in `bench/RESULTS.md`.

---

## M7 — `verify` and agent hooks ☐

**Build**

- `fscale verify` per SPEC §8, in Rust, with the compact agent-readable summary and `--json`.
- Runner detection: Nx, Turborepo, or plain package scripts.
- `fscale hooks install` for Claude Code, Codex and Cursor. Confirm each agent's current hook format from its official docs before writing it.
- Loop guard and retry cap.
- The `agent-ready` rule pack, in Rust.
- From this milestone on, this repo's own agent hooks call `fscale verify`.

**Done when**

- ☐ `fscale verify` exits non-zero with a clear summary when a fixture repo has a failing test.
- ☐ Hook files are generated for three agents, with snapshot tests.
- ☐ The loop guard is covered by a test.
- ☐ p50 and p99 verify latency are recorded in `bench/RESULTS.md`.

---

## M8 — Guarded agent runner ☐ (finish and test locally)

**Build**

- The runner and every guardrail in SPEC §7, in Rust, each with its own test.
- `packages/agents`: the adapter interface plus Claude Code and Codex adapters, in TypeScript, called through the bridge.
- A fake agent adapter for tests, so CI never calls a real model.
- The fix ledger.

**Done when**

- ☐ With the fake agent: a good fix is committed, a bad fix is rolled back, a fix touching a protected path is rejected.
- ☐ The runner refuses to start on a dirty tree or when secrets are tracked.
- ☐ Manual local test: a real agent fixes one real violation end to end. Result noted in the PR.

---

## M9 — `init`, `add`, `sync` ☐

**Build**

- `fscale init` and `fscale init --agents`.
- JSON-first generators in TypeScript, called through the bridge, with `fscale schema` and `--dry-run`.
- `fscale sync` for the CI workflow and the managed AGENTS.md block. `--check` for CI.
- A skill telling agents to use `fscale add` instead of hand-writing boilerplate.

**Done when**

- ☐ `fscale init` on an empty folder yields a repo where `fscale check` and `fscale verify` both pass.
- ☐ `fscale sync --check` fails after a hand edit to a generated file.
- ☐ Generators are covered by snapshot tests.

---

## Later

- M10: publish to npm; docs site.
- M11: rule packs for Expo, Supabase, Next.js, Python, group projects.
- M12: thin MCP server; Nx plugins for EAS and Supabase.
