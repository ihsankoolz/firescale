# 0083 — The stop hook's time limit

Status: accepted (2026-11-18, Track B run). Amends ADR 0014 and ADR 0027 (the hook files) and ADR 0064 (doctor's hook checks). Verification idea 4, first step.

## Context

Claude Code ends a command hook that runs past its `timeout`, in seconds; the default for a command hook is 600, and no maximum is documented ("`timeout` | no | Seconds before canceling"; "Defaults: 600 for `command`, `http`, and `mcp_tool`"; code.claude.com/docs/en/hooks, read through a subagent on 2026-11-18). A cancelled hook's output is discarded, so a stop hook that is ended gives no decision and the agent stops without verify having finished, silently. `hooks install` wrote `{ "type": "command", "command" }` and nothing else, and the README's own example suite takes 1h 36m.

## Decision

- **`fscale hooks install` writes `timeout` on the Claude Code entry**: from `--timeout <time>` (`30m`, `1h 30m`, or seconds; refused by every other command), else from `.firescale/step-times.json` (ADR 0057).
- **From the kept times**: twice the sum of the kept times of every command a full `fscale verify` plans (the root's and every project's), plus `verify.baseBudget` (5 minutes by default; with `false`, the sum again), rounded up to whole minutes, never less than 600 and never more than 2147483 (2^31 - 1 milliseconds, the longest a JavaScript timer waits; how Claude Code times a hook is not documented, and a longer value could fire at once in such a timer; `--timeout` above it is refused). Twice, because a `new` failure runs again (ADR 0039) and a step whose pass is not cached runs in full; the base budget, because a failure may start the base run. A step with no kept time counts as nothing, and the line says how many there were. With no kept time at all, **no `timeout` is written**: Claude Code's default applies, and the line says to run `hooks install` again after a verify, which then adds one. (Writing 600 there made that advice useless, since a value in the file is never changed: the review of this pull request.)
- **One line says the limit and why**, after the list of files: `Claude Code's stop hook may run for 45m (`timeout` in .claude/settings.json): twice the 20m the planned steps took when they last ran, and 5m for the base run.`
- **An entry firescale's hook already has gets `timeout` as a new line** when it has none, and the list says `timeout added`. `jsonedit::insert_merge` learned to add a key inside an item of a list, which is where the entry is; every line a person wrote stays (ADR 0027). An entry that cannot take a new line without changing one already there (written on one line, say) is left alone: the output says what to add by hand, and install does not fail for it.
- **A `timeout` already in the file is never changed**, whatever its value (`3600.0`, `"900"`): the line shows it as the file has it, says what firescale would write, and to edit the value. Doctor reads a number of seconds with a point; any other value counts as Claude Code's default, and the warning says the value is not a number of seconds. No value a person wrote is rewritten (ADR 0027).
- **`--timeout` with only Codex or Cursor is refused** (exit 2): neither has a field firescale writes. With `--all` it goes on the Claude Code entry only.
- **`fscale doctor` warns** (`Agent hooks`, subject `.claude/settings.json`) when the kept times of every planned step add up to more than the hook's `timeout`, or 600 when it sets none. The warning says what happens (the agent stops without verify having finished) and what to do: work on a branch, where the hook runs only the projects a change reaches as `fscale verify --affected` does; give the hook a smaller scope (a faster command for the slow steps under `verify`); or raise the limit (edit `timeout`, or `hooks install --agent claude` when the entry has none).

## Consequences

- **"The steps the hook would run" is every planned step.** On a branch with a base, the hook runs only the projects the change reaches, so the sum is the worst case: a change that reaches everything, or no base. Doctor cannot know the next change. A cached pass takes no time, which the sum does not count either. So doctor may warn about a hook that would finish; it never stays quiet about one whose full run is known to be too long.
- The sum is of the last run of each step, not of the slowest. A step whose time grew since is under-counted until it runs.
- Units run one after another unless `--jobs` is given; the hook passes none, so a sum is right for it.
- **A Claude Code hook file `hooks install` writes has a `timeout` once a step has a kept time.** The help text and the install output's snapshots change; the snapshots of the hook files do not, since their repos have no kept times. A repo that runs `hooks install` again after a verify gets one new line in `.claude/settings.json`; that file is an agent hook file, and a change inside its hook entries reaches only the units whose key holds the root (ADR 0064).
- `fscale sync` (the `agent-ready/verify-hook` fix) writes no `timeout`: it has no step times to read in a new repo, and this step names only `hooks install`.
- Codex and Cursor get no limit and no doctor check: no time limit field was found for either (the research report's reading of their hook pages).
- No test runs a real Claude Code: the field's name, unit and default are from its documentation.

## Later

- `hooks install --background`: an `asyncRewake` entry, so the agent's turn ends and it is woken when verify fails (verification idea 4, the next step). Not built.
- A `timeout` written by `sync`, once a repo has kept times.
- Codex's and Cursor's limits, if their documentation names one.

## Tests

- `packages/cli/src/hooks.test.ts`, "the stop hook's time limit (ADR 0083)", 8 tests, written first: 7 seen failing on `main` in WSL (no `timeout`, `--timeout` unknown, `already installed` where `timeout added` was expected, no doctor warning); the eighth guards that doctor says nothing when the times fit, and passed before and after.
- `hooks::tests::the_hook_limit_is_twice_the_steps_and_the_base_run_in_whole_minutes`, `only_claude_codes_entry_gets_a_timeout_and_a_timeout_in_the_file_is_kept`, and `jsonedit::tests::a_key_goes_into_an_item_of_a_list_where_it_is`.
- Changed on purpose: the install output's snapshots (the limit line) and the help text. One older test, of a pre-0.5.1 pin, also checks that the command is kept.
- From the review, written first and seen failing in WSL: "writes no limit while no step has a kept time, and one when install runs again after a verify" (it was "writes Claude Code's default limit explicitly", the behaviour the review found wrong), "keeps a limit in the file that is not a whole number, and doctor reads a number of seconds with a point", "refuses a limit longer than a timer can wait, and --timeout outside hooks install".

## Not done

- From the review: none of its five findings is left. The fifth (the by-hand message said "written on one line" for every entry that cannot take a new line) is fixed in wording only; no test covers an entry whose brace shares a line with a key.
