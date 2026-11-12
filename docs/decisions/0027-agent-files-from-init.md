# 0027 — Agent files from `init`: managed block, hooks, permissions, skills

Status: accepted (M13). Builds on ADR 0014 (hooks), 0015 (born agent-ready), 0019 (init, adopt, sync).

## Formats, confirmed on 2026-11-12

| Agent       | File                                                                                                                                                                                                                                                                                                    | Source                                                                                              | Status                                                                                     |
| ----------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------ |
| Claude Code | `.claude/skills/<name>/SKILL.md`; `.claude/settings.json` `hooks.Stop`, `permissions.allow`/`deny` (`Bash(cmd *)`, `Read(<gitignore pattern>)`); CLAUDE.md is read instead of AGENTS.md when it exists, and `@AGENTS.md` imports it                                                                     | code.claude.com docs: skills, hooks, permissions, settings, memory                                  | confirmed (docs)                                                                           |
| Codex       | `.agents/skills/<name>/SKILL.md` (repo scope); `.codex/hooks.json` `Stop`; `.codex/config.toml` permission profiles (`default_permissions`, `[permissions.<p>] extends`, `filesystem.":project_roots"` globs = `"deny"`), loaded only in a trusted project; hooks run only after the user approves them | source of `openai/codex` at tag `rust-v0.167.2` (developers.openai.com was blocked)                 | confirmed (source); the profile stanza is assembled from Codex's own tests and was not run |
| Cursor      | `.cursor/hooks.json` `version: 1`, `hooks.stop[]` `{ command }`, reply `followup_message`; `.cursor/skills/<name>/SKILL.md`                                                                                                                                                                             | Cursor's public GitHub repositories `cursor/cookbook` and `cursor/plugins` (cursor.com was blocked) | confirmed (official examples)                                                              |
| Cursor      | `.cursor/cli.json` `permissions.deny` with `Read(...)`                                                                                                                                                                                                                                                  | web search summaries of cursor.com only                                                             | **unconfirmed**                                                                            |

The Cursor hook format that ADR 0014 marked unconfirmed matches Cursor's own examples.

## Decision

### `agent-ready/agents-md` writes a managed block

- The block runs from a line starting `<!-- BEGIN firescale` to the line `<!-- END firescale -->`. Only an exact line counts, so the look-alike markers in the scaffold tests are text. Two BEGIN lines, or a BEGIN with no END, is a manual finding; nothing is written.
- Content, from `fscale facts`: the root's install command and its build, lint, typecheck and test scripts exactly as facts names them (so `commands-documented` passes), then each project that runs by itself (the same list as the contributing guide), then the definition of done: run `<fscale> verify`, never weaken a test, `SETUP NEEDED` is not a code problem, `<fscale> doctor` and `explain`. Python commands are written `python -m ...`, never this machine's `.venv` path, so the block is the same on every teammate's machine and `sync --check` does not flip between Windows and Linux.
- Missing AGENTS.md: created with a heading, one sentence, and the block. Blank: the block is written. No block: it is appended after a blank line. Out of date: only the block is replaced. The file's line endings and byte order mark are kept; text outside the block is never rewritten (tests check it byte for byte).
- The check is now "AGENTS.md has the current block". An AGENTS.md without one is a deterministic finding (it was a pass before). This repo's own AGENTS.md got the block (dogfooding).

### A repo with CLAUDE.md and no AGENTS.md (owner's question)

`adopt` (with `agent-ready` on) and `sync` **create AGENTS.md and add a three-line managed block to the end of CLAUDE.md** with `@AGENTS.md`. Claude Code reads only CLAUDE.md when it exists, so without the import it would never see the commands and the definition of done; Codex and Cursor read AGENTS.md. Nothing in CLAUDE.md is changed: the block is appended (a last line with no newline gets one, which the "every line survives" tests accept). A CLAUDE.md that already imports AGENTS.md outside a code fence is left alone. Nested CLAUDE.md files (`backend/CLAUDE.md`) are never touched. Alternatives were to leave CLAUDE.md alone and only print a note (then Claude Code keeps missing the block), or to move CLAUDE.md's content into AGENTS.md (rewrites a person's file). Listed under "Needs your decision".

### `agent-ready/verify-hook` has a fix

It adds the stop hook for Claude Code, Codex and Cursor (or the agents in its `agents` option) when none has it. The command is the repo's own `fscale` package script when it has one (`pnpm --silent fscale`, `npm run --silent fscale --`), else `npx firescale`, the same as the starter CI. The check's meaning is unchanged: by default one agent with the hook is enough.

### New rule `agent-ready/agent-permissions`

- Claude Code: `permissions.deny` gets `Read(**/.env)`, `**/.env.local`, `**/.env.*.local`, `**/.env.development`, `**/.env.production`, `**/.env.staging`, `**/.env.test`, `**/*.pem`, `**/*.key`, `**/secrets/**`; `permissions.allow` gets `Bash(<fscale> verify *)`, `check`, `facts`, `doctor`, `explain`, and `Bash(<script> *)` for the root's build, lint, typecheck and test scripts. `.env.example` is not denied, because `core/env-example`'s skill tells agents to edit it and a Read deny also blocks Edit; so `.env.*` as a whole is not denied, and other names (`.env.qa`) are not covered.
- Codex: a new `.codex/config.toml` with a `firescale` profile that extends `:workspace` and denies the same globs. A `.codex/config.toml` a person wrote is never edited (TOML cannot take a top-level key appended after its tables); the finding is manual and the skill shows the stanza.
- Cursor: `.cursor/cli.json` `permissions.deny` with the same `Read(...)` entries, written as unconfirmed with a note.
- Severity medium, category security, after `verify-hook` (both write `.claude/settings.json`).

### New rule `agent-ready/agent-skills`

The SKILL.md of every built-in rule that is not `off` (read from `firescale.config.json`; a `.ts` config is an error that says to use JSON or turn the rule off) goes to `.claude/skills/`, `.agents/skills/` and `.cursor/skills/`, in a folder `firescale-<pack>-<rule>`, with `name` set to the folder name (the Agent Skills format asks for that) and a "Generated by firescale" line after the frontmatter. Only files with that line are rewritten; a person's file at the same path is a manual finding. A pack's own extra skills have a place (`pack_skills`), empty until M14's generator skill. Severity low.

### Existing JSON settings get new lines only

A settings file that exists (`.claude/settings.json`, `.cursor/hooks.json`, `.cursor/cli.json`) is not re-serialised. `crates/core/src/jsonedit.rs` inserts missing keys and list items as new lines right after the line that opens their object or list, in the file's own indentation and line endings; every existing line stays as it was, and the result is parsed and compared before it is written. When a bracket does not end its line (`"deny": []`, a minified file) the fix leaves the file and the finding is manual. New list items go first in their list, so hooks the team had run after firescale's. This is stricter than SPEC §15's "merges into a structured file", so the owner's rule for the course project holds: every existing line survives, and firescale writes only inside managed blocks, as new lines in JSON settings, or in files it created.

## Consequences

- `fscale init` in an empty folder now passes the whole `agent-ready` pack (53 changes, 42 of them skills).
- `adopt` writes `core` only, as before; with `agent-ready` in the config it writes all of the above.
- Listed under "Needs your decision": the CLAUDE.md block, the Codex profile that was not run, Cursor's unconfirmed `cli.json`, the env file names that are denied, and 42 skill files in a new repo.
- Not done: removing generated skills of a rule that was turned off (they stay until deleted by hand); custom TypeScript rules' skills; `.cursorignore` (reported not to stop the agent's terminal, so not a boundary).
