# Generators: fscale add and fscale schema

Generators create new files from JSON input, so people and agents scaffold the same way every time (SPEC §16, ADR 0028). They run in Node, through the same bridge as custom rules; the `fscale` binary validates nothing itself and writes the files.

```sh
fscale schema                       # list the generators
fscale schema package               # the input JSON Schema of one
fscale add package --json '{"name": "@acme/utils"}' --dry-run
fscale add package --json '{"name": "@acme/utils", "description": "Small helpers."}'
```

- **The input is checked against the schema before anything is written.** A wrong field stops the run with an error that names it: `fscale add package: input.name must match ^(@[a-z0-9][a-z0-9._-]*/)?[a-z0-9][a-z0-9._-]*$, got "Bad Name". Run fscale schema package to see the input it takes.` (exit 2).
- **`--dry-run` prints a diff of every file it would create and changes nothing.**
- **It never writes over a file.** If any file it would create exists, nothing is written and the error names the files.
- Files are created, not staged. `Next:` lines say what is left to do by hand (for example, adding the new package to the workspace list). Run `fscale check` afterwards.

## The generators

### `package`: a workspace package

| Field         | Required | Meaning                                                                        |
| ------------- | -------- | ------------------------------------------------------------------------------ |
| `name`        | yes      | The npm name, `utils` or `@acme/utils`.                                        |
| `folder`      | no       | Where it goes, relative to the root. Default: `packages/<name without scope>`. |
| `description` | no       | One sentence for `package.json` and the README.                                |

Creates `package.json` (private, ESM, `"test": "node --test"`), `src/index.js`, `test/index.test.js` and `README.md`. When the repo's workspace list (`pnpm-workspace.yaml` or `"workspaces"` in `package.json`) does not cover the folder, it says to add it; it does not edit the list.

### `rule`: a custom firescale rule

| Field         | Required | Meaning                                                                  |
| ------------- | -------- | ------------------------------------------------------------------------ |
| `id`          | yes      | `<pack>/<rule>`, lower case with hyphens: `team/no-todo-file`.           |
| `description` | yes      | One sentence: what must be true in the repo.                             |
| `severity`    | no       | `blocker`, `high`, `medium` (default) or `low`.                          |
| `category`    | no       | `security`, `reliability`, `consistency` or `maintainability` (default). |
| `packFolder`  | no       | The pack's folder. Default: `firescale-rules`.                           |

Creates `<packFolder>/rules/<rule>/rule.ts` (a `defineRule` with an empty check), `SKILL.md` (with every section `validateSkill` asks for) and `rule.test.ts` (a Vitest fixture test with the failing and fixed cases as `todo`), and `<packFolder>/pack.ts` when there is none. An existing `pack.ts` is not edited; the `Next:` line says what to add. Turn the pack on by listing `"./<packFolder>/pack.ts"` in `packs` ([writing-rules.md](writing-rules.md)).

## For agents

The `agent-ready` pack installs a skill, `firescale-add`, in `.claude/skills/`, `.agents/skills/` and `.cursor/skills/`. It tells an agent to list the generators, read the schema, dry-run, then run `fscale add` instead of writing the boilerplate by hand.

## Limits

- Two generators. More come with the stack packs.
- The input schema supports what the generators use: objects of string and boolean fields, `required`, `enum`, `pattern`, `maxLength` and `default`.
- Generators come with the `firescale` npm package; custom generators are not supported yet.
