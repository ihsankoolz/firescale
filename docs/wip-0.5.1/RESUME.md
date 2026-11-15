# Paused: the 0.5.1 run (2026-11-15, 11:25 UTC)

The owner asked to save and pause. This folder is working state, not documentation. Delete it in the pull request that sets 0.5.1.

## Where each pull request stands

| Pull request                    | State                                                                                                                                                                                      | Where the work is |
| ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ----------------- |
| 3. Stopped runs and doctor      | **Merged** as #54 (ADR 0059), green on Linux, macOS and Windows                                                                                                                            | `main`            |
| 4. Leftovers                    | Built, reviewed, review fixes made, full tests green on Linux. **Not yet in a pull request.**                                                                                              | commit `7f97b3a`  |
| 1. Slow steps                   | Built, full tests green on Linux. **Review started and was cut off; no findings received yet.**                                                                                            | commit `6f6cbeb`  |
| 2. What a config change affects | Built and reviewed. The reviewer proved two holes (below). The tests for both are committed (`c44782f`); **the fix is half done, in a commit that was never built or tested** (`5ea536c`). | commit `5ea536c`  |
| 5. Version 0.5.1                | Not started                                                                                                                                                                                |                   |

The three commits above are parents of the merge commit on `claude/quirky-cannon-20uxi6` whose message starts "wip: keep the unmerged branches". That merge changes no file; it only keeps the commits on GitHub. Get them back with:

```
git fetch origin claude/quirky-cannon-20uxi6
git branch pr1-slow-steps 6f6cbeb
git branch pr2-config-affects 5ea536c
git branch pr4-leftovers 7f97b3a
```

Each was branched from `b43a548` (0.5.0), so each needs a rebase onto `main`. Expect small conflicts in `crates/cli/src/main.rs`, `crates/core/src/verify.rs`, `packages/cli/schemas/verify-output.schema.json` and `docs/verify.md`. Pull request 1 sets verify's JSON schema version to 7: after it, any test of another branch that asserts 6 needs 7 (`stopped-labels.test.ts` on `main` validates against the schema).

## What is left, in order

1. **Pull request 4:** rebase `7f97b3a` onto `main`, add `decisions-pr4.md` to DECISIONS.md, open the pull request, merge on green. Windows risk: three tests make a junction and rely on Rust's `is_symlink()` being true for one.
2. **Pull request 1:** get a fresh review of `6f6cbeb` (brief: check the six items of the owner's request word for word; the kept-times file `.firescale/step-times.json` under parallel writes; any path where a run with a NOT RUN step reads OK or is cached; the changed idle-limit test in `verify-run.test.ts`, which now expects `25% done` without an estimate). Fix what it finds, add `decisions-pr1.md`, pull request, merge on green.
3. **Pull request 2:** finish the two review findings, then a full test run, then a second pass by the reviewer on the fix:
   - **High, proven.** The config file left the key even for steps that read it as a file (`prettier --check .` at the repo root). Decided: the by-value rule applies only to steps whose scope does not contain the repo root. For the root's own steps and a root project the config stays an ordinary file: bytes in the key, a changed file of that unit under `--affected` and in the stop hook, and `same_on_base` treats a byte change as reaching them. The owner's case (six projects in subfolders, timeouts only) must still run 0 of 6.
   - **Low, proven.** In `configkey.rs` `shared_parts`, an unknown top-level key named `"verify.ci"` in the base commit's config overwrites the resolved value. Give the other keys their own namespace.
   - Then update ADR 0058 (drop the claim that reformatting the config changes nothing), `docs/verify.md` and `decisions-pr2.md`.
   - `SPEC.md` (the line that says the firescale config affects every project) needs the new rule too.
4. **Pull request 5:** `node scripts/bump-version.mjs 0.5.1`, no tag; `docs/verify.md` for the budget, the warning and the config rule; DECISIONS.md "what you should see change on the course project with 0.5.1" with the exact commands; AGENTS.md command list (`verify --budget`); HANDOFF.md; delete this folder.

## Decisions waiting for the owner's eye (all made the simplest way, all in the ADRs)

- `verify --budget` with `verify.failOnSkipped` on: a run that left a step out fails. Otherwise exit 0 with a PARTIAL headline.
- In the stop hook `--budget` is refused with exit 1 (the agent is not held back), not ignored.
- verify's JSON is schema version 7 (new step status `over-budget`, new required `notRun`).
- A kept step time only rises after a failure or a stop; a pass sets it.
- `protectedPaths` such as `./infra/**`, `/infra/**`, `infra/` or a bare folder name are now config errors.
- A linked `.firescale` stops `fix --agent` and `fscale ledger`, and turns every cache off.

## Files here

- `COMMON.md`: the rules every helper worked under.
- `decisions-pr1.md`, `decisions-pr2.md`, `decisions-pr4.md`: the text for DECISIONS.md, one block per pull request.
