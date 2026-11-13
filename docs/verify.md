# fscale verify

`fscale verify [--affected] [--base <ref>] [--dry-run] [--json] [--jobs <n>] [--no-cache] [--progress | --no-progress]` is the one command that says whether work is done. People, CI and agent hooks all call it. It runs these steps in order:

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

A **failed password login to a local database** is setup needed too, like a refused connection, and also when the output reports failed tests: the database is running, but the user or password in the test settings does not match it, and no code change fixes that. The messages are PostgreSQL's `password authentication failed for user "x"` (in libpq's `connection to server at "localhost" (127.0.0.1), port 5433 failed: FATAL:  ...` or on a socket, as a bare `FATAL:  ...`, asyncpg's `InvalidPasswordError: ...`, node-postgres' `error: ...`, and SQLAlchemy wrapping any of them), MySQL's `Access denied for user 'x'@'localhost' (using password: YES)` (error 1045, also `ERROR 1045 (28000)`), and Prisma's P1000 ``Authentication failed against database server at `localhost` ``. It counts unless the message names a host that is not local (for MySQL, the host after `@`); a message with no address counts unless the output also has a failed login to another host. The summary says `could not log in to PostgreSQL on localhost:5433: password authentication failed; ...`, has no setup command (starting or installing something does not fix it), and never names the user.

For the connection rows, a local host is `localhost`, `127.0.0.1`, `::1` or `0.0.0.0`; a refused connection to any other host is a normal failure. "Refused" is error code 111 (Linux), 61 (macOS), 10061 or 1225 (Windows); a timeout is a normal failure. The summary names the service from the client or the port: `PostgreSQL on localhost:5432`, `Redis on localhost:6379`, `MySQL on localhost:3306`, `MongoDB on localhost:27017`, else `a service on localhost:<port>`. When a Docker Compose file (`compose.yaml`, `compose.yml`, `docker-compose.yaml` or `docker-compose.yml`) is in the project's folder, or in a folder above it up to the repo root, the fix is to start it from the project's folder: `docker compose up -d`, or `docker compose -f ../../docker-compose.yml up -d` when the file is in a folder above. Without one, start the service by hand.

The command that sets it up is the project's `install` command from `fscale facts`: for a Python project it installs the dependency file into the project's interpreter, for a Node project it is `<pm> install`. When the missing program is one an install does not bring (`python`, `node`, `npm`, `pnpm`, `uv`, `poetry`, `go`, `cargo` and the like), the summary says to install that program instead.

## Only new failures block

With a base commit to compare with, every failure gets a label, and only a **new** one blocks (ADR 0037). The labels are on whenever there is a base; there is no flag to turn them on.

| Label          | Meaning                                       | Blocks |
| -------------- | --------------------------------------------- | ------ |
| `new`          | Fails on the head, passed on the base commit. | yes    |
| `pre-existing` | Fails on both, the same way.                  | no     |
| `fixed`        | Failed on the base, passes on the head.       | —      |
| `flaky`        | New, and passed when run again.               | no     |

