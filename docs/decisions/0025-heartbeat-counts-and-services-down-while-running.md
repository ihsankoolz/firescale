# 0025 — Heartbeat counts and services that are down, seen while a step runs

Status: accepted (M12). Changes ADR 0024 (the heartbeat's text; a running step can now end early as setup needed).

## Context

The owner ran 0.1.3 on the course project on Windows. Progress lines, setup detection, parallel projects and Ctrl+C worked. Two things did not:

1. The heartbeat said `last output: E`. pytest prints one marker per test (`.`, `F`, `E`, `s`) and flushes each one with no line end, so the last chunk of output was a single `E`.
2. Three services spent over six minutes each with every test erroring, most likely because their test database was not running. Setup detection reads the output when a step ends, and pytest prints the reason for each error (`connection ... refused`) only in its summary at the very end, so nothing said so while the time went by.

## Decision

### The heartbeat reads the output as lines, and counts tests for known runners

- Output is read per stream as lines; a carriage return also ends a line. The unfinished last line is read too, without being counted twice when it ends.
- **pytest** (known by `pytest` or `py.test` in the command, or by its `test session starts` header) and **unittest** (`unittest` or `manage.py test` in the command) are the known runners. A line made only of their markers, optionally after the file name and before `[ 40%]`, is counted: `.` passed, `F` failed, `E` error, `s` skipped; `x`, `X` and `u` are not counted.
- When markers are the latest output of a known runner, the heartbeat says `tests so far: 12 passed, 0 failed, 40 errors (in tests/test_orders.py)`. All three counts are always shown, so "0 passed" is visible; skipped only when there are some.
- Otherwise it says `last output:` and the last line that has **words**, meaning two letters in a row. A lone `E`, a row of dots or a percentage is never shown. pytest's failure lines (`E       assert 1 == 2`) have words and are shown as they are.

### A running step is watched for a service that is down

Every 500 ms, when new output came, the last 200 lines and the counts are looked at. It is **clear the step cannot succeed** when one of these holds, and a probe confirms it:

- **(a) a refused connection in the output so far:** one of the documented messages (`docs/verify.md`) names a service on this machine, the service is known by its client (libpq, redis-py, Prisma...) or its usual port, or the project's `.env.example` or Compose file names that port; for a known runner at least one test has failed or errored. A refused connection to an unknown port that the repo does not name is ignored, because a test may connect to a closed port on purpose.
- **(b) every test so far has errored:** a known runner has reported at least 3 errors and no pass or failure, and a service named by the `.env.example` or Compose files of the step's folder or a folder above it, up to the root, is not listening. This is the owner's case: pytest shows only `EEEE` until the end.

The **probe** is a TCP connection to the host and port with a 400 ms limit, closed at once; at most one round of probes every 5 seconds per step. Something listening means "not clear", and the step goes on.

When it is clear, verify prints one progress line at once: what is down, why it thinks so, the start command (`docker compose up -d` from the nearest Compose file, as for setup detection), and what happens next.

The services a repo declares are read by a new module, `services.rs`, which `fscale doctor` uses too (ADR 0026). It keeps only the service, host and port: user names, passwords, database names and connection strings are never kept, so never printed. Tests check that a password and a database name in `.env.example` appear nowhere in the output.

### The step is stopped early by default: `verify.stopWhenServiceDown`, default `true`

`"verify": { "stopWhenServiceDown": false }` lets the step run on to its end; the line is still printed, and a failed step is still reported as setup needed with that reason when its own output shows nothing else. With the default the step is stopped with its whole tree and reported as **setup needed** (`SETUP NEEDED after 9.4s (stopped early)`), with the start command as its fix. The rest of that project is not run, as after any blocking step; other projects still run.

Why stop by default:

- The two conditions are each backed by a probe that sees nothing listening on the port right now. The tests that need the service cannot pass while that is so; letting them run on only spends the minutes the owner lost, and in an agent's stop hook it spends them on every attempt.
- The result is the same status the step would get when it ends (setup needed, not a failure), only sooner.
- A wrong stop is visible and cheap to undo: the line names the service and the setting that turns it off.

What a wrong stop would take: a suite that connects on purpose to a known service port (5432, 6379, 3306, 27017, 5672) or to a port the repo's own `.env.example` names, with nothing listening there, and that would still pass. Or a suite whose every test errors for a reason other than the service, while a service the folder names happens to be down; the step fails either way, so only the label (setup needed instead of failed) and the time saved differ.

### Other choices

- `Ending::GaveUp` is the new way a command can end in `process.rs`; verify maps it to `setup-needed`. The JSON schema does not change (version 4): the status already exists.
- The step's detail says `(stopped early after <time>)`, so the JSON tells an early stop apart from a step that ended by itself.
- A step that is stopped early or that times out, and whose output has no setup message, takes the reason seen while it ran.

## Consequences

- The course project's three services would now be stopped about a second after their third error, with the database named, instead of after six minutes.
- Listed under "Needs your decision" in `DECISIONS.md`: the default `true`, and the thresholds (3 errors, the 5-second probe interval).
- Not covered: a suite that waits on a service in silence (no markers, no messages) until it times out, and runners other than pytest and unittest. Jest, Vitest, Go and Cargo print readable lines, so their heartbeat is already useful; their refused-connection messages are covered by (a).
