# 0024 — Verify on slow, real projects

Status: accepted (0.1.3). Changes ADR 0014 and ADR 0022 (verify: every command runs as a process tree with a time limit, `CI=1` and no input; new step statuses `timed-out` and `stopped`, a new result `stopped`, exit code 130, schema version 4) and ADR 0020 (facts: run-once sibling scripts for watch-mode steps, a repo-level `notes` list; a refused connection to a local service is setup needed).

## Context

The owner ran 0.1.2 on the course project on Windows: five Python services and a Vite frontend.

1. `fscale verify` printed nothing for over ten minutes. One service's pytest run took 13 minutes and ended `49 passed, 187 errors`, most likely fixtures waiting for a test database that was not running. The five services run one after another.
2. Ctrl+C stopped fscale but left two `node` and four `python` processes running. Later runs then seemed to hang, with no output.
3. The frontend's `test` script is `vitest`, which watches for changes and never exits in a terminal. There is also `test:run` (`vitest run`).

## What the later runs were waiting on

fscale takes no lock and never did: the result cache, the hook state and the config are read and written without one, so a second `fscale verify` was never waiting on the first. Reading the code and testing found two things that do explain it:

- **Verify waited for the output pipes to close, not for the command.** 0.1.2 ran each step with `Command::output()`, which reads stdout and stderr until every process holding them has closed them. A process that a step leaves running in the background (a test server a fixture started, a worker that outlives its runner) inherits those pipes, so verify kept waiting after the step itself had ended, with nothing printed. The test "does not keep verify waiting on its output" in `verify-run.test.ts` shows it: with the old wait restored, verify hangs until the test gives up; with this change it returns at once.
- **Nothing was printed while the same slow step ran again.** Every later run repeated the 13-minute pytest run, silently, while the processes the interrupted run had left behind (Ctrl+C killed fscale, but not the shell's children, which on Windows include the `.venv` launcher's real `python.exe`) kept running and kept trying the same database.

Not proven on the owner's machine: which of the two each later run hit. Both are fixed below, and the progress lines now show which step is running and what it last printed.

## Decision

### Every command is a process tree that is stopped as a whole

- **macOS and Linux:** the shell is the leader of a new process group (`process_group(0)`). Stopping sends `SIGTERM` to the group, waits up to 2 seconds, then sends `SIGKILL`.
- **Windows:** the shell is started suspended and in a new process group, assigned to a new job object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, and only then resumed (every thread of the process, found with a Toolhelp snapshot), so nothing it starts escapes the job. Stopping terminates the job. When fscale dies for any reason, Windows closes the job handle and its processes end. If no job can be made, `taskkill /T /F` stops the tree.
- When the command ends by itself, what it left in its group or job is stopped too, and verify waits at most 2 seconds for the output pipes to close.
- Ctrl+C, `SIGTERM` and `SIGHUP` (Unix) and Ctrl+C, Ctrl+Break, closing the console, log-off and shutdown (Windows) only set a flag; the loop that watches each command sees it within 20 ms, stops the tree, and verify prints the summary of what finished and exits with **130**. The handler is installed only by `verify` and `hooks run`.
- A second Ctrl+C or signal does not wait for the grace period: on Unix the handler sends `SIGKILL` to every registered command group (a fixed table of atomics, so it is safe in a signal handler) and re-raises the signal with the default action; on Windows the handler returns false, the default handler ends fscale, and the jobs close. For closing the console, log-off and shutdown, the Windows handler waits up to 4.5 seconds so verify can stop its commands and print the summary before Windows ends the process.
- Ctrl+Break and closing the console reach every process on the console, including a step's shell. A command that ends with `STATUS_CONTROL_C_EXIT` while (or within half a second before) the stop flag is set is reported as stopped, not failed. Ctrl+C during the check step, which also reaches the Node host and git, ends verify as stopped (exit 130) instead of with their error.
- New step status `stopped` (`STOPPED`); steps that had not started are `not-run` ("verify was stopped"). New result `stopped`. The headline reads `verify: STOPPED - interrupted after 3 of 9 steps ran`, then any failures.

### Time limits

- Every step has a limit, **10 minutes** by default: long enough for most test suites of a course project, short enough that a run waiting on a database that is not there ends with an answer.
- Keys: `verify.timeout`, `verify.<step>.timeout`, `verify.projects.<folder>.timeout`, `verify.projects.<folder>.<step>.timeout`; the most specific wins. A step's value can now be an object `{ "command", "timeout" }`; with only `timeout`, the detected command runs. A limit is seconds, or `"90s"`, `"10m"`, `"1h"`, `"1h 30m"`; `false` is no limit; `0` is refused.
- A timed-out step is `timed-out` (`TIMED OUT`), with its limit (`timeoutMs`), the key to raise it (`timeoutKey`) and the last 30 lines of output. It counts as a failure (result `failed`, exit 1) and stops the rest of its unit. When its output shows a setup problem, the step also gets the `fix`.
- The human summary shows the limit, never the elapsed time, so it stays stable for snapshots.

