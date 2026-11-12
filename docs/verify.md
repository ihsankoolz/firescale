# fscale verify

`fscale verify [--affected [--base <ref>]] [--json] [--jobs <n>] [--progress | --no-progress]` is the one command that says whether work is done. People, CI and agent hooks all call it. It runs these steps in order:

1. `check`: the firescale rules, exactly as `fscale check` runs them. It fails when an enforced rule has violations. **When `check` fails, nothing else runs.**
2. `lint`, `typecheck` and `test` at the repo root.
3. `lint`, `typecheck` and `test` in each project found below the root, in that project's folder.

The repo root and each project are **independent**. A failure stops the rest of that project's steps, which are listed as `not run`, and every other project still runs. The headline names every step that failed (ADR 0022).

| Exit code | Meaning                                                                                           |
| --------- | ------------------------------------------------------------------------------------------------- |
| 0         | No step failed. The headline says `OK` when every step ran, and `PARTIAL` when some were skipped. |
| 1         | A step failed, a step needs setup, or steps were skipped and `verify.failOnSkipped` is on.        |
| 2         | Bad usage or a bad config.                                                                        |
| 130       | Verify was stopped by Ctrl+C or a termination signal. The summary lists what finished.            |

## Setup needed is not a failure in the code

A step can fail because the machine is not ready: a dependency is not installed, a program is not on the PATH, or a service the tests need on this machine (a database, a cache) is not running. Changing code cannot fix that, so verify reports such a step as **setup needed** (`SETUP` in the summary, `setup-needed` in JSON), with the command that sets it up, apart from real failures:

```
verify: SETUP NEEDED for test in backend/audit_service
  ...
Setup needed for test in backend/audit_service: the environment is not ready, so this is not a problem in the code. Do not change code for it.
Set it up in backend/audit_service: .venv/bin/python -m pip install -r requirements-dev.txt
Then run again in backend/audit_service: .venv/bin/python -m pytest
```

With failures of both kinds the headline names both: `verify: FAILED at lint in frontend; SETUP NEEDED for test in backend/audit_service`.

The detection is a short list of exact messages in the step's output. Anything else is a normal failure, and so is anything that could be a mistake in the code. Output that reports failed tests (a line starting with `FAILED `, or a count such as `2 failed`) is always a normal failure, even when a test failed on a missing module: the tests ran, so the environment was ready enough. A pytest collection error, which stops pytest before any test runs, is setup. The one exception is a refused connection to a local service (the last rows of the table): it is setup even when the output also reports failed tests, because tests that need the service cannot be judged while it is down.

