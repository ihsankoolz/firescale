# fscale facts

`fscale facts [--json]` prints what firescale detects about the repo: package manager, languages, tools, commands and a simple project graph. It reads marker files on disk only. It does not load `firescale.config.*` and does not start Node. It always exits 0 unless usage is wrong (exit 2).

`--json` prints the format described by `packages/cli/schemas/facts-output.schema.json` (JSON Schema draft-07, published as `firescale/schemas/facts-output.schema.json`). Output is pretty-printed with 2 spaces and a trailing newline. All lists are sorted, and all paths use forward slashes.

## Fields

| Field            | Meaning                                                                                                                                                 |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `schemaVersion`  | Always `1`. Changes only on a breaking change.                                                                                                          |
| `packageManager` | `"pnpm"`, `"npm"`, `"yarn"`, `"bun"` or `null`.                                                                                                         |
| `lockfiles`      | Sorted lockfiles at the repo root: `Cargo.lock`, `bun.lock`, `bun.lockb`, `package-lock.json`, `pnpm-lock.yaml`, `poetry.lock`, `uv.lock`, `yarn.lock`. |
| `languages`      | Sorted subset of `javascript`, `python`, `rust`, `typescript`.                                                                                          |
| `tools`          | Sorted subset of `cargo-workspace`, `github-actions`, `nx`, `pnpm-workspace`, `turbo`.                                                                  |
| `commands`       | Object with optional keys `build`, `test`, `lint`, `typecheck`.                                                                                         |
| `projects`       | Sorted by name. Each has `name`, `root`, `kind` (`node` or `rust`) and `dependsOn`.                                                                     |
| `warnings`       | Strings. Things that were skipped.                                                                                                                      |

Each project: `root` is repo-relative with forward slashes (`.` is the repo root). `dependsOn` is the sorted list of other projects of the same kind in this repo.

## Detection rules

- **packageManager**: the name before `@` in the `packageManager` field of the root `package.json`, if it is one of the four. Otherwise by lockfile, first match of `pnpm-lock.yaml`, `package-lock.json`, `yarn.lock`, `bun.lock`, `bun.lockb`.
- **languages**: `package.json` gives javascript. A root `tsconfig.json`, or `typescript` in `dependencies` or `devDependencies`, gives typescript. `Cargo.toml` gives rust. `pyproject.toml` or `requirements.txt` gives python.
- **tools**: `nx.json` gives `nx` (the Nx graph itself is not read), `turbo.json` gives `turbo`, `pnpm-workspace.yaml` gives `pnpm-workspace`, a `[workspace]` table in `Cargo.toml` gives `cargo-workspace`, and a `.yml` or `.yaml` file in `.github/workflows` gives `github-actions`.
- **commands**: for each of build, test, lint and typecheck that is a script of the root `package.json`: `"<pm> run <script>"`. The package manager defaults to `npm`. Keys without a script are left out.
- **Node projects**: the root `package.json` if it has a `name`, plus workspace members. Members come from the `packages:` list of `pnpm-workspace.yaml`, or else from `workspaces` in `package.json` (an array, or an object with `packages`). A member is a folder with a `package.json` that has a `name`. `dependsOn` uses `dependencies`, `devDependencies`, `peerDependencies` and `optionalDependencies`.
- **Rust projects**: the root `[package]` if present, plus the `[workspace] members` of the root `Cargo.toml`. `dependsOn` uses the keys of `[dependencies]`, `[dev-dependencies]` and `[build-dependencies]`, including `x.workspace = true`, `x = { path = ... }` and `[dependencies.x]` tables.

## Limits

- The Nx project graph is not read yet. With `nx.json` only the tool `nx` is reported, and the graph comes from workspace files.
- Workspace patterns: only `dir/*`, literal paths and `!negated` entries. Anything else (`**`, `{a,b}`, `a/*/b`) is skipped with a warning. Folders named `node_modules`, `target` or starting with `.` are skipped by `dir/*`.
- There is no YAML parser. `pnpm-workspace.yaml` is read for the `packages:` list with items written as `- 'a/*'`, `- "a/*"` or `- a/*`. Flow style (`packages: [a, b]`) is not read.
- There is no TOML parser. `Cargo.toml` is read line by line: section headers, `key = value` lines (also dotted keys) and arrays that span lines. `exclude`, `default-members` and renamed dependencies (`package = "..."`) are ignored.
- A marker file that is not valid JSON (`package.json` of the root or a member) is reported in `warnings`, and its facts are left out.
- Only the repo root is checked for lockfiles, `tsconfig.json` and the Python markers. Python (uv) workspaces are not part of the graph yet.
