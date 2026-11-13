# 0031 — Affected-only verify and the step cache

Status: accepted (2026-11-12, before M17). Renumbers the plan of ADR 0020. Amends the blame-aware milestones of ADR 0015 and ADR 0017.

## Context

Running verify on the course project took over ten minutes: one service's tests alone took 13 minutes (0.1.3, ADR 0024). Blame-aware verify, which was next in the plan, makes that worse before it makes it better: it runs the base commit too. The owner decided to make verify cheap first, then make it fair:

1. Run only what a change can affect. A change in one service should run that service's lint, typecheck and tests, not every project's.
2. Do not run again what already passed on the same inputs.
3. Then build blame-aware verify on both: its base run only needs the projects whose inputs differ between base and head.
4. Then make the CI that firescale generates fast too, with a cache that cannot be poisoned.

## Decision

### Order of milestones

| Milestone                                    | Was | Now |
| -------------------------------------------- | --- | --- |
| Project graph and affected verify            | —   | M17 |
| Local step-result cache and timings          | —   | M18 |
| Blame-aware verify: the base run             | M17 | M19 |
| Blame-aware verify: per-test labels          | M18 | M20 |
| Blame-aware verify: reruns and the stop hook | M19 | M21 |
| Faster generated CI and a shared cache       | —   | M22 |
| Tamper guard: design and the first language  | M20 | M23 |
| Tamper guard: more languages and policy      | M21 | M24 |
| Tamper guard: config and CI                  | M22 | M25 |
| Tamper guard: approvals and measured rates   | M23 | M26 |
| Guarded agent runner                         | M24 | M27 |
| Agent adapters and `fscale ledger`           | M25 | M28 |
| Stack pack: Supabase                         | M26 | M29 |
| Stack pack: Expo                             | M27 | M30 |

M0 to M16 keep their numbers. Older ADRs and the older sections of DECISIONS.md keep the numbers they were written with; this table translates them.

### M17: the project graph and affected verify

- The graph in `fscale facts` grows beyond workspace files. Edge sources, most trusted first: local paths in manifests; Dockerfile `COPY` of a sibling folder and Compose `build.context` and `depends_on`; Python imports that resolve to a sibling project's package and JavaScript or TypeScript relative imports that cross a project root, read with a real parser.
- A folder that projects import or copy but that is not a project itself is a shared-source node. A change there affects only the projects that use it.
- When unsure, more is affected, never less: a changed file no project owns, a root lockfile, a CI workflow, a Compose file or the firescale config affects every project. A dynamic Python import counts as depending on every other Python project, with a warning.
- `fscale graph` prints the graph; `--explain` names the source of every edge.
- `fscale verify --affected` runs lint, typecheck and test only for affected projects. The others are reported as `not affected`, a state apart from `skipped`, and the headline says how many projects ran out of how many. It never reads as a plain OK for the whole repo.
- **Verify schema 5 goes to M17.** Blame-aware verify takes 6.
- The stop hook does not change in M17.

### M18: a local step-result cache and timings

- Only passes are cached. A failure always runs again.
- The key of a step's result is: the firescale version, the command, the hashes of the project's own files, the hashes of the projects it depends on (from the M17 graph, shared-source nodes included), the lockfile, the versions of the tools the step runs, the OS and CPU architecture, and the values of an allowlist of environment variables. Nothing else from the environment is read, so a secret in the environment can never end up in a key.
- **A step that needs a network service that is not in the repo is never cached.** Its result depends on something the key cannot see. A service the repo starts itself (a Compose service, ADR 0025) can be.
- Timings of each step are recorded, so the summary can say what the cache saved.
- The cache lives under `.firescale/` on the machine. It is never shared in M18.

### M19 to M21: blame-aware verify

As planned before (ADR 0017, PLAN.md), with one change: **the base run reuses the M18 cache.** A project whose inputs are equal on the base and the head needs no base run: its result on the head is its result on the base. Only projects whose inputs differ run in the base worktree. This is what keeps the base run inside its time budget. Blame-aware verify bumps the verify schema to 6.

### M22: faster generated CI and a shared cache

- The CI workflow that `init` writes runs `fscale verify --affected` on pull requests and keeps the M18 cache in the GitHub Actions cache.
- **The shared cache is written only by pushes to the default branch.** Pull requests and laptops read it and never write it. A cache that a pull request can write lets a contributor plant a result that a later run on the default branch trusts: CVE-2025-36852 ("CREEP") is that attack against remote build caches. Only code that has been merged can write.
- A restored result is used only when its whole key matches, and a run never uploads results it restored.

### Later

Test-level selection (running only the tests a change can reach) is not planned. It goes under "Later" in PLAN.md.

## Consequences

- Verify becomes cheaper before it becomes stricter. An agent's stop hook stays fast when blame-aware labels arrive, because the base run skips unchanged projects.
- M18's cache depends on M17's graph being complete enough: an edge the graph misses is a dependency whose change does not invalidate a cached pass. That is why M17 errs toward affecting more, and why a dynamic import or a file with no owner widens the set instead of narrowing it.
- Three milestones were added and every milestone from the old M17 on moved by two or three. HANDOFF.md, PLAN.md and SPEC §8 use the new numbers.