| Message                                                                                                                                                                                                                            | Setup needed unless                                                                                                                                       |
| ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `No module named 'x'` (Python, also `python: No module named pytest`)                                                                                                                                                              | `x` (its first part) is a folder or `x.py` in the project's folder or its `src/`, or in a folder above it or that folder's `src/`, up to the root.        |
| `Python was not found` (the Windows stand-in for a missing Python)                                                                                                                                                                 | never                                                                                                                                                     |
| `'x' is not recognized as an internal or external command` (cmd)                                                                                                                                                                   | never                                                                                                                                                     |
| `x: not found` or `x: command not found` (sh), only with exit code 127                                                                                                                                                             | never                                                                                                                                                     |
| `canceled due to missing packages` (`npx --no` with the tool not installed)                                                                                                                                                        | never                                                                                                                                                     |
| `Command "x" not found` (pnpm), `Command not found: x` (poetry), `Failed to spawn: x` (uv)                                                                                                                                         | never                                                                                                                                                     |
| `Cannot find package 'x'` with `ERR_MODULE_NOT_FOUND` (Node)                                                                                                                                                                       | `x` is a path (`./`, `/`, a drive letter).                                                                                                                |
| `connection to server at "localhost" (127.0.0.1), port 5432 failed: Connection refused` (libpq 14+: psycopg2, psycopg, SQLAlchemy)                                                                                                 | the host is not local                                                                                                                                     |
| `connection to server on socket "/run/postgresql/.s.PGSQL.5432" failed: No such file or directory` or `Connection refused` (libpq 14+)                                                                                             | never                                                                                                                                                     |
| `could not connect to server: Connection refused`, then `Is the server running on host "localhost"` within 3 lines (libpq 13 and earlier)                                                                                          | the host is not local                                                                                                                                     |
| `Error 111 connecting to localhost:6379. Connection refused.` (redis-py; also `Error 10061 ... actively refused it.`)                                                                                                              | the host is not local                                                                                                                                     |
| `connect ECONNREFUSED 127.0.0.1:5432`, `connect ECONNREFUSED ::1:6379` (Node)                                                                                                                                                      | the host is not local                                                                                                                                     |
| `Can't connect to MySQL server on 'localhost'` with `(111)`, `(10061)` or PyMySQL's `([Errno 111] ...)`; also `'localhost:3306'`                                                                                                   | the host is not local                                                                                                                                     |
| `Can't connect to local MySQL server`                                                                                                                                                                                              | never                                                                                                                                                     |
| ``Can't reach database server at `localhost:5432` `` (Prisma P1001, also `` `localhost`:`5432` ``)                                                                                                                                 | the host is not local                                                                                                                                     |
| `[Errno 111] Connect call failed ('127.0.0.1', 5432)` (asyncio: asyncpg and other async clients)                                                                                                                                   | the host is not local                                                                                                                                     |
| `localhost:27017: [Errno 111] Connection refused` (pymongo)                                                                                                                                                                        | the host is not local                                                                                                                                     |
| `[Errno 111] Connection refused`, `[WinError 10061] No connection could be made because the target machine actively refused it`, `[WinError 1225] The remote computer refused the network connection` (Python sockets, no address) | the nearest Python address before it in the same pytest section (a frame argument such as `address = ('localhost', 5432)`) is not local, or there is none |

For the connection rows, a local host is `localhost`, `127.0.0.1`, `::1` or `0.0.0.0`; a refused connection to any other host is a normal failure. "Refused" is error code 111 (Linux), 61 (macOS), 10061 or 1225 (Windows); a timeout is a normal failure. The summary names the service from the client or the port: `PostgreSQL on localhost:5432`, `Redis on localhost:6379`, `MySQL on localhost:3306`, `MongoDB on localhost:27017`, else `a service on localhost:<port>`. When a Docker Compose file (`compose.yaml`, `compose.yml`, `docker-compose.yaml` or `docker-compose.yml`) is in the project's folder, or in a folder above it up to the repo root, the fix is to start it from the project's folder: `docker compose up -d`, or `docker compose -f ../../docker-compose.yml up -d` when the file is in a folder above. Without one, start the service by hand.

The command that sets it up is the project's `install` command from `fscale facts`: for a Python project it installs the dependency file into the project's interpreter, for a Node project it is `<pm> install`. When the missing program is one an install does not bring (`python`, `node`, `npm`, `pnpm`, `uv`, `poetry`, `go`, `cargo` and the like), the summary says to install that program instead.

## Running on slow, real projects

These parts of verify were written after running 0.1.2 on a course project with five Python services and a frontend, on Windows (ADR 0024).

### Progress

While verify runs, it prints a line on stderr when each step starts and when it ends, and a heartbeat line every minute for a step that is still running, with the last line that step printed:

```
[verify] check: started: fscale check
[verify] check: passed after 412ms
[verify] test in backend/audit_service: started: .venv/bin/python -m pytest
[verify] test in backend/audit_service: still running after 1m 00s; last output: E   psycopg2.OperationalError: connection to server at "localhost" (127.0.0.1), port 5432 failed: Connection refused
[verify] test in backend/audit_service: TIMED OUT after 10m 00s (limit 10m)
```

Progress is on when stderr is a terminal, and off when stderr goes to a file or a pipe (CI logs, agent hooks), so logs and hook messages stay as they were. `--progress` turns it on anyway, `--no-progress` turns it off. It never touches stdout: the summary and `--json` are the same with or without it. `fscale hooks run` prints no progress unless `--progress` is given, because an agent reads the hook's stderr. The variable `FSCALE_HEARTBEAT_MS` sets the heartbeat interval in milliseconds (`0` turns heartbeats off); it is meant for tests.

