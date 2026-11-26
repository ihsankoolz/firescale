# Releasing firescale

A release is one pushed tag and one approval. The Release workflow (`.github/workflows/release.yml`) audits the dependencies, builds the six platform tarballs once, smokes them, publishes all eight packages under the dist-tag `next` (staging), tests what npm serves there, waits for your approval, moves `latest` to the new version (production), smokes `latest`, and moves `latest` back if that smoke fails. The same bytes go all the way: every job checks the tarballs against the build's SHA-256 sums, and the registry's integrity against the tarballs. ADR 0092 has the design.

Publishing and moving dist-tags use **npm trusted publishing** (OIDC). There is no npm token anywhere: not in this repo, not in GitHub's secrets. npm accepts them because they come from this repository's `release.yml`. ADR 0021 and ADR 0092 have the reasons.

The eight packages:

| Package                       | What it is                         |
| ----------------------------- | ---------------------------------- |
| `@firescale/cli-linux-x64`    | the binary for Linux x64           |
| `@firescale/cli-linux-arm64`  | the binary for Linux arm64         |
| `@firescale/cli-darwin-x64`   | the binary for macOS Intel         |
| `@firescale/cli-darwin-arm64` | the binary for macOS Apple silicon |
| `@firescale/cli-win32-x64`    | the binary for Windows x64         |
| `@firescale/cli-win32-arm64`  | the binary for Windows arm64       |
| `@firescale/sdk`              | the rule SDK                       |
| `firescale`                   | the launcher and Node host         |

## Once: set up the trusted publishers

Do this for **each of the eight packages**. It takes about a minute per package.

