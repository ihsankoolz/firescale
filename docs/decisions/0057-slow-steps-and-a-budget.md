# 0057 — Slow steps: a kept time, a warning, a budget, and a steadier estimate

Status: accepted (2026-11-15, for 0.5.1). Refines ADR 0024 (progress, the heartbeat, what a stopped run leaves), ADR 0025 (the heartbeat line), ADR 0043 (the time left) and ADR 0020 (a run that left a step out is never a clean pass). Verify's JSON is schema version 7.

## Context

The owner ran 0.5.0 on the course project, on Windows. A full `fscale verify` took over 1h 33m and was stopped by hand. One project's tests take about 1h 40m; the other five projects finish in about 35 minutes together. Nothing warned that the step was slow, the stopped run left nothing behind, there was no way to run "everything but the slow step", the heartbeat printed a line every minute for all of that time, and the time left it gave went from 2h to 3h 12m to 1h 43m within minutes.

What verify kept before this ADR could not answer "how long did this step take last time":

- The run records (`runs.rs`, ADR 0024) live in the temp folder, name the commands a run has started, and are removed when the run ends.
- The step cache (`stepcache.rs`, ADR 0036) keeps a duration, but only for a pass, under a key made of the content of every file the step reads. Once a file changes, which is when the step runs again, the entry is not found.

## Decision

### The time of each step is kept by what the step is, not by what it reads

`.firescale/step-times.json` holds one record per step: its project folder, its name, its command, a duration, whether that run finished, and when it ran. A step is found by project, name and command. The kind of the project is not part of it: two kinds in one folder have different commands.

It is one small file, read before each step and rewritten when a step ends. That is the simplest place that survives a change of the step's cache key. The alternatives were worse: a second index inside the step cache would be cleaned with it (30 days, 500 entries) and tied to passes; the run records are per run and in the temp folder.

What a run writes:

- **A pass sets the time**, shorter or longer than before.
- **A run that ended without passing only raises it.** A failure is often quicker than a whole run (an import error, a first failing test with `-x`), and must not make a step of 1h 40m look quick.
- **A run that did not finish only raises it too**, and marks the record as not finished: stopped by Ctrl+C or a signal, stopped at a time limit (the step's, the project's, `verify.timeout`, the idle limit), or stopped early for a service that is down. The next run reads it as "it ran at least 1h 33m when it last ran and did not finish".
- The base run of blame-aware verify, a rerun for flakiness and the extra `--maxfail=1` run keep nothing: they are not the step as the user runs it.

The file is read again before each write, so two verify runs of one repo keep each other's records, and it is written beside and renamed (up to five tries, 20 ms apart, for Windows, where a rename fails while another program holds the file). A record that cannot be written is not an error. At most 2000 records are kept.

**Symbolic links.** As for the step cache since ADR 0056 (bug 17): when `.firescale` or the file is a symbolic link, nothing is read and nothing is written. The check is written in place, as that ADR chose; no shared writer was built.

### A stopped run leaves the time on the path Windows really takes

The record of a stopped step is written where `run_step` learns how the command ended. Every stop that verify handles goes through there, on every platform: the handler only sets a flag, the watch loop stops the tree and returns. On Windows that covers Ctrl+C, Ctrl+Break (also when the command gets the event first and exits with `STATUS_CONTROL_C_EXIT`) and closing the console, where verify has about 5 seconds.

Some stops run no code of verify at all: a second Ctrl+C, Task Manager, `taskkill /F`, a closed terminal that does not wait. So the same record is also **written at each heartbeat while the step runs**. A killed verify leaves the time of the step's last heartbeat: within a minute in the first ten minutes, within five after. The "only raises" rule makes these writes safe: they never replace a longer time.

### The warning

Before verify starts a step whose kept time is over 5 minutes, it prints one progress line, in the form of the other progress lines (stderr, `[verify] <place>: ...`): `slow step: it took 1h 40m when it last ran. To leave out steps this slow, run fscale verify --budget <time>`, or `it ran at least 1h 33m when it last ran and did not finish`. It is a progress line, so it shows where progress shows (a terminal, or `--progress`), and never in the summary or the JSON. A step that comes from the cache is not announced: it does not run.

### `--budget <time>`

A step whose kept time is longer than the budget is not started. It is reported as `NOT RUN`, with the budget and the kept time.