### Stopping cleanly

On Ctrl+C, on `SIGTERM` or `SIGHUP` (macOS and Linux), and on Ctrl+Break or closing the console (Windows), verify stops every command it started **with every process that command started**, prints the summary of what finished, and exits with code 130. The step that was running is `STOPPED`, the steps that had not started are `not run`, and the headline reads `verify: STOPPED - interrupted after 3 of 9 steps ran`, followed by any failures found before that.

- On macOS and Linux each command runs in its own process group. Stopping sends `SIGTERM` to the group, then `SIGKILL` to whatever is left after 2 seconds.
- On Windows each command runs in its own job object, which it enters before it can start anything. Stopping terminates the job. If fscale itself is killed outright, Windows closes the job and its processes end with it.

When a step's command ends by itself, whatever it left running in its group is stopped too, and verify waits at most 2 seconds for its output to close. A background process that a test left behind can no longer keep verify waiting.

### Time limits

Every step has a time limit: **10 minutes** unless the config says otherwise. A step that runs past it is stopped with its whole process tree and reported as `TIMED OUT`, with its limit, the last lines of its output and the config key that raises it. Like a failure, it stops the rest of its project; other projects still run. When the output shows a setup problem (such as a database that is not running), the summary also names the command that sets it up.

```json
{
  "verify": {
    "timeout": "15m",
    "test": { "timeout": "30m" },
    "projects": {
      "backend/audit_service": {
        "timeout": "20m",
        "test": { "command": "python -m pytest -x", "timeout": "1h" }
      },
      "frontend": { "lint": { "timeout": false } }
    }
  }
}
```

The most specific setting wins: the project's step (`projects.<folder>.<step>.timeout`), the project (`projects.<folder>.timeout`), the step for the whole repo (`<step>.timeout`), `timeout`, then the default. A limit is a number of seconds, or text such as `"90s"`, `"10m"`, `"1h"` or `"1h 30m"`; `false` means no limit. A step object with only a `timeout` keeps the command verify detects.

### No input, and CI=1

Commands get no input: their stdin is empty and closed, so a command that asks a question gets an end of file instead of waiting for ever. They also run with **`CI=1`**, unless the environment already sets `CI` (then its value is kept) or the config says `"ci": false`. `CI` changes how many tools behave, which is the point, but know what it does:

- Vitest and `react-scripts test` run once instead of watching for changes.
- Jest does not write new snapshots: a test with a snapshot that does not exist yet fails. Run Jest yourself once to write it.
- Playwright configs often turn on retries and `forbidOnly` when `CI` is set, so a stray `test.only` fails the run.
- Package managers and many tools drop prompts, spinners and colours.

To run without it: `{ "verify": { "ci": false } }`. Stdin stays empty either way.

A test script that is a known watch command (such as `vitest` or `jest --watch`) is replaced by a run-once sibling script (`test:run`, `test:ci` or `test:once`) when one exists, and `fscale facts` says so; without one, `fscale facts` warns and verify relies on `CI=1`. [facts.md](facts.md) has the list.

### Projects in parallel

`--jobs <n>` runs up to `n` independent units at once: the repo root and each project. The default is 1. Each unit's steps still run in order, and a failure still stops only the rest of its own unit. The summary and the JSON list the units in the same order as with one job. Progress lines are printed as things happen, each a whole line that names its project, so lines from different projects never mix.

Run projects in parallel only when they do not share anything that cannot be shared, such as one test database that each project's tests reset.

### A second run never waits on another

Verify takes no lock, so a second `fscale verify` never waits for another one. Instead, each run keeps a small record in the temp folder (`firescale-runs`) with its process id and the commands it is running, and removes it when it ends. A new run reads the records of the same repo and says on stderr:

- when another verify is running there: which process, since when, and what it is running. The two may compete for the same files, ports or test database.
- when an earlier verify ended without stopping its commands (it was killed outright on macOS or Linux) and they are still running: which ones, and the command that stops them (`kill -TERM -<group>`; on Windows `taskkill /T /F /PID <pid>`).

## Python projects use their own virtual environment

