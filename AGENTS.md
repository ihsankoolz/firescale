# AGENTS.md

Guidance for any coding agent working in this repo.

## Read first

1. `SPEC.md` — what we are building. It is the source of truth.
2. `PLAN.md` — milestones and their "Done when" checklists.
3. `docs/decisions/` — past decisions. Don't reverse one without writing a new ADR.

## How to work

- Work on **one milestone at a time**, on a branch named `m<N>-<short-name>`.
- Do not start the next milestone. Stop when the current one's "Done when" list is fully true.
- Open one pull request per milestone. In the PR body, copy the "Done when" list and tick each item with evidence (test name or command output).
- Update the milestone's status in `PLAN.md` in the same PR.
- If the spec is unclear or seems wrong, pick the simplest reading, note it in an ADR, and flag it at the top of the PR. Don't stall.
- Commit in small steps with Conventional Commits (`feat:`, `fix:`, `chore:`, `test:`, `docs:`).

## Commands

Keep this list accurate. Run `pnpm build` before `pnpm test`: the Vitest suite runs the built `fscale` binary (`target/debug/fscale`) and the built Node host.

- Install: `pnpm install`
- Build everything (`cargo build`, then the TS packages): `pnpm build`
- Test everything (`cargo test` + Vitest): `pnpm test`
  - TypeScript only: `pnpm test:ts`. Rust only: `pnpm test:rust`
- Lint (ESLint, Prettier check, `cargo fmt --check`, clippy): `pnpm lint`
  - Parts: `pnpm lint:ts`, `pnpm lint:format`, `pnpm lint:rust`
- Typecheck: `pnpm typecheck`
- Format: `pnpm format`
- Run the local CLI: `pnpm fscale --version`, `pnpm fscale check`, `pnpm fscale fix --dry-run`, `pnpm fscale facts`, `pnpm fscale doctor`, `pnpm fscale explain <rule>`, `pnpm fscale schema`, `pnpm fscale add <generator> --json '<input>' --dry-run`
- Scaffold commands (docs/init.md): `pnpm fscale adopt --dry-run`, `pnpm fscale sync --check`; try `init` in a scratch folder with `pnpm fscale --cwd <empty dir> init`
- One test file: `pnpm exec vitest run <path>`. Update snapshots: `pnpm exec vitest run -u`
- One Rust test: `cargo test -p firescale-rules <name>`
- Rust only: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`
- Startup benchmark: `node bench/startup.mjs --runs 50 -- target/release/fscale --version`
- 50k-file benchmark: `node bench/generate-fixture.mjs <dir> --files 50000`, then `node bench/check-bench.mjs --bin target/release/fscale --fixture <dir>`
- See cache hits and misses: `FSCALE_LOG=cache pnpm fscale check`
- Build npm tarballs for this platform: `cargo build --release -p firescale-cli`, `pnpm -r run build`, `node scripts/pack-npm.mjs`
- Set the version everywhere it is recorded: `node scripts/bump-version.mjs <version>`. Check it: `node scripts/bump-version.mjs --check`. Releasing is in `RELEASING.md`; never push a version tag unless the owner asks.
- See what a release would publish, from downloaded workflow artifacts: `node scripts/publish-npm.mjs --dir <folder> --version <version> --dry-run`
- Smoke-test an install from those tarballs: `node scripts/smoke-npm.mjs --pm npm` (also `pnpm`, `yarn`)
- Definition of done (from M7 on): `pnpm fscale verify`. The agent hooks in `.claude/`, `.codex/` and `.cursor/` run it when you try to stop.
- Verify latency benchmark: `cargo build --release -p firescale-cli`, then `node bench/verify-bench.mjs`

## Hard rules

- **Originality.** Write everything fresh from public documentation. Never copy code, rule text, names or docs from any private or employer codebase. Never use the names "Forge" or "Codeak".
- **Cross-platform.** Everything must pass on Windows, macOS and Linux.
  - Never build paths with string concatenation or hardcoded `/`. Use path APIs.
  - Normalise paths to forward slashes in all output, cache keys and snapshots.
  - No bash-only scripts in `package.json`. Use Node scripts for anything non-trivial.
  - Handle CRLF and LF. Add a `.gitattributes`.
- **No real model calls in tests or CI.** Use the fake agent adapter.
- **No secrets.** Never read, print or commit `.env*`, keys or tokens.
- **Check current docs before using an external API or CLI flag.** Agent CLIs, hook formats, Nx plugin APIs and binding tools change often. Confirm against official docs, then pin the version and add a test.
- **Don't add a dependency lightly.** Prefer the standard library. Record any significant new dependency in an ADR.
- **Don't weaken a test to make it pass.** Fix the code, or explain in the PR why the test was wrong.

## Code style

- TypeScript: strict mode, no `any`, ESM, named exports.
- Rust: stable toolchain, typed errors, no `unwrap()` outside tests, clippy clean.
- Errors shown to users name the problem and the next step to take.
- Every rule has: check, tests with fixtures, and a SKILL.md. A deterministic fix is optional but must be idempotent.
- Built-in rules are Rust (`crates/rules`). TypeScript is for the SDK, the Node host, generators and agent adapters (ADR 0008).

## Testing

- Rules are tested with fixture repos: passing, failing, and fixed.
- Anything with output has a snapshot test.
- Cache behaviour and guardrails each get explicit tests.
- Benchmarks live in `bench/` and their results in `bench/RESULTS.md`.

<!-- BEGIN firescale: written by `fscale sync` from `fscale facts`. Edits inside this block are replaced; write your own notes outside it. -->

## Commands

Run each command in the folder it names.

### The repo root

- Install: `pnpm install`
- Build: `pnpm run build`
- Lint: `pnpm run lint`
- Typecheck: `pnpm run typecheck`
- Test: `pnpm run test`

## Definition of done

Before you say a task is done, run `pnpm --silent fscale verify` from the repo root and fix what it reports. It runs the firescale rules, then lint, typecheck and tests.

- Never skip, delete or weaken a test, a lint rule or a check to make it pass.
- `SETUP NEEDED` means the machine is not ready: a dependency, a program or a service is missing. Run the setup command it names, or tell the user. Do not change code for it. `pnpm --silent fscale doctor` lists what is missing.
- To learn how to fix a rule's finding: `pnpm --silent fscale explain <rule>`.

<!-- END firescale -->
