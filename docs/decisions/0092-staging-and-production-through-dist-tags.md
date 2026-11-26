# 0092 — Staging and production through dist-tags

Status: accepted (2026-11-26, M40). Amends ADR 0021 (automated releases): its stages, its "no GitHub environment and no approval step", and its "no `npm install -g npm`" for the jobs that move dist-tags. Everything else in ADR 0021 stands: no npm token, no secret, `release.yml` keeps its name, only a commit on `main` is published, a run can be repeated, and **a run started by hand never calls npm to publish or to move a tag** (a rehearsal, below, uses a stand-in for npm).

## Context

Today a pushed tag publishes the eight packages straight to `latest`, and the first check that anything installs is the `verify` job after that. A bad build is then what every `npx firescale` gets until a fix is out. The owner asked for one pipeline from a tag to production on npm, with the same bytes all the way: audit, build once, smoke the tarballs before anything is public, publish to a staging channel, test it, wait for the owner's approval, then move `latest`, smoke that, and move `latest` back if the smoke fails.

npm's documentation was read on 2026-11-25 from its documentation repository (`npm/documentation` on GitHub, the source of docs.npmjs.com), as for ADR 0021:

- **Trusted publishing can run `npm dist-tag`** since 2026-09-30 (commit "Support `npm dist-tag` OIDC trusted publishing", #2040). Each trusted publisher configuration has its own **Allow npm dist-tag**, off by default and independent of `npm publish`. It needs **npm 11.21.0 or later** (or 12.2.0 or later). `npm dist-tag ls`, `add` and `rm` authenticate through OIDC; `npm whoami` "is not a check of trusted publishing permissions; verify them using the operation you intend to perform".
- **No Node release ships npm 11.21.0 yet.** From nodejs.org's release index on 2026-11-25: the newest, Node 26.13.1 (2026-11-10), ships npm 11.20.0. npm 11.21.3 was released on 2026-11-19.
- **npm's `--dry-run` is not honoured by `dist-tag`** (the `npm stage` page's option table: "This is NOT honored by other network related commands, eg `dist-tags`"). A rehearsal cannot rely on it.
- **A new trusted publisher configuration must complete a successful publish within 2 days**, or it expires and must be made again. The page does not say whether a dist-tag operation counts for a configuration that does not allow `npm publish`.
- **A configuration's environment name is optional.** The page does not say whether a configuration with no environment name accepts a token from a job that runs in an environment.
- **Staged publishing** (`npm stage publish`, npm 11.15.0 or later): a version goes to a private queue and becomes public only when a maintainer approves it with 2FA, on npmjs.com or with `npm stage approve`. `npm stage publish` works with OIDC; `npm stage list`, `view`, `approve` and `reject` "require interactive authentication and cannot use OIDC tokens". A staged version's contents "are not publicly available until a maintainer approves". Approval is one per version, except that npmjs.com **now offers batches** (up to 100 versions, one 2FA challenge; documented on 2026-11-10, "rolling out gradually and may not be available on your account yet"). The brief's "one approval per package" was true before that.

## Decision

### Staging is the public dist-tag `next`, not npm's staged publishing

|                                           | `next` plus a GitHub approval (chosen)                          | `npm stage publish`                                                                              |
| ----------------------------------------- | --------------------------------------------------------------- | ------------------------------------------------------------------------------------------------ |
| Staging tests on real installs            | Yes: `npx firescale@next` works on any runner                   | No: a staged version cannot be installed before it is approved, so step 6 has nothing to install |
| The owner's approval                      | One click on the run (environment `production`), from anywhere  | 2FA on npmjs.com, per version (8), or one batch where batches have reached the account           |
| Can the pipeline continue after approval? | Yes, in the same run                                            | No: approval is not an OIDC operation, so the run cannot see it or go on to smoke `latest`       |
| Rollback                                  | Move `latest` back; the version stays installable by its number | Same, once approved                                                                              |
| Public before approval                    | Yes, under `next` and by exact version                          | No                                                                                               |
| A rejected candidate                      | Its version number is used up; the next try is a new patch      | Rejected versions are not published                                                              |

`next` is chosen. The deciding row is the first: the brief's step 6 tests what npm will serve, and a staged version cannot be served. The cost is that a candidate is public under `next` before the owner approves it. `next` is already the channel for pre-releases (ADR 0021), so nobody gets it without asking for it, and `npx firescale` keeps giving `latest`. A candidate that fails staging uses up its version number, like any published version.

### Every publish goes to `next`; only promotion moves `latest`

- **The staging job always publishes with `--tag next`**, for a normal version too. `npm publish` without `--tag` would set `latest`; with it, `latest` is not touched. So **a failure at any step before promotion leaves `latest` where it was**, by construction and not by a check that could be skipped.
- **A pre-release** (a version with a `-` part, such as `0.6.0-rc.1`) runs steps 1 to 6 and stops: the approval, promotion and production jobs have an `if:` on it, and the promotion script refuses a pre-release.
- **A version lower than the current `latest` of `firescale` is refused by the promotion script**, as ADR 0021 left it unhandled: moving `latest` down is a rollback, which is a different command.