- **A flag only.** No config setting: a budget in the config would make every run of the repo partial without anyone asking for it.
- **The time format** is the config's (`30m`, `90s`, `1h 30m`, or a number of seconds). The text parser of `parse_limit` was taken out into `config::time_text_seconds` and is used by both. Zero, and anything else, is a usage error that shows the forms.
- **Its own state.** The step status is `over-budget` in JSON and `NOT RUN` in the summary. It is not `skipped` (the step has a command, and no config key would fix it), not `not-affected`, and not `not-run` (an earlier step failed). It does not stop the steps after it in its project.
- **Never OK.** The run's result is `partial`, the result ADR 0020 made for a run that verified less than the whole. The headline is built where the skipped one is, and counts the two apart: `verify: PARTIAL - 2 of 4 steps ran, 1 skipped, 1 not run (over the --budget of 30m)`. Beside a failure the headline adds `; 1 step not run (...)`. With a base to compare with, a run whose only failures are `pre-existing` or `flaky` would read `OK - no new failures`; with a step left out it is `partial` instead, and the headline still names those failures.
- **Exit code 0**, as for a partial run with skipped steps.
- **`verify.failOnSkipped` covers it.** With the setting on, a run that left a step out fails (exit 1, `verify: FAILED - 1 of 3 steps not run (over the --budget of 30m), and verify.failOnSkipped is on`). The setting says "in this repo a run that verified less than everything is not a pass"; the safe reading is that a budget does not get around it. It was also the simplest: one more condition where the setting is read. The cost: in such a repo `--budget` is only good for seeing the quick steps' results, not for a green run.
- **Nothing is kept about it.** Only a step with status `passed` is stored in the step cache and the shared cache, and only `passed`, `failed` and `timed-out` steps get a base result and a label (`blame::head_outcome`), so no code had to change for either; tests pin both. Its kept time is not touched, since nothing ran.
- **A step with no kept time runs.** So does a step whose time is exactly the budget. A step whose pass is in the step cache is taken from the cache, not left out: the budget check sits after the cache lookup.
- **JSON, schema version 7.** A step gains `lastKnownMs` and `lastKnownFinished`; the report gains `budgetMs` and `notRun` (each with `step`, `project`, `reason`, `lastKnownMs`, `lastKnownFinished`). `notRun` is always there, as `skipped` is, and its length is the count. The version goes up because a step status was added and a field became required: that is what this repo has bumped for every time (version 2 "new enum values and new required fields could break a strict consumer", versions 3, 4 and 5 each for a new status), while optional fields alone never did (ADR 0017, ADR 0036).

### `--budget` is refused in CI and in the stop hook

Both are places where a run stands for the whole repo, and where exit code 0 is read as "it passed".

- **CI** is `GITHUB_ACTIONS=true` (what the shared cache goes by, ADR 0041) or a `CI` variable that is set and not empty (what verify's own commands go by, ADR 0024). Verify exits with 2 before anything runs, the dry run included, and says to run without the flag there and to bound a slow step with its time limit.
- **The stop hook** (`fscale hooks run`, the hook mode of verify) refuses the flag with a message that says to remove it from the hook command. Its exit code is 1, not 2, and verify does not run: Claude Code and Codex read 2 from a stop hook as "do not stop", and the rule that a hook's own errors never trap an agent (ADR 0014) is kept. An agent that can edit its hook command could already free itself with any bad flag; this adds no new way.

Tests in this repo run under verify, which sets `CI=1` for its commands, and in GitHub Actions: every test of a laptop run removes `CI` and `GITHUB_ACTIONS`.

### The heartbeat slows down after ten minutes

A step gets a heartbeat every minute for its first ten minutes, then every five. `progress::next_heartbeat(at, every)` is the pure rule the watch loop uses, tested with no clock. An interval set with `FSCALE_HEARTBEAT_MS` is kept for the first ten minutes; after that the gap is five minutes or the interval, whichever is longer.

### The time left is worked out from when the percent was printed

The swing was reproduced first, with the 0.5.0 rule and a realistic sequence: pytest prints the percent when a test file is done, and one slow file held it at 8% from 10m 27s to 16m 42s. The rule `time so far * (100 - percent) / percent` gives 2h 00m, then 3h 12m, then, when 14% shows at 16m 48s, 1h 43m: the owner's three numbers. The percent was not wrong; the time kept running while the percent stood still.

The fix: the watch loop notes how long the step had run when each percent first showed. The whole run is worked out once, at that moment, when the percent is exact, and the time left is that, less the time so far. The same sequence gives 2h 00m, 1h 53m, 1h 43m. When a percent has stood still for longer than the whole time worked out from it, no time is given, only the percent.

This was the simplest of the options. Calling the estimate "rough" until the percent has moved three times would not have helped: in the sequence above the swing comes after the percent has moved more often than that. A moving average needs a window and a history. The words of the line are unchanged.

## Consequences

- A repo gets a new file, `.firescale/step-times.json`. It is ignored by git with the rest of the folder, is never part of a cache key, and can be deleted at any time.
- The first run on a machine knows no times: nothing is announced and `--budget` runs everything. A step whose command changes starts again with no time.
- A step that fails quickly keeps its old, longer time. If a step really got quicker, the warning stays until it passes once.
- `verify --json` consumers must accept schema version 7. The stop hook is the only consumer in this repo, and it reads `ok` and the steps' statuses it already knew.
- A step stopped at a time limit now leaves a time, so a suite that always times out at 60 minutes is announced as "ran at least 1h 00m ... and did not finish" and is left out by a budget below that.
- After ten minutes a killed verify's record is up to five minutes short. Records written at a clean stop are exact.
- One existing end-to-end expectation changed: a step that printed `[ 25%]` and then nothing for its whole idle limit used to say "25% done, about 6s left at this pace", three times the time it had sat silent. It now says "25% done".
- `--budget` with `--dry-run` is accepted and changes nothing in the plan that is printed.

## Not done

- No config setting for the budget, by the owner's brief.
- No estimate across test sessions: a command that runs several sessions prints a percent that starts again, and the time left follows the session it is in.
- The other stores under `.firescale` that ADR 0056 found writing through a linked folder are as they were.
