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

These are filled in during M0. Keep this list accurate.

- Install: `pnpm install`
- Build everything: `pnpm build`
- Test everything: `pnpm test`
- Lint and typecheck: `pnpm lint` and `pnpm typecheck`
- Rust only: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`
- Definition of done (from M4 on): `pnpm fscale verify`

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

## Testing

- Rules are tested with fixture repos: passing, failing, and fixed.
- Anything with output has a snapshot test.
- Cache behaviour and guardrails each get explicit tests.
- Benchmarks live in `bench/` and their results in `bench/RESULTS.md`.
