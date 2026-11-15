# 0056 — Fixes for the bugs of the ponytail audit

Status: accepted (2026-11-15, for 0.5.0). Refines ADR 0034 and 0039 (what `--affected` and the stop hook keep) and ADR 0035 (what a progress line may show).

## Context

The audit of 2026-11-14 (`docs/reviews/ponytail-audit.md`) lists 30 bugs. The owner asked for numbers 1 to 14, 17 and 20 to be fixed, and nothing else: each bug first gets a test that fails because of it, then the smallest fix. None of the shared pieces the audit proposes (its items 1 to 79 and parts A, B and C) is built here. A bug that no test could show is not fixed; it is logged in `DECISIONS.md` with what was tried.

This ADR records the choices made where a bug's entry left more than one fix open.

## Decision

### Bug 1: a finding on a repo-level file is kept when the change is one its rule reads

`--affected` (and so the stop hook) kept a finding only if its file changed or its owner project was affected. A finding on `AGENTS.md`, caused by a new script in `web/package.json`, was dropped.

`narrow_report` now also keeps a finding when both hold:

- its file belongs to no project below the repo root (it has no owner, or its owner is the `.` project), and
- a changed file matches the cache `inputs` of the finding's rule.

Why both conditions:

- Keeping every finding whose rule reads a changed file brings back the findings of unaffected sibling projects (a new `tools/.env.local` brought back every `packages/*/.env` finding), which is the narrowing `--affected` exists for.
- Keeping every repo-level finding would report them all whatever changed.

An `inputs` glob that does not compile counts as matching, so the finding is kept. A rule with no inputs (`group-project/branch-name`) is not kept by this clause.

**Seen change:** an old, unchanged repo-level finding now shows under `--affected`, and blocks the stop hook, once any file its rule reads has changed. Two cases:

- In a repo with no root project: a tracked root `.env` shows once another env file changes. One test changed for it (`works when the firescale root is a folder inside a larger git repo`); it now has a project in its fixture, so that it still proves paths are relative to the firescale root.
- In a repo with a root project, when the change is in a sub-project: a finding of a rule with wide inputs, such as `core/no-secrets-in-source` (`**/*`) in `scripts/deploy.sh`, shows on every change. Before, it showed only when the root project was affected.

No narrower rule was found that still keeps the finding of bug 1: the engine does not know which of a rule's inputs caused a finding.

**Rules that do not declare what they read.** A rule that is not built in was never cached, so its `inputs` meant nothing until now and may be empty. Such a rule counts as reading every change. A change to `firescale.config.json` or `firescale.config.ts` counts as read by every rule, since it can turn any rule on.

**Limit, left open:** a finding inside a project that is not affected, caused by a change elsewhere, is still dropped. Closing it needs the graph (audit item 23), which was not asked for. `docs/affected.md` lists it. The clause is also only as good as a rule's `inputs`: where a rule reads a file it does not list (bug 10), a change to that file still drops the finding.

### Bug 2: verify runs nx and turbo through `npx --no`

`exec_prefix` in `verify.rs` returned a bare `npx`, which downloads a missing tool when stdin is closed. It now returns `npx --no`, the form facts already uses. The one literal was changed; the two functions were not merged (audit item 69). With bun, `bunx` can still fetch a missing tool; `docs/facts.md` already says so for facts.

The test `nx_and_turbo_take_precedence_and_honour_affected` asserted `npx turbo run lint`. It pinned the bug, so its two lines now expect `npx --no turbo run …`.

### Bug 3: the heartbeat hides what the report hides

The progress line's own `redact` knew only `scheme://user:password@` and stopped at the first `/`. The heartbeat now calls the existing `verify::hide_secrets` (ADR 0035), and `redact`, which had no other caller, is removed. No new redactor was written. The file name a known test runner's markers belong to (`(in tests/test_a.py)`) goes through `hide_secrets` too: a line that is only a connection string was read as that file name. `Activity::recent()` still returns lines as printed; its callers only search them for a refused service and print nothing from them.

## Consequences

- The stop hook blocks on a stale `AGENTS.md`, a missing permission or any other repo-level finding that the change itself caused.
- A fresh clone of an Nx or Turborepo repo that uses npm gets `SETUP NEEDED`, never a download.
- `hide_secrets` is now the one place that decides what output may be shown. It hides `password=`, `pwd=` and `passwd=` values; `token=` and `secret=` are still shown, in the report as in the heartbeat.
