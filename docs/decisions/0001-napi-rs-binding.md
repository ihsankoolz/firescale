# 0001 — Rust-to-Node bridge: napi-rs v3

Status: accepted (M0)

## Context

SPEC §9 wants a Rust core reachable from a TypeScript CLI, with prebuilt binaries per platform later so `npx firescale` needs no Rust toolchain. The options were napi-rs, Neon, a WASM build, or spawning a Rust binary and talking JSON.

## Decision

Use **napi-rs v3** (`napi` 3.16.1, `napi-derive` 3.7.0, `napi-build` 2.6.0, `@napi-rs/cli` 3.10.12, all pinned).

- `crates/napi` is both a Cargo crate (`firescale-napi`, a `cdylib`) and a pnpm workspace package (`@firescale/native`). Its `build` script is `napi build --platform --release --esm`, which writes `firescale.<platform>.node`, an ESM loader `index.js` and `index.d.ts` next to the crate.
- The generated `index.js`, `index.d.ts` and `.node` files are **not committed**. They are build output.
- `crates/napi` has `test = false`. A test binary for an N-API addon cannot link outside Node, so logic and Rust tests live in `crates/core`, and the binding is tested from TypeScript.
- The binding layer stays thin: convert types, call `firescale_core`.

## Consequences

- `pnpm build` must run before `pnpm typecheck` and `pnpm test`, because the binding's types and binary are generated.
- Contributors need a Rust toolchain. End users will not, once M7 publishes prebuilt per-platform packages (napi-rs supports this layout directly; the target list is already in `crates/napi/package.json`).
- N-API is ABI-stable, so one binary per platform works across Node versions.
