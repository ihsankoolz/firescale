# fscale facts

`fscale facts [--json]` prints what firescale detects about the repo: package managers, languages, tools, commands and the projects, with a simple graph between them. It reads marker files on disk only. It does not load `firescale.config.*` and does not start Node. It always exits 0 unless usage is wrong (exit 2).

`--json` prints the format described by `packages/cli/schemas/facts-output.schema.json` (JSON Schema draft-07, published as `firescale/schemas/facts-output.schema.json`). Output is pretty-printed with 2 spaces and a trailing newline. All lists are sorted, and all paths use forward slashes.

## Fields

| Field             | Meaning                                                                                                                                                                           |
| ----------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `schemaVersion`   | Always `2`. Changes only on a breaking change. Version 2 added the per-project fields, `packageManagers`, the project kinds `python` and `go`, and projects found in subfolders.  |
| `packageManager`  | The JavaScript package manager of the repo root: `"pnpm"`, `"npm"`, `"yarn"`, `"bun"` or `null`.                                                                                  |
| `packageManagers` | Every package manager in use, the root's and each project's: a sorted subset of `bun`, `cargo`, `go`, `npm`, `pip`, `pipenv`, `pnpm`, `poetry`, `uv`, `yarn`.                     |
| `lockfiles`       | Sorted lockfiles at the repo root: `Cargo.lock`, `Pipfile.lock`, `bun.lock`, `bun.lockb`, `go.sum`, `package-lock.json`, `pnpm-lock.yaml`, `poetry.lock`, `uv.lock`, `yarn.lock`. |
| `languages`       | Sorted subset of `go`, `javascript`, `python`, `rust`, `typescript`: the root's languages and every project's.                                                                    |
| `tools`           | Tools of the repo root: a sorted subset of `cargo-workspace`, `docker-compose`, `github-actions`, `nx`, `pnpm-workspace`, `turbo`.                                                |
| `commands`        | Commands to run from the repo root: optional keys `build`, `test`, `lint`, `typecheck`, one for each script of that name in the root `package.json` (see "Watch-mode scripts").   |
| `projects`        | Sorted by name. See below.                                                                                                                                                        |
| `warnings`        | Strings. Things that were skipped, and things that may go wrong, such as a watch-mode script with no run-once sibling (see "Watch-mode scripts").                                 |
| `notes`           | Sorted strings. Things worth knowing that need no action, such as a watch-mode script replaced by its run-once sibling. Always printed, `[]` when there are none.                 |

Each project:

| Field            | Meaning                                                                                                                         |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| `name`           | The name in the project's manifest, or else the name of its folder. `.` for a project at the repo root with no name of its own. |
| `root`           | Repo-relative folder with forward slashes. `.` is the repo root.                                                                |
| `kind`           | `node`, `rust`, `python` or `go`.                                                                                               |
| `source`         | `root` (it is the repo root), `workspace` (a workspace file at the root lists it) or `discovered` (found in a subfolder).       |
| `languages`      | The project's languages.                                                                                                        |
| `packageManager` | The project's package manager, or `null` for a Node project that names none and has no lockfile.                                |
| `lockfiles`      | Names of the lockfiles in the project's folder.                                                                                 |
| `tools`          | Linters, test runners and other tools found in the project's folder.                                                            |
| `commands`       | Commands to run **from the project's folder**: optional keys `install`, `build`, `test`, `lint`, `typecheck`.                   |
| `dependsOn`      | Sorted names of other projects of the same kind in this repo that this one depends on.                                          |
| `interpreter`    | Python projects only: `source`, `command` and `detail` of the interpreter the commands use. See "The Python interpreter" below. |

## Projects in subfolders

Many repos keep their code below the root with no workspace file: a `backend/` with a few services, a `frontend/` with an app. `fscale facts` finds these.

