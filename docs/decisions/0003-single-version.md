# 0003 — One version, owned by the Rust core

Status: accepted (M0)

## Context

M0 requires `fscale --version` to print a version that came through the Rust binding.

## Decision

- The Cargo workspace version is the firescale version. `firescale_core::version()` returns it, `@firescale/native` exposes it as `coreVersion()`, and `fscale --version` prints exactly that string.
- Every `package.json` carries the same version. A test (`keeps the Rust core and the npm package on the same version`) fails if they drift.

## Consequences

- A release bumps `Cargo.toml` and the package versions together.
- The CLI cannot report a version without loading the native binding. That is intended: a broken bridge is visible immediately.
