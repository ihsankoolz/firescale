# Agent hooks

`fscale hooks install` makes a coding agent run `fscale verify` when it is about to stop, and sends it back to work while verify fails.

```
fscale hooks install --all                # Claude Code, Codex and Cursor
fscale hooks install --agent claude       # one agent
fscale hooks install --all --bin "npx firescale"
```

| Agent       | File written            | Hook                                                            |
| ----------- | ----------------------- | --------------------------------------------------------------- |
| Claude Code | `.claude/settings.json` | `hooks.Stop[].hooks[]`: `{ "type": "command", "command" }`      |
| Codex       | `.codex/hooks.json`     | The same shape as Claude Code.                                  |
| Cursor      | `.cursor/hooks.json`    | `version: 1`, `hooks.stop[]`: `{ "command" }`. **Unconfirmed.** |

The Claude Code format was checked against its documentation and the Codex format against its source on 2026-11-11. Cursor's documentation could not be reached from the build environment. On 2026-11-12 the format was checked against Cursor's own example repositories on GitHub (`cursor/cookbook`, `cursor/plugins`), and it matches (ADR 0027); `hooks install` still prints a note until it is checked against the docs page. `fscale sync` with the `agent-ready` pack writes the same hooks (see [init.md](init.md)).

Installing keeps everything else that is in the file, and running it again changes nothing. A hook counts as installed only when the command is under the agent's stop event. A file that is not valid JSON is left alone and reported. Commit the files so every teammate's agent gets the hook.

## `--bin`: how the hook starts fscale

The hook command is `<bin> hooks run --agent <name>`, and `<bin>` defaults to `fscale`. That only works where `fscale` is on the `PATH`. In a repo that installs firescale from npm, use `--bin "npx firescale"` (not `npx fscale`: that npm name belongs to another package). For the fastest hook, point `--bin` at the native binary (see [install.md](install.md)).

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

`fscale hooks run` never exits with code 2 for its own errors, such as a mistyped agent name or a broken config. It exits 1 with the reason, which agents show but do not treat as "keep working".

## Limits

- There is no `hooks uninstall`. Remove the entry by hand.
- Hooks are written to the repo's project config only, never to user-level settings.
- The hook runs the whole of `fscale verify`. If the repo's tests take longer than the agent's own hook time limit, the agent ends the hook. Give the hook a faster command with the `verify` config key if that happens.
- Gemini CLI is not supported yet.

## When verify is only partial

A run where steps were skipped for lack of a command is `PARTIAL`, not a pass (see [verify.md](verify.md)). The hook does not block on it: the agent may stop, and the hook prints one line on stderr that names the skipped steps. To make the hook block until every step has a command or is set to `false`, turn on `verify.failOnSkipped` in the firescale config. ADR 0020 gives the reasons for the default.