- **What is searched.** Every folder down to **four levels** below the repo root. `backend/auth_service` is level 2.
- **What is skipped.** Everything `.gitignore` ignores (only `.gitignore` files inside the repo count, so the answer is the same on every machine), every folder whose name starts with a dot (`.venv`, `.pytest_cache`, `.ruff_cache`, `.git`), and these folders: `node_modules`, `bower_components`, `vendor`, `site-packages`, `venv`, `env`, `__pycache__`, `dist`, `build`, `out`, `target`, `coverage`, `htmlcov`, `test-results`, `playwright-report`, `tmp`, `temp`, `logs`, and the folders of samples and test input `fixtures`, `__fixtures__`, `test-fixtures`, `testdata`, `examples`, `example`, `samples`, `templates`. Symbolic links are not followed.
- **What makes a folder a project.**
  - `package.json`: a `node` project.
  - `Cargo.toml`: a `rust` project.
  - `go.mod`: a `go` project.
  - `setup.py`, `Pipfile`, or a `pyproject.toml` that describes a package (it has a `[project]`, `[tool.poetry]` or `[build-system]` table): a `python` project. So is a `requirements.txt` together with a `.py` file or one of `pyproject.toml`, `pytest.ini`, `tox.ini`, `setup.cfg`, `conftest.py`, `ruff.toml`, `.ruff.toml`, `mypy.ini`, `manage.py`, `alembic.ini`.
  - A `requirements.txt` with nothing beside it is not a project: it is often only the input of a docs build or a container image. A `pyproject.toml` with only `[tool.*]` tables is not one either: it is shared tool config, often placed above the real projects, and the search goes on below it.
  - A `package.json` that is not valid JSON still makes a project. It has no commands, and the problem is listed under `warnings`.
  - A folder with marker files of two kinds is two projects with the same `root`.
- **Nested projects.** A project's folder is not searched any further. A marker file inside a project (an example, a fixture, a `fuzz/` crate) belongs to that project. The repo root is the one exception: it is always searched.
- **Workspace files decide.** When the root declares a Node workspace (`pnpm-workspace.yaml`, or `workspaces` in `package.json`), the Node projects are its members and no others are looked for; a folder the workspace leaves out stays out. The same goes for a Cargo workspace and Rust projects. Projects of the other kinds are still searched for.
- **Deeper projects.** A project below level 4 is not found. Name its folder under `verify.projects` in the config to have `fscale verify` run commands there (see [verify.md](verify.md)).

## Detection rules

At the repo root:

- **packageManager**: the name before `@` in the `packageManager` field of the root `package.json`, if it is one of the four. Otherwise by lockfile, first match of `pnpm-lock.yaml`, `package-lock.json`, `yarn.lock`, `bun.lock`, `bun.lockb`.
- **languages**: `package.json` gives javascript. A root `tsconfig.json`, or `typescript` in `dependencies` or `devDependencies`, gives typescript. `Cargo.toml` gives rust. `pyproject.toml` or `requirements.txt` gives python. `go.mod` gives go. The languages of every project are added.
- **tools**: `nx.json` gives `nx` (the Nx graph itself is not read), `turbo.json` gives `turbo`, `pnpm-workspace.yaml` gives `pnpm-workspace`, a `[workspace]` table in `Cargo.toml` gives `cargo-workspace`, a `.yml` or `.yaml` file in `.github/workflows` gives `github-actions`, and `docker-compose.yml`, `docker-compose.yaml`, `compose.yml` or `compose.yaml` gives `docker-compose`.
- **commands**: for each of build, test, lint and typecheck that is a script of the root `package.json`: `"<pm> run <script>"`. The package manager defaults to `npm`. Keys without a script are left out. These are scripts only, because rules such as `agent-ready/commands-documented` compare them with what the repo documents. A test, lint or typecheck script that watches for changes gives way to its run-once sibling, as in a project (see "Watch-mode scripts").

For each project, from the files in its own folder:

- **Node** (`package.json`)
  - Package manager: the `packageManager` field, else a lockfile in the folder, else the root's package manager for a workspace member, else `null`.
  - Languages: javascript, plus typescript with a `tsconfig.json` or a `typescript` dependency.
  - Tools, each from a config file or a dependency: `biome` (`biome.json`), `cypress` (`cypress.config.*`), `eslint` (`eslint.config.*`, `.eslintrc*`), `jest` (`jest.config.*`), `knip` (`knip.json`, `knip.jsonc`, `knip.config.*`), `next` (`next.config.*`), `oxlint` (`.oxlintrc.json`, `oxlint.config.*`), `playwright` (`playwright.config.*`), `prettier` (`.prettierrc*`, `prettier.config.*`), `vite` (`vite.config.*`), `vitest` (`vitest.config.*`, `vitest.workspace.*`), and `docker` (`Dockerfile`).
  - install: `<pm> install` (`npm install` when the package manager is not known). Not for a workspace member, which the root installs, nor for a `package.json` that cannot be read.
  - Commands: a script named `build`, `test`, `lint` or `typecheck` gives `"<pm> run <script>"`, or `"<pm> run <step>:run"` when the script watches for changes (see "Watch-mode scripts"). Without a script, the tools give the command:
    - lint: `oxlint`, else `eslint .`, else `biome check .`, and only when that linter's config file is in the folder. A linter that is only a dependency gives no command.
    - test: `vitest run`, else `jest`. Playwright and Cypress are not used as the test command: end-to-end tests need browsers and running services.
    - typecheck: `tsc --noEmit` when there is a `tsconfig.json` without `references`. Not at the repo root, where a tsconfig is often only a base for the packages below it. A tsconfig of project references gets no command: `tsc --noEmit` would check nothing there, and `tsc -b` could write files.
    - Tools run through `pnpm exec`, `yarn`, `bunx`, or `npx --no` for npm. `npx --no` never downloads a tool that is not installed.
- **Python**
  - Package manager: `uv` (`uv.lock` or `[tool.uv` in `pyproject.toml`), `poetry` (`poetry.lock` or `[tool.poetry`), `pipenv` (`Pipfile`), else `pip`.
  - Tools: `pytest` (`pytest.ini`, `conftest.py`, `[tool.pytest` in `pyproject.toml`, `[pytest]` in `tox.ini`, `[tool:pytest]` in `setup.cfg`, or `pytest` in a dependency list), `ruff` (`ruff.toml`, `.ruff.toml` or `[tool.ruff` in `pyproject.toml`, in the folder or any folder above it, or `ruff` in a dependency list), `mypy` (`mypy.ini`, `.mypy.ini` or `[tool.mypy`, in the folder or above, `[mypy` in `setup.cfg`, or `mypy` in a dependency list), `alembic` (`alembic.ini`), `docker` (`Dockerfile`).
  - Dependency lists are read in the project's own folder only, never above it:
    - requirements files: every `.txt` file whose name contains `requirements` (`requirements.txt`, `requirements-dev.txt`, `dev-requirements.txt`), and every `.txt` file in a `requirements/` folder. A `-r <file>` or `--requirement <file>` line is followed, relative to the file it is in, up to 5 levels deep, and only to a file inside the repo. Constraint files (`-c`), editable installs (`-e`), other options and URLs are skipped.
    - `pyproject.toml`: the arrays `dependencies` under `[project]`, every list under `[project.optional-dependencies]`, `[dependency-groups]` (an `{include-group = …}` entry is skipped) and `[tool.pdm.dev-dependencies]`, and `dev-dependencies` under `[tool.uv]`; the keys of `[tool.poetry.dependencies]`, `[tool.poetry.dev-dependencies]` and `[tool.poetry.group.<name>.dependencies]`.
    - `Pipfile`: the keys of `[packages]` and `[dev-packages]`.
    - A name is compared without its version, extras or markers, ignoring case and treating `_` as `-`: `Ruff>=0.4` and `ruff[toml]` are `ruff`, but `ruff-lsp` is not.
  - Commands: test is `pytest`, lint is `ruff check .`, typecheck is `mypy .`, each only when the tool was found. They start with the project's interpreter (below): `.venv/bin/python -m pytest`, `uv run pytest`, `python -m pytest`.
  - install: `<interpreter> -m pip install -r <file>` with the first of `requirements-dev.txt`, `requirements_dev.txt`, `dev-requirements.txt`, `requirements-test.txt`, `requirements_test.txt`, `test-requirements.txt`, `requirements.txt` in the folder. When that is a development list without a `-r requirements.txt` line and a `requirements.txt` is there, both are installed: `-r requirements.txt -r requirements-dev.txt`. Without a requirements file, `<interpreter> -m pip install -e .` for a `pyproject.toml` or `setup.py`. A uv, poetry or Pipfile project gets `uv sync`, `poetry install` or `pipenv install --dev`, also when its `.venv` runs the commands (a venv made by uv has no pip).
  - Name: `name` under `[project]` or `[tool.poetry]` in `pyproject.toml`, else the folder's name.