Each Python project's steps run with the interpreter `fscale facts` shows for it: a `.venv` or `venv` in the project's folder first, then the project's uv, poetry or pipenv, then an active `VIRTUAL_ENV`, and `python` from the PATH last. [facts.md](facts.md) has the order and why. When verify falls back to `python` from the PATH for a project, it says so in a `note:` line under the summary and in `notes` in JSON.

## A skipped step is never a clean pass

A step with no command cannot verify anything. It is **skipped**, and verify says so in three places:

- The headline. It reads `verify: OK` only when every step ran and passed. With a skipped step it reads `verify: PARTIAL - 1 of 4 steps ran, 3 skipped`.
- The step's own line, with the reason no command was found.
- A `warning:` line under the summary that names every skipped step and the config key that gives the first one a command.

A partial run still exits 0, so adopting firescale does not break a build on day one. To make skipped steps fail, turn on `failOnSkipped`:

```json
{ "verify": { "failOnSkipped": true } }
```

Then the headline reads `verify: FAILED - 3 of 4 steps skipped, and verify.failOnSkipped is on`, and the exit code is 1.

A step can also be **off**: you set it to `false` in the config, or another step covers it (`cargo clippy` already type-checks). An off step is listed as `off`, is not counted, and does not make a run partial.

## Which commands run

### At the repo root

For each of `lint`, `typecheck` and `test`, the first match wins:

| Order | Source                                             | Command                                                                                              |
| ----- | -------------------------------------------------- | ---------------------------------------------------------------------------------------------------- |
| 1     | `verify.<step>` in the firescale config            | The string you wrote, or `false` to turn the step off.                                               |
| 2     | Nx (`nx.json` exists)                              | `<exec> nx run-many -t <step>`, or `<exec> nx affected -t <step>` with `--affected`.                 |
| 3     | Turborepo (`turbo.json` exists)                    | `<exec> turbo run <step>` (plus `--affected`), only if `turbo.json` defines the task.                |
| 4     | A root `package.json`                              | The script of that name: `<pm> run <step>`. Without a script, the command from a tool's config file. |
| 5     | A Rust-only repo (`Cargo.toml`, no `package.json`) | `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace`.                |
| 6     | A Python or Go project at the root                 | The commands `fscale facts` shows for it, for example `python -m pytest`.                            |

`<exec>` is `pnpm exec`, `yarn`, `bunx` or `npx`, by package manager. `fscale facts` shows the package manager, scripts and tools that were detected, and [facts.md](facts.md) lists which config files give which command.

### In projects below the root

`fscale facts` finds projects in subfolders (see [facts.md](facts.md)). Each one whose `source` is `discovered` runs its own `lint`, `typecheck` and `test`, with the commands `fscale facts` lists for it, **in its own folder**. A project with no command for a step has that step skipped, with the reason.

- Workspace members are not run one by one: the root's commands cover them.
- The root's commands come from one source, the first match in the table above. A project of another kind at the root, for example a Python package beside a `Cargo.toml`, is not dropped: it runs its own steps, listed as `in the repo root (python)`. Its key under `verify.projects` is `"."`.
- A folder with projects of two kinds lists them apart, as `in app (go)` and `in app (node)`. A command set for that folder under `verify.projects` runs once, in place of both kinds' own command for that step.
- When the root has no commands of its own (no `package.json`, `Cargo.toml`, Nx, Turborepo, Python or Go project there), it lists no steps, and `runner` is `projects`.
- Projects run in the order of their folders. They always run in full, also with `--affected`.

### Config

```json
{
  "verify": {
    "lint": "pnpm lint",
    "typecheck": false,
    "test": "pnpm test --run",
    "failOnSkipped": true,
    "projects": {
      "backend/auth_service": { "typecheck": false, "test": "python3 -m pytest -q" },
      "docs": false,
      "tools/deep/down/below/level4": { "test": "make test" }
    }
  }
}
```

