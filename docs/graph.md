# fscale graph

`fscale graph [--explain] [--json]` prints the project graph: the projects `fscale facts` finds, the folders and files they share, and the edges between them. `fscale verify --affected` uses it to run only the projects a change can reach (see [verify.md](verify.md)). It reads files only: it loads no config, starts no Node and runs no command. Decisions are in ADR 0032 (the graph) and ADR 0033 (tree-sitter).

An edge `a -> b` means **a depends on b: a change in b affects a**.

## Nodes

- **Projects**: every project of `fscale facts` (see [facts.md](facts.md)), with the id `<kind>:<folder>`, for example `python:backend/auth_service`.
- **Shared nodes**: a folder or a file that projects use but that belongs to no project, for example `backend/shared` that three services import, or a root `.env.example`. Id `shared:<path>`. **A change there affects only the projects that use it**, not every project. A shared folder's own imports are read too, so `backend/shared` importing `backend/common` makes `common` reach every user of `shared`.

## Edge sources, most trusted first

Each edge lists every piece of evidence for it, most trusted first, with its file and line. `--explain` prints them; `--json` always carries them.

| Tier | Source               | What gives the edge                                                                                                                                                                                                                             |
| ---- | -------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1    | `dependency`         | A dependency on another project of the repo by name, in `package.json` or `Cargo.toml` (as `dependsOn` in facts).                                                                                                                               |
| 1    | `requirements`       | In a Python project's requirements files: `-e ../x`, `--editable ../x`, a local path `../x` (with extras), `name @ file:../x`, and the files of `-r` and `-c`.                                                                                  |
| 1    | `pyproject`          | In `pyproject.toml`: `[tool.uv.sources]` and Poetry path dependencies (`path = "../x"`), and `file:` URLs in PEP 621, PEP 735 and PDM lists (`${PROJECT_ROOT}` and Hatch `{root:uri}` are understood).                                          |
| 1    | `package-json`       | In `package.json` dependency lists: `file:`, `link:`, `portal:`, and `workspace:` (by package name, or a relative path).                                                                                                                        |
| 1    | `cargo`              | A `path` dependency in `Cargo.toml`.                                                                                                                                                                                                            |
| 2    | `dockerfile`         | A `COPY` or `ADD` in a project's Dockerfile of a path outside the project. The build context comes from the Compose file that builds it; without one, it is the Dockerfile's folder, or the first folder above it where the copied path exists. |
| 2    | `compose-build`      | A Compose `build.context` outside the project the image builds (the owner of its Dockerfile, else of the context).                                                                                                                              |
| 2    | `compose-depends-on` | A Compose `depends_on` between two services that are both built from projects. Image-only services such as `postgres` are not nodes.                                                                                                            |
| 2    | `shared-input`       | A `.env.example` or OpenAPI spec (`openapi.yaml`, `swagger.json`, `*.openapi.yaml`...) that no project owns is an input of every project in its folder or below. A Compose file is an input of every project it builds.                         |
| 3    | `python-import`      | A Python import, read with tree-sitter, that resolves to another project or a shared folder (below).                                                                                                                                            |
| 3    | `js-import`          | A JavaScript or TypeScript relative import (`import`, `export ... from`, `require`, `import()` with a string), read with tree-sitter, that leaves the project's folder.                                                                         |
| 4    | `dynamic-import`     | `importlib.import_module`, `__import__` (also through an alias): the project gets a warning and counts as depending on every other Python project and every shared folder with Python code.                                                     |

### How a Python import resolves

Python finds modules on its import path, which the repo's files do not fully state. The graph follows what the layout says:

1. **The importer's own code**: the file's folder and the folders above it up to the project's folder, and its `src/`. A module found there is the project's own; no edge.
2. **A folder above the project**: `backend/` holding `shared/` for services that run with `backend/` on the import path. The first folder above the project that has the module (a `.py` file, or a folder with Python in it) wins.
3. **Another Python project's package**: a folder with an `__init__.py` in another project's folder or its `src/`.
4. Anything else is the standard library or an installed package: no edge.

