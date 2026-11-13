# 0033 — tree-sitter for reading imports

Status: accepted (M17). Brings forward part of the tamper guard's dependency decision (SPEC §8, PLAN M23).

## Context

The project graph (ADR 0032) needs the imports of Python, JavaScript and TypeScript files. The owner asked for a real parser, not patterns: an import inside a string, a comment, a docstring or a template literal must not count, and a JavaScript regex literal cannot be told from a division without parsing. The tamper guard planned for M23 needs a syntax tree of the same languages and already names tree-sitter (SPEC §8).

Options considered:

- **tree-sitter** with its Python and TypeScript grammars. One API for every language the tamper guard will need. C code compiled by the `cc` crate, which the build already uses for `blake3`'s x86 code.
- **Pure-Rust parsers** (`rustpython-parser` for Python, `oxc_parser` or `swc` for JS/TS): two unrelated APIs and large dependency trees, and the tamper guard would still need tree-sitter or a third parser.
- **A hand-written tokenizer**: smaller, but it is exactly the "pattern" approach the owner ruled out, and it would be wrong on regex literals and nested templates.

## Decision

- `tree-sitter =0.26.3`, `tree-sitter-python =0.25.0`, `tree-sitter-typescript =0.23.2`, pinned exactly. `tree-sitter-language` is pinned to `=0.1.7`: 0.1.9 declares Rust 1.90, above the workspace's declared 1.85, and resolver 2 would pick it.
- **No JavaScript grammar.** The TSX grammar parses `.js`, `.jsx`, `.mjs` and `.cjs` (TypeScript is a superset, and TSX allows JSX); `.ts`, `.mts`, `.cts` use the TypeScript grammar, since `<T>x` casts are not JSX there. One grammar fewer in the binary.
- tree-sitter 0.27 needs Rust 1.90; 0.26.3 is kept until the workspace's declared minimum moves.
- The readers live in `crates/core/src/imports.rs` and walk the tree by hand (no queries). The walk is a loop, so deeply nested input cannot overflow the stack.

## Cost (measured on the build machine, Linux x64, 4 cores)

| Release build of `fscale`          | Before (0.2.0) | After (M17) |
| ---------------------------------- | -------------- | ----------- |
| Binary size                        | 3,908,688 B    | 8,213,104 B |
| Clean `cargo build --release` time | 51 s           | 69 s        |
| `fscale --version`, p50 of 30 runs | —              | 2.8 ms      |

Most of the 4.3 MB is the grammars' parse tables (TypeScript and TSX are each over 1 MB); the graph code and the reference readers are the rest. The npm package of each platform grows by the same amount. Startup is not affected: tables are only touched when a file is parsed.

## Consequences

- The tamper guard (M23) starts with the dependency in place; its ADR adds the Rust grammar and re-measures.
- Any platform that cannot compile C cannot build fscale. The two cross-compiled release targets (macOS x64 on an Arm runner, Windows Arm64 on an x64 runner) now compile C for the target as well; a Release run started by hand built both on 2026-11-13 (DECISIONS.md, "M17 follow-up").
- A grammar upgrade can change node kinds; `imports.rs` has tests for every form it reads, so a change shows up as a failing test.