| Key                                      | Meaning                                                                                                                                                                                                                               |
| ---------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `lint`, `typecheck`, `test`              | A setting for the **whole repo**. A command runs once, from the repo root, and no project runs its own command for that step. `false` turns the step off everywhere.                                                                  |
| `failOnSkipped`                          | `true` makes a skipped step fail the run. Default `false`.                                                                                                                                                                            |
| `timeout`                                | The time limit of every step, unless a more specific one is set. Default `"10m"`. See "Time limits".                                                                                                                                  |
| `ci`                                     | `false`: do not set `CI=1` for the commands. Default `true`.                                                                                                                                                                          |
| `<step>` as an object                    | `{ "command": "...", "timeout": "20m" }`: a command, a time limit, or both. Without `command`, the detected command runs with that limit.                                                                                             |
| `projects.<folder>`                      | Settings for one project, by its folder. `false` turns all its steps off. An object sets single steps to a command or to `false`, and wins over the repo-wide setting for that step. Its `timeout` is the limit of each of its steps. |
| `projects.<folder>` for any other folder | A folder `fscale facts` did not list as discovered runs exactly the commands given for it, in that folder. Use it for a project below the search depth.                                                                               |

A folder named under `projects` must exist, or verify stops with exit code 2. Write the folder as `fscale facts` prints it, in the same letter case. The top-level keys are for the root's own commands; `"."` under `projects` is only for other kinds of project at the root.

Commands run through the platform shell (`sh -c` on macOS and Linux, `cmd /C` on Windows) with `NO_COLOR=1`, `CI=1` and no input (see "No input, and CI=1" above). Root steps run from the repo root, project steps from the project's folder.

`--affected` narrows the `check` step to changed files (see [affected.md](affected.md)) and is passed on to Nx and Turborepo. `--base <ref>` is also passed to `nx affected`; for Turborepo, set `TURBO_SCM_BASE` yourself. Plain package scripts, Cargo and the projects below the root always run in full.

On Windows the command line is handed to `cmd` exactly as written, so `cmd` quoting works (`node -e "..."`, `"C:\Program Files\tool\tool.exe" arg`).

## Output

The summary is short and written for an agent to act on: what failed, the end of its output, the command to run again, and what did not run.

```
verify: FAILED at test
  check      passed   8 rules, no enforced violations
  lint       passed   pnpm run lint
  typecheck  off      turned off in your firescale config
  test       FAILED   pnpm run test (exit 1)

Last lines of test:
  FAIL adds two numbers: expected 4, got 5

Fix the failure, then run again: pnpm run test
```

In a repo with projects below the root, each project's steps are listed under its folder:

```
verify: PARTIAL - 5 of 7 steps ran, 2 skipped
  check      passed   8 rules, no enforced violations
  in backend/auth_service:
    lint       passed   python -m ruff check .
    typecheck  skipped  no type checker was found (looked for mypy)
    test       passed   python -m pytest
  in frontend:
    lint       passed   npx --no oxlint
    typecheck  passed   npx --no tsc --noEmit
    test       skipped  package.json has no test script, and no vitest or jest config was found

warning: steps were skipped because no command was found: typecheck in backend/auth_service, test in frontend. Set verify.projects["backend/auth_service"].typecheck in your firescale config to a command, or to false if that project has none.

note: no virtual environment was found for backend/auth_service, so its Python commands use python from the PATH. To use one, create it in the project's folder (python -m venv .venv) or activate it.
```

A failure in a project names it: `verify: FAILED at test in backend/auth_service`, and `Fix the failure, then run again in backend/auth_service: python -m pytest`.

At most the last 30 non-empty lines of the failed command are kept, with colour codes removed. The steps of the same project after the failure are listed as `not run`; other projects run. Each failure gets its own `Last lines of ...` block and its own line with the command to run again. When `check` is the step that failed, the lines are the violations of enforced rules, one per line, with the same cap; report-only rules are left out.

`--json` prints the format described by `packages/cli/schemas/verify-output.schema.json` (JSON Schema draft-07, published as `firescale/schemas/verify-output.schema.json`).

