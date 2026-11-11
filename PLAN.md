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

## M1 — Rule SDK and the first rule ☐

**Build**

- `packages/sdk`: `defineRule`, `Diagnostic`, `FixResult`, rule context, and a fixture-based test helper.
- Config loader for `firescale.config.ts` / `.json` (SPEC §10).
- `fscale check` with human output and `--json`.
- `fscale explain <rule>`.
- One real rule end to end: `core/no-committed-env`, with check, fix, SKILL.md and tests.
- A SKILL.md template and a test that validates every SKILL.md has the required sections.

**Done when**

- ☐ `fscale check` finds a tracked `.env` in a fixture repo and exits 1 when the rule is `enforced`.
- ☐ `fscale check --json` output matches a documented schema, with a snapshot test.
- ☐ The rule test covers: passing repo, failing repo, fixed repo, fix run twice (idempotent).

---

## M2 — Rust core ☐

**Build**

- Parallel, gitignore-aware file walker.
- Glob matching and content hashing.
- Rule-result cache keyed by rule version + input-file hashes + config hash. Stored under `.firescale/cache/`.
- Parallel scheduling of rule checks.
- SARIF output (`--sarif`).
- `bench/`: a generator for large fixture repos and a benchmark script. Record results in `bench/RESULTS.md`.

**Done when**

- ☐ A second `fscale check` with no file changes is served from cache, proven by a test.
- ☐ Changing a file inside a rule's `inputs` invalidates only that rule's result, proven by a test.
- ☐ Benchmarks for cold and warm runs on a 50k-file fixture are recorded.
- ☐ All Rust code passes clippy with warnings denied.

---

## M3 — Core rule pack and `fix` ☐

**Build**

- The remaining seven `core` rules from SPEC §6, each with check, SKILL.md, tests, and a deterministic fix where one is safe.
- `fscale fix` and `fscale fix --dry-run`.
- Stack detection (`fscale facts`) from marker files, since several rules depend on it.
- `--affected` support using git diff plus the project graph.

**Done when**

- ☐ All 8 core rules pass their fixture tests on all three OSes.
- ☐ `fscale fix --dry-run` prints a diff and changes nothing.
- ☐ Every fix is idempotent, proven by tests.
- ☐ Running `fscale check` on this repo itself reports zero violations (dogfooding).

---

## M4 — `verify` and agent hooks ☐

**Build**

- `fscale verify` per SPEC §8, with the compact agent-readable summary and `--json`.
- Runner detection: Nx, Turborepo, or plain package scripts.
- `fscale hooks install` for Claude Code, Codex and Cursor. Confirm each agent's current hook format from its official docs before writing it.
- Loop guard and retry cap.
- The `agent-ready` rule pack.
- From this milestone on, this repo's own agent hooks call `fscale verify`.

**Done when**

- ☐ `fscale verify` exits non-zero with a clear summary when a fixture repo has a failing test.
- ☐ Hook files are generated for three agents, with snapshot tests.
- ☐ The loop guard is covered by a test.
- ☐ p50 and p99 verify latency are recorded in `bench/RESULTS.md`.

---

## M5 — Guarded agent runner ☐ (finish and test locally)

**Build**

- `packages/agents`: adapter interface plus Claude Code and Codex adapters.
- Every guardrail in SPEC §7, each with its own test.
- A fake agent adapter for tests, so CI never calls a real model.
- The fix ledger.

**Done when**

- ☐ With the fake agent: a good fix is committed, a bad fix is rolled back, a fix touching a protected path is rejected.
- ☐ The runner refuses to start on a dirty tree or when secrets are tracked.
- ☐ Manual local test: a real agent fixes one real violation end to end. Result noted in the PR.

---

## M6 — `init`, `add`, `sync` ☐

**Build**

- `fscale init` and `fscale init --agents`.
- JSON-first generators with `fscale schema` and `--dry-run`.
- `fscale sync` for the CI workflow and the managed AGENTS.md block. `--check` for CI.
- A skill telling agents to use `fscale add` instead of hand-writing boilerplate.

**Done when**

- ☐ `fscale init` on an empty folder yields a repo where `fscale check` and `fscale verify` both pass.
- ☐ `fscale sync --check` fails after a hand edit to a generated file.
- ☐ Generators are covered by snapshot tests.

---

## Later

- M7: publish to npm with prebuilt binaries; docs site.
- M8: rule packs for Expo, Supabase, Next.js, Python, group projects.
- M9: thin MCP server; Nx plugins for EAS and Supabase.
