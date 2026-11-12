# 0028 — Generators: `fscale add` and `fscale schema`

Status: accepted (M14).

## Decision

- **Where the code lives.** The generator interface and the two generators are TypeScript in `packages/cli/src/generators/`, so they ship inside the `firescale` npm package, next to the Node host that serves them. SPEC §9 names `packages/generators`; that package stays a placeholder whose README points here. Putting them in their own package would mean publishing an eighth package (or bundling a private one into `firescale`), with changes to the release scripts and their tests, for no gain to users.
- **The bridge.** Three new host methods, `listGenerators`, `generatorSchema` and `runGenerator`, on protocol version 1 (the methods are additive; an older host answers "unknown bridge method"). The host validates and generates; it **writes nothing**. The binary checks every path (`validate_relative`), refuses the whole run when any file exists, then creates each file with `create_new`. `--dry-run` prints a unified diff per file.
- **Validation.** A small validator for the subset of JSON Schema the generators use (string and boolean fields, `required`, `enum`, `pattern`, `maxLength`, `default`, `additionalProperties: false`), written in the package, rather than adding Ajv as a runtime dependency of `firescale`. The error names `input.<field>`, what it must be, and `fscale schema <generator>`. Ajv is only used in the tests, to prove each printed schema is a valid draft-07 schema that accepts the example input.
- **CLI shape.** `fscale add <generator> --json <input>` reuses the existing `--json` flag: with `add`, the word after the generator is the input. `fscale schema` with no name lists the generators.
- **The two generators.** `package` (a plain JavaScript ESM package with a `node:test` test, so it needs no toolchain) and `rule` (a custom TypeScript rule with SKILL.md and a Vitest test, in a pack created when missing). Neither edits an existing file: a workspace list or a `pack.ts` that needs a new line is a `Next:` note.
- **The skill.** The `agent-ready` pack ships `firescale-add`, installed by `agent-ready/agent-skills` (ADR 0027) for each agent, through `pack_skills`.
- **Default pack folder** for `rule`: `firescale-rules`, not `tools/rules` (the docs' example), so the files are not at `tools/rules/rules/<rule>`.

## Consequences

- `fscale add` and `fscale schema` start Node, as SPEC §4 marks them.
- Listed under "Needs your decision": the generator code living in `packages/cli`, and the default folders.