| Field           | Meaning                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| --------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `schemaVersion` | Always `4`. Changes only on a breaking change. Version 4 added the result `stopped`, the step statuses `timed-out` and `stopped`, and a step's `timeoutMs` and `timeoutKey`. Version 3 added the result and step status `setup-needed`, the step's `fix`, and `notes`. Version 2 added `result`, the counts, `skipped`, `failOnSkipped`, `failedProject`, the `project` of a step, and the step status `off`.                                                                                        |
| `version`       | The firescale version.                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| `ok`            | `true` when the exit code is 0: `result` is `ok` or `partial`.                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| `result`        | `ok` (every step ran and passed), `partial` (nothing failed, at least one step skipped), `failed` (a step failed or timed out, or steps were skipped with `failOnSkipped` on), `setup-needed` (nothing failed in the code, but a step needs setup) or `stopped` (Ctrl+C or a termination signal stopped the run; exit code 130). **Read this, not `ok` alone.**                                                                                                                                      |
| `runner`        | How the root's commands were chosen: `config` (all three steps set in the config), `nx`, `turbo`, `scripts`, `cargo`, `python`, `go`, `projects` (only the projects below the root) or `none`.                                                                                                                                                                                                                                                                                                       |
| `stepsTotal`    | Steps that should run: every step except those that are `off`.                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| `stepsRun`      | Steps that ran to a result: passed, failed, timed out or needs setup.                                                                                                                                                                                                                                                                                                                                                                                                                                |
| `failOnSkipped` | The config setting.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| `failedStep`    | The first step that failed, timed out or needs setup. The others are in `steps` and in the headline. Absent when no step failed.                                                                                                                                                                                                                                                                                                                                                                     |
| `failedProject` | The folder of the project whose step failed. Absent for a root step.                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `rerun`         | The command to run again, from the folder in `failedProject` when that is present. Absent when no step failed.                                                                                                                                                                                                                                                                                                                                                                                       |
| `steps`         | `check` first, then the root's steps, then each project's. Each has `name`, `status` (`passed`, `failed`, `setup-needed`, `timed-out`, `stopped`, `skipped`, `not-run`, `off`), `durationMs`, `detail`, and where they apply `project`, `kind`, `command`, `exitCode`, `outputTail`, `fix` (the setup command of a `setup-needed` step, or of a `timed-out` one whose output shows a setup problem), `timeoutMs` and `timeoutKey` (the limit of a `timed-out` step and the config key that sets it). |
| `skipped`       | Every skipped step: `step`, `reason`, and `project` for a project's step. Empty when nothing was skipped.                                                                                                                                                                                                                                                                                                                                                                                            |
| `notes`         | Things that are not failures: Python projects that ran with `python` from the PATH. Absent when there are none.                                                                                                                                                                                                                                                                                                                                                                                      |
| `warnings`      | One sentence naming the skipped steps. Absent when there are none.                                                                                                                                                                                                                                                                                                                                                                                                                                   |

## The stop hook

`fscale hooks run` blocks an agent only when verify fails. When a step needs setup, the message the agent gets starts with `SETUP NEEDED:`, says that it is not a problem in the code, tells the agent not to edit code, tests or config to get past it, and names the setup command to run or to ask the user for. On a partial run the agent may stop, and the hook prints a line on stderr that says the run was not a clean pass and names the skipped steps. That line is for the person reading the hook's output or its log: an agent may not be shown the stderr of a hook that lets it stop. With `verify.failOnSkipped` on, a partial run is a failure and the hook blocks. ADR 0020 explains the default.

## Limits

- A step's own output is captured, not streamed. Progress lines say what runs, and the heartbeat shows the last line of output.
- On macOS and Linux, a process that leaves its process group (a daemon that calls `setsid`) is not stopped with its command, and if fscale is killed with `SIGKILL` its commands keep running; the next run names them. On Windows, a daemon started by a step (such as a build server) is stopped at the end of the step, because it is in the step's job.
- With Nx, the task names must be `lint`, `typecheck` and `test`. Use the `verify` config key for other names.
- A Node, Rust or Go project's commands run with the tools found on the PATH. A Python project's run with its interpreter (see above).
- Setup detection only knows the messages in the table. A setup problem with another message is a normal failure.
- Projects run one after another unless `--jobs` is given.
- A step that is skipped in every run stays in the summary until it gets a command or is set to `false`.
- `verify` is as fast as the repo's own commands. firescale's share is a few milliseconds (see `bench/RESULTS.md`).
