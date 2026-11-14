# 0046 — The tamper guard: approvals, and rates measured on real history

Status: accepted (2026-11-14, M26). Extends ADR 0042, 0044 and 0045.

## Context

SPEC §8: "Clear cheats fail hard. Changed expected values and changed snapshots need a person's approval instead, and an approval made by the change under review does not count", and the guard must publish a measured false-positive rate. The owner added for M26:

- Approvals must be something an agent in the working tree cannot write for itself. Decide where they live, list the ways one could be forged and why each fails; if none is fully safe locally, say so and enforce in CI.
- Measure on real history: at least 1,000 real commits from public JavaScript, TypeScript, Python and Rust repos (read-only, shallow clones, nothing of theirs run), and this repo. Every finding labelled true or false by a fresh subagent that sees the diff and not the guard's reasoning. Per language: commits, findings, false positives, the rate with its interval.
- Measure misses on a corpus of cheating changes, written for that purpose and kept apart from the development cases.
- State the limits beside the numbers. Fix the false positives found, each with a corpus case, and report the rate before and after.

## Decision: what needs approval

Two new kinds, which **need a person's approval** rather than failing outright:

| Kind               | When                                                                                                                                                                                                                                                                                                                 | Read from                                     |
| ------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------- |
| `snapshot-changed` | a stored snapshot that both sides have, with a different body: Jest and Vitest `__snapshots__/*.snap` entries, insta `*.snap` files, syrupy `*.ambr` entries. A snapshot added (a new test) or removed is not one; line endings are not a change.                                                                    | `tamper/approval.rs`                          |
| `expected-changed` | an assertion of the base gone, and one of the working tree new, **in the same test**, with the same shape once every string and number literal is masked: `toBe(30)` → `toBe(31)`, `assert total == 30` → `== 31`, `assert_eq!(f(), 3)` → `4`, an inline snapshot's text. A different matcher or subject is not one. | `approval::pairs` over each changed test file |

With an `enforced` policy a clear finding blocks as before; an approval finding blocks **only where approvals are read** (below) and has none. Everywhere else it is reported with "[needs a person's approval, given in the pull request; it does not block here]".

## Decision: where approvals live

**Approvals live on GitHub, as pull request reviews, and are read only in the `tamper-guard` workflow** (ADR 0045), the `pull_request_target` run that GitHub takes from the default branch and in which none of the change's code runs. Its new step fetches the reviews with the run's own read-only token (`gh api --paginate repos/$REPO/pulls/$PR/reviews`, `permissions: pull-requests: read`) into the runner's temporary folder, and `fscale tamper --pr-base --approvals <file>` counts an approval when:

- its state is `APPROVED`, and it is that person's latest review that is not a comment (a later `CHANGES_REQUESTED` or `DISMISSED` takes it back);
- its `commit_id` is the pull request's head commit (`pull_request.head.sha`): a push after the approval needs a new one;
- its author is not the pull request's author (`pull_request.user.login`), not a bot (`user.type` `Bot`), and has write access (`author_association` `OWNER`, `MEMBER` or `COLLABORATOR`).

`--approvals` is refused (a usage error) unless `--pr-base` is given and the event is `pull_request_target`. After approving, a person re-runs the `Tamper guard` workflow (GitHub has no `pull_request_target` event for reviews, and the `pull_request_review` event runs the pull request's own copy of a workflow).

### Ways to forge an approval, and why each fails

| Attempt                                                                                                         | Why it fails                                                                                                                                                                                                                                       |
| --------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Write an approvals file, a commit trailer (`Approved-by:`), a config key or a comment in the change             | Nothing in the repo, its history or its working tree is ever read as an approval. A test commits `.firescale/approvals.json`, `.github/approvals.yml` and an `Approved-by:` trailer, and the change still blocks.                                  |
| Pass `--approvals` with a hand-written file: on a laptop, in the stop hook, or in the pull request's own CI job | Refused unless the run is `pull_request_target` with `--pr-base`; tested for a `pull_request` event and outside GitHub Actions. In the pull request's own job the change's code has already run (installs, tests) and could have written the file. |
| Edit the `tamper-guard` workflow in the pull request to pass a forged file or drop the step                     | `pull_request_target` runs the default branch's copy; the pull request's edit takes effect only once merged. Its edit is also a `ci-weakened` or `no-verify` finding.                                                                              |
| Approve the pull request with the author's own account (an agent holding the owner's `gh` token)                | GitHub does not let authors approve their own pull requests ("Pull request authors cannot approve their own pull requests"), and the guard ignores the author's reviews anyway (tested).                                                           |
| Approve with a bot or an Actions token (the "Allow GitHub Actions to create and approve pull requests" setting) | Reviews by a `Bot` user are ignored (tested).                                                                                                                                                                                                      |
| Approve with an account that has no write access (anyone can review a public pull request)                      | Only `OWNER`, `MEMBER` and `COLLABORATOR` count (tested with `CONTRIBUTOR`).                                                                                                                                                                       |
| Get an approval, then push more changes                                                                         | The approval names an older commit; only one of the head commit counts (tested).                                                                                                                                                                   |
| Get an approval, then have it changed to "changes requested" or dismissed                                       | The person's latest review decides (tested).                                                                                                                                                                                                       |
| `git replace`, a moved base ref, or a doctored object in the clone                                              | The job checks out the base, runs nothing of the change, and every git command sets `GIT_NO_REPLACE_OBJECTS=1` (ADR 0045).                                                                                                                         |