- **Rust** (`Cargo.toml`): package manager `cargo`; commands `cargo build`, `cargo test` and `cargo clippy --all-targets -- -D warnings`, each with `--workspace` when the file has a `[workspace]` table.
- **Go** (`go.mod`): package manager `go`; commands `go build ./...`, `go test ./...` and `go vet ./...`. The name is the last part of the module path.

### Watch-mode scripts

A script such as `"test": "vitest"` starts the tool in watch mode: in a terminal it never exits. `fscale verify` runs the commands `fscale facts` lists, so for the `test`, `lint` and `typecheck` steps of a Node project, and of the repo root, facts looks for a script that runs once instead.

- **Watch commands.** Each command of the script is read, split at `&&`, `||`, `;` and `|`: its words after any `NAME=value`, and after `npx` (with its options), `pnpm exec`, `yarn`, `yarn exec` or `bunx`. A script with a command that starts with one of these watches:
  - `vitest`, `vitest watch` or `vitest dev`, also with filters and options, unless it has `run`, `--run`, `--watch=false` or `--no-watch`. Other subcommands (`related`, `bench`, `list`, `init`, `typecheck`) do not count.
  - `jest --watch` or `jest --watchAll`.
  - `react-scripts test`, unless it has `--watchAll=false`.
  - `ng test` or `ng t` (Karma watches by default), unless it has `--watch=false` or `--no-watch`.
  - `mocha --watch` or `mocha -w`, `ava --watch` or `ava -w`.
  - `node --test --watch`.
  - `tsc --watch` or `tsc -w`.
  - `vue-cli-service test:unit --watch`.
- **Run-once siblings.** When the step's script watches, the first of `<step>:run`, `<step>:ci` and `<step>:once` that exists and does not watch itself is used instead: `npm run test:run` in place of `npm run test`, with the same package manager. A note says so:

  ```text
  frontend: the test script runs `vitest`, which watches for changes; verify uses test:run (`vitest run`) instead.
  ```

- **No sibling.** The step keeps its own script, and a warning says so. `fscale verify` sets `CI=1` for every step, which turns watch mode off for vitest and react-scripts:

  ```text
  frontend: the test script runs `vitest`, which watches for changes and may never exit. verify runs it with CI=1, which turns watch mode off for most tools (vitest, react-scripts); add a test:run script that runs once to be sure.
  ```

  For the other tools the warning says that `CI=1` does not help, and asks for a `<step>:run` script.

- Messages start with the project's folder, or `repo root`. Human output lists them under `Notes:` and `Warnings:`; JSON under `notes` and `warnings`.
- The repo root's `commands` follow the same rule, so the root's test command can be `npm run test:run`. That is what `agent-ready/commands-documented` then asks `AGENTS.md` to name, because it is the command an agent should run.
- Scripts are not followed: a `"test": "npm run test:unit"` is not looked into, and a `build` script is never replaced.

### The Python interpreter

Each Python project's commands run with one interpreter, chosen in this order (ADR 0022):

| Order | `source`                   | When                                                                                                                                                    | Commands start with                                               |
| ----- | -------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------- |
| 1     | `venv`                     | A `.venv` folder, else a `venv` folder, in the project's folder holds an interpreter: `Scripts\python.exe` on Windows, `bin/python` on macOS and Linux. | `.venv/bin/python -m` (`.venv\Scripts\python.exe -m` on Windows)  |
| 2     | `uv`, `poetry` or `pipenv` | The project is a uv, poetry or Pipfile project (see "Package manager" above). Its manager picks the environment.                                        | `uv run`, `poetry run`, `pipenv run`                              |
| 3     | `active`                   | `VIRTUAL_ENV` names a folder that holds an interpreter, as it does in a shell where a virtual environment is activated.                                 | that interpreter's full path and `-m`, quoted when needed (below) |
| 4     | `path`                     | None of the above: no virtual environment was found. The commands are what they were before 0.1.2, and `detail` and `fscale verify` say so.             | `python -m`                                                       |

