# 0083 — The stop hook's time limit

Status: accepted (2026-11-18, Track B run). Amends ADR 0014 and ADR 0027 (the hook files) and ADR 0064 (doctor's hook checks). Verification idea 4, first step.

## Context

Claude Code ends a command hook that runs past its `timeout`, in seconds; the default for a command hook is 600, and no maximum is documented ("`timeout` | no | Seconds before canceling"; "Defaults: 600 for `command`, `http`, and `mcp_tool`"; code.claude.com/docs/en/hooks, read through a subagent on 2026-11-18). A cancelled hook's output is discarded, so a stop hook that is ended gives no decision and the agent stops without verify having finished, silently. `hooks install` wrote `{ "type": "command", "command" }` and nothing else, and the README's own example suite takes 1h 36m.

## Decision

- **`fscale hooks install` writes `timeout` on the Claude Code entry**: from `--timeout <time>` (`30m`, `1h 30m`, or seconds), else from `.firescale/step-times.json` (ADR 0057).
- **From the kept times**: twice the sum of the kept times of every command a full `fscale verify` plans (the root's and every project's), plus `verify.baseBudget` (5 minutes by default; with `false`, the sum again), rounded up to whole minutes, and never less than 600. Twice, because a `new` failure runs again (ADR 0039) and a step whose pass is not cached runs in full; the base budget, because a failure may start the base run. A step with no kept time counts as nothing, and the line says how many there were. With no kept time at all, 600 is written out, and the line says to run `hooks install` again after a verify.
- **One line says the limit and why**, after the list of files: `Claude Code's stop hook may run for 45m (`timeout` in .claude/settings.json): twice the 20m the planned steps took when they last ran, and 5m for the base run.`
- **An entry firescale's hook already has gets `timeout` as a new line** when it has none, and the list says `timeout added`. `jsonedit::insert_merge` learned to add a key inside an item of a list, which is where the entry is; every line a person wrote stays (ADR 0027). An entry written on one line cannot take a new line: the output says what to add by hand, and install does not fail for it.
- **A `timeout` already in the file is never changed**: the line says what firescale would write and to edit the value. No value a person wrote is rewritten (ADR 0027).
- **`--timeout` with only Codex or Cursor is refused** (exit 2): neither has a field firescale writes. With `--all` it goes on the Claude Code entry only.
- **`fscale doctor` warns** (`Agent hooks`, subject `.claude/settings.json`) when the kept times of every planned step add up to more than the hook's `timeout`, or 600 when it sets none. The warning says what happens (the agent stops without verify having finished) and what to do: work on a branch, where the hook runs only the projects a change reaches as `fscale verify --affected` does; give the hook a smaller scope (a faster command for the slow steps under `verify`); or raise the limit (edit `timeout`, or `hooks install --agent claude` when the entry has none).

## Consequences

- **"The steps the hook would run" is every planned step.** On a branch with a base, the hook runs only the projects the change reaches, so the sum is the worst case: a change that reaches everything, or no base. Doctor cannot know the next change. A cached pass takes no time, which the sum does not count either. So doctor may warn about a hook that would finish; it never stays quiet about one whose full run is known to be too long.
- The sum is of the last run of each step, not of the slowest. A step whose time grew since is under-counted until it runs.
- Units run one after another unless `--jobs` is given; the hook passes none, so a sum is right for it.
- **Every Claude Code hook file `hooks install` writes now has a `timeout`**, 600 where nothing is known. The snapshots of the hook files and the help text change. A repo that runs `hooks install` again gets one new line in `.claude/settings.json`; that file is an agent hook file, and a change inside its hook entries reaches only the units whose key holds the root (ADR 0064).
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
- Changed on purpose: four snapshots (the Claude hook file has `"timeout": 600`; the install output has the limit line), the help text, and one older test whose pre-0.5.1 pin now gains its `timeout` (it also checks that the command is kept).
