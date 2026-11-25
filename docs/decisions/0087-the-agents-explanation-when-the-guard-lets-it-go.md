# 0087 — The agent's explanation, shown when the loop guard lets it go

Status: accepted (2026-11-25, Track B run). Amends ADR 0084 (a way out for agents) and ADR 0014 (the loop guard).

## Context

ADR 0084 tells an agent that believes a test is wrong to say which and why, and stop. The hook cannot tell that stop from any other: it blocks again, up to the cap of three, and then lets the agent go with a line on stderr that the person may never see. ADR 0084 left "let a stated stop through" for the owner. The owner's decision (2026-11-25): **keep blocking**; when the guard lets the agent go, the final message must show the person the agent's explanation.

## Decision

- **The hook reads `last_assistant_message`** from its input. Claude Code's hooks page: Stop hooks receive `last_assistant_message`, "the text content of Claude's final response"; Codex's hooks page: "Latest assistant message text, if available" (both read through a subagent on 2026-11-25). An empty or blank one counts as none.
- **When the loop guard lets the agent go** (`GiveUp`), Claude Code and Codex get, on stdout with exit 0, `{"systemMessage": "..."}`: the same line as before (verify still fails after N attempts, run `fscale verify`), then `The agent's last message:` and its first 4000 characters, with a note when it was cut. Both pages document `systemMessage` as the message shown to the person ("Warning message shown to the user"; Codex: "Surfaced as a warning in the UI or event stream"); Claude Code's Stop section does not say it discards it. The stderr line is unchanged.
- **Cursor is unchanged** (`{}`): its stop hook gets no last message, and has no output field shown to the person (its hooks page, the same day).
- **Blocking is unchanged.** The count, the cap and the block message are as before; nothing makes a stop easier to get. The agent's message is never sent back to the agent.

## Consequences

- With no last message (an older Claude Code, or a hook run by hand), the person still sees why the agent was let go, as a `systemMessage`.
- Codex's Stop hook now gets JSON on stdout when it is let go, where it got nothing; its page says a Stop hook that exits 0 should print JSON.
- No test runs a real agent: the field names are from the documentation.

## Later

- Cursor: the transcript (`transcript_path`) holds the last message, but Cursor shows the person nothing from a stop hook; nothing to do until it does.
- Whether the explanation names a test (and which) is not checked.

## Tests

- `hooks.test.ts`, "loop guard: when it lets the agent go, the person sees the agent's last message (ADR 0087)": seen failing on `main` (no JSON on stdout).
- `hooks::tests::letting_the_agent_go_shows_the_person_its_last_message`: the message for each agent, the cut, a blank message, and that a block carries nothing of it.

## From the fresh review

Fixed: the line saying why the agent was let go was written twice (stderr and `systemMessage`); one function now gives both.

## Not done

- The cut at 4000 characters can split a grapheme cluster (an emoji sequence, a combining accent): cosmetic, and no dependency is added for it.
- That Claude Code and Codex never feed a Stop hook's `systemMessage` back to the model rests on their documentation. If one did, the agent would see its own message once, after it was already let go: no stop is made easier.
