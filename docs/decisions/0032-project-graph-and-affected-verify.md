# 0032 — The project graph and affected verify

Status: accepted (M17). Extends ADR 0011 (facts and `--affected`) and ADR 0020 (projects in subfolders). Changes the verify JSON to schema 5. Follows the plan of ADR 0031.

## Context

On the course project a full `fscale verify` runs five Python services and a frontend, and one service's tests took 13 minutes. A change to one service should run that service. But the services share code: `backend/shared` is imported by most of them, and is not a project. The graph of 0.2.0 knew only workspace members by name, and nothing for Python, so it could not say which services a change in `backend/shared` reaches.

## Decision

### The graph

- **A separate module, built on demand** (`crates/core/src/graph.rs`). `detect_facts` does not build it: four rules call `detect_facts` on every `check`, and reading every source file there would slow each check. `fscale facts`, `fscale graph` and `verify --affected` build it.
- **Nodes are projects and shared nodes.** A shared node is a folder or file that a project uses but that no project owns. A change there affects only its users. Without it, `backend/shared` would be a file with no owner, and every change to it would run everything.
- **Edge sources, most trusted first**, each kept as evidence (file, line, the text) so `--explain` can show it:
  1. manifests: requirements files, `pyproject.toml` path sources, `package.json` `file:`/`link:`/`portal:`/`workspace:`, Cargo `path`, and dependencies by name (the old `dependsOn`);
  2. Dockerfile `COPY`/`ADD`, Compose `build.context` and `depends_on`, and shared inputs;
  3. Python imports and JS/TS relative imports, read with tree-sitter (ADR 0033);
  4. dynamic Python imports.
     All are kept, not only the most trusted, so one edge can say "pyproject.toml and a Python import".
- **Shared inputs.** A `.env.example` or OpenAPI spec that no project owns is an input of every project in its folder or below. That is the simplest reading of "read by several projects": a root `.env.example` belongs to every project below it, which is also how `core/env-example` reads it (ADR 0020). Finding the code that opens an OpenAPI spec would need a reader per language and per tool; the folder rule over-approximates safely. A Compose file is an input of each project it builds.
- **Where a path lands.** A path a manifest, Dockerfile or Compose file names is its own node (the folder or file). An imported file stands for its package: the folder just below the deepest folder importer and target share (`backend/shared/db.py` from `backend/auth` gives `backend/shared`). A target inside another project is that project.
- **Python resolution follows the layout**, since the import path is not written down: the importer's own folders and `src/` first (no edge), then the folders above the project (services run with `backend/` on the path), then other projects' packages (folders with `__init__.py`). Anything else is installed or standard: no edge.
- **Docker build contexts** come from the Compose file that builds the Dockerfile. Without one, the context is the Dockerfile's folder, or the first folder above it where the copied path exists. A variable or a wildcard copies the context itself.
- **Dynamic imports** (`importlib.import_module`, `__import__`, through aliases too) give one warning per project and edges to every other Python project **and every shared folder with Python in it**. The owner asked for "every sibling Python project"; shared folders were added because a dynamic import can load them as well, and leaving them out would make the importer miss a change there.
- **No new parser for TOML or YAML.** `pyproject.toml`, `Cargo.toml` and Compose files are read with the existing line readers, extended (`TomlEntry` now knows its line).

### What a change affects

- A changed file marks the projects that hold it (the deepest ones; two kinds can share a folder), every shared node that holds it, and the repo root's project when no other project holds it. Then every node with an edge to a marked node, until nothing changes.
- **Wide triggers, from the owner:** a file nothing holds, a root lockfile, a CI file, a Compose file (anywhere, not only at the root: an override file changes services as much), and the firescale config affect every project.
- **No base and a clean tree affects everything, with a note.** With nothing to compare, the honest answer is "unknown", which must not read as "nothing to do". With `--base` and no change, nothing is affected: that is a real comparison.
- `check --affected` keeps its own rules (ADR 0011). It hides findings in untouched code; reporting less there costs nothing, while making it report everything on a clean tree would change its tested meaning.

### Verify

- Units are the root's own steps and each project plan. A unit not affected reports each step as `not-affected`: a new status, not `skipped` (the step has a command) and not `off` (nothing turned it off). It is not counted in `stepsTotal`, gives no skip warning, and is one line in the summary.
- The root's own steps run when a project they cover is affected (the root's project or a workspace member of the kind they run). Nx and Turborepo pick their own projects and a config command can run anything, so they run whenever anything is affected. A folder named only in `verify.projects` is not in the graph, so it runs whenever anything is affected.
- **The headline counts projects and is never a plain OK:** `verify: OK (affected only) - 2 of 6 projects ran, 4 not affected`. A failure keeps its words and adds `(affected only: ...)`. The root's own steps count as one project. `result` keeps its meaning for the steps that ran; the new `scope` says `affected`, so a consumer that reads only `result` should read `scope` too.
- **Schema 5**: `scope`, `affected` (base, everything and its reason, counts, and every unit with its reason) and the status `not-affected`. Blame-aware verify takes 6 (ADR 0031).
- **`verify --dry-run`** prints the plan and runs nothing, not even the check. It was needed to run the planning on real repos without running their scripts, and it is how a person sees what `--affected` would do. No JSON form: the plan is for people, and the verify schema describes results.
- The stop hook does not change.

### `fscale graph`

`fscale graph` prints projects, shared nodes and edges; `--explain` adds every piece of evidence; `--json` follows `graph-output.schema.json` (schema version 1). `fscale facts` shows the same edges, under `Graph` and as `graph` in JSON. The facts schema stays at version 2: the field is new and optional, the policy of ADR 0022.

## Consequences

- On the course project a change in one service runs one of six projects; a change in `backend/shared` runs the three services that import it.
- A README change runs everything in a repo with no root project, because the README belongs to no project. That is the owner's rule; listed under "Needs your decision" in DECISIONS.md.
- The graph reads every source file once per `verify --affected`, `graph` and `facts`, in parallel: 20 to 50 ms on the three real repos with a release build.
- Missing edges are the risk M18's cache inherits (ADR 0031). The known gaps (TypeScript path aliases, `sys.path` changes at run time, uv workspace sources, Compose volumes) are listed in `docs/graph.md`.
- Tests changed on purpose, none weakened: verify snapshots and two assertions show schema version 5 and `"scope": "all"`; the facts snapshots of the course project show its graph; the help snapshot shows `graph`, `--explain` and verify's `--dry-run`. The course fixture's `main.py` in three services now imports `backend/shared`, in both copies of the fixture.
