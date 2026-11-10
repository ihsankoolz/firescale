# Kickoff prompts

Paste one prompt per session. Start a new session for each milestone.

---

## Prompt 1 — first session (M0 + M1)

```
You are building `firescale`, an open-source developer CLI. This repo currently
contains only planning files.

Read these fully before doing anything:
1. SPEC.md   — what we are building (source of truth)
2. PLAN.md   — milestones with "Done when" checklists
3. AGENTS.md — how to work in this repo and the hard rules

Your job this session: complete Milestone M0, then Milestone M1.

How to run the session:
- Work autonomously. Do not ask me questions. If something is unclear, choose the
  simplest reasonable option, record it as an ADR in docs/decisions/, and flag it
  at the top of the pull request.
- Before using any external tool, library or API (the Rust-to-Node binding tool,
  pnpm, GitHub Actions, test runners), check its current official docs and use
  current stable versions. Pin versions.
- M0 first, on branch `m0-skeleton`. Open a PR when every "Done when" box is true.
  Then branch `m1-rule-sdk` from it and do M1. Open a second PR.
- Everything must pass on Windows, macOS and Linux. The CI matrix is the proof.
  If CI fails on one OS, fix it before moving on.
- Run the full build, lint, typecheck and test suite before each PR. Do not open
  a PR with failing checks.
- In each PR body: copy that milestone's "Done when" list and tick each item with
  evidence (test name or command output). List any ADRs you wrote and anything
  you want me to decide.
- Update AGENTS.md "Commands" so it matches the real commands. Update the
  milestone status in PLAN.md.
- Stop after the M1 pull request. Do not start M2.

Finish with a short summary: what was built, what you decided on my behalf, and
anything that is not fully done.
```

---

## Prompt template — every later milestone

Replace `<N>` and `<name>`.

```
Read SPEC.md, PLAN.md, AGENTS.md and docs/decisions/ fully.

Your job this session: complete Milestone M<N> from PLAN.md, and only that.

- Work autonomously. Don't ask questions. Unclear points: choose the simplest
  option, write an ADR, flag it in the PR.
- Branch `m<N>-<name>` from main.
- Check current official docs before using any external API or CLI flag. Pin versions.
- Must pass on Windows, macOS and Linux in CI.
- Run the full check suite before opening the PR (use `pnpm fscale verify` once it exists).
- PR body: the "Done when" list ticked with evidence, ADRs written, open decisions.
- Update PLAN.md status and keep AGENTS.md commands accurate.
- Stop when the PR is open. Do not start the next milestone.

End with a short summary of what was built, decisions made, and anything not done.
```

---

## Notes for M5 (agent runner)

- Do the build in a cloud session using the fake agent adapter.
- Then pull the branch to your own machine and run the one manual test with a real agent.
- Add the result to the PR before merging.