- The **head** is the working tree, uncommitted changes included. The **base** is the merge base of `HEAD` and `--base <ref>`, or of the default branch (`origin/HEAD`, else `origin/main`, `origin/master`, `main`, `master`). On the default branch itself the base is its upstream (`origin/main`): a commit made there is part of the change, never the base.
- There is **no base** outside git, before the first commit, with no default branch, on a default branch with no upstream (pass `--base` to name one), or on the base commit with a clean tree. Then every failure blocks, as before, and a failing summary says why there was nothing to compare.
- **"The same way" needs evidence**: a failure is `pre-existing` only when every line of its output on the head (with numbers and the project's folder normalised) is also in its output on the base. A base that fails another way, a missing module, a missing build output, one error fewer, never hides a head failure: that failure blocks, with a note.
- **With a base, every step runs**: a failure does not stop the rest of its project, and a failed check stops nothing, so an old lint failure cannot hide a new test failure.
- **Each step's base result is found the cheapest way.** When no file the step reads (its [cache key](#the-step-cache)'s files) differs between the base and the working tree, its head result is its base result: no base run. Next, a result kept in `.firescale/base/` from an earlier run with the same base and the same lockfiles. Only what is left runs on the base commit, in a temporary git worktree, failing steps first; passing steps run there only when a failure needs the worktree anyway, so a clean run costs no base run. Your working tree and index are never touched.
- **The worktree uses the head's installed dependencies**: every ignored `node_modules`, `.venv` and `venv` is linked in, except one holding a link into the repo (a workspace package, an editable install), which would make the base run the head's code; so the base run needs no network and never installs into a shared interpreter. When a lockfile or a `requirements*.txt` differs from the base, the head's dependencies do not stand for the base's: set `verify.baseInstall` to a command that installs them in the worktree (for example `"pnpm install --frozen-lockfile --offline"`), or the failures block as without a base. With `verify.baseInstall`, nothing is linked. A pnpm workspace's recursive scripts (`pnpm -r`) refuse a linked `node_modules`; such a base step counts as setup needed, so set `verify.baseInstall` there. If your tests need a build output the repo does not commit (a compiled binary, `dist/`), put the build in `verify.baseInstall` too. Base steps get `FSCALE_BASE_RUN=1`.
- **The base run has a time budget**, `verify.baseBudget` (default `"5m"`, `false` for none): the worktree, the install and the steps together. A base step gets its own limit or what is left of the budget, whichever is shorter.
- **When the base cannot judge a failure**, because its base run needed setup, timed out, ran over the budget or was stopped, the failure gets no label and blocks, with a note that says why. **A step that needs setup on the head is never labelled**: it blocks with the setup message, as before.
- **Per-test labels** for pytest, Vitest and Jest (ADR 0038): a failing test that does not fail on the base makes the step `new`, and the summary names it under `New failing tests:`. pytest's results come from a JUnit report that verify asks for through `PYTEST_ADDOPTS` (your command is not changed); Vitest's and Jest's from their default reporters' failure lines. Per-test results never make a failure `pre-existing` on their own: the output evidence above still has to hold. A base on which no test passed is not evidence. A test file renamed between the base and the head keeps its tests (an old failure there is never `new`). Other runners are compared as a whole step, with a note.
- **A new failure runs once more before it blocks.** If it passes the second time, it is `flaky` and does not block. No flaky history is kept: nothing about it is written anywhere, and the second run's pass is not cached. A step that timed out is not run again.
- `cargo test` gets per-test labels too, from its text output (`<crate>::<test path>`).
- **The check** is compared too: when it fails on the head, it runs on the base, and each enforced violation (rule, file and message; the line may move) is `[new]` or `[pre-existing]`. A check whose violations are all on the base does not block.

```
verify: FAILED at test (new); pre-existing (fails on the base commit 3f2a9c1 too, does not block): lint
  check      passed   8 rules, no enforced violations
  lint       FAILED   [pre-existing] pnpm run lint (exit 1)
  typecheck  passed   pnpm run typecheck
  test       FAILED   [new] pnpm run test (exit 1)

Pre-existing: lint fails on the base commit 3f2a9c1 too, so it does not block this change.

New: test passes on the base commit 3f2a9c1, so this change broke it.

Last lines of test:
  FAIL adds two numbers: expected 4, got 5

Fix the failure, then run again: pnpm run test

Compared with the base commit 3f2a9c1 (main): 1 step with the same inputs as on the head, 2 steps run in a worktree of it.
```

When every failure is pre-existing, verify exits 0 with the result `pre-existing` and the headline `verify: OK - no new failures; pre-existing (fails on the base commit 3f2a9c1 too): lint`. It never reads as a plain `OK`. The stop hook lets an agent stop then, since nothing it did broke.

## The step cache

A lint, typecheck or test step that **passed** is remembered on this machine, in `.firescale/steps/`, and is not run again while nothing it depends on has changed (ADR 0036). Only passes are kept: a step that failed, needs setup, timed out, was stopped or skipped always runs again.

