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
- otherwise the firescale the repo installs, when the root `package.json` lists `firescale` in `devDependencies` or `dependencies`: `pnpm exec fscale`, `yarn --silent fscale`, `bun run --silent fscale` or `npx --no -- fscale` (never a download; `--` ends npx's own options, since npm 10 takes the `--version` of `npx --no fscale --version` for itself and prints npm's version);
- otherwise `npx --yes firescale@<version>`, with the version of the fscale that wrote it. The pin means a release of firescale does not change every teammate's hook, nor empty every cache key, until someone runs a newer fscale on purpose. Not `npx fscale`: that npm name belongs to another package.

This is the command the `agent-ready/verify-hook` fix and the AGENTS.md block use too. For the fastest hook, point `--bin` at the native binary (see [install.md](install.md)), or at `fscale` where every machine has it on the PATH.

`hooks install` does not rewrite an installed hook when you upgrade firescale: a hook counts as installed whatever command comes before `hooks run --agent <name>`. **`fscale sync` moves an older pin firescale wrote** to its own version: a stop hook command that is exactly `npx --yes firescale@<older version> hooks run --agent <name>` gets the new version, and the output says so under `agent hooks`. It never changes any other command (one with more text after it, an unpinned or package-script command, another hook), never moves a pin back to an older version, and leaves a file that is not JSON or is a symbolic link as it is. `sync --check` reports the move and exits 1 (ADR 0064, section 7). A pin an older firescale wrote (`npx --yes firescale@0.5.0`) is still firescale's own command: the check below and `fscale doctor` start it, take the plain version that firescale 0.5.1 and older print for `--version --json`, and say that the agent runs that version, as a note, not a failure. `hooks install` then names the command each file keeps.

**A new pin before its release is published.** Run from a build whose version is not on npm yet (a release being prepared), `hooks install` writes a pin npx cannot resolve, so the check says the hook cannot start until that version is published. After the release, it starts.

### The check after installing

After writing the files, `hooks install` starts each command the hook files hold, as `<command> --version --json`, the way an agent starts it: in the repo root, through `cmd /D /S /C` on Windows and `sh -c` elsewhere, with stdin closed, for up to 120 seconds, and with `CLAUDE_PROJECT_DIR` set to the repo root for the command of Claude Code's file. It asks for `--version --json` because only firescale (0.5.2 and later) answers it with `{"name":"firescale","version":"..."}`: a program that prints a version (`npx --version` prints npm's) is not taken for firescale, and one start answers both questions.

Before it starts anything, it says on stderr what it runs, and:

- that it may download firescale from the npm registry, for `npx`, `npm exec`, `pnpm dlx`, `yarn dlx`, `bunx` and `bun x` (not with `npx --no`);
- that it runs the fscale in the repo's `node_modules`, for `pnpm exec fscale` and `npx --no -- fscale`;
- that it runs the repo's own `fscale` package script, for a package script.

It leaves out of the PATH it gives the command the folders a package manager puts first while it runs fscale (`npx firescale hooks install`, `pnpm fscale hooks install`), and says which: `node_modules/.bin` of the repo root or of a folder above it, npx's `_npx/<hash>/node_modules/.bin`, the download folders of `pnpm dlx`, `yarn dlx` and `bunx`, and `node-gyp-bin`. An agent does not have them. Without this, `--bin fscale` "started" through `node_modules/.bin/fscale` and nowhere else. Any other folder inside a `node_modules` stays: `pnpm/action-setup` puts pnpm itself in `~/setup-pnpm/node_modules/.bin`, which is on the PATH an agent has too.

It starts only a command fscale writes itself: the one this run writes (the default or `--bin`), the repo's default, and a pin any release wrote (`npx --yes firescale@<version>`). A hook file that already holds another command before `hooks run --agent <name>` is text from the repo, and starting it would run that text on your machine. That command is printed, not started: `The hook in .cursor/hooks.json was not started: it runs ...`, with the command to run yourself once you have read it. The exit code is then 1.

```
installed         .claude/settings.json
installed         .codex/hooks.json
installed         .cursor/hooks.json

Each agent now runs `npx --yes firescale@0.5.2 hooks run --agent <name>` before it stops, and keeps working while `fscale verify` fails.
The hook starts on this machine: `npx --yes firescale@0.5.2 --version --json` answered firescale 0.5.2.
```

When the answer is another version, the line says which firescale the agent will run. When the command does not start, exits with an error, or answers something that is not firescale's answer, the line says `The hook cannot start on this machine:` with the command and what it printed, the next lines say what to do, and the command exits with 1 (2 when a file was refused). The files are still written: the command may start on a teammate's machine.

The check is close to what an agent does, not the same: Claude Code runs the command through Git Bash or PowerShell on Windows, Codex through the session's shell, and an agent started from a desktop app may have another PATH than your terminal.

### `fscale doctor` checks it on any machine