### The pipeline (`release.yml`)

1. **check version** (as before).
2. **audit**: `pnpm audit --audit-level high` and `cargo deny check` (ADR 0093). Before any build.
3. **build** the six platforms once (as before). Each build job writes `SHA256SUMS` beside its tarballs, before uploading them. Every later job that reads a tarball first checks it against those sums.
4. **smoke**, before anything is public: `scripts/smoke-npm.mjs` on Linux, macOS and Windows (x64 runners for Linux and Windows, Apple silicon for macOS), with npm, then pnpm, then yarn, one job per OS, from the downloaded tarballs. The build job's own npm smoke stays.
5. **staging** (environment `staging`): publish the eight packages under `next` (the order of ADR 0021), then compare, for each, the registry's `dist.integrity` for that version with the SHA-512 of the tarball in hand, and check that `next` is that version.
6. **staging tests**: on Linux, macOS and Windows, `npx firescale@next` in a new empty folder with an empty npm cache: `--version` prints the version (retried while the registry catches up), then `init` (exit 0), `check` (0), `verify` (0) and `doctor` (0; in a folder made by `init`, doctor checks that the hook's `npx --yes firescale@<version>` starts, so it checks the published version once more). And `real-repos.yml`, now also a `workflow_call` workflow, with fscale installed from npm at `next` instead of built: its steps stay observations, plus one gate per repo that fails when the installed fscale is not the version, or when any fscale command exits with a code fscale never uses (anything but 0, 1, 2 and 130: a panic, a launcher that found no binary, a missing program).
7. **approval**: the promotion job is in the environment `production`, whose required reviewer is the owner. "Prevent self-review" is off: the owner is the only reviewer and pushes the tag.
8. **production**, in two jobs. **record** (no environment, no OIDC, after the staging tests): read the current `latest` of all eight from the registry and upload it as the artifact `promotion-record`. It refuses when any package's `latest` is already the candidate, so a record never holds the candidate as a previous `latest`. It runs once per run: "Re-run failed jobs" does not run a job that passed, so a re-run of promotion uses the first record. **promote** (environment `production`, so it waits for the owner): download the tarballs and check the sums; check the registry's integrity again for all eight; download the record and check that each package's `latest` is still the recorded one or already the candidate (anything else means `latest` moved since, and the job stops before moving anything); then **move `latest`** on each package not yet on the candidate, the platform packages first, `@firescale/sdk`, then `firescale` last, and read each back.
9. **production smoke**: `npx firescale@latest --version` prints the version, then the same `init`, `check`, `verify` and `doctor`, on Linux, macOS and Windows, a new empty npm cache for every attempt.
10. **rollback** (environment `rollback`, which has no reviewer, so it starts at once), when promotion or the production smoke failed **or was cancelled** (`always()` with the results of both jobs, so a cancelled run still rolls back): move `latest` back on every package whose `latest` is not the recorded one, and read each back; then a separate job opens an issue that names the version, what failed and the run, says to ship a patch, and suggests `npm deprecate firescale@<version> "<reason>"`. The run fails. When promotion never started (a pre-release, a failure in staging or its tests, no approval), there is no record and nothing to undo: rollback does not run.

### Dist-tags through OIDC, never silently

- **No npm token and no secret**, as in ADR 0021. The test that fails if the workflow names `secrets.` or an npm token stays.
- **`id-token: write` only on the three jobs that publish or move tags**, each in its environment: staging publish (`staging`), promotion (`production`) and rollback (`rollback`). The record job reads public data only and has no OIDC. The issue job has `issues: write` and no OIDC. A test pins all of it.
- **The jobs that move tags install npm 11.21.3** (`npm install -g npm@11.21.3`, an exact version) because no Node ships 11.21.0 yet. This amends ADR 0021's "no `npm install -g npm`" for those two jobs; the publishing job keeps the npm Node 24 ships. The promotion script refuses an npm older than 11.21.0 before it moves anything.
- **A dist-tag call that fails stops the job** with the package, the npm output and the next step ("allow npm dist-tag in that package's trusted publisher"). And because a command that exits 0 proves nothing about the registry, **every tag is read back** from the registry (`GET /-/package/<name>/dist-tags`, public, no authentication) after it is moved, a few times while the registry catches up, and the job fails unless it is the expected version.
- **Rollback only moves dist-tags. Nothing is ever unpublished.**

### The trusted publisher configuration

The owner turns on **Allow npm dist-tag** on the configuration each package already has (repository `ihsankoolz/firescale`, workflow `release.yml`, no environment name). That configuration was validated by earlier releases, so the 2-day expiry does not apply, and nothing has to be made new. Its environment name stays empty: the GitHub environments, with their branch rules and the required reviewer, are the gate.

Pinning configurations to environments (one for `staging` allowing `npm publish`, one for `production` allowing only `npm dist-tag`) would let npm itself refuse a tag move from any job but the approved one. It is not done now, because npm's page does not say whether a configuration that allows only dist-tags is ever validated within its 2 days. See "Needs your decision".

### The environments

Created with `gh api`: `staging` (no reviewer), `production` (required reviewer `ihsankoolz`, "prevent self-review" off) and `rollback` (no reviewer). The brief named two; the third exists because GitHub holds every job in an environment with a required reviewer until someone approves it, and a rollback that waits for the owner leaves a bad `latest` in place meanwhile. **All three accept deployments only from the branch `main` and from tags `v*`**, so a run started by hand on another branch never reaches a job that has `id-token: write`. On `main`, the jobs that publish or move tags run only for a pushed tag or a rehearsal (their `if:`), and the scripts refuse to call the real npm in a run whose event is not a tag push (`GITHUB_EVENT_NAME`, `GITHUB_REF_TYPE`).

### The rehearsal

A run started by hand with the input `rehearsal` runs the whole pipeline with **a stand-in for npm** (`scripts/fake-npm.mjs`, through `FSCALE_NPM_COMMAND`), whose registry is a JSON file passed from job to job as an artifact. It is seeded with the real `latest` of the eight packages (a public read). In a rehearsal:

- steps 1 to 4 run for real (audit, build, smoke);
- staging publishes to the stand-in, and **every registry read** (whether a version exists, its integrity, its dist-tags) asks the stand-in, not npm;
- the staging tests on `next` and `real-repos.yml` are skipped: they would install from npm, where nothing was published;
- the approval is the real `production` environment: the owner approves the rehearsal like a release;
- promotion moves `latest` in the stand-in;
- the production smoke fails on purpose (`rehearsal: forced failure`), which starts the rollback, which moves `latest` back in the stand-in and checks it;
- the issue is opened with the title `[rehearsal]` and closed at once.

**A rehearsal therefore ends as a failed run**, with the rollback and issue jobs green. That is its pass: it proves that a failed production smoke fails the run. The workflow sets `FSCALE_NPM_COMMAND` only when the run is a rehearsal, and the scripts refuse the real npm in any run that is not a tag push, so a rehearsal can never reach npm with OIDC. A rehearsal from a ref other than `main` stops at the first environment.

## Consequences

- A release is still bump, merge, tag, push; then one approval on the run. `RELEASING.md` gets the new steps, the one-time npm setting, and what each failure means.
- **The cost of a release goes up**: an audit job, three smoke jobs, three staging test jobs, `real-repos.yml` (fifteen repo jobs, three of them on macOS, up to 90 minutes each; no build, since fscale comes from npm), and three production smoke jobs, on top of ADR 0021's jobs. The owner is short on Actions minutes; `real-repos.yml`'s `only` input is passed through so a re-run can be narrowed.
- **The publish, promotion and rollback against the real npm have never run.** What is tested: the scripts against a stand-in registry and the stand-in npm; the workflow's structure; and the rehearsal, which runs every job of the real workflow except npm's side. The first tag is the real test of OIDC for dist-tags, as the first tag was for ADR 0021.
- If npm refuses a token from a job in an environment because the configuration has no environment name, the staging publish fails with `ENEEDAUTH`: nothing is public, `latest` is untouched, and the fix is to set the environment name in each configuration (`RELEASING.md`).
- A candidate that fails staging stays on npm under its version number and under `next`. The next attempt is a new version; `next` then moves to it.
- During promotion, for a few seconds, some packages' `latest` is the new version and some the old. `firescale` moves last and its dependencies are exact versions, so `npx firescale` installs either the old set or the new set, never a mix.
- A re-run of the promotion job needs a new approval (GitHub asks again for a job in an environment with a reviewer). It moves only the packages not yet on the candidate. After a rollback, re-running promotion would promote the same bad version: `RELEASING.md` says to ship a patch instead.

## Not done

(Filled in by the pull requests that build this.)

## From the fresh review

Fixed in the ADR before it was merged: the rollback was in `production` and would have waited for the owner's approval (now its own environment `rollback`, no reviewer); a re-run of promotion would have recorded the candidate as the previous `latest` (now a separate record job, run once, that refuses the candidate); a cancelled promotion or smoke did not roll back (now it does); ADR 0021's "a run started by hand publishes nothing" was not restated (now it is, and the scripts check the event); a rehearsal's registry reads outside staging were not said to use the stand-in; and rollback with no record was not covered (it does not run).

## Needs your decision

- **Pin the trusted publishers to environments.** Safer (npm itself would refuse a tag move from any other job), but a configuration that allows only `npm dist-tag` may expire before it is validated. Try it on one package after the first release through this pipeline, if you want it.
- **The cost of `real-repos.yml` on every release.** It is in, as asked. If minutes run short, it could run only for a pre-release, or only on Linux.
