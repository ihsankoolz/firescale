# 0002 — Toolchain and pinned versions

Status: accepted (M0)

## Context

M0 asks for a pnpm workspace, strict TypeScript, ESLint, Prettier, Vitest and CI on three OSes, using current stable versions, pinned. Versions were checked against the npm registry, crates.io and the action repositories on 2026-11-10.

## Decision

| Tool           | Version                                                                                          | Note                                                                                                                                               |
| -------------- | ------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| pnpm           | 12.18.0                                                                                          | Pinned through `packageManager`; CI reads it from there.                                                                                           |
| Node.js        | `>=22.18.0`, CI runs 24                                                                          | 22.18 is the first release that runs `.ts` files without a flag, which the `firescale.config.ts` loader in M1 will rely on.                        |
| TypeScript     | 6.0.3                                                                                            | **Not 7.x.** `typescript-eslint` 8.72 declares `typescript >=4.8.4 <6.1.0`, so 7 cannot be linted yet. Revisit when typescript-eslint supports it. |
| ESLint         | 10.14.0 + typescript-eslint 8.72.2                                                               | Flat config, `strict` preset, `any` is an error, default exports banned except in config files and `rule.ts`.                                      |
| Prettier       | 3.9.10                                                                                           | `endOfLine: lf`. The planning docs (`SPEC.md`, `PLAN.md`, `KICKOFF.md`) are excluded so they stay as written.                                      |
| Vitest         | 5.1.0                                                                                            | One root config that runs every `packages/*/src/**/*.test.ts`. Chosen over `node:test` for snapshot support, which M1 onwards needs.               |
| Rust           | 1.97.0 (`rust-toolchain.toml`)                                                                   | `cargo fmt`, clippy with `-D warnings`, `clippy::unwrap_used` denied outside tests.                                                                |
| GitHub Actions | `actions/checkout@v7`, `actions/setup-node@v7`, `pnpm/action-setup@v6`, `Swatinem/rust-cache@v2` | Latest major tags. Rust is installed by `rustup toolchain install`, which reads `rust-toolchain.toml`.                                             |

Root scripts are the single entry points: `pnpm build`, `pnpm test`, `pnpm lint`, `pnpm typecheck`. `pnpm test` and `pnpm lint` include the Rust side so one command covers everything. Scripts use only `&&` chaining, which works in every shell pnpm uses.

CI runs on `pull_request` and on pushes to `main`, across `ubuntu-latest`, `macos-latest` and `windows-latest`.

## Consequences

- Dev dependencies use exact versions. Upgrades are deliberate.
- Actions are pinned to major tags, not commit SHAs. Simple, but it trusts the action owners. Pinning to SHAs is a possible later hardening step.
- Node 22 is the local floor while CI proves Node 24. If that gap ever hides a bug, add Node 22 to the matrix.
