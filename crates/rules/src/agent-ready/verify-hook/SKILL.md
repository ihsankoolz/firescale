---
name: verify-hook
description: Fixes violations of the firescale rule agent-ready/verify-hook by installing the stop hook that makes a coding agent run fscale verify before it stops. Use when fscale check reports agent-ready/verify-hook.
---

# agent-ready/verify-hook

## What this rule enforces and why

At least one supported coding agent has firescale's stop hook in its project config: `.claude/settings.json` (Claude Code), `.codex/hooks.json` (Codex) or `.cursor/hooks.json` (Cursor). The hook is recognised by its command, which contains `hooks run --agent <name>`, whatever comes before it.

With the option `agents`, for example `["claude", "codex"]`, each listed agent must have the hook, and each missing one is reported on its own.

The hook runs `fscale verify` when the agent is about to stop and sends it back to work while verify fails. Without it, "done" is whatever the agent says it is.

## Manual fix

1. Run `fscale hooks install --all`, or `fscale hooks install --agent <claude|codex|cursor>` for one agent.
2. If `fscale` is not on the `PATH` of your teammates and CI, say how to start it with `--bin`, for example `fscale hooks install --all --bin "npx firescale"`.
3. Commit the files it wrote, so every teammate's agent gets the hook.

## Agent fix instructions

### DETERMINISTIC

None. Which command starts `fscale` differs from repo to repo, so the user chooses it.

### REPORT

Do not edit the hook files by hand and do not run the installer yourself: a hook changes how every agent session in this repo ends, so the user decides. Report:

- Which agents have no hook, from the findings.
- The exact command to run: `fscale hooks install --all`, or with `--agent <name>` for the agents listed in the findings.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules agent-ready/verify-hook
```
