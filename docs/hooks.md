# Agent hooks

`fscale hooks install` makes a coding agent run `fscale verify` when it is about to stop, and sends it back to work while verify fails.

```
fscale hooks install --all                # Claude Code, Codex and Cursor
fscale hooks install --agent claude       # one agent
fscale hooks install --all --bin fscale   # a command of your own, written as it is
```

| Agent       | File written            | Hook                                                       |
| ----------- | ----------------------- | ---------------------------------------------------------- |
| Claude Code | `.claude/settings.json` | `hooks.Stop[].hooks[]`: `{ "type": "command", "command" }` |
| Codex       | `.codex/hooks.json`     | The same shape as Claude Code.                             |
| Cursor      | `.cursor/hooks.json`    | `version: 1`, `hooks.stop[]`: `{ "command" }`.             |

The Claude Code format was checked against its documentation and the Codex format against its source on 2026-11-11. Cursor's documentation could not be reached from the build environment; on 2026-11-12 its format was checked against Cursor's own example repositories on GitHub (`cursor/cookbook`, `cursor/plugins`), and it matches (ADR 0027). So `hooks install` no longer prints a note that the Cursor format is unchecked. `fscale sync` with the `agent-ready` pack writes the same hooks (see [init.md](init.md)).

Installing never rewrites a file that has something in it. The hook goes in as new lines, in the file's own indentation and line endings, first in its list, and every line you wrote stays as it was (ADR 0027). Running it again changes nothing. A hook counts as installed only when the command is under the agent's stop event. A file that is not valid JSON is left alone and reported. So is a file the hook cannot go into as new lines: one written on one line, or with the object or list the hook goes into written on one line (`"hooks": {}`) or set to `null`. The error then shows what to add by hand. With `--all`, a refused file does not stop the others: every agent's file is tried, the list printed at the end says `installed`, `already installed` or `refused` for each, each refusal's reason is on stderr, and the command exits with 2 when any file was refused (ADR 0060). Commit the files so every teammate's agent gets the hook.

## `--bin`: how the hook starts fscale

The hook command is `<bin> hooks run --agent <name>`. A command you pass with `--bin` is written as it is. Without `--bin`, the repo decides, never the PATH of the machine you install from, so every teammate's agent runs the same command (ADR 0064):

- the repo's own `fscale` package script, when the root `package.json` has one: `pnpm --silent fscale`, `yarn --silent fscale`, `bun run --silent fscale` or `npm run --silent fscale --`;
- otherwise `npx firescale`, which runs the firescale of the repo's `node_modules` when there is one, and otherwise one that npm downloads into its cache the first time. Not `npx fscale`: that npm name belongs to another package.

This is the command the `agent-ready/verify-hook` fix and the AGENTS.md block use too. For the fastest hook, point `--bin` at the native binary (see [install.md](install.md)), or at `fscale` where every machine has it on the PATH.

### The check after installing

After writing the files, `hooks install` starts each command the hook files hold, as `<command> --version`, the way an agent starts it: in the repo root, through `cmd /D /S /C` on Windows and `sh -c` elsewhere, with stdin closed, for up to 120 seconds. Before it does, it says on stderr what it runs, and that npx may download firescale from the npm registry, or that a package script of the repo runs.

```
installed         .claude/settings.json
installed         .codex/hooks.json
installed         .cursor/hooks.json

Each agent now runs `npx firescale hooks run --agent <name>` before it stops, and keeps working while `fscale verify` fails.
The hook starts on this machine: `npx firescale --version` answered 0.5.2.
```

When the answer is another version, the line says which firescale the agent will run. When the command does not start, exits with an error, or answers something that is not a version, the line says `The hook cannot start on this machine:` with the command and what it printed, the next lines say what to do, and the command exits with 1 (2 when a file was refused). The files are still written: the command may start on a teammate's machine.

The check is close to what an agent does, not the same: Claude Code runs the command through Git Bash or PowerShell on Windows, Codex through the session's shell, and an agent started from a desktop app may have another PATH than your terminal.

## When the hook cannot start

None of the three agents keeps an agent working when its stop hook cannot start: each takes it as a failed hook and lets the agent stop, so verify has not run. That is why `hooks install` checks the command. What each agent does, from its documentation or source, read on 2026-11-16 (ADR 0064); "unverified" means the source does not say:

|                                        | Claude Code                                                                                                                                         | Codex                                                                                                                                                                                                                 | Cursor                                                                                                            |
| -------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------- |
| Runs the command through               | `sh -c` on macOS and Linux; Git Bash on Windows, or PowerShell when Git Bash is not installed (a `shell` field can choose `bash` or `powershell`)   | the session's shell; without one, `$SHELL -lc` (default `/bin/sh`), or `%COMSPEC% /C` (default `cmd.exe`) on Windows. `commandWindows` replaces the command on Windows. Which shell a Windows session has: unverified | PowerShell, in the Windows desktop app. macOS, Linux and the CLI: unverified                                      |
| Exit 2 on stop                         | keeps the agent working; stderr goes to it                                                                                                          | keeps the agent working when stderr is not empty; with empty stderr it is a failed hook                                                                                                                               | blocks "the action" in general; for stop, unverified. firescale answers Cursor with exit 0 and `followup_message` |
| Any other non-zero exit                | a non-blocking error: the agent stops                                                                                                               | a failed hook run, `hook exited with code N`: the agent stops                                                                                                                                                         | "Hook failed, action proceeds (fail-open by default)": the agent stops                                            |
| A command that cannot start            | a non-blocking error; in shell form the shell exits "with a code like 127"                                                                          | the shell's own exit code (127 from `sh`, 9009 from `cmd`) or a spawn error: a failed hook run                                                                                                                        | not stated; unverified (a crash is fail-open)                                                                     |
| What the user sees                     | a hook error notice in the transcript, `Failed with non-blocking status code: <stderr>` (shown in the docs for another event; for stop, unverified) | the failed hook run with its message (how the TUI shows it: unverified)                                                                                                                                               | the failure is logged; the Hooks output channel shows it. Anything in the chat: unverified                        |
| A setting to block when the hook fails | `"onFailure": "block"`, but it "has no effect" on Stop hooks: "Claude can't repair a hook that won't run"                                           | none found                                                                                                                                                                                                            | `failClosed: true` for "crash, timeout, non-zero exit code, no output"; whether it applies to stop: unverified    |

Claude Code also treats a stop hook that exits 2 with nothing on stdout and a stderr that says a file is missing ("No such file or directory") as a non-blocking error.

firescale does not wrap the command to turn a failed start into a block: one file serves every teammate's OS and shell, npx's own errors exit 1 like fscale's deliberate release, and a wrapper's block would have no loop guard. ADR 0064 gives the reasons. If an agent seems to stop without verify having run, run the hook command by hand from the repo root, for example `npx firescale --version`.

## What the hook does

`fscale hooks run --agent <name>` reads the agent's JSON from stdin, runs `fscale verify`, and answers in the way that agent expects:

| Outcome                          | Claude Code and Codex                         | Cursor                                |
| -------------------------------- | --------------------------------------------- | ------------------------------------- |
| verify passes                    | exit 0, no output                             | exit 0, no output                     |
| verify fails                     | exit 2, the verify summary on stderr          | exit 0, `{"followup_message": "..."}` |
| verify fails, the cap is reached | exit 0, a one-line note on stderr             | exit 0, `{}`                          |
| verify cannot run at all         | exit 1, the reason on stderr (does not block) | the same                              |

## The loop guard

An agent that cannot fix a failure must not be held forever. The hook counts how many times in a row it has sent one session back, in `.firescale/hooks/<agent>-<session>.json` (the folder ignores itself in git).

- A failure adds one to the count and blocks the stop: "attempt 1 of 3".
- After 3 blocks in a row the agent is allowed to stop, with a note that verify still fails. The count starts again.
- A pass resets the count. So does a stop attempt the agent marks as fresh (`stop_hook_active: false` from Claude Code and Codex, `loop_count: 0` from Cursor). An agent that says nothing either way is counted, so the cap always holds.
- Cursor's own `loop_count` is used when it is higher than firescale's.
- A count older than one hour is ignored.
- If the count cannot be stored (a read-only folder, or `.firescale/hooks` is a symlink), the agent is allowed to stop.
- `FSCALE_HOOK_MAX_BLOCKS=<n>` changes the cap. `0` never blocks.

`fscale hooks run` never exits with code 2 for its own errors, such as a mistyped agent name or a broken config. It exits 1 with the reason, which agents show but do not treat as "keep working". That holds wherever the options are written: `fscale --agent claude hooks run` and `fscale --cwd <dir> hooks run --agent claude` are the hook too. The value of an option is never read as the command, so `fscale --rules hooks run` is not the hook and keeps exit code 2.

## Limits

- There is no `hooks uninstall`. Remove the entry by hand.
- Hooks are written to the repo's project config only, never to user-level settings.
- The hook runs the whole of `fscale verify`. If the repo's tests take longer than the agent's own hook time limit, the agent ends the hook. Give the hook a faster command with the `verify` config key if that happens.
- Gemini CLI is not supported yet.

## When verify is only partial

A run where steps were skipped for lack of a command is `PARTIAL`, not a pass (see [verify.md](verify.md)). The hook does not block on it: the agent may stop, and the hook prints one line on stderr that names the skipped steps. To make the hook block until every step has a command or is set to `false`, turn on `verify.failOnSkipped` in the firescale config. ADR 0020 gives the reasons for the default.
