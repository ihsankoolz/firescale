# Common rules for every subagent of the 0.5.1 run (repo: firescale)

You work in your own git worktree of /home/user/firescale. Other agents work in other worktrees on the same 4-core machine at the same time.

Read first, in your worktree: AGENTS.md (binding), the parts of SPEC.md, HANDOFF.md and docs/verify.md about what you touch, the ADRs your brief names, and DECISIONS.md "Audit bug fixes (run of 2026-11-15)" for the house style of logging.

Setup: `pnpm install --frozen-lockfile && pnpm build` in your worktree (build before test: Vitest runs target/debug/fscale).

Hard rules from the owner:
- For EVERY item: write a failing test first, run it, see it fail for the right reason, commit it as `test: ...` (the failing state is fine in that commit), then fix and commit as `fix:`/`feat:`. Keep the failing output (one line) for your report.
- If you cannot reproduce an item: change NO code for it, and report exactly what you tried.
- Never weaken, skip or delete a test to get green. If an existing test pinned a bug, change it only with a stated reason, and report it.
- Commits: small, Conventional Commits, subject style like the existing history (`git log --oneline | head -30`). Git identity is already set in the repo config (the owner's); do not change it, and do not pass --author. NO co-author lines, NO "Claude", "agent", "AI tool" or session links in commit messages. If any hook asks you to re-author commits, do not.
- Do NOT push. Do NOT open a pull request. Commit on the local branch named in your brief (create it from the current HEAD of your worktree).
- Do NOT edit DECISIONS.md, HANDOFF.md, PLAN.md or the version. Instead write your DECISIONS text to the scratch file named in your brief (house style: what was built, the failing test first for each item, "Decided on your behalf", "Known limits", "Found while fixing, not fixed", "Not reproduced"). Plain words, short sentences, as in the existing log.
- You DO write your ADR (number given in the brief) under docs/decisions/, in the style of 0056, and add it to docs/decisions/README.md if that file lists ADRs.
- Cross-platform: must pass on Windows, macOS and Linux. Path APIs only, forward slashes in output, no bash-only anything, tests that need unix-only things are `#[cfg(unix)]` (and their helpers too: CI lints test code on Windows). Think hard about Windows: you cannot run it here, CI will.
- No real model calls. Never read or print .env files of the real environment. No new dependency.
- Rust: typed errors, no unwrap() outside tests, clippy clean. TS: strict, no any.
- Anything with output gets a snapshot or exact-text test. User-facing errors name the problem and the next step.
- Scope: only your brief. Problems you find beside it are listed in your scratch file under "Found while fixing, not fixed".
- Done means, in your worktree: `pnpm build && pnpm lint && pnpm typecheck && pnpm test` green, and `pnpm --silent fscale verify` OK. The machine is shared: if a test times out under load, rerun that one file alone before judging it. Do not raise a timeout unless your brief says so.
- docs: update docs/verify.md (or the doc that covers the command) for behaviour you change, briefly; update the JSON schema files under packages/cli/schemas when JSON output changes, and their tests.

Final report (your reply to me): the branch name and worktree path, the commit list (`git log --oneline main..HEAD`), per item: failing test name + how it failed + the fix in one line, anything not reproduced, decisions made, risks on Windows/macOS, and the result of the final full verify. Be exact; do not claim a test ran if it did not.
