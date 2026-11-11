# firescale

> Fire scale is the rough crust that forms on hot metal in a forge. A smith knocks it off to show clean work.

`firescale` is a free, stack-agnostic conformance and verify engine. Every rule ships a **check**, an optional **deterministic fix**, and a portable **SKILL.md**, so a human, a CI job or any coding agent (Claude Code, Codex, Gemini CLI, Cursor) can find what is wrong with a repo and fix it the same way. One `fscale verify` command is the "definition of done" that all of them call. It works on any repo, with or without Nx or Turborepo, on Windows, macOS and Linux.

Status: early development. See [SPEC.md](SPEC.md) for the design and [PLAN.md](PLAN.md) for progress.

## Usage

```
fscale check                          # run the rules, report violations
fscale check --json                   # the same, as JSON for CI and agents
fscale check --rules "core/*"         # only some rules
fscale check --no-cache               # ignore the result cache in .firescale/
fscale explain core/no-committed-env  # what a rule checks and how to fix it
```

Built-in rules run with no Node process. Node is started only for a `firescale.config.ts` or custom TypeScript rules.

Rules are report-only (`evaluated`) by default. Set a rule to `enforced` in `firescale.config.ts` or `firescale.config.json` to make `fscale check` exit 1 when it fails. See [docs/config.md](docs/config.md), [docs/check-json.md](docs/check-json.md) and [docs/writing-rules.md](docs/writing-rules.md).

## Dev setup

`fscale` is a native Rust binary. TypeScript is used for the rule SDK and for the Node host that runs custom rules. Nothing is published to npm yet, so run it from a checkout.

You need:

- Rust via `rustup` (the pinned toolchain in `rust-toolchain.toml` is installed automatically)
- Node.js 22.18 or newer
- pnpm 12 (`corepack enable` or `npm install -g pnpm@12`)
- git

```
pnpm install
pnpm build
pnpm test
pnpm fscale --version
```

| Command          | What it does                                                          |
| ---------------- | --------------------------------------------------------------------- |
| `pnpm build`     | `cargo build` for the binary, then builds every TS package            |
| `pnpm test`      | Runs `cargo test` and the Vitest suite (build first)                  |
| `pnpm lint`      | ESLint, Prettier check, `cargo fmt --check`, clippy (warnings denied) |
| `pnpm typecheck` | `tsc --noEmit` in every package (build first)                         |
| `pnpm format`    | Formats TypeScript, Markdown and Rust                                 |
| `pnpm fscale`    | Runs the CLI through `cargo run`                                      |

`pnpm test` needs `pnpm build` first: the Vitest suite runs the built binary at `target/debug/fscale` and the built Node host.

## Layout

```
crates/core      Rust: the engine (model, config, git, walker, cache, rule trait, reports, Node bridge)
crates/rules     Rust: built-in rule packs
crates/cli       Rust: the fscale binary
crates/testkit   Rust: test-only helpers (a stand-in for node)
packages/cli     The firescale npm package: Node host for TS rules, config helpers
packages/sdk     Rule authoring API for custom TypeScript rules
packages/agents  Agent adapters (later)
bench/           Benchmarks and bench/RESULTS.md
docs/decisions/  Architecture decision records
```

## Licence

MIT
