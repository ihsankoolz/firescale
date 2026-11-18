# fscale doctor

`fscale doctor` checks whether **this machine** is ready to work on the repo: the tools it declares, the env variables its `.env.example` files name, the installed dependencies of each project, and the local services it expects (from Compose files, `.env.example`, and the local database URLs in your `.env` and `.env.local`). It looks at the machine, not at the code; `fscale check` and `fscale verify` look at the code (SPEC §17, ADR 0026).

```bash
fscale doctor
fscale doctor --json
```

It exits **0** when nothing is wrong (warnings are allowed) and **1** when something is. Every problem comes with a `next:` line, the command or step that fixes it. Some warnings have one too:

```
Tools
  ok       node: node 22.11.0 matches >=22.0.0 (package.json (engines.node))
  PROBLEM  pnpm: pnpm is not installed, or not on the PATH; package.json (packageManager) asks for pnpm@9.12.0.
           next: npm install -g pnpm@9.12.0

Environment variables
  warning  backend/.env.example: 2 of 6 variables are not set in the environment or backend/.env: JWT_SECRET, SMTP_HOST.
           next: add JWT_SECRET, SMTP_HOST to backend/.env (see backend/.env.example for what each one is)

Dependencies
  ok       backend/.venv: virtual environment with Python 3.12.4
  PROBLEM  backend packages: 1 of 14 packages in backend/requirements-dev.txt are not installed: pytest-asyncio.
           next: in backend: .venv/bin/python -m pip install -r requirements-dev.txt
  ok       frontend node_modules: all 31 packages in frontend/package.json are installed

Services
  PROBLEM  PostgreSQL on localhost:5432: not reachable: nothing listens on port 5432 (docker-compose.yml (service db)).
           next: docker compose up -d db

doctor: 3 problems, 1 warning - run the `next:` commands above, then `fscale doctor` again.
```

With warnings and no problem, the last line is `doctor: OK with 1 warning - read them above.` and the exit code is 0.

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

**The programs verify will run** (ADR 0085). Doctor builds the plan of a full `fscale verify`, as `fscale verify --dry-run` shows it, and looks up the first word of each step's command as the step would start it: on the PATH (on Windows with the `PATHEXT` extensions, as `cmd` does), or, for a path such as `./gradlew`, in the step's folder. This is the step cache's lookup. A program that is not found is a **problem** that names the step and its command:

```
  PROBLEM  zzlint: zzlint is not on the PATH, and verify's lint step runs `zzlint --strict`.
           next: install zzlint, or change the lint step's command under `verify` in the firescale config
```

A program doctor's own list knows (Maven, Gradle, .NET, CMake and the others of each ecosystem) is also asked its version, as that list asks it, and that list then leaves the tool to the step: one line, naming the step, not two. Any other program is only looked up, never run. A step whose command starts with a shell builtin (`cd`, `set`, `call`, ...), a variable (`FOO=1 cmd`) or shell syntax is not judged. Only the first program of a command is looked up: in `npm run lint && cargo test`, `cargo` is not.

### Environment variables

For each `.env.example` at the root and in each project's folder, every name it lists is looked for: it counts as set when it is set (to something) in the environment, or be present in a `.env` or `.env.local` **in the same folder or in a folder above it, up to the repo root** (ADR 0059). **For this check only names are read from those files; values are dropped as each line is read, and never printed.** (The services check below reads one more thing from them: the host and port of a local database URL.) The fix is to copy the example to `.env` and fill it in, or to add the missing names to the `.env` that exists.

A repo with one `.env` at the root and services below it is common: `backend/orders/.env.example` lists the names, and the root's `.env` sets them. Doctor then says in which file it found each name:

```
  ok       backend/orders/.env.example: all 2 variables are set in the environment, or in an env file of a folder above (ORDERS_REGION in backend/.env.local; ORDERS_API_KEY in .env)
```

A name found nowhere is a **warning**, and the line names every env file that was read:

```
  warning  backend/orders/.env.example: 1 of 2 variables are not set in the environment or .env, and there is no backend/orders/.env: ORDERS_API_KEY.
           next: add ORDERS_API_KEY to the env file that backend/orders reads: .env, or a new backend/orders/.env (see backend/orders/.env.example for what each one is)
```

**A warning, not a problem (ADR 0068).** A `.env.example` often lists names that only some work needs, and doctor cannot tell which ones the tests need. So a missing name does not make doctor exit 1. To make it a problem again, as it was before 0.5.3, set this in `firescale.config.json`:

```json
{ "doctor": { "missingEnv": "problem" } }
```

`missingEnv` is `"warning"` (the default) or `"problem"`; anything else is a config error. Doctor reads it from `firescale.config.json` only. It runs no code from the repo, and a `firescale.config.ts` is code, so the setting in one is not read: a missing name is then a warning, and doctor adds a line that says the setting was not read.

**What this check can and cannot know.** `fscale verify` loads no env file: a step gets the environment verify was started in, plus `CI`, `NO_COLOR`, `FORCE_COLOR` and `PYTHONUNBUFFERED`. The project's own tools read the files, each in its own way: python-dotenv's `load_dotenv()` looks in the folders above; pydantic-settings and Vite read the folder they are pointed at (often the root, by a path in the code or `envDir`); Docker Compose reads the `.env` beside the Compose file. Doctor cannot tell which of these a project uses, so a name in any `.env` or `.env.local` at or above the project counts as set, the same folders the services check and the rule `core/env-example` look in. A file above the repo root is never read.

