# 0008 — Rust is the front door

Status: accepted (M2). Supersedes ADR 0001 and ADR 0003. Amends ADR 0005 and ADR 0007.

## Context

Through M1 the `fscale` command was a Node program that loaded a small Rust addon through napi-rs. Every run paid Node's startup cost, even `fscale --version`. `fscale verify` will run inside agent hooks on every stop attempt, so startup time is paid constantly.

The design changes: `fscale` becomes a native Rust binary, core rules are written in Rust and built into it, and TypeScript is kept for the parts where a JavaScript ecosystem matters.

## Decision

### 1. One Rust binary

- `crates/cli` builds the `fscale` binary. It owns argument parsing, config, rule selection, reporting and exit codes.
- `crates/core` is the engine library: the diagnostics model, config, git and file access, the rule trait, the report formats and the Node bridge.
- `crates/rules` holds the built-in rule packs. Each rule is a Rust module with its `SKILL.md` embedded in the binary by `include_str!`.
- A repo that uses only built-in rules and a JSON config (or no config) runs with **no Node process**. A test proves it by pointing the binary at a Node that does not exist.

### 2. TypeScript keeps four jobs

The rule SDK for custom rules, generators (`init`, `add`, `sync`), agent adapters, and Nx plugins. The Rust binary starts Node only when one of these is needed. In M2 that means: a `firescale.config.ts`, or a custom rule pack.

### 3. The napi binding is removed

`crates/napi` and the `@firescale/native` package are deleted, with the `napi`, `napi-derive`, `napi-build` and `@napi-rs/cli` dependencies.

- Its only job was to let a Node CLI call Rust in-process. With Rust in front, nothing calls in that direction.
- Keeping it "in case" means a second native artifact to build, test and ship per platform.
- If the TypeScript SDK ever needs engine functions (for example the gitignore-aware walker for custom rules), the bridge below can serve them as requests from Node to Rust. A binding can come back by a new ADR if a measured need appears.

### 4. How Rust talks to Node: a child process and JSON lines

When Node is needed, the binary starts **one** Node child for the whole run and exchanges newline-delimited JSON over the child's stdin and stdout.

- Request: `{"id": 1, "method": "loadConfig", "params": {...}}`
- Response: `{"id": 1, "result": ...}` or `{"id": 1, "error": {"message": "..."}}`
- The first exchange is `hello`, which returns `protocolVersion`. A mismatch is an error that tells the user to align the versions.
- Methods in M2: `hello`, `loadConfig` (import a `.ts` config and return its value), `describePacks` (import custom packs and return their rules' metadata and SKILL.md paths), `checkRule` (run one custom rule's check with the M1 SDK and return its diagnostics).
- The child's stderr is passed through, so a rule's own logging is visible. The child exits when its stdin closes.

Why this and not something else:

- **Embedding a JS engine** (V8 or QuickJS in the binary) would not run npm packages the way users expect, and it would add a large dependency.
- **A socket or HTTP server** adds ports, lifecycle and firewalls for no gain. Stdio works the same on Windows, macOS and Linux.
- **One process per rule** would pay Node startup once per rule. One long-lived child pays it once.
- JSON lines are trivial to debug: the host can be driven by hand from a shell.

**Finding the host script.** In order: the `FSCALE_NODE_HOST` environment variable; a `firescale` package next to the binary (the installed layout and this repo's dev layout); otherwise Node's own resolution of `firescale/host` from the repo root. A repo with a `.ts` config or custom rules has `firescale` installed anyway, because those files import from it.

**Finding Node.** The `FSCALE_NODE` environment variable, otherwise `node` on `PATH`. If Node is needed and cannot be started, the error says which feature needed it and how to avoid it (use `firescale.config.json`, or install Node 22.18+).

### 5. Custom rule packs in config

`packs` entries are either a built-in pack name (`core`) or a path to a module, relative to the repo root, starting with `./` or `../` (for example `./tools/rules/pack.ts`). The module's default export (or an export named `pack`) is a `definePack(...)` value from the M1 SDK. A custom pack cannot use the name of a built-in pack. Packs published as npm packages are not supported yet.

### 6. Shipping through npm (design; built in the packaging milestone)

Checked against how current native CLIs are published (`npm view` and `npm pack` of `@biomejs/biome` 2.6.6, `esbuild` 0.28.2 and `turbo` 2.11.10; the npm and pnpm doc sites were not reachable from the build session):

- The main package `firescale` lists one package per platform in `optionalDependencies` (`@firescale/cli-linux-x64`, `-linux-arm64`, `-darwin-x64`, `-darwin-arm64`, `-win32-x64`, `-win32-arm64`, plus musl variants), all pinned to the same version. Each platform package declares `os`, `cpu` and, on Linux, `libc`, so the package manager installs only the matching one. Biome and Turbo do exactly this.
- The main package's `bin` is a small JavaScript launcher that resolves the platform package and runs the binary with `spawnSync(..., { stdio: 'inherit' })`. All three projects ship a JS launcher as `bin`; none points `bin` at the native file, because the Windows command shim expects a script.
- The launcher is a Node process in front of the binary. To honour "no Node process", the main package also gets an optional `postinstall` that hard-links the native binary over the launcher on platforms where that works, as esbuild does. Where install scripts are blocked (pnpm blocks dependency scripts by default) or on Windows, the launcher stays and still works.
- The launcher sets `FSCALE_NODE_HOST`, so the binary never has to search for the host script in an installed layout.
- Users who want zero Node can also call the platform binary directly, or install it from a release archive.

So "no Node process" is guaranteed for the binary itself, and for npm installs where the hard-link step ran. With the plain launcher there is one short-lived Node process in front, which adds its startup time but runs no firescale logic.

### 7. Rules: two kinds, one contract

- **Built-in rules** are Rust: `crates/rules/src/<pack>/<rule-id>/` with `mod.rs` (metadata, `check`, optional `fix`), `SKILL.md` and `tests.rs`.
- **Custom rules** are TypeScript, written with `defineRule` exactly as in M1 (ADR 0007). Nothing changes for rule authors.
- Both produce the same diagnostics, appear in the same report, and need the same SKILL.md sections.
- The TypeScript `@firescale/rules-core` package is removed. `core/no-committed-env` is ported, with its tests.

### 8. Version

The Cargo workspace version is the firescale version. `fscale --version` prints it. A test fails if any `package.json` version differs. (ADR 0003's rule, without the binding.)

### 9. Dependencies

Rust: `serde` and `serde_json` only. `serde_json` is built with `preserve_order` (which brings in `indexmap`), so config errors name keys in the order the user wrote them. Argument parsing, glob matching for `--rules` and error types are small and hand-written. `tempfile` is a dev dependency for tests.

## Consequences

- Startup no longer includes Node for the common case. Numbers are in `bench/RESULTS.md`.
- Two implementations must agree on some behaviour: config validation lives in Rust only, and the Node host returns raw config values for Rust to validate.
- Custom TypeScript rules pay one Node start per run and one JSON round trip per rule. Their file and git access still uses the TypeScript SDK's own implementation, not the Rust engine.
- Building from source now needs Rust for everything, not just for the addon. End users still need no Rust toolchain once prebuilt packages are published.
- Until the packaging milestone, firescale is run from a source checkout (`pnpm fscale`). Nothing is published to npm yet.
- The agent runner and generators will be driven the same way: Rust owns the command, the guardrails and the exit code; Node is called through the bridge for the adapter or generator itself.