Why this order: a virtual environment in the project's own folder is the most specific, and running its interpreter directly never installs anything. uv and poetry decide for themselves which environment a project uses, and both prefer the project's own over an activated one. An activated environment belongs to the shell, not to one project, so it only fills in for projects that name none. `fscale facts` shows the choice as `interpreter: ...` under each Python project.

A `.venv` with no interpreter for this platform (for example one made in WSL and used from Windows) is not used, and `detail` says so.

On Windows the path in a command uses backslashes, because `cmd` cannot start a program written as `.venv/Scripts/python.exe`. Commands that name an interpreter (`.venv\Scripts\python.exe`, or the full path of an active environment) are the one place where fscale output has a backslash in a path. An active environment's path is quoted when it has a space or a shell character: in double quotes, or on macOS and Linux in single quotes when it holds `$`, a backquote, `"`, `\` or `!`. On Windows an active environment whose path has `%` or `"` is skipped, because `cmd` expands `%NAME%` even inside quotes.

The graph:

- **Node projects**: the root `package.json` if it has a `name`, plus workspace members. Members come from the `packages:` list of `pnpm-workspace.yaml`, or else from `workspaces` in `package.json` (an array, or an object with `packages`). A member is a folder with a `package.json` that has a `name`. `dependsOn` uses `dependencies`, `devDependencies`, `peerDependencies` and `optionalDependencies`.
- **Rust projects**: the root `[package]` if present, plus the `[workspace] members` of the root `Cargo.toml`. `dependsOn` uses the keys of `[dependencies]`, `[dev-dependencies]` and `[build-dependencies]`, including `x.workspace = true`, `x = { path = ... }` and `[dependencies.x]` tables.
- **Python and Go projects** have no `dependsOn`: names are not how they depend on each other.
- **The full graph** is in `graph`: nodes for projects and for the shared folders and files they use, and edges from manifests, Dockerfiles, Compose files and imports, each with its evidence. It is the same graph `fscale graph --json` prints, and is described in [graph.md](graph.md). Its warnings (dynamic imports) are added to `warnings`. Human output lists the edges under `Graph`. `dependsOn` keeps its old meaning.

## Limits

- The Nx project graph is not read yet. With `nx.json` only the tool `nx` is reported, and the graph comes from workspace files.
- Workspace patterns: only `dir/*`, literal paths and `!negated` entries. Anything else (`**`, `{a,b}`, `a/*/b`) is skipped with a warning. Folders named `node_modules`, `target` or starting with `.` are skipped by `dir/*`.
- There is no YAML parser. `pnpm-workspace.yaml` is read for the `packages:` list with items written as `- 'a/*'`, `- "a/*"` or `- a/*`. Flow style (`packages: [a, b]`) is not read.
- There is no TOML parser. `Cargo.toml` and `pyproject.toml` are read line by line: section headers, `key = value` lines (also dotted keys) and arrays that span lines. `exclude`, `default-members` and renamed dependencies (`package = "..."`) are ignored. Python tool sections are found by searching the text for `[tool.<name>`.
- A marker file that is not valid JSON (`package.json` of the root, a member or a discovered project) is reported in `warnings`, and its facts are left out.
- A project found in a subfolder that is itself a workspace (a `frontend/` with `workspaces`, a `backend/` with a Cargo `[workspace]`) is one project. Its members are not listed.
- Two projects can have the same name, for example two folders both called `api`. `root` tells them apart.
- With no virtual environment found, Python commands use `python` from the PATH. On a machine with only `python3`, create a `.venv` in the project's folder, or set the command under `verify.projects`.
- `VIRTUAL_ENV` is read when `fscale` starts, so `fscale facts` and `fscale verify` describe the shell they run in.
- uv workspaces are not part of the graph yet.
- With bun, uv, poetry and pipenv the commands go through `bunx`, `uv run`, `poetry run` and `pipenv run`. Those can install what is missing or create a virtual environment. Only `npx --no` is guaranteed not to. `tsc --noEmit` writes a `.tsbuildinfo` file when the tsconfig turns on `incremental` or `composite`.
- The search has no config: it follows `.gitignore` and the list above. A folder that should not count as a project can be turned off for `fscale verify` with `verify.projects`.