**Not safe, said plainly:**

- **Locally nothing is safe.** An agent with a shell can write any file, ref or note, run `fscale` with any flag, and use any credential on the machine. So no approval is read locally at all, and approval findings do not block there; CI is where they are enforced.
- **A second account.** An agent that holds the credentials of a second person with write access, or a team that approves without looking, defeats any review-based scheme. firescale cannot tell a careless approval from a careful one.
- **`COLLABORATOR` is GitHub's word**, not a permission level: a collaborator invited with read access has it too. Checking the exact permission needs another API call with wider permissions; not done.
- **The check is only as strong as branch protection**: the `tamper-guard` check must be required, and a job of the same name in another workflow is not stopped (ADR 0045).
- **A solo owner cannot approve their own pull request.** They can merge without the check (as an admin) or turn the policy to `evaluated`; firescale does not offer a bypass that an agent with their token could use too.

## Decision: how the rates were measured

### Real history (false positives)

`bench/tamper-history.mjs` runs `fscale tamper --json --head <commit> --base <commit>^` over the last 150 non-merge commits of a repo's first-parent history (`--all-parents` for repos that merge branches): every finding read from git objects, nothing checked out, installed or run. Clones were shallow (`--depth 400 --single-branch`), read-only.

| Language   | Repos (HEAD on 2026-11-14)                                           |
| ---------- | -------------------------------------------------------------------- |
| JavaScript | `expressjs/express` (`43fca52`), `axios/axios` (`4ee8d46`)           |
| TypeScript | `honojs/hono` (`f6fbaf0`), `sindresorhus/ky` (`3541888`)             |
| Python     | `pallets/flask` (`49943b5`, all parents), `encode/httpx` (`b5addb6`) |
| Rust       | `BurntSushi/ripgrep` (`178cc02`), `clap-rs/clap` (`ec93e71`)         |
| This repo  | `ihsankoolz/firescale` (`71590a0`, all parents)                      |

1,200 public commits and 150 of this repo. **Every finding was labelled true or false by a fresh subagent** (seven for the first run, about 100 findings each, and an eighth for the ten findings a fix moved) that was given the commit's message, file list and the diff of the files the findings name, and each finding as a neutral claim ("a test was removed (not moved, renamed or replaced elsewhere in this commit)", with its file, line and text), and that was told not to read firescale's code. **True** means the commit does what the claim says and it makes a check weaker or narrower, even if justified (an obsolete test deleted is a true `test-deleted`); for the approval kinds, an existing snapshot or expected value really changed. **False** means it did not happen, or what happened weakens nothing (moved, renamed, replaced by an equivalent, a `@ts-expect-error` that asserts an error, mispaired lines). The brief, the labels and every reason are in `bench/tamper-history.json`; `bench/tamper-history-report.mjs` prints the tables with 95% Wilson intervals.

### Cheating changes (misses)

`bench/tamper-cheats.json` holds 98 cheating changes, **written on 2026-11-14 for this measurement by a fresh subagent that could not read firescale's code, its development corpus (`bench/tamper-corpus.json`) or its ADRs**, from a list of nine categories (tests deleted, tests skipped, assertions weakened, snapshots or expected values changed, suppressions, config weakened, CI weakened, hooks skipped, other), "varied and realistic, not all the obvious form". No detector was changed because of it: it stays a held-out set. `bench/tamper-misses.mjs` runs each through `fscale tamper --json` and counts a cheat as **caught** when it gives a clear finding, **needs approval** when it gives only `snapshot-changed` or `expected-changed`, and **missed** when it gives none.