**A `.env` or `.env.local` that is a symbolic link is not read**, by this check or by the services check: a link can point at a file outside the repo, and the base run of `fscale verify` does not copy a linked env file into its worktree either. A name set only in such a file counts as set nowhere, and the line names the link:

```
  warning  backend/orders/.env.example: 1 of 1 variables are not set in the environment: ORDERS_API_KEY. backend/orders/.env is a symbolic link, so it was not read.
           next: replace backend/orders/.env with a real file that sets ORDERS_API_KEY, or set the variables in your shell (see backend/orders/.env.example for what each one is)
```

When every name is set in the environment or in a real env file, the link is not mentioned.

### Dependencies

For a Dart or Flutter project, doctor looks for `.dart_tool/package_config.json` in the project or in a folder above it (a pub workspace keeps one at its root). Without it, `pub get` has not run: a problem, with `dart pub get` or `flutter pub get` as the next step. Doctor does not compare the file with `pubspec.lock`.

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

### Agent hooks

The stop hook in each agent hook file at the repo root (`.claude/settings.json`, `.codex/hooks.json`, `.cursor/hooks.json`), started the way `fscale hooks install` checks it after writing (see [hooks.md](hooks.md)): `<command> --version --json` in the repo root, through `cmd /D /S /C` or `sh -c`, with stdin closed, without the PATH folders a package manager put there for its own run, and with `CLAUDE_PROJECT_DIR` set for Claude Code's file. Before it starts one, doctor says on stderr what it runs. Each distinct command is started once.

Doctor runs no script of the repo and downloads nothing here either:

- **A pin** (`npx --yes firescale@<version>`, the default where the repo has no fscale of its own) is started as `npx --no -- firescale@<version>`: npx runs firescale only when it is installed or already in its cache. When it is not, the check is a **warning**, `firescale <version> is not downloaded yet, and doctor downloads nothing`: the agent's first stop downloads it, which needs the network.
- **The repo's own `fscale` package script** is never started, whatever runs it (`pnpm --silent fscale`, `npm run --silent fscale --`, ...): a **warning** gives the command to run yourself.
- **The fscale the repo installs** (`pnpm exec fscale`, `npx --no -- fscale`, `yarn --silent fscale`, `bun run --silent fscale`, when the repo has no `fscale` script) is started as it is.

- **ok** when it answers this firescale; **warning** when it answers another version, the one the agent runs.
- **problem** when it cannot start, exits with an error or answers something that is not firescale's answer: the agent would stop without verify having run.
- **warning** for a command that is not the one fscale writes in this repo: it is text from the repo, so doctor prints it and does not start it.
- **warning** for a pin not downloaded yet, and for the repo's own package script, as above.

The check runs on whichever machine runs doctor, so a teammate whose machine cannot start the hook finds out, not only the person who installed it (ADR 0064).

**The stop hook's time limit** (ADR 0083). For Claude Code's file, doctor adds up the kept times (`.firescale/step-times.json`) of every step a full `fscale verify` plans. When that is more than the hook's limit, its `timeout` (seconds; a point is allowed) or Claude Code's default of 10 minutes when it has none or its value is not a number of seconds, it is a **warning**: Claude Code ends a stop hook that runs past its limit, and the agent stops without verify having finished. The warning says to work on a branch (the hook then runs only the projects a change reaches, as `fscale verify --affected` does), to give the hook a smaller scope (a faster command for the slow steps under `verify` in the config), or to raise the limit. Steps with no kept time count as nothing, so a repo where verify never ran gets no warning. Codex and Cursor are not checked: firescale knows no time limit field for them.

## JSON

`fscale doctor --json` prints the same checks. The schema is [`packages/cli/schemas/doctor-output.schema.json`](../packages/cli/schemas/doctor-output.schema.json).

| Field              | Meaning                                                                                    |
| ------------------ | ------------------------------------------------------------------------------------------ |
| `schemaVersion`    | Always `2`. Changes only on a breaking change. Version 2 added the section `hooks`.        |
| `ok`               | `true` when no check is a problem. The exit code is 0 when true.                           |
| `problems`         | How many checks are problems.                                                              |
| `warnings`         | How many checks are warnings.                                                              |
| `checks`           | Every check, in the order tools, env, dependencies, services, hooks.                       |
| `checks[].section` | `tools`, `env`, `dependencies`, `services` or `hooks`.                                     |
| `checks[].project` | The project's folder, as `fscale facts` prints it. Absent for checks about the whole repo. |
| `checks[].subject` | What was checked: `node`, `backend/.env.example`, `PostgreSQL on localhost:5432`.          |
| `checks[].status`  | `ok`, `problem` or `warning`.                                                              |
| `checks[].detail`  | One sentence on what was found.                                                            |
| `checks[].fix`     | The command or step that fixes a problem or warning.                                       |

## With verify

When a `fscale verify` step ends as **setup needed**, the summary ends with one line: ``Run `fscale doctor` to check this machine's tools, env variables, dependencies and services in one go.``

## Limits

- The plan check looks up the first program of each step's command only, and judges no step that starts with a shell builtin or a variable. A program found on the PATH may still fail to start; for a program doctor's list does not know, nothing runs it.
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
