# 0009 — CI: Node 22 and 24, actions pinned to commit SHAs

Status: accepted (M2). Amends ADR 0002.

## Context

ADR 0002 left two gaps open: `engines.node` says `>=22.18.0` but CI only ran Node 24, and GitHub Actions were pinned to major tags, which the action owner can move.

## Decision

- The CI matrix is three OSes times two Node versions: 22 and 24. Node only matters for the TypeScript packages and the bridge, but the whole job runs for each cell so a failure is easy to read.
- Every action is pinned to a full commit SHA, with the tag in a trailing comment:
  - `actions/checkout` `3d3c42e5aac5ba805825da76410c181273ba90b1` (v7.0.1)
  - `pnpm/action-setup` `73e46992e05b19abd31f2f3c20e0e7370d762115` (v6.1.1)
  - `actions/setup-node` `949feb2413d6458794dcd2491c4babbbce0c15c1` (v7.1.0)
  - `Swatinem/rust-cache` `d1527c8f67ef9af357290475d4af4b0fab8f12e0` (v2.9.3)
- The SHAs were read from the action repositories' tags with `git ls-remote --tags` on 2026-11-11. For annotated tags the peeled commit (`^{}`) is used.

## Consequences

- An action update is now an explicit change in a pull request. Nothing updates them automatically yet; Dependabot or Renovate can be added later.
- CI runs six jobs per push instead of three.

## Amendment (M6): fewer jobs per push

The owner asked to reduce GitHub Actions usage. From M6:

- `check` runs four jobs per push, not six: Linux on Node 22 and 24, macOS and Windows on Node 24 only. Node only affects the TypeScript packages and the bridge, so one OS is enough to cover the older version.
- The `bench` and `package` jobs run only when the workflow is started by hand (`workflow_dispatch`). Their last full runs on all three OSes are linked from the M3 and M6 pull requests.

The cost: a change that breaks the npm install or slows `check` on macOS or Windows is not caught on every push. Run the workflow by hand before a release.
