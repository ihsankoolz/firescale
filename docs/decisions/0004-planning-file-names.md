# 0004 — Planning files renamed to SPEC.md and PLAN.md

Status: accepted (M0)

## Context

The repo arrived with `SPEC_1.md` and `PLAN_1.md`, but `AGENTS.md`, `KICKOFF.md` and the kickoff prompt all refer to `SPEC.md` and `PLAN.md`.

## Decision

Rename the files to `SPEC.md` and `PLAN.md` with `git mv`. Content is unchanged apart from milestone status marks in `PLAN.md`.

## Consequences

Every reference in the repo now resolves.
