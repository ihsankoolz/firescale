# fscale doctor

`fscale doctor` checks whether **this machine** is ready to work on the repo: the tools it declares, the env variables its `.env.example` files name, the installed dependencies of each project, and the local services it expects (from Compose files, `.env.example`, and the local database URLs in your `.env` and `.env.local`). It looks at the machine, not at the code; `fscale check` and `fscale verify` look at the code (SPEC §17, ADR 0026).

```bash
fscale doctor
fscale doctor --json
```

It exits **0** when nothing is wrong (warnings are allowed) and **1** when something is. Every problem comes with a `next:` line, the command or step that fixes it:

```
Tools
  ok       node: node 22.11.0 matches >=22.0.0 (package.json (engines.node))
  PROBLEM  pnpm: pnpm is not installed, or not on the PATH; package.json (packageManager) asks for pnpm@9.12.0.
           next: npm install -g pnpm@9.12.0

Environment variables
  PROBLEM  backend/.env.example: 2 of 6 variables are not set in the environment or backend/.env: JWT_SECRET, SMTP_HOST.
           next: add JWT_SECRET, SMTP_HOST to backend/.env (see backend/.env.example for what each one is)

Dependencies
  ok       backend/.venv: virtual environment with Python 3.12.4
  PROBLEM  backend packages: 1 of 14 packages in backend/requirements-dev.txt are not installed: pytest-asyncio.
           next: in backend: .venv/bin/python -m pip install -r requirements-dev.txt
  ok       frontend node_modules: all 31 packages in frontend/package.json are installed

Services
  PROBLEM  PostgreSQL on localhost:5432: not reachable: nothing listens on port 5432 (docker-compose.yml (service db)).
           next: docker compose up -d db

doctor: 3 problems - run the `next:` commands above, then `fscale doctor` again.
```