### Progress

- Lines on **stderr** prefixed `[verify] `: one when a step starts (with the command), one when it ends (result and time), and a heartbeat every 60 seconds with the last line of output. Each line is written whole under a lock.
- On when stderr is a terminal; off otherwise (CI logs, agent hooks, tests). `--progress` and `--no-progress` override. `hooks run` is quiet unless `--progress` is given, because agents read a hook's stderr. `FSCALE_HEARTBEAT_MS` sets the interval, for tests.

### No input, and CI=1

- Stdin was already empty; it stays so, with no option to change it, because with `--jobs` no step owns the terminal and a step that waits for input would only run into its time limit.
- `CI=1` is set unless the environment already sets `CI` (its value is kept) or `verify.ci` is `false`. Documented in `docs/verify.md` with what it changes: Vitest and `react-scripts test` stop watching, Jest does not write new snapshots, Playwright configs often turn on retries and `forbidOnly`.

### Watch-mode scripts

- `fscale facts` replaces a `test`, `lint` or `typecheck` script that is a known watch command with its first run-once sibling (`<step>:run`, `<step>:ci`, `<step>:once`) that does not watch, and says so in a new repo-level `notes` list. Without a sibling it warns and the command is unchanged; verify relies on `CI=1`. The list of watch commands is in `docs/facts.md`.
- The root's `commands` switch too, because verify's root runner reads them. `agent-ready/commands-documented` therefore asks for `npm run test:run`, which is right: `npm test` would hang an agent.
- The facts schema stays at 2: only a field was added.

### Services that are not running

- A refused connection to `localhost`, `127.0.0.1`, `::1` or `0.0.0.0`, in the messages of libpq, redis-py, MySQL, Prisma, Node, asyncio, pymongo and Python sockets, is **setup needed**, even when tests also failed: tests that need a service cannot be judged while it is down. A remote host stays a normal failure.
- With a Compose file in the project's folder or above it, up to the root, the fix is `docker compose up -d` (with `-f <relative path>` for a file above).

### Parallel projects

- `--jobs <n>` runs up to `n` units (the root and each project) at once, default 1. Units are independent already (ADR 0022). Reports are collected per unit and listed in the plan's order, so the summary and JSON are the same as with one job.

### Runs never wait on each other

- No lock. Each run writes a record to `<temp>/firescale-runs/<repo hash>-<pid>.json`: its pid, start time and the commands it is running with their process ids, and removes it at the end. A new run reads the records of the same repo and, on stderr, names another run that is alive and what it runs, or commands that an earlier, killed run left behind, with `kill -TERM -<group>`. On Windows the second case cannot happen (the job ends with fscale), so a live process with a recorded id is another process that reuses the id and is never named. Stale records with nothing left, broken records and records older than a day are removed. A record that cannot be read at the moment is left alone. Process ids can be reused, so a note about a running verify can, rarely, be wrong; the day limit bounds that.

### Dependencies

`libc` 0.2.191 (Unix) and `windows-sys` 0.61.2 (Windows) are now direct dependencies of `firescale-core`, pinned to the versions already in `Cargo.lock` through `ignore` and `tempfile`, so the dependency tree did not grow. The standard library cannot signal a process group, catch Ctrl+C or use job objects. The `ctrlc` crate would add `nix`; hand-written FFI declarations for the job-object structures would be easy to get wrong.

## Consequences

- Verify's JSON is schema version 4. Blame-aware verify (M17) takes 5.
- On Windows a daemon a step starts (a build server, the Nx daemon) is stopped at the end of the step, since it is in the step's job. On Unix one that leaves its process group survives, as it does today.
- If fscale is killed with `SIGKILL` on Unix, its commands keep running; the next run names them. Linux's parent-death signal would only reach the shell, not its children, so it was not used.
- The time limit and `ci` are config a change can loosen; the tamper guard (M20 to M23) should read them from the base.
- In containers whose first process does not reap orphans, the stop of a tree can take the full 2-second grace period, because a killed grandchild stays a zombie in the group until it is reaped.
- The Ctrl+Break test on Windows gives the test helper a console of its own (`AllocConsole`), because console control events only reach processes on the sender's console.