The key of a pass is a hash of the firescale version, the step, its folder and command, and:

- **The bytes of the files it can read, as they are now.** Its project's folder, the projects and shared folders it depends on in the project graph ([graph.md](graph.md)), the files directly in each folder above those, and every file that belongs to no project. The files are the tracked ones (uncommitted changes included) and the untracked ones git does not ignore. Bytes, not text: a CRLF checkout is a different key, never a wrong hit. The repo root's own steps cover the whole repo.
- **The tools and what is installed.** The program the command starts (found on the PATH, by its path, size and time, without running it), the version of known tools by their exact name (Node and its package managers, Python and its managers, Cargo, Go) and of the runtime behind them, the package manager's marker in `node_modules`, and the `pyvenv.cfg` and packages of a `.venv`. A script of the repo is never run for a version.
- **The OS, the CPU architecture, and the values of a fixed list of environment variables** (`CI`, `NODE_PATH`, `PYTEST_ADDOPTS`, `NODE_ENV`, `NODE_OPTIONS`, `TZ`, `LANG`, `LC_ALL`, `PYTHONPATH`, `PYTHONHASHSEED`, `VIRTUAL_ENV`, `PYENV_VERSION`, `RUSTFLAGS`, `RUSTUP_TOOLCHAIN`, `CARGO_BUILD_TARGET`, `GOFLAGS`, `GOOS`, `GOARCH`, `CGO_ENABLED`, `GOTOOLCHAIN`, `JAVA_HOME`). Their values are hashed and never written down. No other variable is read.

Each step is keyed on the files as they are when it starts, so a step whose files an earlier step rewrote runs again.

A step is **never cached** when:

