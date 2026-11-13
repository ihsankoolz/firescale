# 0035 — Name a cause only with evidence, stop all-errors steps, and show the first error

Status: accepted (0.2.1). Changes ADR 0025 (a service that is down is no longer named as the cause without evidence) and ADR 0022 (a failed login to a local database is setup needed). Adds to ADR 0026 (doctor reads local URLs in `.env` and `.env.local`).

## Context

The owner ran 0.2.0 on the course project twice:

- **Run 1, Redis down.** Three pytest projects were stopped early, and Redis was named as the cause.
- **Run 2, Redis up.** The same three still errored on every test (0 passed, about 13 errors a minute) and ran to the 10-minute limit. So Redis had not been their cause, and nothing stopped them the second time.
  - Their real problem was the test database. Each test errored with asyncpg's `ConnectionRefusedError` to `localhost:5433`, a port that comes from the tests' own settings (`TEST_DATABASE_URL` in `.env`, or a default in `conftest.py`). It was not in the service list, so nothing watched it.
  - After a stop or a timeout the summary showed only rows of `E`, never an error message, so the refused connection never reached it.
- **A fourth project** had 96 passed and 15 errors, each saying "Cannot reach the test Redis" and "docker compose up -d redis". It was labelled FAILED with "Fix the failure", which tells an agent to change code.

## Decision

### 1. A service is the cause only with evidence from the step's own output