The check above runs on the machine that installs the hook. `fscale doctor` checks the stop hook of each hook file on whichever machine runs it, in a section `Agent hooks` (`hooks` in `--json`), but it runs no script of the repo and downloads nothing:

- a pin, `npx --yes firescale@<version>`, is started as `npx --no -- firescale@<version>`, which runs firescale only when it is installed or already in npx's cache. When npx would have to download it, a `warning` says it is not downloaded yet: the agent's first stop downloads it, which needs the network;
- a command that runs the repo's own `fscale` package script (the repo's `package.json` has one) is not started: a `warning` gives the command to run yourself;
- the fscale the repo installs (`pnpm exec fscale`, `npx --no -- fscale`, ...) is started as it is.

It is `ok` when it answers this firescale, a `warning` when it answers another version, and a `PROBLEM` (exit 1) when it cannot start. Any command fscale does not write is a `warning` that says it was not started, with the command to run yourself. Each distinct command is started once.

## When the hook cannot start

None of the three agents keeps an agent working when its stop hook cannot start: each takes it as a failed hook and lets the agent stop, so verify has not run. After `hooks install`, a hook that stops starting later (npx gone, no network for a first download, a package script whose dependencies are not installed) still lets the agent stop; run `fscale doctor` to find it. What each agent does, from its documentation or source, read on 2026-11-16 (ADR 0064). A quote is from the agent's documentation; "source reading" means it comes from reading the agent's code, which its documentation does not state; "unverified" means neither says:

|                                        | Claude Code                                                                                                                                                                                                                                           | Codex                                                                                                                                                                                                                                  | Cursor                                                                                                           |
| -------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------- |
| Runs the command through               | `sh -c` on macOS and Linux; Git Bash on Windows, or PowerShell when Git Bash is not installed (a `shell` field can choose `bash` or `powershell`). With `args` (exec form), no shell                                                                  | the session's shell; without one, `$SHELL -lc` (default `/bin/sh`), or `%COMSPEC% /C` (default `cmd.exe`) on Windows (source reading). `commandWindows` replaces the command on Windows. Which shell a Windows session has: unverified | PowerShell in the Windows desktop app; macOS, Linux and the CLI: unverified                                      |
| Runs a project's hook only when        | always                                                                                                                                                                                                                                                | "the project `.codex/` layer is trusted", and Codex "records trust against the hook's current hash": "new or changed hooks are marked for review and skipped until trusted", with a warning at startup to open `/hooks`                | unverified                                                                                                       |
| Exit 2 on stop                         | keeps the agent working; stderr goes to it                                                                                                                                                                                                            | keeps the agent working when stderr is not empty; with empty stderr it is a failed hook (source reading)                                                                                                                               | "Block the action" in general; for stop: unverified. firescale answers Cursor with exit 0 and `followup_message` |
| Any other non-zero exit                | a non-blocking error: the agent stops                                                                                                                                                                                                                 | a failed hook run, `hook exited with code N`: the agent stops (source reading; the docs do not say)                                                                                                                                    | "Hook failed, action proceeds (fail-open by default)": the agent stops                                           |
| A command that cannot start            | "In shell form, when the script path doesn't exist or isn't executable, the shell exits with a code like 127", and "a hook that can't start is a non-blocking error too"                                                                              | the shell's own exit code (127 from `sh`, 9009 from `cmd`) or a spawn error: a failed hook run (source reading; the docs do not say)                                                                                                   | not stated: unverified                                                                                           |
| What the user sees                     | a `<hook name> hook error` notice, `Failed with non-blocking status code: <stderr>` (shown in the docs for `PreToolUse`; for stop, unverified)                                                                                                        | the failed hook run with its message (how the TUI shows it: unverified)                                                                                                                                                                | logged, in the Hooks output channel; anything in the chat: unverified                                            |
| A setting to block when the hook fails | `"onFailure": "block"`, but "The field has no effect on these hooks: `Stop`, `SubagentStop`, `TaskCompleted`, and `TeammateIdle` hooks: exit code 2 on these events sends Claude back to keep working, and Claude can't repair a hook that won't run" | none found                                                                                                                                                                                                                             | `failClosed`; whether it applies to stop: unverified                                                             |

Claude Code's documentation also says: "`Stop`, `SubagentStop`, `TaskCompleted`, and a plugin's `UserPromptSubmit` hook: when your hook exits 2 with nothing on stdout and its stderr says a file is missing, such as `No such file or directory`, Claude Code treats the run as a non-blocking error."

firescale does not wrap the command to turn a failed start into a block. ADR 0064 gives the reasons, and "Needs your decision" in DECISIONS.md keeps the one option found: a small launcher for Claude Code's exec form. If an agent seems to stop without verify having run, run `fscale doctor`, or the hook command by hand from the repo root with `--version --json` in place of `hooks run --agent <name>`.

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
