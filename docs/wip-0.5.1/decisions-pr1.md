**Pull request 1: slow steps** (branch `pr1-slow-steps`, ADR 0057)

From your run of 0.5.0 on the course project on Windows: a full verify ran over 1h 33m and you stopped it. For each item: a test that failed first, then the fix.

- **1. A slow step is announced, built.** Failing tests first: `warns before a step that took more than 5 minutes last time, also after its files changed` and `keeps the time of a project step under its folder, and warns for it by name` (slow-steps.test.ts: "ENOENT ... .firescale/step-times.json", nothing was kept). Built: verify keeps each step's time in `.firescale/step-times.json`, by project, step and command, and prints `[verify] test in backend/api: slow step: it took 1h 40m when it last ran. To leave out steps this slow, run fscale verify --budget <time>` before it starts a step whose kept time is over 5 minutes. Nothing is read or written through a symbolic link (`nothing_is_read_or_written_through_a_symbolic_link`, steptimes.rs).
- **2. A stopped or timed-out step leaves its time, built.** Failing tests first, in slow-steps.test.ts: `after Ctrl+C, and the next run says it ran at least that long and did not finish`, `when verify itself is killed outright, from what it wrote while the step ran`, and `after it hit its own time limit | its project's time limit | the idle limit` (all five: no record). The two Ctrl+Break tests of verify-run.test.ts, which run on Windows only, now also expect the record. Built: the record is written when the step's command ends, however it ended, and at each heartbeat while it runs. The next run says `it ran at least 1h 33m when it last ran and did not finish`.
- **3. `verify --budget <time>`, built.** Failing tests first: thirteen in slow-steps.test.ts under `verify --budget leaves out the steps that are known to take longer` ("unknown option --budget"), then `a_step_over_the_budget_is_not_run_and_counted_apart_from_a_skipped_one` and `a_run_that_left_a_step_out_never_reads_ok_whatever_else_happened` (verify.rs). Built: a step whose kept time is over the budget is not started and is `NOT RUN`; the headline reads `verify: PARTIAL - 2 of 3 steps ran, 1 not run (over the --budget of 30m)`; JSON has the step status `over-budget`, `lastKnownMs`, `lastKnownFinished`, and the report's `budgetMs` and `notRun`.
- **4. `--budget` is refused in CI and in the stop hook, built.** Failing tests first: `in CI (...): says why, and what to do, and runs nothing` (three cases: the run exited 0) and `in the stop hook: says why, and what to do, and runs nothing` (slow-steps.test.ts), then `a_ci_run_is_known_by_github_actions_or_a_ci_variable_that_is_not_empty` (main.rs).
- **5. The heartbeat slows down, built.** Failing test first: `a_step_gets_a_heartbeat_a_minute_and_after_ten_minutes_one_every_five` (progress.rs: it got minutes 1 to 40, expected 1 to 10, then 15, 20, ...). A pure function, no waiting.
- **6. The time left is steadier, fixed.** Reproduced first: `the_estimate_from_the_time_so_far_swings_while_a_slow_test_file_runs` (activity.rs) gives your three numbers, 2h 00m, 3h 12m, 1h 43m, from one pytest file that holds the percent at 8% from 10m 27s to 16m 42s. Failing test: `the_estimate_is_steady_while_a_slow_test_file_runs` (got those three, expected 2h 00m, 1h 53m, 1h 43m). Fix: the time left is worked out from the moment the percent was printed, less the time since.

**Decided on your behalf** (ADR 0057):

- **Where the time is kept.** One file, `.firescale/step-times.json`, not the step cache (keyed by content, passes only) and not the run records (temp folder, removed at the end of a run).
- **What changes a kept time.** A pass sets it. A failure, a stop or a timeout only raises it. So a step that fails in 3 seconds does not make a step of 1h 40m look quick, and stopping a slow step early does not either. The cost: a step that really got quicker keeps its old time until it passes once.
- **The record is also written at each heartbeat.** A verify that is killed with no chance to run its own code (a second Ctrl+C, Task Manager, a closed terminal) still leaves the step's time, to its last heartbeat.
- **A step left out is its own state**: `over-budget` in JSON, `NOT RUN` in the summary. Not `skipped`, not `not-affected`, not the `not-run` of a step after a failure. The run's result is `partial`, and its exit code is 0, as for skipped steps.
- **`verify.failOnSkipped` also fails a run that left a step out.** The setting says a run that verified less than everything is not a pass; a budget does not get around it. In such a repo `--budget` shows the quick steps' results but never a green run. Say if you want the other reading.
- **With a base, a run with a step left out is `PARTIAL`, never `OK - no new failures`**, and the headline still names the pre-existing and flaky steps.
- **Verify's JSON is schema version 7.** A new step status and a new required field (`notRun`) have always meant a new version here (versions 2 to 5); optional fields alone never did.
- **A pass in the step cache wins over the budget**: the step is taken from the cache, not left out. A step with no kept time runs. A step whose time is exactly the budget runs.
- **CI is `GITHUB_ACTIONS=true`, or a `CI` variable that is set and not empty.** `CI=false` therefore counts as CI, as it already does for verify's commands.
- **The stop hook's refusal exits with 1, not 2**, and verify does not run, so the agent is not held back. That is the hook's standing rule for its own errors (ADR 0014). An agent that edits its hook command could already do this with any bad flag. Say if you would rather have the hook ignore the flag and run a full verify.
- **The heartbeat after ten minutes** comes every five minutes, or at the interval of `FSCALE_HEARTBEAT_MS` if that is longer.
- **The time left: one rule, no "rough" label.** When a percent has stood still for longer than the whole time worked out from it, only the percent is shown.
- **`--budget` takes the config's time format**; its text parser was moved out of `parse_limit` into `config::time_text_seconds`, unchanged, and both use it.

**Tests changed, none weakened:**

- Four assertions and two snapshots show schema version 7 and `"notRun": []` (blame.test.ts, python-env.test.ts, verify-run.test.ts, verify.rs, verify.test.ts.snap). The help snapshot shows `--budget`.
- `stops a step that prints nothing new for its idle limit, and says so` (verify-run.test.ts) expected `25% done, about ... left at this pace` for a step that printed 25% and then nothing. That figure was three times the time it had sat silent. It now expects `25% done`.

**Known limits:**

- The first run on a machine knows no times. A step whose command changes starts again with none.
- After ten minutes a killed verify's record is up to five minutes short. With `FSCALE_HEARTBEAT_MS=0` nothing is written while a step runs, only at its end.
- `--budget` with `--dry-run` changes nothing in the plan that is printed.
- A command that runs several test sessions prints a percent that starts again; the time left follows the session it is in.
- **Windows.** The stop and kill paths were read, not run: this machine is Linux. The two Ctrl+Break tests now check the record on Windows in CI, and the kill test runs on every platform. Ctrl+C in a real console and closing the console window have no test; they go through the same code as Ctrl+Break.

**Found while fixing, not fixed:**

- `fscale --agent claude hooks run` (options before the command) is not seen as the stop hook by `is_stop_hook`, so a usage error there exits with 2, which Claude Code and Codex read as "do not stop". `hooks install` never writes that order.
- SPEC.md §8 does not yet mention the kept times or `--budget`. I did not edit it.
- AGENTS.md's command list does not name `--budget`. No listed command changed.

**Not reproduced:** nothing.