- ADR 0025 rule (b) named a service as the likely cause when every test errored and a service the folder names was down. That rule now also requires **the output to name the service**: its host and port (`localhost:5433`, `('127.0.0.1', 5433)`, `port 5433`), its name (`Redis`, or the Compose service's key), or its client library (`asyncpg`, `psycopg`, `libpq`, `redis`, `aioredis`, `pymysql`, `pymongo`, `pika`...).
- A service that is down without such evidence is listed as `also not running, though the output does not name it`, never as the cause.
- Refused connections (rule (a)) were already evidence and are unchanged.

### 2. Stop early without a known service: `verify.stopAfterErrors`

- A pytest or unittest step that has reported **10 errors** (default), with no test passed and none failed, is stopped as likely setup, whether or not a service is down.
- `verify.stopAfterErrors` sets the count. `false` turns this stop off.
- Ten, not ADR 0025's three: there is no probe behind this stop, so it waits for a little more evidence. On the course project, ten errors took under a minute.
- **How the two settings relate.** They are independent:
  - `stopWhenServiceDown` covers the stops that a service gives the evidence for: a refused connection, or a named service that is down.
  - `stopAfterErrors` covers the stop with no service to blame.
  - To let every test step run to its end, set both to `false`.
- A suite with any pass is never stopped this way, so a slow passing suite is left alone.

### 3. Show the first error: one extra run with `--maxfail=1`

- After a pytest step is stopped early or times out, verify runs **the same command with `--maxfail=1` appended**, once, with its own limit of **1 minute**, in the same folder and environment.
- The user's command is never changed for the main run. This is a separate, short run, announced on the progress line.
- pytest stops at the first failing or erroring test and prints its error, so the run is short even for a suite of thousands.
- From its output, verify keeps:
  - the first block of `E   ...` lines (at most 12);
  - the first line of pytest's short summary.
- Each kept line is cut to 300 characters. A connection string's user and password are replaced by `***`, and so is the user name of a failed login.
- These lines are shown as `First error of test ...` in the summary and as `firstError` in JSON.
- Why a second run and not a reporter plugin: changing the main run's arguments would change what the user's command does (its output, its exit code, its plugins). A plugin would also need writing to the user's environment. One extra run of their own command, plus a flag pytest has always had, needs neither.
- Only pytest gets the extra run. Unittest has no `--maxfail`.
- **When the first error shows setup:** a refused connection to a service on this machine, or a failed password login to a local database (below), makes the step **setup needed**, also after a timeout. The summary names the host and port from that text, which is how the course project's `localhost:5433` reaches it.
- **Otherwise the step is failed.** Its detail says that every test errored, which usually means the environment is not ready, and lists any service that is also not running.

### 4. Errors that all name an unreachable service, in a finished step

- This applies to a pytest step that ends with errors and **no failures**.
- If every `ERROR` line of pytest's short summary names a local service that is not running, the step is **setup needed**, also when other tests passed.
- "Names a service that is not running" means one of:
  - a refused connection to it;
  - its name, client or address in the line, while a probe of its port (from the line, the folder's services, or the usual port) finds nothing listening.
- One `FAILED` line, or one error that names no such service, keeps the step **failed**. A real code failure still says "Fix the failure".
- The agent text for setup needed already says not to change code (ADR 0022).

### 5. A timeout of errors does not say "raise the timeout"

- verify counts the tests of a finished or stopped pytest or unittest step (`tests` in JSON).
- When a timed-out step's counts show errors and fewer passes than errors, the summary says the tests are erroring, not running slowly, and that a longer limit would not help. It points at the first error instead of naming the timeout key.

### 6. A failed password login to a local database is setup needed

The forms recognised, each counted only when its address is local or it names none:

- libpq and psycopg: `connection to server at ... failed: FATAL:  password authentication failed`
- asyncpg: `InvalidPasswordError`
- Node `pg`: `error: password authentication failed`
- MySQL: `Access denied for user ... (using password: ...)`
- Prisma: P1000

The step gets no fix command: there is nothing to start, and the user or password in the test settings is what to fix. The user name is never shown.

### 7. Doctor checks local database and Redis URLs in `.env` and `.env.local`

- Doctor also reads `.env` and `.env.local`, in the folders it reads `.env.example` from. It takes only URL values of known services whose host is `localhost`, `127.0.0.1` or `::1`.
- It keeps only the variable name, the host and the port. The user, the password, the database name and the full URL are never stored, so they can never be printed. Tests check the `Debug` text and doctor's output.
- Verify's service watch does not read these files: verify finds the test database through the first error instead.
- "Reachable" in doctor means something accepts a connection on that port. It does not mean that it is the right database or that the login works (`docs/doctor.md`).

### After the review

An independent review before the pull request found ways a code failure could be called setup, which tells an agent not to fix its code. All are fixed, each with a test:

- **The login check.**
  - A test that quotes "password authentication failed" in an `assert` (pytest prints the line) no longer counts as a login failure.
  - With failed tests, a login failure makes the step setup needed only when every `FAILED` line is itself a login failure. A plain `assert 41 == 42` beside it keeps the step failed.
- **The "every error names an unreachable service" rule (item 4).**
  - It reads only each error's message, after `-`, never the test's path or name.
  - It matches client libraries as whole words, so `redistribute` is not `redis`.
  - The message must also say a connection failed: refused, cannot reach, could not connect, `ConnectionError`, timed out and similar. An error about what the database returned (`relation "orders" does not exist`) is a code or migration problem.
  - It uses the folder's own service port of that kind before the usual port. When that service is up, the rule does not apply.
  - Any failure the runner counted, even with no `FAILED` line (`-rE`), keeps the step failed.
- **Evidence while a step runs.** Only lines that say a connection failed count. A progress line such as `tests/test_redis_cache.py EEE` names a file, not a cause.
- **After a stop with a named service,** the first error's own text wins. A refused connection to the test database makes the step setup needed even though Redis was also down and named. This is run 1 of the course project.
- **The extra run happens only when it can reach pytest:**
  - The command must name pytest and be one program with its arguments: no pipe, `&&`, `;`, redirection or substitution. `pytest | tee log` would hand `--maxfail=1` to `tee`, and `make test` to make.
  - At least one test must have errored or failed, so a passing suite that only timed out gets no extra run.
  - It must not be after a stop. It is recorded as a running command like any step.
- **Secrets.**
  - The last lines of a failed step are now cleaned like the first error.
  - A URL's user and password are hidden up to the last `@`, so a password with `/` is hidden too.
  - `password=`, `pwd=` and `passwd=` values are hidden.
  - The names after `user`, `role` and `credentials for` are hidden, in `"`, `'` or backquotes.
- **Address matches are exact.** `localhost:5432` is not found in `localhost:54321`.

## Consequences

- On the course project, run 2 would now stop each of the three services after about ten errors. A one-minute `--maxfail=1` run would then show asyncpg's refused connection to `localhost:5433`, and the step would be setup needed, naming that host and port. Doctor would list `localhost:5433` from `.env` before the run.
- The fourth project would be setup needed ("every error (15) names Redis on localhost:6379"), and not "Fix the failure".
- A wrong early stop now costs a little more: up to a minute of extra pytest run. The step was failing anyway: no pass, only errors.
- Tests changed on purpose, none weakened:
  - The ADR 0025 test in which a down service named only in `.env.example` was the cause now proves the opposite. The service is not named, and the step is stopped by `stopAfterErrors`. The Rust test of that rule now feeds output that names the service.
  - The verify error message for an unknown key lists `stopAfterErrors`.
  - A setup test's assertion that a local password failure was not setup needed now checks only that it is not a refused connection.
