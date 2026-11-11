# 0017 — Plan order, the publishing checkpoint, and three owner decisions

Status: accepted (2026-11-11, before M8). Amends ADR 0015 ("Order of work") and ADR 0014.

## Context

The owner read the plan written with ADR 0015 and asked for three things: stack packs sooner, an explicit point at which firescale is published to npm, and a firm answer to three questions that DECISIONS.md had left open.

## Decision

### Order of milestones

The `nextjs` and `python` stack packs move to directly after M13, before blame-aware verify. `supabase` and `expo` stay at the end. Milestones after M13 are renumbered:

| Milestone                                    | Was | Now |
| -------------------------------------------- | --- | --- |
| Stack pack: Next.js                          | M24 | M14 |
| Stack pack: Python                           | M23 | M15 |
| Blame-aware verify: the base run             | M14 | M16 |
| Blame-aware verify: per-test labels          | M15 | M17 |
| Blame-aware verify: reruns and the stop hook | M16 | M18 |
| Tamper guard: design and the first language  | M17 | M19 |
| Tamper guard: more languages and policy      | M18 | M20 |
| Tamper guard: config and CI                  | M19 | M21 |
| Tamper guard: approvals and measured rates   | M20 | M22 |
| Guarded agent runner                         | M21 | M23 |
| Agent adapters and `fscale ledger`           | M22 | M24 |
| Stack pack: Supabase                         | M25 | M25 |
| Stack pack: Expo                             | M26 | M26 |

M8 to M13 keep their numbers. ADR 0015 and the "Direction change" section of DECISIONS.md use the old numbers; this table translates them.

Next.js is placed before Python because the owner named it first. Nothing in either pack depends on the other.

### Publishing checkpoint

PLAN.md gains a checkpoint between M9 and M10: **the owner publishes 0.1.0 to npm**, by hand, following "Publishing to npm" in DECISIONS.md. It is not a build milestone and has no pull request. M10 and M11 each note that the generated CI needs it, and say so at the top of their pull requests if it has not happened.

### Owner decisions

1. **Blame-aware labels are on by default** whenever there is a base commit. There is no flag to turn them on. (Built in M16.)
2. **The stop hook releases an agent after 3 blocks in a row.** This is what M7 built; it is now a decision and not a default awaiting review. `FSCALE_HOOK_MAX_BLOCKS` still changes it.
3. **A verify step with no command is skipped and prints a visible warning.** M7 skipped such a step and gave the reason only in the step's own row. Now `fscale verify` also prints a `warning:` line under the summary that names the skipped steps and how to set a command, and `--json` lists the same text under `warnings`. A step the config turns off with `false` is a choice, not a gap, and gives no warning. The exit code is unchanged: a skipped step is still not a failure.

### CI

CI is trimmed as described in ADR 0016, which lands in the same pull request as this ADR.

## Consequences

- Real stacks can be scaffolded two milestones after generators exist, without waiting for the referee. The cost is that the first two stack packs are verified by plain `fscale verify`, without blame-aware labels or the tamper guard.
- The `verify --json` format gains an optional `warnings` list. It is additive, so `schemaVersion` stays 1.
- The verify summary snapshots change, because skipped steps now add a warning line.
- The stop hook stays silent when verify passes, so a warning is seen by a person or in CI, and by an agent only when verify fails for another reason.
