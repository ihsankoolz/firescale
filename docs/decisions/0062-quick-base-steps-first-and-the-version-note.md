# 0062 — The base run goes quickest first, a failing step names its tests, and a new version is named

Status: accepted (2026-11-16, for 0.5.2). Refines ADR 0037 (the order of the base run and its budget), ADR 0057 (who reads the kept step times), ADR 0043 and ADR 0058 (the note about the step cache's keys), and ADR 0038 (what the summary says about failing tests). Verify's JSON schema is unchanged (version 7); after the review, three of its descriptions were corrected (section 2).

## Context

The owner ran 0.5.1 on the course project on Windows (`docs/reviews/cs464-0.5.1.md`). A full verify took 1h 41m and failed at two steps: the ledger service's tests (3 failing timing tests, 1h 36m) and the frontend's tests (3 failures, 1m 09s). Both fail on `origin/main` too. Neither was labelled `pre-existing`:

```
[verify] base: comparing with bc43f27 (origin/main): 2 steps to run in a worktree of the base commit
[verify] base: test in backend/ledger_service: started: .venv\Scripts\python.exe -m pytest
[verify] base: test in backend/ledger_service: OVER BUDGET on the base after 4m 56s
note: test in backend/ledger_service: the base run ran out of its time budget (5m, verify.baseBudget) before it finished, so this failure blocks as it would without a base.
note: test in frontend: the base run ran out of its time budget (5m, verify.baseBudget) before it finished, so this failure blocks as it would without a base.
```

The base run took its steps in the report's order, and `backend/` comes before `frontend`. verify already knew the ledger step's time: `.firescale/step-times.json` held 5,763,707 ms (ADR 0057), written a minute before the base run started.

The same run printed, after an upgrade from 0.5.0 with an unchanged config: `note: A setting of the firescale config that every step's result can depend on changed since the last run, ...`. And the frontend's summary showed only its last lines, so it did not say whether a fourth test had failed.

## Decision

### 1. The base run goes quickest first

The steps to run in the base worktree are sorted by `blame::run_order`:

- **Failing steps first, then passing ones**, as ADR 0037 had it. Passing steps run on the base only with `verify.labelFixed`, and the labels are for failures.
- **Within each, the quickest first**, by the time `blame::base_time` gives (section 2, "Which time counts"). A step with **no known time goes after those with one**, in the report's order. The sort is stable.

`compare` sorts with `blame::sort_base_run`, after the head's steps ran. The kept time comes from the same `StepTimes` the head run writes (`RunOptions::times`). The base run itself still keeps no time (ADR 0057).

### 2. A base step known to take longer than what is left of the budget is not started

Before a base step starts, `blame::longer_than_left` compares its known time with what is left of `verify.baseBudget` at that moment. When that time is longer, the step is not started:

- Its base status is `over-budget`, its source `ran` (a run was due and was not made), and it has no label: its failure blocks as it would without a base, as ADR 0037 has it for a base run that runs out.
- **Its note says why, with the time and where it came from:** `test in backend/ledger_service: it was not started on the base commit bc43f27, because it is known to take longer than what was left of the base run's time budget: its kept time is 1h 36m (.firescale/step-times.json), and 4m 58s of 5m (verify.baseBudget) was left, so this failure blocks as it would without a base.` This run's head time reads `it took 1h 36m on the head in this run`. A kept time from a run that did not finish reads `its kept time is at least 1h 33m, from a run that did not finish (.firescale/step-times.json)`: such a step takes at least that long, so it counts the same way.
- The progress line is `base: test in backend/ledger_service: not started: its kept time is 1h 36m (.firescale/step-times.json), more than the 4m 58s left of the budget (5m, verify.baseBudget)`.
- **When every failing step is known to take longer than the whole budget left** before the worktree is made, none would start: no worktree is made, `verify.baseInstall` does not run, and the `comparing with ... to run in a worktree` line is not printed. Each step gets its not-started note and progress line. Passing steps (`verify.labelFixed`) run on the base only beside a failure, so they are not run either. A failing `check` still makes the worktree, since the check has no known time.
- **JSON:** the schema is unchanged. After the review, the descriptions of a step's `base.status` (`over-budget`), `base.source` (`ran`) and the report's `base.overBudget` were corrected to cover a step that was not started. No existing field tells "not started" from "ran out" without the note (both are `over-budget` from source `ran`, and both have a note), and no field was added.
- The report's `base.overBudget` stays false for it: the budget did not run out. The summary's `Compared with the base commit ...` line counts only the steps that ran. The step's own note is what says it was left out.
- A step with no kept time, a step whose kept time is not more than what is left, and any step of a base run with `"baseBudget": false` start as before. A step that starts and then runs past the budget is stopped as before, with the old note.

**Which time counts** (changed after the review). `blame::base_time` gives, for the head step's project, name and command:

- **this run's head time, when the head step ran to its end in this run**: it passed or failed, and did not come from the cache. It is the freshest measure of the same command. The note says `it took ... on the head in this run`;
- **else its kept time** in `.firescale/step-times.json` (the head step timed out, was stopped, or came from the cache). The note says `its kept time is ...`.

The first version used the kept time alone. The review showed why that is wrong: a kept time only rises after a failure or a stop (ADR 0057), so one stale "did not finish, 6m" record would keep a pre-existing failure out of the base run on every later run, with a note saying `it took 6m when it last ran` though the head ran it in 30 s this run.

The head's time stands for the base run because:

- the base run runs the **same command** (`StepReport::command`, the head's), in a worktree with the head's dependencies linked in (ADR 0037);
- the base commit differs from the head **only by the change being judged**. A step's time is mostly the size of its suite and the machine. A change can make it quicker or slower, but a step that took 1h 36m on the head will not fit in 5 minutes on the base;
- there is no other time to use. The base run keeps no time (ADR 0057), and a base result kept in `.firescale/base/` is found before any run is planned, so a step that runs on the base has none.

The cost: when the change itself makes a step much slower or quicker, the order and the "not started" choice follow the head's time. A step that failed quickly on the head in this run (an import error) is taken as quick, so it is started on the base and may run out the budget there, as before this ADR. When the guess is wrong one way, a step is started and runs out the budget (as before this ADR); the other way, a step that would have fit is not started and blocks (it would have blocked without a base too). Neither way can label a failure that is new.

Rejected: the kept time alone (it can be stale, as above); keeping a separate time per base commit (nothing would be known for a new base, which is every run after a merge).

### 3. A failing step names its failing tests

When a step that failed or timed out has per-test results (pytest's JUnit report through `PYTEST_ADDOPTS`, Vitest, Jest, `cargo test`; `pertest.rs`), the summary prints, before its last lines:

```
Failing tests of test in backend/ledger_service (3):
  unit_test/service/test_settlement.py::test_a_thousand_winners_and_a_thousand_losers_settle_within_five_seconds
  unit_test/service/test_void.py::test_a_void_finishes_within_five_seconds[1000-positions]
  unit_test/service/test_void.py::test_a_void_finishes_within_five_seconds[10000-trades]
```

- Each test as its runner names it (the same `id` as in `failedTests`), in that order (sorted by name). After the review, a name is printed as safely as an output line: `verify::hide_secrets` hides what could be a secret (a pytest parameter can be `postgresql://app:s3cret@db/x`), a line break is written `\n` so one name stays on one line, and a name is cut at 300 characters with `...`.
- **A count that may not be the total says so.** When the step timed out, or its results are marked partial (it ran its runner more than once), the header is `Failing tests of test (3 found; the results do not cover the whole run):`.
- **At most 20**, then one line `and N more;` that points to `fscale verify --json`, whose `failedTests` lists every one.
- **The last lines stay**, after the list. The list says which tests failed and how many; the last lines say how (the assertion, a timeout). Taking them out would lose the first error of a runner whose report has no message.
- **Not printed** when the step has no per-test results (the summary is as before), for a `pre-existing` or `flaky` step (it gets no last lines either), for a step that needs setup (its failing tests are not failures in the code), and for a `new` step whose `New failing tests:` list was printed: that list names the new ones and counts the others, and a second list would repeat them. After the review, that list has the same cap of 20 with `and N more;`, and prints names the same safe way.
- **JSON is unchanged.** Each step's failing tests were already in `failedTests` (ADR 0038), with their labels when there is a base. No schema change.

### 4. A new version is named, not taken for a config change

**Why a version change read as a config change.** The report guessed right. 0.5.0 kept in `.firescale/step-cache-config.digest` a blake3 digest of the config file's bytes (ADR 0043). 0.5.1 kept a digest of the settings every step shares (ADR 0058), made another way. The first 0.5.1 run found a digest it did not make and, since passes were kept (0.5.0's, which no 0.5.1 key can find), said a setting changed. Had the recipe stayed the same, the next upgrade would have said nothing at all, though the version is part of every step's key and every step runs again.

**The fix.** The file now holds the version too:

```
version 0.5.2
settings <digest>
```

On the first run with kept passes after the file changed:

- **another version wrote it:** `firescale is 0.5.2 now, and the last run used 0.5.1. The version is part of every step's cache key, so no pass kept before applies: every step runs again this time.` The settings are not compared: another version may make the digest another way, and every step runs again anyway;
- **this version wrote it, and the digest differs:** the config note, as before;
- **a file written before 0.5.2** holds the digest alone, 64 hex digits. It is read as another version's. It is named `0.5.1` when it is 0.5.1's digest of the same settings (0.5.1 made it exactly as 0.5.2 does, and 0.5.0's digest of the file's bytes cannot match), and `0.5.1 or older` otherwise (0.5.0, or 0.5.1 with another config).
- **a file that holds no record** (empty, a `version` line with no version, or text in neither format) makes no claim about the last run: no note. Added after the review; before it, an empty file was read as `0.5.1 or older`.
- **a file that is a link, or under a `.firescale` that is one,** is not read (ADR 0060, ADR 0061), as it was already not written. Before the review it was read, and only the write was skipped.

No flag was added for tests. The Rust test sets the version through `CacheSettings::version`, which the cache already takes from its caller.

## Tests

Each failed first, as below, then passed after the fix.

- Items 1 and 2, end to end: `blame.test.ts` "runs the quick base step first, and does not start one known to take longer than the budget (ADR 0062)". Two projects, `backend/ledger_service` and `frontend`, each with a `node test.js` that fails the same way on the head and the base; the ledger's takes 30 s and runs into its own 3 s step limit on the head (since the review, so its kept time decides); `.firescale/step-times.json` holds the ledger step's 5,763,707 ms; `verify.baseBudget` is small. Before the fix both notes were the report's: `the base run ran out of its time budget (10s, verify.baseBudget) before it finished` (the frontend's base status was `over-budget`, expected `failed`). After: the frontend step is `pre-existing`, the ledger step is `over-budget` with the new note, and only the frontend ran on the base.
- Items 1 and 2, pure: `blame::tests::the_base_run_goes_quickest_first_and_leaves_out_a_step_known_to_take_longer_than_what_is_left` (the order, and the decision for a kept time longer than the whole budget, longer than what is left, exactly what is left, not finished, none, and no budget).
- Item 3: `verify::tests::a_failing_test_step_lists_its_failing_tests_by_name_up_to_twenty`, with the report's three pytest names, 23 Vitest-style names, and a step with no per-test results.
- Item 4: `stepcache::tests::a_new_version_is_named_in_the_note_and_never_read_as_a_config_change`: a digest file as 0.5.0 wrote it (before the fix: the report's config note), as 0.5.1 wrote it (before the fix: no note at all), a file of this format from an older version, and a real config change on one version (still the config note).

### After the review

Each failed first, then passed after the fix:

- `blame.test.ts` "runs the quick base step first, ...": now also asserts that `[verify] base: test in frontend: started` comes before `[verify] base: test in backend/ledger_service: not started` in stderr. The ledger step's head now runs into its own 3 s step limit, so its kept time decides, and the notes read `its kept time is 1h 36m`. Before the fix it failed on the frontend's base status (`over-budget`, expected `failed`). With the fix in place but the sort taken out of `compare` (the `sort_base_run` call commented out), it failed on the new assertion: `expected 733 to be less than 555`. The Rust test above now also sorts six steps through `sort_base_run`, as `compare` does.
- `blame.test.ts` "starts a base step by this run's head time, not a stale kept time (ADR 0062)": a kept time of 6m, not finished, and a quick head run. Before the fix: `expected 1 to be +0` (the exit code: the failure blocked, since the step was not started on the base).
- `blame.test.ts` "makes no worktree and runs no install when every base step is known to take longer than the budget (ADR 0062)". Before the fix it failed on the note, which said `it took 1h 36m when it last ran` where `its kept time is 1h 36m` is now expected; the worktree and install checks come first in the committed test.
- `verify::tests::failing_test_names_hide_secrets_and_stay_on_one_short_line`, `a_failing_test_list_that_may_not_be_whole_says_so`, `the_new_failing_tests_list_has_the_same_cap_and_hides_secrets`: before the fix the summary printed `tests/test_db.py::test_connects[postgresql://app:s3cret@db/x]` in both lists, and `Failing tests of test (3):` for results that were not whole.
- `stepcache::tests::an_empty_or_broken_digest_file_names_no_version`: before the fix, for an empty file, `left: Some("firescale is 0.5.2 now, and the last run used 0.5.1 or older. ...")`, `right: None`. `the_config_digest_is_never_read_through_a_symlink` is unix only and was not run here (Windows).
- `verify.test.ts` "names each failing test of a test step before its last lines (ADR 0062)": a snapshot of a whole summary with the list (new; the list was already there).

## Consequences

- On the course project the frontend's base run starts first and fits in the budget; the ledger step is not started, and its note says it takes 1h 36m against 5 minutes. To have it judged on the base, set `verify.baseBudget` above its time, or `false`.
- The base run reads `.firescale/step-times.json` once per planned base step. Where that file cannot be read, or `.firescale` is a link, no step has a kept time: the order is the report's and every step is started, as before.
- The first 0.5.2 run after 0.5.1 says `firescale is 0.5.2 now, and the last run used 0.5.1` and nothing about the config. After that the file names its version and every later upgrade names both.
- A summary of a failing test step with per-test results is longer by one line per failing test, up to 21 lines.
- One existing test changed its budget: `blame.test.ts` "falls back to blocking when the base run runs out of its budget, and says so" went from `2s` to `15s`, its step's sleep on the base from 20 s to 60 s, and its limit from 30 s to 60 s. Its step's time is its quick failure on the head (about a second with Node's start); with a small budget, a worktree that took most of it would leave less than that, and the step would be not started, with the new note, instead of running out the budget. The review saw this with `4s` (124 ms of 4 s left). What it proves is unchanged.

## Not done

- Within one project, lint, typecheck and test run in the same base worktree, and the sort may put them in another order than the head's (a quick test before a slow lint). When no step leans on another having run first, the labels are the same and only the order of the progress lines differs. A step that does (a test that reads what typecheck built in the tree) can get another base result than in the head's order; it is not handled.

- A control run in a worktree of the head (ADR 0040, `fixed`) and the rerun of a new failure (flakiness) do not use the kept times. Both run after the base steps and within what is left of the budget, as before.
- The order is by the kept time alone. It does not weigh how many tests a step has failing, or how likely the base is to fail the same way.
