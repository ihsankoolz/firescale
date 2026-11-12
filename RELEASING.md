# Releasing firescale

A release is one pushed tag. The Release workflow (`.github/workflows/release.yml`) builds the six platform tarballs, publishes all eight packages to npm, and checks from clean Linux, macOS and Windows runners that `npx firescale@<version> --version` prints the version.

Publishing uses **npm trusted publishing** (OIDC). There is no npm token anywhere: not in this repo, not in GitHub's secrets. npm accepts the publish because it comes from this repository's `release.yml`. ADR 0021 has the reasons.

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

5. Under **Allowed actions**, tick **`npm publish`**. Without it the workflow's publish is refused. (`npm stage publish` may stay ticked; the workflow does not use it.)
6. Save.

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

4. Watch the **Release** workflow in the repository's Actions tab. It runs four stages:

   | Stage          | What it does                                                                                                     |
   | -------------- | ---------------------------------------------------------------------------------------------------------------- |
   | check version  | Fails at once if the tag is not `v` plus the version in `Cargo.toml` and the package manifests.                  |
   | build          | Six jobs, one per platform. Each packs its tarballs and uploads them.                                            |
   | publish to npm | Publishes the six platform packages, then `@firescale/sdk`, then `firescale`.                                    |
   | verify         | On Linux, macOS and Windows: `npx firescale@0.1.2 --version` from an empty folder. Tries for up to five minutes. |

5. When all of it is green, the release is done. `npm view firescale version` prints the new version.

## If something goes wrong

A published version cannot be replaced, only deprecated. So the workflow is built to be **run again**, not to be undone: the publish job skips every package version that is already on the registry.

| What you see                                               | What to do                                                                                                                                                                                                                                                                                                                                                             |
| ---------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| "check version" fails: the tag does not match              | Nothing was built or published. Delete the tag (`git push origin :refs/tags/v0.1.2`, `git tag -d v0.1.2`), fix the version or the tag, and push the right tag.                                                                                                                                                                                                         |
| A build job fails                                          | Nothing was published. Open the run and choose **Re-run failed jobs**.                                                                                                                                                                                                                                                                                                 |
| Publish fails with "Unable to authenticate" or `ENEEDAUTH` | The trusted publisher of that package is missing, expired or mistyped. Fix it on npmjs.com (see above), then **Re-run failed jobs**. The packages already published are skipped.                                                                                                                                                                                       |
| Publish fails with `E404` on the `PUT`                     | npm hides a trusted publisher that does not match as "not found". The usual cause is a wrong **workflow filename** in that package's trusted publisher (it must be exactly `release.yml`). Fix it on npmjs.com, then **Re-run failed jobs**. The packages already published are skipped.                                                                               |
| Publish fails partway for any other reason                 | **Re-run failed jobs**. The log lists `skip` for what is already there and `publish` for the rest.                                                                                                                                                                                                                                                                     |
| Verify fails, publish was green                            | npm can take a few minutes to serve a new version. **Re-run failed jobs** after a while. If it still fails, run `npx firescale@0.1.2 --version` yourself and read the error.                                                                                                                                                                                           |
| The workflow or a script has a bug                         | **Re-run failed jobs** runs the same commit again, so it cannot pick up a fix. Merge the fix to `main` without changing the version, delete the tag (`git push origin :refs/tags/v0.1.2`, `git tag -d v0.1.2`), tag the new commit on `main` with the same version and push the tag. What is already published is skipped. This is the only case where a tag is moved. |
| "check version" fails: the commit is not on main           | The tag was put on a branch commit. Merge the branch, delete the tag, and tag the commit on `main`.                                                                                                                                                                                                                                                                    |
| The release itself is bad                                  | Deprecate it and release a fix: `npm deprecate firescale@0.1.2 "use 0.1.3"`, then start again from step 1 with the next version. Do not reuse a version number.                                                                                                                                                                                                        |

Do not publish by hand to finish a release that the workflow started, unless the workflow cannot be made to work. If you must, the old manual steps are under "Publishing to npm" in `DECISIONS.md`.

## Pre-releases and older lines

- A version with a pre-release part, such as `1.0.0-rc.1`, is published under the npm dist-tag `next`, not `latest`. `npx firescale` keeps giving the last normal release; `npx firescale@next` gives the pre-release.
- A version lower than the current `latest` (a fix for an older line, such as 0.1.3 after 0.2.0) is refused by npm without a dist-tag. The workflow does not handle that case. Ask for it to be added before you need it.

## Reading a failed publish

npm prints little when the OIDC exchange fails: usually just `ENEEDAUTH`, and for a wrong workflow filename an `E404` on the `PUT`. It nearly always means that package's trusted publisher is missing, mistyped, expired, or does not allow `npm publish`. The log line before the error names the package.

## Building without publishing

Start the **Release** workflow by hand from the Actions tab (**Run workflow**). A run started this way builds the six sets of tarballs and uploads them as artifacts. It publishes nothing: the publish job only exists for a pushed tag.

To see what a publish would do without doing it, download the run's artifacts so that each one has its own subfolder (`npm-linux-x64`, `npm-darwin-arm64` and so on), which is what the GitHub CLI does:

```bash
gh run download <run id> --dir artifacts
```

Then run:

```bash
node scripts/publish-npm.mjs --dir artifacts --version 0.1.2 --dry-run
```

## What the workflow needs, and why

From npm's trusted publishing documentation, read on 2026-11-12:

- npm 11.5.1 or later and Node 22.14.0 or later. The publish job uses Node 24, which ships a new enough npm, and the publish script checks both before it publishes.
- The permission `id-token: write` on the publish job. No other job has it.
- A GitHub-hosted runner. Self-hosted runners are not supported.
- The `repository.url` of each package must be this GitHub repository. The pack script takes it from `packages/cli/package.json`.
- Provenance statements are added automatically, because the repository and the packages are public.
- npm validates the workflow that the run started from. That is why the publish job is in `release.yml` itself and not in a workflow that `release.yml` calls.