- an `.env.example` it covers (for the repo root's own steps, any in the repo), or one in a folder above it, names a service (a database, a cache, a queue) that no Compose file in the repo starts: its result depends on something outside the repo;
- its project, or one it depends on, has a dynamic import (`importlib.import_module`, `__import__`), or a tsconfig with `paths`: the graph cannot see what it loads;
- its command picks what to run from git history (`nx affected`, `turbo run --affected`);
- the repo root is a folder inside a larger git repository;
- an input could not be read: a file, a link to a folder, a submodule or nested repository, the list of files, the program, or a tool's version.

A pass is stored only if the files it depends on are the same at the end of the run as at the start, so a step that writes one of its own inputs is never stored. An entry older than 30 days is never served; such entries, and the least recently used beyond 500, are removed at the end of each run; deleting `.firescale/steps/` is always safe.

A step from the cache says so. Its status column reads `cached`, and its line says how long it took and when it passed:

```
verify: OK (2 of 4 steps passed from the cache, not run now)
  check      passed   8 rules, no enforced violations
  lint       cached   from the cache: it passed in 2.1s when it last ran (2026-11-13 07:16 UTC), and nothing it depends on has changed since
  typecheck  passed   pnpm run typecheck
  test       cached   from the cache: it passed in 41.3s when it last ran (2026-11-13 07:16 UTC), and nothing it depends on has changed since

From the cache: 2 steps passed before on the same inputs, so they were not run again; they took 43.4s when they last ran. Run fscale verify --no-cache to run every step.
```

In `--json` such a step keeps the status `passed`, has `durationMs: 0` and a `cache` object with the `durationMs` and `ranAt` of the run that passed; the report's `cache` says whether the cache was read and how many `hits` it gave.

- `--no-cache` runs every step and still stores the passes.
- `FSCALE_LOG=cache fscale verify` prints, per step, `hit`, `miss`, `not cached` with the reason, `stored` or `not stored`.
- A file git ignores is not in the key: a generated file or a `.env` a test reads can change without a miss. Use `--no-cache` after changing one.

## Running on slow, real projects

These parts of verify were written after running 0.1.2 on a course project with five Python services and a frontend, on Windows (ADR 0024).

### Progress

While verify runs, it prints a line on stderr when each step starts and when it ends, and a heartbeat line every minute for a step that is still running, saying what that step is doing:

```
[verify] check: started: fscale check
[verify] check: passed after 412ms
[verify] test in backend/audit_service: started: .venv/bin/python -m pytest
[verify] test in backend/audit_service: still running after 1m 00s; tests so far: 12 passed, 0 failed, 40 errors (in tests/test_orders.py)
[verify] test in frontend: still running after 1m 00s; last output: ✓ src/cart.test.ts (14 tests) 812ms
[verify] test in backend/audit_service: TIMED OUT after 10m 00s (limit 10m)
```

What the heartbeat says (ADR 0025):

- **pytest and unittest** print one character per test (`.` passed, `F` failed, `E` error, `s` skipped), often with no line end until a file is done. When those markers are the latest output, the heartbeat gives a running count, `tests so far: 12 passed, 0 failed, 40 errors`, with the file pytest is in. pytest is known by `pytest` in the command or its `test session starts` header; unittest by `unittest` or `manage.py test` in the command.
- `pytest -v` result lines (`tests/test_a.py::test_x ERROR`) are counted the same way.
- **Anything else** gets `last output:` and the last line that contains words (two letters in a row), so a line of dots or a lone `E` is never shown. A URL's user name and password are shown as `***`.

Progress is on when stderr is a terminal, and off when stderr goes to a file or a pipe (CI logs, agent hooks), so logs and hook messages stay as they were. `--progress` turns it on anyway, `--no-progress` turns it off. It never touches stdout: the summary and `--json` are the same with or without it. `fscale hooks run` prints no progress unless `--progress` is given, because an agent reads the hook's stderr. The variable `FSCALE_HEARTBEAT_MS` sets the heartbeat interval in milliseconds (`0` turns heartbeats off); it is meant for tests.

### Stopping cleanly

On Ctrl+C, on `SIGTERM` or `SIGHUP` (macOS and Linux), and on Ctrl+Break or closing the console (Windows), verify stops every command it started **with every process that command started**, prints the summary of what finished, and exits with code 130. The step that was running is `STOPPED`, the steps that had not started are `not run`, and the headline reads `verify: STOPPED - interrupted after 3 of 9 steps ran`, followed by any failures found before that.

- On macOS and Linux each command runs in its own process group. Stopping sends `SIGTERM` to the group, then `SIGKILL` to whatever is left after 2 seconds.
- On Windows each command runs in its own job object, which it enters before it can start anything. Stopping terminates the job. If fscale itself is killed outright, Windows closes the job and its processes end with it.

A **second** Ctrl+C (or signal) does not wait: it kills every running command at once and ends fscale, with no summary. Closing the console window on Windows still prints the summary: fscale has about 5 seconds before Windows ends it, and its job ends the commands either way. Ctrl+C during the `check` step stops verify with `verify: STOPPED - interrupted before the check finished` and exit code 130.

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

### A service that is not running, seen while the step runs

Setup detection (above) reads a step's output when the step ends. A test suite whose database is not running can take many minutes to end, with every test erroring, and pytest prints why only at the very end. So verify also watches a running step (ADR 0025). It is clear the step cannot succeed when:

- the output so far shows a refused connection (a message from the table above) to a service on this machine that is known by its client or its usual port, or that the project's `.env.example` or Compose file names, and nothing listens on that port; or
- pytest or unittest has reported at least 3 tests, every one an error (no pass, no failure), a service that the project's `.env.example` or Compose file names, in its folder or a folder above it, is not listening, **and the output names that service**: its host and port, its name, or its client library (`asyncpg`, `psycopg`, `redis`...). A service that is merely down is never named as the cause (ADR 0035).

verify checks the port itself, with a connection that it closes at once, and **checks again 10 seconds later**: only a service that is still down then counts, so a step that starts its own database, or waits for one, is left alone. It then prints one line at once, and by default stops the step. A refused connection makes the step **setup needed**. Tests that all error while a named service is down make it **failed**, with the service named as the likely cause, because the errors could also come from the code:

```
[verify] test in backend/audit_service: LIKELY SETUP, seen while running: PostgreSQL on localhost:5432 is not running (every test so far has errored (0 passed, 0 failed, 3 errors), the output names it, and backend/audit_service/.env.example (DATABASE_URL) lists it); start it with: docker compose -f ../../docker-compose.yml up -d; stopping the step (set verify.stopWhenServiceDown to false to let it run on)
[verify] test in backend/audit_service: FAILED after 14.2s (stopped early)
```

**Every test erroring, with no service to blame** (ADR 0035). When pytest or unittest has reported `verify.stopAfterErrors` errors (default 10) and no test has passed or failed, the step is stopped as likely setup, whether or not a service is down:

```
[verify] test in backend/audit_service: LIKELY SETUP, seen while running: every test so far has errored (0 passed, 0 failed, 10 errors) and none passed; stopping the step to find the first error (set verify.stopAfterErrors to false to let it run on)
[verify] test in backend/audit_service: running `python -m pytest --maxfail=1` (at most 1m 00s) to find the first error
```

**The first error.** After a pytest step with errors or failures is stopped early or times out, pytest never printed why. When the command is one pytest command with its arguments (no pipe, `&&`, `;` or redirection, which would hand the flag to another program), verify runs it once more with `--maxfail=1` added, with its own limit of 1 minute (the main run is never changed), and prints the first error's `E   ...` lines and its summary line under `First error of test ...`, with a connection string's user and password and a failed login's user name shown as `***`. When that text shows a refused connection to a service on this machine, or a failed password login to a local database, the step is **setup needed** and the summary names the host and port from the text. Otherwise it is **failed**, says that every test errored (which usually means the environment is not ready), and lists any service the folder names that is down as `also not running, though the output does not name it`.

**When it finishes.** A pytest step that ends with errors and no failures is **setup needed**, also when other tests passed, when the message of every error in pytest's short summary says a connection failed and names a local service that is not running: a refused connection, or the service's name, client or address as a whole word while nothing listens on its port. The test's own path and name never count, and an error about what a service returned (`relation "orders" does not exist`) is a failure.

**A timeout of errors** does not say "if it is only slow, raise the limit": when the counts show more errors than passes, the summary says the tests are erroring, not running slowly, and points at the first error.

How the two settings relate: `stopWhenServiceDown` covers stops that a service gives the evidence for (a refused connection, or a named service that is down); `stopAfterErrors` covers the stop with no service to blame. They are independent; to let every test step run to its end, set both to `false`.

To let such a step run to its end instead, set `"stopWhenServiceDown": false` in `verify`. The line is still printed, and the step is still reported as setup needed if it fails. Only host, port and service name are ever shown, never a connection string or a password. `fscale doctor` checks the same services before a run (see [doctor.md](doctor.md)).

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
- when an earlier verify ended without stopping its commands (it was killed outright on macOS or Linux) and they are still running: which ones, and the command that stops them (`kill -TERM -<group>`). On Windows a killed run's job ends its commands, so this never comes up there.

Records older than a day are removed without a word, since their process ids may belong to other processes by then.

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
- Projects run in the order of their folders. With `--affected`, only the projects a change reaches run (see "Affected only" below).

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

| Key                                      | Meaning                                                                                                                                                                                                                                                   |
| ---------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `lint`, `typecheck`, `test`              | A setting for the **whole repo**. A command runs once, from the repo root, and no project runs its own command for that step. `false` turns the step off everywhere.                                                                                      |
| `failOnSkipped`                          | `true` makes a skipped step fail the run. Default `false`.                                                                                                                                                                                                |
| `timeout`                                | The time limit of every step, unless a more specific one is set. Default `"10m"`. See "Time limits".                                                                                                                                                      |
| `ci`                                     | `false`: do not set `CI=1` for the commands. Default `true`.                                                                                                                                                                                              |
| `stopAfterErrors`                        | Errors a pytest or unittest step may report, with no test passed and none failed, before it is stopped as likely setup and its first error is found. Default `10`; `false` never stops it. See "A service that is not running, seen while the step runs". |
| `stopWhenServiceDown`                    | `false`: a step whose output shows that a service it needs is not running runs on to its end. Default `true`: it is stopped as soon as that is clear. See "A service that is not running, seen while the step runs".                                      |
| `<step>` as an object                    | `{ "command": "...", "timeout": "20m" }`: a command, a time limit, or both. Without `command`, the detected command runs with that limit.                                                                                                                 |
| `projects.<folder>`                      | Settings for one project, by its folder. `false` turns all its steps off. An object sets single steps to a command or to `false`, and wins over the repo-wide setting for that step. Its `timeout` is the limit of each of its steps.                     |
| `projects.<folder>` for any other folder | A folder `fscale facts` did not list as discovered runs exactly the commands given for it, in that folder. Use it for a project below the search depth.                                                                                                   |

A folder named under `projects` must exist, or verify stops with exit code 2. Write the folder as `fscale facts` prints it, in the same letter case. The top-level keys are for the root's own commands; `"."` under `projects` is only for other kinds of project at the root.

Commands run through the platform shell (`sh -c` on macOS and Linux, `cmd /C` on Windows) with `NO_COLOR=1`, `CI=1` and no input (see "No input, and CI=1" above). Root steps run from the repo root, project steps from the project's folder.

`--affected` narrows the `check` step to changed files (see [affected.md](affected.md)), runs only the projects a change reaches (below), and is passed on to Nx and Turborepo. `--base <ref>` is also passed to `nx affected`; for Turborepo, set `TURBO_SCM_BASE` yourself.

### Affected only

`fscale verify --affected [--base <ref>]` runs lint, typecheck and test only for the projects a change can affect (ADR 0032):

- The changed files are the uncommitted and untracked ones, plus, with `--base <ref>`, everything committed since the merge base with `<ref>` ([affected.md](affected.md)).
- The projects that own a changed file are affected, and so is every project that depends on an affected project or shared folder in the project graph ([graph.md](graph.md)), directly or not.
- **Docs change nothing.** A changed file that no project owns and that cannot change a build or a test (Markdown, text files other than requirement lists and `CMakeLists.txt`, a licence, anything under `docs/`) runs no project; the check still runs, and a note names the file (ADR 0034).
- **More, never less, when unsure.** Every project runs when a changed file belongs to no project, or is a root lockfile, a CI file (`.github/workflows/`, `.github/actions/`, `.gitlab-ci.yml` and the like), a Compose file anywhere, or the firescale config; when there is no commit to compare with; and when no `--base` is given and nothing has changed (there is nothing to compare, so a note says everything ran).
- A project left out is reported as `not affected`: one line per project in the summary, and the status `not-affected` for each of its steps in JSON. It is not `skipped`: its steps have commands, they just cannot be broken by this change. It is not counted in "N of M steps".
- The repo root's own steps (package scripts, Cargo, Nx, Turborepo) run when a project they cover is affected: the root's own project or a workspace member. Nx and Turborepo, and root commands set in the config, are not tied to projects, so they run whenever anything is affected. A folder named only under `verify.projects` runs whenever anything is affected, because `--affected` cannot tell what it uses.
- **The headline says how many projects ran, and never reads as a plain OK:** `verify: OK (affected only) - 2 of 6 projects ran, 4 not affected`, `verify: PARTIAL (affected only) - ...; 5 of 7 steps ran, 2 skipped`, or `verify: FAILED at test in backend/auth (affected only: 2 of 6 projects ran, 4 not affected)`. The repo root's own steps count as one project.
- Below the steps, `Affected only (...)` says what was compared, how many files changed and why each project ran: `changed: backend/auth/main.py`, or `depends on backend/shared (Python import)`.
- The stop hook runs only the affected projects, measured from the base commit (ADR 0039); see "The stop hook" below.

```
verify: OK (affected only) - 1 of 6 projects ran, 5 not affected
  check      passed   8 rules, no enforced violations
  in backend/audit_service: not affected
  in backend/auth_service:
    lint       passed   python -m ruff check .
    test       passed   python -m pytest
  in backend/ledger_service: not affected
  ...

Affected only (uncommitted and untracked changes; 1 changed file). Why each ran:
  backend/auth_service: changed: backend/auth_service/main.py
```

### Dry run

`fscale verify --dry-run` prints the plan and runs nothing: not the check, not a script, not an install, not Node. It reads only the `verify` settings of `firescale.config.json`; a `firescale.config.ts` is code, so a dry run refuses it (exit 2). With `--affected` it says which projects would run and why. It has no `--json` form.

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

| Field           | Meaning                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| --------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `schemaVersion` | Always `6`. Changes only on a breaking change. Version 6 added a step's `label` and `base`, the result `pre-existing` and the report's `base` (ADR 0037). Version 5 added `scope`, `affected` and the step status `not-affected` (ADR 0032). Version 4 added the result `stopped`, the step statuses `timed-out` and `stopped`, and a step's `timeoutMs` and `timeoutKey`. Version 3 added the result and step status `setup-needed`, the step's `fix`, and `notes`. Version 2 added `result`, the counts, `skipped`, `failOnSkipped`, `failedProject`, the `project` of a step, and the step status `off`. |
| `version`       | The firescale version.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| `ok`            | `true` when the exit code is 0: `result` is `ok` or `partial`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| `result`        | `ok` (every step ran and passed), `partial` (nothing failed, at least one step skipped), `pre-existing` (every failure also fails on the base commit; exit code 0), `failed` (a step failed or timed out, or steps were skipped with `failOnSkipped` on), `setup-needed` (nothing failed in the code, but a step needs setup) or `stopped` (Ctrl+C or a termination signal stopped the run; exit code 130). **Read this, not `ok` alone.**                                                                                                                                                                  |
| `runner`        | How the root's commands were chosen: `config` (all three steps set in the config), `nx`, `turbo`, `scripts`, `cargo`, `python`, `go`, `projects` (only the projects below the root) or `none`.                                                                                                                                                                                                                                                                                                                                                                                                              |
| `stepsTotal`    | Steps that should run: every step except those that are `off` or `not-affected`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| `stepsRun`      | Steps with a result: passed (now or from the cache), failed, timed out or needs setup.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| `failOnSkipped` | The config setting.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| `failedStep`    | The first step that failed, timed out or needs setup. The others are in `steps` and in the headline. Absent when no step failed.                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| `failedProject` | The folder of the project whose step failed. Absent for a root step.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| `rerun`         | The command to run again, from the folder in `failedProject` when that is present. Absent when no step failed.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| `steps`         | `check` first, then the root's steps, then each project's. Each has `name`, `status` (`passed`, `failed`, `setup-needed`, `timed-out`, `stopped`, `skipped`, `not-run`, `off`, `not-affected`), `durationMs`, `detail`, and where they apply `project`, `kind`, `command`, `exitCode`, `outputTail`, `fix` (the setup command of a `setup-needed` step, or of a `timed-out` one whose output shows a setup problem), `timeoutMs` and `timeoutKey` (the limit of a `timed-out` step and the config key that sets it).                                                                                        |
| `skipped`       | Every skipped step: `step`, `reason`, and `project` for a project's step. Empty when nothing was skipped.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| `notes`         | Things that are not failures: Python projects that ran with `python` from the PATH. Absent when there are none.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `warnings`      | One sentence naming the skipped steps, and with `--affected` what the graph cannot see (dynamic imports). Absent when there are none.                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| `scope`         | `all`, or `affected` for `--affected`: then `result` describes the projects that ran, not the whole repo.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| `affected`      | With `--affected`: `base` (the ref, or null), `everything` and its `reason`, `changedFiles`, `projectsTotal`, `projectsRun`, and `projects`, one per unit in plan order, each with `project` (absent for the root's own steps), `kind`, `affected` and `reason`.                                                                                                                                                                                                                                                                                                                                            |
| `cache`         | What the step cache did: `read` (`false` with `--no-cache`), `hits` (steps whose pass came from the cache instead of running now) and `savedMs` (what they took when they last ran). A step from the cache has `cache` with `durationMs` and `ranAt`, and `durationMs: 0`.                                                                                                                                                                                                                                                                                                                                  |
| `base`          | Blame-aware verify: `compared`; with a base its `reference` and `commit`, how many steps had the `sameInputs`, came `fromCache` or `ran` on the base, `budgetMs`, `durationMs` and `overBudget`; without one the `reason`. A step's `label` is `new`, `pre-existing`, `fixed` or `flaky`, and its `base` has the `status` on the base (`passed`, `failed`, `setup-needed`, `timed-out`, `over-budget`, `stopped`), the `source` (`same-inputs`, `cache`, `ran`) and a `note` when a failure blocks for want of a base result.                                                                               |     |
| `failedTests`   | For pytest, Vitest and Jest: each failing test of a step (`id`: its file and name), with its `label` (`new` or `pre-existing`) when there is a base.                                                                                                                                                                                                                                                                                                                                                                                                                                                        |