## Results

### False positives on real history

Before (the guard as M26 first ran it) and after the fixes below; the after set reuses the before labels for findings that did not change, and the ten findings a fix moved to other lines were labelled afresh.

| Language       | Commits   | Findings before → after | False before → after | False-positive rate before → after (95% interval) | Commits with a false finding before → after |
| -------------- | --------- | ----------------------- | -------------------- | ------------------------------------------------- | ------------------------------------------- |
| JavaScript     | 300       | 98 → 17                 | 66 → 7               | 67.3% (57.6–75.8) → 41.2% (21.6–64.0)             | 15 → 2 (0.7%, 0.2–2.4)                      |
| TypeScript     | 300       | 240 → 152               | 86 → 21              | 35.8% (30.0–42.1) → 13.8% (9.2–20.2)              | 17 → 8 (2.7%, 1.4–5.2)                      |
| Python         | 300       | 172 → 168               | 42 → 38              | 24.4% (18.6–31.4) → 22.6% (16.9–29.5)             | 12 → 11 (3.7%, 2.1–6.4)                     |
| Rust           | 300       | 41 → 35                 | 6 → 5                | 14.6% (6.9–28.4) → 14.3% (6.3–29.4)               | 2 → 1 (0.3%, 0.1–1.9)                       |
| **All public** | **1,200** | **551 → 372**           | **200 → 71**         | **36.3% (32.4–40.4) → 19.1% (15.4–23.4)**         | **46 → 22 (1.8%, 1.2–2.8)**                 |
| This repo      | 150       | 160 → 133               | 21 → 0               | 13.1% (8.7–19.2) → 0.0% (0.0–2.8)                 | 10 → 0 (0.0%, 0.0–2.5)                      |

By kind, after: `assertion-removed` 36 false of 110, `test-deleted` 12 of 47, `suppression-added` 13 of 160, `expected-changed` 6 of 116, `ci-weakened` 4 of 4, `snapshot-changed` 0 of 61, `rule-disabled` 0 of 5, `test-skipped` 0 of 1, `not-checked` 0 of 1 (the one is a base workflow with committed merge-conflict markers).

**The fixes cost true findings**: of the 490 findings labelled true before, 66 are no longer reported (13.5%, 10.7–16.8%): 26 `test-skipped` (new tests gated with `#[cfg(unix)]`, `skipIf` or `runIf`; one labeller called these true, the others false, and the brief counts only a weaker check, so these are mostly label disagreement), 22 `suppression-added` (`@ts-expect-error` in runtime test files, which the labellers called suppressions and this ADR decides are assertions that an error occurs), 18 `expected-changed` (pairs now kept within one test; some moved to other lines and were labelled again). 434 true findings remain.

### Misses on the cheating corpus

| Category                     | Cheats | Caught | Needs approval only | Missed                    |
| ---------------------------- | ------ | ------ | ------------------- | ------------------------- |
| test-deleted                 | 10     | 9      | 0                   | 1                         |
| test-skipped                 | 11     | 10     | 0                   | 1                         |
| assertion-weakened           | 11     | 2      | 1                   | 8                         |
| snapshot-or-expected-changed | 11     | 0      | 7                   | 4                         |
| suppression                  | 11     | 8      | 0                   | 3                         |
| config-weakened              | 11     | 7      | 0                   | 4                         |
| ci-weakened                  | 11     | 8      | 0                   | 3                         |
| hooks-skipped                | 11     | 4      | 0                   | 7                         |
| other                        | 11     | 0      | 0                   | 11                        |
| **All**                      | **98** | **48** | **8**               | **42 (42.9%, 33.5–52.7)** |