Relative imports (`from ..common import x`) resolve from the file's folder. An imported file stands for the package it is part of: `backend/shared/db.py` gives the node `backend/shared`.

### How a JS or TS import resolves

Only relative specifiers (`./x`, `../x`) count; a bare package name is a dependency, covered by `package.json`. The path is tried as written, then with `.ts`, `.tsx`, `.d.ts`, `.js`, `.jsx`, `.mjs`, `.cjs`, `.json`, and a `.js` written for a `.ts` file. A target outside the project is an edge to the project that owns it, or to a shared node: the folder just below the deepest folder the two have in common (`../../shared/ui/x.ts` from `web/src` gives `shared`).

## What a change affects

`fscale verify --affected` marks as affected the projects and shared nodes that hold a changed file, then every node with an edge to an affected one, until nothing changes. A file in a shared folder affects that folder; a file in a folder of the repo root's project (when there is one) affects that project.

A changed file that no project and no narrow shared node holds, and that cannot change a build or a test, affects **no** project (ADR 0034): Markdown (`.md`, `.markdown`), text files (`.txt`, except names with `requirements` or `constraints`, and `CMakeLists.txt`), a licence (`LICENSE*`, `LICENCE*`) and anything under the root `docs/` folder. `check` still runs.

When unsure, more is affected, never less. These affect **every** project:

- a root lockfile, a CI file, a Compose file anywhere, or the firescale config;
- a file that no project and no narrow shared node holds. A shared node is **wide** when a project's folder is inside it (a Docker build context of `.`, say): it says that one project uses everything there, not that nobody else reads a file in it, so it does not count as an owner;
- with a repo root project, a file directly in the repo root folder that no shared node holds: settings that projects below read (`ruff.toml`, `tsconfig.base.json`, `.prettierrc`) live there.

An imported file gives the narrowest shared folder that holds no project: `../backend/shared/x.ts` from `frontend` gives `backend/shared`, not `backend`.

## Output

```
Projects (6):
  backend/audit_service [python]
  backend/auth_service [python]
  ...
  frontend [node]
Shared, used by projects but part of none (3):
  .env.example (file)
  backend/shared (folder)
  docker-compose.yml (file)
Edges (10), "a -> b" means a change in b affects a:
  backend/auth_service -> backend/shared (Python import)
      Python import: backend/auth_service/main.py:1: from shared.db import URL
  ...
```

`--json` prints the format of `packages/cli/schemas/graph-output.schema.json` (published as `firescale/schemas/graph-output.schema.json`): `schemaVersion` (1), `nodes` (`id`, `type` `project` or `shared`, `path`, `kind`, and `name` for a project), `edges` (`from`, `to`, `sources` with `source`, `tier`, `file`, `line` and `detail`) and `warnings`. `--json` and `--explain` together are a usage error. `fscale facts --json` carries the same `nodes` and `edges` under `graph`.

## Limits

- Files are found with the same skips as `fscale facts` (`.gitignore`, dot folders, dependency, build and fixture folders); files over 1 MiB and `*.min.js` are not read.
- TypeScript path aliases (`paths` in `tsconfig.json`) and Python import paths set at run time (`sys.path`, `PYTHONPATH`) are not read. An alias to a sibling folder gives no edge.
- uv workspace dependencies (`{ workspace = true }`) and Go `replace` directives are not read; Go projects get edges only from Docker and Compose.
- Paths are compared as written. On a file system that ignores case (Windows, macOS), a `COPY Backend/Shared` of `backend/shared` is not matched to changes there.
- Compose `volumes`, `env_file` and `extends` are not read. A Compose file is read line by line, as in `fscale doctor`.
- A Dockerfile that no Compose file builds and whose copied paths do not exist anywhere gets its own folder as its build context.
- `fscale check --affected` does not use this graph (see [affected.md](affected.md)).
