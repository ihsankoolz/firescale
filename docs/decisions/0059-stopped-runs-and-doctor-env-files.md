# 0059 — A stopped run says what it did not compare; doctor reads env files in the folders above

Status: accepted (2026-11-15, for 0.5.1). Refines ADR 0024 (what a stopped run prints), ADR 0037 and ADR 0042 (the base comparison and the tamper guard, which a stopped run leaves out) and ADR 0026 (doctor's env check).

## Context

The owner ran 0.5.0 on a course project on Windows: a frontend and Python services under `backend/<service>/`. Two reports came back.

1. A full `fscale verify` was stopped with Ctrl+C after more than an hour and a half. Steps that had already failed showed a plain `FAILED`, with no `new` or `pre-existing` label, and there was no tamper guard line. Nothing said why.
2. `fscale doctor` reported a PROBLEM for `backend/<service>/.env.example`, "there is no `backend/<service>/.env`", in a repo whose one `.env` is at the root and where every test of that service passes.

Both were reproduced with a test before anything changed.

### The facts behind the first report

`verify_report` (`crates/cli/src/main.rs`) runs the steps, then `blame::compare`, then the tamper guard, then prints. When the steps end as stopped it skips `compare` and the guard, and left the report's `base` empty. The summary then looked like a run with no base at all, minus the sentence that says so.

### The facts behind the second report

- **verify loads no env file.** A step's command gets the environment verify was started in, plus `CI`, `NO_COLOR`, `FORCE_COLOR`, `PYTHONUNBUFFERED` (and `FSCALE_BASE_RUN` on the base). See `environment` in `crates/core/src/verify.rs`. Whether a variable reaches a test is up to the project's own tools.
- **Those tools differ.** python-dotenv's `load_dotenv()` searches the folders above. pydantic-settings reads the `env_file` it is given, relative to the working folder, or a path the code builds (often the repo root). Vite reads its `envDir`. Docker Compose reads the `.env` beside the Compose file. firescale cannot tell which one a project uses without running its code.
- **The rest of firescale already looks above.** The rule `core/env-example` accepts a name listed in a `.env.example` at or above the reading file. Doctor's own services check reads `.env` and `.env.local` in each project's folder and every folder above it. The base run copies the root's and each project's `.env` into the worktree (`blame::env_files`).
- **Doctor's env check did not.** It read `.env` and `.env.local` only beside the `.env.example`, and the process environment.

So the root `.env` is a place a service's variables can really come from, and doctor was the one part that did not look there.

## Decision

### A run stopped before its steps finished compares nothing, and says so

When the stop arrives before the head's own steps have finished, verify starts nothing more: no base run, no reading of kept base results, no tamper guard. The report now carries the base it would have used, with `compared: false`, the base's `reference` and `commit`, and the `reason` "the run was stopped before failures were compared with the base commit and before the tamper guard ran". The summary prints one paragraph:

```
Not compared with the base commit 1a2b3c4 (main): the run was stopped first, so no failure is labelled `new` or `pre-existing`, and the tamper guard did not run. Run fscale verify again and let it finish to get both.
```

It is printed whenever a run with a base is stopped, with or without a failure, because the tamper guard line is missing either way. `--json` carries the same fact in `base`; no field was added, and the schema's text for `base.reason` says what the combination means.

Why not label the finished failures:

- The person pressed Ctrl+C and wants the process gone. A base run in a worktree can take as long as the run they just stopped.
- On Windows the whole console gets the event, so commands started for a base run right after it would start into a console that is being torn down.
- Labels that need no run (the same inputs as on the head, or a kept base result) would be cheap, but `compare` reaches them through git and file hashing over the whole plan, and it would label some failures and not others. A partly labelled, stopped run reads worse than a plain statement. The next run labels everything, and steps that passed come from the cache.

### Doctor's env check reads the folders above, up to the root

For each `.env.example`, a name counts as set when it is in the environment (as before), or in a `.env` or `.env.local` in the example's folder or in any folder above it up to the firescale root. Only names are read; values are dropped as each line is read, as before.

- All found, none from above: the text is unchanged.
- A name found only in a file above: `ok`, and the line says where, nearest file first: `all 2 variables are set in the environment, or in an env file of a folder above (SVC_REGION in backend/.env.local; SVC_API_KEY in .env)`.
- A name found nowhere: still a PROBLEM. The line names every env file that was read, and says when there is none beside the example: `1 of 1 variables are not set in the environment or .env, and there is no backend/svc/.env: NAME.` The fix names the files that exist and the new file beside the example, since doctor cannot know which one the service reads.
- No env file anywhere at or above a project below the root: `... and there is no lonely/.env, nor a .env in a folder above it`.

The JSON format is unchanged: the same `detail` and `fix` strings carry the new text.

## Consequences

- An agent or script that reads `--json` of a run stopped before its steps finished now finds `base` present with `compared: false` and a `commit`. Before, `base` was absent. `compared: false` with a `commit` only happens for a stopped run.
- Doctor can now say `ok` for a variable that a service never reads from the file above (pydantic-settings pointed at the service's own folder, say). It already could not know whether anything loads the file beside the example. The test run is what proves it; doctor's docs say what the check can and cannot know.
- **Known limit: a stop that arrives during the base run is not changed here.** The head's steps have finished by then, so `compare` is already running: the base steps end as stopped, the report has `compared: true`, a failure the base could not judge blocks with a note (ADR 0037), and the tamper guard still runs afterwards, which reads the base commit through git. So "starts nothing more" holds only for a stop before the head's steps finished. Skipping the guard in that case would leave a report that says it compared and has no tamper line, with nothing to explain it, so it was left as it is.
- A `.env` above the firescale root is never read.