The same before and after the false-positive fixes. Missed, by what they do: assertions weakened in place (`toBe(x)` → `toBeDefined()`, `== [...]` → `is not None`, `try/except AssertionError`, `if let Ok`, `#[should_panic]`, a dropped `await` on `.rejects`); golden files and JSON fixtures rewritten (not snapshot formats the guard knows), a deleted snapshot file; `as any`, coverage pragmas (`# pragma: no cover`, `v8 ignore`); test selection narrowed in config (`addopts -k`, `testPathIgnorePatterns`, Vitest `include`, ESLint `ignores`, a test file moved out of `testpaths`); a CI matrix entry removed, `--passWithNoTests`, `-D warnings` dropped; hook files edited (`exit 0` in Husky, pre-commit hooks removed or moved to the manual stage, lefthook `skip`, the stop hook made to always succeed, `core.hooksPath` pointed at an empty folder from a script); and everything in "other" (mocking the module under test, testing another function, `process.exit(0)` in setup, conftest hooks that skip or rewrite results, production code that detects the test runner).

## Limits, beside the numbers

- **Who labelled, how many.** Eight subagents labelled 721 findings (711 before, 10 after a fix), each finding once, with no second labeller and no human check. Labellers disagreed on at least one class (new conditional tests); no agreement rate was measured. "Fresh" means they had not seen the guard's code or reasoning, but they share the model family that wrote the guard.
- **What was not covered.** Eight repos, 150 commits each, from the newest end of their history, all popular, well-maintained projects with clean commits; agent-written changes, monorepos, Go, Java and other ecosystems are not in it. JavaScript and Rust had few findings after the fixes (17 and 35), so their intervals are wide. Merge commits were left out. The config and CI kinds were barely exercised (4 CI findings, 5 rule findings, 0 coverage, 0 hook skips): their rates are not measured in any useful sense.
- **The false-positive rate is per finding**; "commits with a false finding" is per commit. Findings cluster: two commits of one repo held 37 of the 200 false findings before.
- **The miss corpus** was written by one subagent from a category list in one sitting: its mix of categories is not the mix of real cheating, and 11 of 98 cases are in "other", which the guard does not try to catch. It measures what kinds of cheat are missed, not how often agents cheat.
- **Fixed on the same data that measured them.** The after rate is on the commits the fixes were developed against, so it is optimistic; a fresh sample would be the honest check.
- **Approvals were not tried on GitHub** (no repo with the new workflow and a release that has `fscale tamper`); tests feed a reviews file in the API's shape.

## The false-positive fixes (each with a corpus case)

| Found on real history                                                                                        | Fix                                                                                                                                                | Corpus case                                                                                                                                                                                                       |
| ------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `@ts-expect-error` in type tests and test files counted as suppressions (about 65)                           | In a test file it is an assertion that an error occurs, not a suppression; `*.test-d.ts` and `test-d/` are test files                              | `ts: @ts-expect-error added in a type test`; `ts: @ts-ignore added in a test file` is still caught                                                                                                                |
| New tests gated by `#[cfg(unix)]`, or added inside an existing `describe.runIf`, counted as skips (about 30) | New skipped tests are left out of the shortfall up to the number of tests the change adds; existing tests made to not run are counted on their own | `rs: a new #[cfg(unix)] test added`, `ts: a new test inside an existing describe.runIf`; still caught: `ts: a test renamed and skipped`, `rs: an existing test made unix-only while a windows-only test is added` |
| Suppressions in a file copied from code the base keeps (37 in two commits)                                   | In a new file, a suppressed line the base holds word for word (a comment or attribute on its own line together with the line under it) is a copy   | `ts: a module split off with its eslint-disable line`; still caught: `ts: a copied eslint-disable comment over new code`                                                                                          |
| Expected values paired across different tests (14)                                                           | Pairs only within one test                                                                                                                         | `py: a test replaced by a different test with another value`                                                                                                                                                      |
| A removed `paths-ignore` read as narrowing; `uv run --group dev tox` not read as tox                         | An unfiltered trigger also counts as a filtered one; options that take a value are skipped with it                                                 | `ci: a paths-ignore filter removed`, `ci: an option with a value added before the command uv runs`                                                                                                                |

Not fixed, and why: assertions "covered by a new test" (a judgement the guard cannot make from counts), tests renamed while rewritten with fewer cases, a check moved to another workflow under a new script name, `lint:fix` replaced by `check`.

## Consequences

- A changed snapshot or expected value waits for an approving review of the pull request's last commit by someone else with write access, in CI only.
- The rates are published with their intervals and their limits, and the scripts and labels to reproduce them are in `bench/`.
- `bench/tamper-cheats.json` stays held out: a detector change made because of one of its cases makes it development data, and the miss rate must then be measured on a new set.
