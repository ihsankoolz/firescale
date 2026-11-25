# 0084 — A way out for agents: say which test is wrong, and stop

Status: accepted (2026-11-18, Track B run). Amends ADR 0027 (the AGENTS.md block), and the stop hook's block message of ADR 0014 and ADR 0022. Verification idea 5, first step: wording only. Amended by ADR 0087 (the owner's decision: keep blocking; show the agent's explanation to the person when the guard lets it go).

## Context

The stop hook sends an agent back while verify fails, and the AGENTS.md block says "Never skip, delete or weaken a test, a lint rule or a check to make it pass." Neither says what to do when the agent believes the test itself is wrong. Left with "make it pass" and "do not weaken it", editing the test is the only move that ends the loop. ImpossibleBench (Zhong et al., arXiv 2510.20270) reports that letting the model flag a task as impossible and stop cut cheating sharply for some models (the research report's figures: GPT-5 from 54% to 9%, o3 from 49% to 12%, less for Claude Opus 4.1). Those numbers are for benchmark scaffolds, not hooks, and were read through the research report, not checked again here.

## Decision

- **One line**, the same in both places: `If you believe a test or check is wrong, do not edit it: say which and why, and stop.` It is `hooks::WAY_OUT`.
- **The stop hook's block message** (Claude Code and Codex on stderr, Cursor in `followup_message`) carries it as its second line, right after `fscale verify failed, so the work is not done yet (attempt N of 3).` and before the verify summary. **Not when a step needs setup**: then the summary opens with the `SETUP NEEDED:` note of ADR 0022, which says what to run; a missing dependency is not a wrong test, and a line before the note telling the agent to stop would compete with it (the review of this pull request).
- **The AGENTS.md block** carries it as the bullet after "Never skip, delete or weaken ...". `fscale sync` replaces the block, so it puts the line into a block an older firescale wrote, and `sync --check` reports the old block as out of sync (`agent-ready/agents-md`). This repo's own block is updated in the same change.

## Consequences

- **Every repo with the block is out of sync after the upgrade** until `fscale sync` runs: `sync --check` exits 1 with a one-line diff, and the `agent-ready/agents-md` rule reports it. Expected, and the same as any change to the block.
- An agent that takes the way out stops while verify fails. **The hook does not know it did**: it blocks again, with the same line, up to the cap of three blocks (ADR 0014), and only then lets the agent stop. So taking the way out costs up to two more turns of the same message. This step is wording only; letting a stated stop through is a change to the loop guard, left for later. Nothing checks that the agent said which test and why.
- The line does not tell the agent to edit nothing else: an agent can still weaken a test and claim the test was wrong. The tamper guard still runs.
- `GiveUp` (the cap reached) and the CLAUDE.md block are unchanged: CLAUDE.md imports AGENTS.md.

## Later

- `hooks install --guard-edits`: a `PreToolUse` entry that denies writes to the files the guard depends on (verification idea 5, the second step). Not built.
- Measure whether agents use the line: the ledger of `fix --agent` could record a stop with a stated reason.
- Let a stated stop through: the loop guard could end the blocks early when the agent's last message names a test it believes wrong. Not built: it would make a stop easier to get, which needs the owner's decision.
- A way out in a block that has both a setup note and a failing step: today the setup note alone leads.

## Tests

- `hooks::tests::the_way_out_is_the_second_line_and_is_left_out_of_a_setup_block` (from the review, seen failing before the fix) and `managed::tests::the_block_gives_the_way_out_after_the_rule_on_tests`: fast tests of the line's place.

- `hooks.test.ts`, "a way out for an agent that believes a test is wrong (ADR 0084)": Claude Code, Codex and Cursor each get the line as the message's second line. Seen failing on `main` in WSL.
- `agent-files.test.ts`, the same name: `sync` writes the line after "Never skip ..."; a block without it fails `sync --check` (`agent-ready/agents-md`), and `sync` restores it, text outside the block unchanged. Seen failing on `main`.
- Two snapshots gain the line on purpose: the Claude Code block message and the generated AGENTS.md.

## Not done

- From the review, all five findings are dealt with: the setup case (fixed, with a test), the repeated message (described above, not changed: the loop guard is out of this step), the agents-md skill's description (updated), the missing fast tests (added), the wrong ADR cited (fixed).