## The stop hook

`fscale hooks run` blocks an agent only when verify fails, and it uses blame-aware verify, so an agent is only sent back for what it broke (ADR 0039):

- **It runs only what the change reaches**: the projects of the files changed since the base commit (uncommitted ones included), and the projects and shared folders that depend on them in the project graph, as `--affected` does. The rest are `not affected`.
- **Every failure is compared with the base**, but only failures run on the base: a passing step never does in the hook. The base run stays within `verify.baseBudget`.
- **A `new` failure runs once more** before it blocks; `flaky` ones do not block.
- It lets the agent stop when every failure is `pre-existing` or `flaky`, and blocks on a `new` one, or on a failure the base could not judge.

When a step needs setup, the message the agent gets starts with `SETUP NEEDED:`, says that it is not a problem in the code, tells the agent not to edit code, tests or config to get past it, and names the setup command to run or to ask the user for. On a partial run the agent may stop, and the hook prints a line on stderr that says the run was not a clean pass and names the skipped steps. That line is for the person reading the hook's output or its log: an agent may not be shown the stderr of a hook that lets it stop. With `verify.failOnSkipped` on, a partial run is a failure and the hook blocks. ADR 0020 explains the default.

## Limits

- A step's own output is captured, not streamed. Progress lines say what runs, and the heartbeat shows a running count of tests (pytest, unittest) or the last line with words.
- Watching a running step for a service that is down knows pytest and unittest markers and the refused-connection messages in the table. A suite that waits silently on a service (no markers, no messages) is only caught when it ends or times out.
- On macOS and Linux, a process that leaves its process group (a daemon that calls `setsid`) is not stopped with its command, and if fscale is killed with `SIGKILL` its commands keep running; the next run names them. On Windows, a daemon started by a step (such as a build server) is stopped at the end of the step, because it is in the step's job. If Windows refuses to make a job (rare: a parent job that forbids it), `taskkill /T` stops a step on a timeout or Ctrl+C, but what a step leaves running after it ends is not stopped.
- With Nx, the task names must be `lint`, `typecheck` and `test`. Use the `verify` config key for other names.
- A Node, Rust or Go project's commands run with the tools found on the PATH. A Python project's run with its interpreter (see above).
- Setup detection only knows the messages in the table. A setup problem with another message is a normal failure.
- Projects run one after another unless `--jobs` is given.
- A step that is skipped in every run stays in the summary until it gets a command or is set to `false`.
- `verify` is as fast as the repo's own commands. firescale's share is a few milliseconds (see `bench/RESULTS.md`).