Doctor changes nothing and downloads nothing. The only programs it runs are version commands (`node --version`, `pnpm --version`, the project's Python with `--version`, `go version`, `rustup toolchain list`), each with a 10-second limit, with `COREPACK_ENABLE_NETWORK=0` and `GOTOOLCHAIN=local` so neither corepack nor Go fetches anything. It opens a TCP connection to each service's port, which it closes at once.

## What it checks

### Tools

| Declared in                                                                                 | Checked against                                                                                                                                                                                                                                                         |
| ------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `engines.node` in a `package.json`, `.nvmrc`, `.node-version`                               | `node --version`. npm ranges are read: `>=22`, `^20 \|\| ^22`, `>=18 <=22`, `22.x`, `~1.2`, `14 - 16`. A bare `22` matches any `22.*.*`, and `<=22` any version below 23.                                                                                               |
| `packageManager` (`pnpm@9.12.0`)                                                            | `<manager> --version`. A different major version is a problem; the same major with another minor or patch is a warning. A manager in use but not declared is only checked to be installed.                                                                              |
| `rust-toolchain.toml` (`[toolchain] channel`) or `rust-toolchain`                           | `rustup toolchain list`, run outside the repo so rustup never reads the file and never downloads a toolchain. Without rustup, `rustc --version`.                                                                                                                        |
| `.python-version` (in the project's folder or above), `requires-python` in `pyproject.toml` | The project's Python: its `.venv` (or `venv`), else `python3` (`python` on Windows) from the PATH. Python specifiers are read: `>=3.10,<4`, `~=3.11`. For uv, Poetry and Pipenv projects with no environment yet, the version is left to the manager, which fetches it. |
| `go` in `go.mod`                                                                            | `go version`: at least that version.                                                                                                                                                                                                                                    |

A tool that is missing is a problem with the page or command that installs it.

### Environment variables

For each `.env.example` at the root and in each project's folder, every name it lists must be set (to something) in the environment, or be present in `.env` or `.env.local` beside it. **For this check only names are read from those files; values are dropped as each line is read, and never printed.** (The services check below reads one more thing from them: the host and port of a local database URL.) The fix is to copy the example to `.env` and fill it in, or to add the missing names to the `.env` that exists.

### Dependencies

- **Node projects:** every package in `dependencies` and `devDependencies` (except `workspace:`, `file:` and `link:` ones) has a folder in `node_modules`, here or in a folder above, as Node itself looks them up. When all are there, the lockfile must not be newer than the file the package manager writes on each install (`node_modules/.package-lock.json` for npm, `.modules.yaml` for pnpm, `.yarn-integrity` or `.yarn-state.yml` for Yarn, `.bun-tag` for Bun); if it is, the lockfile changed since the last install. The fix is the project's install command from `fscale facts`, in the form that installs what the lockfile says and never rewrites it when the project has one (see [Install commands that keep the lockfile](#install-commands-that-keep-the-lockfile)).
- **Python projects:**
  - A virtual environment, `.venv` or `venv`, in the project's folder (for a uv workspace member, at the workspace root). With none there, an active `VIRTUAL_ENV` is checked instead, as verify uses it. No environment is a problem, with the commands that make one and install into it. An environment whose interpreter does not run (made on another machine, or with a Python that is gone) is a problem too. For Poetry and Pipenv, which may keep it elsewhere, a missing one is a warning.
  - Every package in the requirements files that the project's install command names (and the files they include with `-r`), else every `requirements*.txt` in its folder, else `[project] dependencies` in `pyproject.toml`, is installed in that environment (read from its `.dist-info` folders), and a version pinned with `==` matches (`2.31` and `2.31.0` are the same). Requirements with an environment marker (`; python_version < "3.11"`) are not checked.
  - Repos with several services, each with its own `requirements.txt`, `requirements-dev.txt` and `.venv`, are checked service by service.
- **Rust and Go:** not checked; Cargo and Go fetch dependencies when they build.

### Services

Local services the repo expects, read by the same code as verify's "service not running" check (`docs/verify.md`):

- **Compose files** (`compose.yaml`, `compose.yml`, `docker-compose.yaml`, `docker-compose.yml`, each with its `.override` file) in each project's folder and every folder above it, up to the root: each service that publishes a port on this machine and is a known database, cache, queue or search service (PostgreSQL, MySQL, MariaDB, MongoDB, Redis, Valkey, RabbitMQ, Elasticsearch, OpenSearch, Memcached, MinIO, Kafka, NATS, SQL Server), by its image or its name. The app's own containers (a backend, a proxy, an admin page) are left out unless an `.env.example` names their port. The fix is `docker compose up -d <service>` (with `-f <folder>/<file>` when the file is not at the root).
- **`.env.example` files:** URLs with a service scheme (`postgresql://`, `postgres+asyncpg://`, `mysql://`, `redis://`, `mongodb://`, `amqp://`...) and pairs such as `REDIS_HOST=localhost` with `REDIS_PORT=6379` (also `_SERVER` and `_HOSTNAME`). Only hosts on this machine count: `localhost`, `127.0.0.1`, `::1`, `0.0.0.0`. Without a Compose file, the fix names a `docker run` command for the service, with `<password>` for you to fill in.
- **`.env` and `.env.local` files** in the same folders as the `.env.example` files above (each project's folder and every folder above it, up to the root), even when there is no `.env.example`: a test database that only your own `.env` names, such as `TEST_DATABASE_URL=postgresql+asyncpg://...@localhost:5433/test`, is checked too. Only **URL values** with a service scheme (the same schemes as above, such as `postgresql+asyncpg://`, `mysql+pymysql://`, `redis://`, `rediss://`, `mongodb://`, `amqp://`; never `mongodb+srv://`) whose host is `localhost`, `127.0.0.1` or `::1` (also `[::1]`) are taken; `HOST`/`PORT` pairs and every other variable are not. **Of each URL only the variable's name, the host and the port are kept** (the scheme's usual port when it names none); the user, the password, the database name and the URL itself are dropped as the line is read, so they cannot be printed. The source reads like `backend/.env (TEST_DATABASE_URL)`. A port that a Compose file or `.env.example` already names is one service, with that file as its source. Verify's watch for a service that is down while a step runs does not read these files.

Each one is **reachable** or **not reachable**. **Reachable means only that something accepts a connection on that host and port**: not that it is the right database, that the database the tests use exists, or that the user and password in your settings can log in. A wrong login shows up when the tests run, and verify reports it as setup needed (see [verify.md](verify.md)). Only the service name, the host and the port are shown. User names, passwords, database names and connection strings are never kept, so they cannot be printed; a test proves it, for `.env.example`, `.env` and `.env.local` alike.

## JSON

`fscale doctor --json` prints the same checks. The schema is [`packages/cli/schemas/doctor-output.schema.json`](../packages/cli/schemas/doctor-output.schema.json).

| Field              | Meaning                                                                                    |
| ------------------ | ------------------------------------------------------------------------------------------ |
| `schemaVersion`    | Always `1`. Changes only on a breaking change.                                             |
| `ok`               | `true` when no check is a problem. The exit code is 0 when true.                           |
| `problems`         | How many checks are problems.                                                              |
| `warnings`         | How many checks are warnings.                                                              |
| `checks`           | Every check, in the order tools, env, dependencies, services.                              |
| `checks[].section` | `tools`, `env`, `dependencies` or `services`.                                              |
| `checks[].project` | The project's folder, as `fscale facts` prints it. Absent for checks about the whole repo. |
| `checks[].subject` | What was checked: `node`, `backend/.env.example`, `PostgreSQL on localhost:5432`.          |
| `checks[].status`  | `ok`, `problem` or `warning`.                                                              |
| `checks[].detail`  | One sentence on what was found.                                                            |
| `checks[].fix`     | The command or step that fixes a problem or warning.                                       |

## With verify

When a `fscale verify` step ends as **setup needed**, the summary ends with one line: ``Run `fscale doctor` to check this machine's tools, env variables, dependencies and services in one go.``

## Limits

- Version checks know the declarations in the table. `engines` for package managers, `volta`, `.tool-versions` (asdf, mise) and `.sdkmanrc` are not read.
- "Matching the lockfile" for Node is a file-time comparison, not a full comparison of versions. For uv and Poetry projects the packages are read from `pyproject.toml`'s `[project] dependencies`, not from `uv.lock` or `poetry.lock`.
- Compose files are read with a small line reader, not a YAML parser: `services:`, `image:` and `ports:` in the short and long forms. Anchors, `extends` and `include` are not followed.
- A service on another host (a cloud database) is not checked.
- A service check is a TCP connection, not a login: doctor does not know whether the user and password work or whether the database exists.

## Install commands that keep the lockfile

`fscale doctor` and verify's setup-needed text name the install command that installs exactly what a project's lockfile says and fails instead of rewriting it, when the lockfile is in the folder the command runs in. A setup step should never leave a changed lockfile behind for someone to commit. With no lockfile, the plain command is named. What `fscale facts` records, and the commands the rules write into AGENTS.md, stay the plain ones.

| `fscale facts` install | Lockfile                                              | Named instead                                                                                                                                                                       |
| ---------------------- | ----------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `npm install`          | `package-lock.json` or `npm-shrinkwrap.json`          | `npm ci`                                                                                                                                                                            |
| `pnpm install`         | `pnpm-lock.yaml`                                      | `pnpm install --frozen-lockfile`                                                                                                                                                    |
| `yarn install`         | `yarn.lock`                                           | `yarn install --immutable` for Yarn 2 and later (`packageManager: yarn@2` or later, a `.yarnrc.yml`, or a lockfile with `__metadata:`), `yarn install --frozen-lockfile` for Yarn 1 |
| `bun install`          | `bun.lock` or `bun.lockb`                             | `bun install --frozen-lockfile`                                                                                                                                                     |
| `uv sync`              | `uv.lock`, here or in a folder above (a uv workspace) | `uv sync --locked`                                                                                                                                                                  |
| `pipenv install --dev` | `Pipfile.lock`                                        | `pipenv sync --dev`                                                                                                                                                                 |
| `poetry install`       | `poetry.lock`                                         | `poetry install`: it installs from the lockfile and does not rewrite it                                                                                                             |
| pip (`-m pip install`) | none                                                  | unchanged                                                                                                                                                                           |
