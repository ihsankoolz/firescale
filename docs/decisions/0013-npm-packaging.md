# 0013 — npm packaging

Status: accepted (M6). Amends ADR 0008 §6.

## Context

ADR 0008 §6 sketched how the native `fscale` binary ships through npm. M6 builds it, which settled the open points.

## Decision

### Packages

Eight packages share one version, the Cargo workspace version:

- `firescale`: the package users install. It holds the launcher (`bin/fscale.js`), the Node host for TypeScript rules and config (`dist/`), and the JSON schemas. Its `bin` entries are `fscale` and `firescale`.
- `@firescale/sdk`: the rule authoring API. `firescale` depends on it and re-exports `defineConfig`, `definePack`, `defineRule` and `FirescaleError`, so most repos only need `firescale`.
- Six platform packages that hold only the binary at `bin/fscale` (`bin/fscale.exe` on Windows): `@firescale/cli-linux-x64`, `-linux-arm64`, `-darwin-x64`, `-darwin-arm64`, `-win32-x64`, `-win32-arm64`. Each declares `os` and `cpu` (and `libc: ["glibc"]` on Linux). `firescale` lists all six as `optionalDependencies`, so the package manager installs the one that matches.

### The launcher

`bin/fscale.js` finds the platform package, runs the binary with the same arguments and stdio, and exits with its exit code (or re-raises its signal). It holds no firescale logic.

- It resolves the platform package from its own location first, then from the project folder, which covers package managers that hide a package's optional dependencies from it.
- It sets `FSCALE_NODE_HOST` to its own `dist/host.js` and `FSCALE_NODE` to the Node that is running it. So when the binary needs Node for a TypeScript rule, it uses the same Node and never has to search for the host.
- `FSCALE_BINARY` overrides the binary, for development and for platforms without a prebuilt package.
- A missing platform package or an unsupported platform is an error that says what to install or build.

### No postinstall step

ADR 0008 §6 proposed an optional `postinstall` that hard-links the binary over the launcher, so that no Node process sits in front. **That is not built.**

- pnpm does not run dependency install scripts unless the user allows them, and many teams set `ignore-scripts`. The step would silently not run for a large share of installs.
- An install script in a security tool's own package is a poor example to set.
- The cost it avoids is one Node start per run, measured in `bench/RESULTS.md`.

Users who need zero Node overhead (for example in an agent hook) can call the binary directly at `node_modules/@firescale/cli-<os>-<arch>/bin/fscale`. The principle in SPEC §3 still holds for the engine: the binary never starts Node for built-in rules.

### How packages are built

`scripts/pack-npm.mjs` stages each package in a temp folder and runs `npm pack`. Staging, not `pnpm pack` in place, because the published `firescale` manifest differs from the workspace one: `workspace:*` becomes an exact version and `optionalDependencies` is added. Putting unpublished optional dependencies in the workspace manifest would make every `pnpm install` in this repo ask the registry for packages that do not exist.

### Not covered yet

- **musl (Alpine) and other platforms.** The launcher says so and points to building from source.
- **Yarn Berry with Plug'n'Play.** The smoke test uses Yarn Classic. The platform packages set `preferUnplugged`, but PnP is untested.

### Verification

`scripts/smoke-npm.mjs` installs the packed tarballs into an empty project with npm, pnpm and Yarn Classic, on Linux, macOS and Windows in CI, and runs `fscale --version`, `fscale check`, `fscale explain`, and a custom TypeScript rule. The other five platform packages are not on the registry during that test; npm and pnpm skip a missing optional dependency, which was checked before relying on it.

`.github/workflows/release.yml` builds all six targets and uploads the tarballs as artifacts. It does not publish. Publishing is a manual step, written out in DECISIONS.md.

## Consequences

- `npx firescale` works with no Rust toolchain. `npx fscale` does **not** run this tool: the npm name `fscale` belongs to an unrelated package. Docs say `npx firescale`; after a local install the command is `fscale`.
- Every run through the launcher pays one Node start. Agent hooks that care should call the binary directly.
- The `@firescale` npm scope must exist and belong to the publisher before the first publish.
- The arm64 targets and the two cross-compiled targets in the release workflow have not been run yet.