1. Sign in at [npmjs.com](https://www.npmjs.com) as the owner of the packages.
2. Open the package's page (for example `https://www.npmjs.com/package/firescale` or `https://www.npmjs.com/package/@firescale/sdk`) and go to its **Settings** tab.
3. Find the **Trusted Publisher** section and choose **GitHub Actions**.
4. Fill in the fields. All of them are case-sensitive.

   | Field                | Value         |
   | -------------------- | ------------- |
   | Organization or user | `ihsankoolz`  |
   | Repository           | `firescale`   |
   | Workflow filename    | `release.yml` |
   | Environment name     | leave empty   |

5. Under **Allowed actions**, tick **`npm publish`** and **Allow npm dist-tag**. Without the first the staging publish is refused; without the second promotion and rollback are refused (dist-tag access is off by default and independent of `npm publish`). (`npm stage publish` is always allowed; the workflow does not use it.)
6. Save.

If the eight trusted publishers already exist (from an earlier release), edit each one instead: tick **Allow npm dist-tag** and save. An existing, validated entry does not expire.

Things to know:

- **npm does not check the fields when you save.** A typo shows up only when the workflow tries to publish: as an "Unable to authenticate" (`ENEEDAUTH`) error, or, for a wrong workflow filename, as an `E404 Not Found` on the `PUT` of the package.
- **A new trusted publisher must be used within 2 days.** If no publish succeeds in that time the entry expires: delete it and add it again. So set the eight up shortly before you push the tag.
- The workflow filename is only the file's name, `release.yml`, not its path. If the file is ever renamed, every package's entry must be made again.
- All eight packages already exist on npm (0.1.0 was published by hand), so each has a settings page.

After the first release through the workflow has worked, npm recommends closing the other door: for each package, **Settings**, **Publishing access**, **Require two-factor authentication and disallow tokens**. Trusted publishing keeps working with that setting.

## For each release

1. Pick the version, for example `0.1.2`, and set it everywhere it is recorded:

   ```bash
   node scripts/bump-version.mjs 0.1.2
   ```

   That writes `Cargo.toml`, `Cargo.lock` and every `package.json`.

2. Check, commit on a branch, open a pull request and merge it when CI is green:

   ```bash
   node scripts/bump-version.mjs --check
   ```

   ```bash
   git checkout -b release-0.1.2
   ```

   ```bash
   git commit -am "chore: release 0.1.2"
   ```

   ```bash
   git push -u origin release-0.1.2
   ```

3. Tag the merged commit on `main` and push the tag. The tag is `v` plus the version:

   ```bash
   git checkout main
   ```

   ```bash
   git pull
   ```

   ```bash
   git tag v0.1.2
   ```

   ```bash
   git push origin v0.1.2
   ```

4. Watch the **Release** workflow in the repository's Actions tab. It runs these stages:

   | Stage            | What it does                                                                                                                                                                                |
   | ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
   | check version    | Fails at once if the tag is not `v` plus the version in `Cargo.toml` and the package manifests.                                                                                             |
   | audit            | `pnpm audit --audit-level high` and `cargo-deny check` (`deny.toml`). A finding stops the release before any build.                                                                         |
   | build            | Six jobs, one per platform. Each packs its tarballs, writes their SHA-256 sums (`SHA256SUMS`) and uploads them.                                                                             |
   | smoke            | On Linux, macOS and Windows: the tarballs, checked against their sums, installed with npm, pnpm and yarn.                                                                                   |
   | staging          | Environment `staging`: publishes the eight packages under the dist-tag `next` (never `latest`), then checks that the registry has the bytes of each tarball and that `next` is the version. |
   | staging test     | On Linux, macOS and Windows: `npx firescale@next --version` is the version, then `init`, `check`, `verify` and `doctor` in an empty folder each exit 0.                                     |
   | real repos       | `real-repos.yml` with fscale from npm under `next`: fails a repo only when fscale crashes or is not the version.                                                                            |
   | record latest    | The current `latest` of all eight, kept as the artifact `promotion-record`. Not for a pre-release.                                                                                          |
   | production       | **Waits for your approval** (environment `production`). Then checks the sums and the registry once more, and moves `latest` on all eight, `firescale` last, reading each back.              |
   | production smoke | On Linux, macOS and Windows: `npx firescale@latest --version` is the version, then `init`, `check`, `verify` and `doctor`, with a new empty npm cache.                                      |
   | rollback         | Only when promotion or the production smoke failed or was cancelled: moves `latest` back to the record on all eight (environment `rollback`, no approval), then opens an issue.             |

5. **Approve the production job.** When the staging tests and the real repos are green, the run waits at **production / move latest**: open the run, choose **Review deployments**, tick `production`, **Approve and deploy**. Until you approve, nothing moves `latest`; you can also reject, and `latest` stays where it was.

6. When all of it is green, the release is done. `npm view firescale dist-tags` shows `latest` on the new version.

A pre-release (a version such as `1.0.0-rc.1`) stops after the staging tests: it is on npm under `next` only, and never becomes `latest`.

## If something goes wrong

A published version cannot be replaced, only deprecated. So the workflow is built to be **run again**, not to be undone: the staging job skips every package version that is already on the registry. Before the production job nothing touches `latest`, so a failure there leaves `npx firescale` on the previous release.

| What you see                                                  | What to do                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| ------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| "check version" fails: the tag does not match                 | Nothing was built or published. Delete the tag (`git push origin :refs/tags/v0.1.2`, `git tag -d v0.1.2`), fix the version or the tag, and push the right tag.                                                                                                                                                                                                                                                                                                                                                                                                                         |
| The audit fails                                               | A high or critical npm advisory, or a RustSec advisory, a license or a source that `deny.toml` does not allow. Nothing was built. Update the dependency in a pull request and release a new patch; for an advisory that does not apply, an `ignore` in `deny.toml` with the reason, through review.                                                                                                                                                                                                                                                                                    |
| A smoke job fails                                             | Nothing was published. Read the failed check (`FAIL  ...`); re-run with `--keep` locally to see the project.                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| "The tarballs match their sums" fails                         | The tarballs are not the bytes the build made. Do not publish them: run the whole workflow again.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| A staging test or a real repo fails                           | The version is on npm under `next`, and `latest` was never touched. Fix the cause and release a new patch. A real repo fails only when fscale crashed or is not the version; read that job's uploaded output.                                                                                                                                                                                                                                                                                                                                                                          |
| Promotion fails with `ENEEDAUTH` or "cannot manage dist-tags" | That package's trusted publisher does not allow `npm dist-tag`: tick **Allow npm dist-tag** on npmjs.com. The rollback job has moved back what had moved. Then start a new patch release, or re-run the failed jobs of this run (promotion asks for your approval again and moves only what is not yet on the version).                                                                                                                                                                                                                                                                |
| Promotion says `latest` moved since the record                | Someone moved `latest` after this run recorded it. Nothing was moved. Find out who, then release again.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| The production smoke fails                                    | The rollback job moved `latest` back on all eight and opened an issue. Ship a patch; `npm deprecate` the bad version as the issue says. Never re-run promotion for that version.                                                                                                                                                                                                                                                                                                                                                                                                       |
| The rollback fails                                            | `latest` may still be on the bad version on some packages. Re-run the rollback job. If it keeps failing, move each back by hand while logged in to npm: `npm dist-tag add <package>@<version in the promotion-record artifact> latest`.                                                                                                                                                                                                                                                                                                                                                |
| A build job fails                                             | Nothing was published. Open the run and choose **Re-run failed jobs**.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| Publish fails with "Unable to authenticate" or `ENEEDAUTH`    | The trusted publisher of that package is missing, expired or mistyped. Fix it on npmjs.com (see above), then **Re-run failed jobs**. The packages already published are skipped.                                                                                                                                                                                                                                                                                                                                                                                                       |
| Publish fails with `E404` on the `PUT`                        | npm hides a trusted publisher that does not match as "not found". The usual cause is a wrong **workflow filename** in that package's trusted publisher (it must be exactly `release.yml`). Fix it on npmjs.com, then **Re-run failed jobs**. The packages already published are skipped.                                                                                                                                                                                                                                                                                               |
| Publish fails partway for any other reason                    | **Re-run failed jobs**. The log lists `skip` for what is already there and `publish` for the rest.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| Verify fails, publish was green                               | npm can take a few minutes to serve a new version. **Re-run failed jobs** after a while. If it still fails, run `npx firescale@0.1.2 --version` yourself and read the error.                                                                                                                                                                                                                                                                                                                                                                                                           |
| The workflow or a script has a bug                            | **Re-run failed jobs** runs the same commit again, so it cannot pick up a fix. **If nothing was published yet**, merge the fix to `main` without changing the version, delete the tag (`git push origin :refs/tags/v0.1.2`, `git tag -d v0.1.2`), tag the new commit on `main` with the same version and push the tag. **Once any package of the version is on npm, ship a new patch version instead**: the new run builds the tarballs again, their bytes can differ from what was published, and the staging check refuses a registry that does not hold the built bytes (ADR 0092). |
| "check version" fails: the commit is not on main              | The tag was put on a branch commit. Merge the branch, delete the tag, and tag the commit on `main`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| The release itself is bad                                     | Deprecate it and release a fix: `npm deprecate firescale@0.1.2 "use 0.1.3"`, then start again from step 1 with the next version. Do not reuse a version number.                                                                                                                                                                                                                                                                                                                                                                                                                        |

Do not publish by hand to finish a release that the workflow started, unless the workflow cannot be made to work. If you must, the old manual steps are under "Publishing to npm" in `DECISIONS.md`.

## Pre-releases and older lines

- A version with a pre-release part, such as `1.0.0-rc.1`, is published under the npm dist-tag `next` and tested there, and the run stops: it is never moved to `latest`. `npx firescale` keeps giving the last normal release; `npx firescale@next` gives the pre-release.
- A version lower than the current `latest` (a fix for an older line, such as 0.1.3 after 0.2.0) is published under `next` and tested, but the record job refuses to move `latest` down. Ask for older lines to be handled before you need it.

## Reading a failed publish

npm prints little when the OIDC exchange fails: usually just `ENEEDAUTH`, and for a wrong workflow filename an `E404` on the `PUT`. It nearly always means that package's trusted publisher is missing, mistyped, expired, or does not allow `npm publish`. The log line before the error names the package.

## Building without publishing

Start the **Release** workflow by hand from the Actions tab (**Run workflow**). A run started this way audits, builds the six sets of tarballs, smokes them and uploads them as artifacts. It publishes nothing: the jobs that publish or move tags run only for a pushed tag (and the scripts refuse the real npm in any other run).

To see what a publish would do without doing it, download the run's artifacts so that each one has its own subfolder (`npm-linux-x64`, `npm-darwin-arm64` and so on), which is what the GitHub CLI does:

```bash
gh run download <run id> --dir artifacts
```

Then run:

```bash
node scripts/publish-npm.mjs --dir artifacts --version 0.1.2 --dry-run
```

## A rehearsal

To see the whole pipeline work, the rollback included, without touching npm: start the **Release** workflow by hand on `main` with **rehearsal** ticked. It runs the audit, the builds and the smoke for real, then staging, promotion and rollback against a stand-in for npm (`scripts/fake-npm.mjs`) whose registry is a file passed from job to job (the artifact `rehearsal-registry`), seeded with npm's real `latest`. The staging tests and the real repos are skipped (nothing was published to install). It **waits for your approval** at the production job, like a release. The production smoke then fails on purpose, the rollback moves `latest` back in the stand-in, and an issue titled `[rehearsal]` is opened and closed at once. **A rehearsal ends as a failed run, with the rollback and issue jobs green**: that is its pass.

Rehearse a version that is not yet `latest` on npm (the seed refuses otherwise): for example right after the version pull request is merged, before the tag.

## What the workflow needs, and why

From npm's trusted publishing documentation, read on 2026-11-12:

- npm 11.5.1 or later and Node 22.14.0 or later for `npm publish`. The staging job uses Node 24, which ships a new enough npm, and the publish script checks both before it publishes.
- The permission `id-token: write` on the three jobs that publish or move tags (staging, production, rollback), each in its GitHub environment. No other job has it.
- For `npm dist-tag` through OIDC (read on 2026-11-25): npm 11.21.0 or later, which no Node ships yet, so the production and rollback jobs install npm 11.21.3; and **Allow npm dist-tag** on each trusted publisher.
- A GitHub-hosted runner. Self-hosted runners are not supported.
- The `repository.url` of each package must be this GitHub repository. The pack script takes it from `packages/cli/package.json`.
- Provenance statements are added automatically, because the repository and the packages are public.
- npm validates the workflow that the run started from. That is why the publish job is in `release.yml` itself and not in a workflow that `release.yml` calls.
