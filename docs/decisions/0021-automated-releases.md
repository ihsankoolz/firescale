# 0021 — Automated releases with npm trusted publishing

Status: accepted. Replaces the manual publishing steps of ADR 0013 for every release after 0.1.0.

## Context

0.1.0 was published by hand on 2026-11-12: eight `npm publish` commands with a one-time code each, from tarballs downloaded out of a workflow run. That is slow and easy to get wrong in the order or the count. The owner asked for a tag push to do all of it, with no npm token and no secrets.

npm's trusted publishing lets a named GitHub Actions workflow publish a package with a short-lived OIDC token. Its documentation was read on 2026-11-12 from npm's documentation repository (`npm/documentation` on GitHub, the source of docs.npmjs.com, which the build session could not reach directly). What it requires:

- npm 11.5.1 or later and Node 22.14.0 or later.
- `id-token: write` on the job that publishes.
- A GitHub-hosted runner.
- Per package on npmjs.com: organization or user, repository, workflow filename, and `npm publish` among the allowed actions.
- The package's `repository.url` must match the GitHub repository.
- A new trusted publisher entry expires unless a publish succeeds within 2 days.
- With `workflow_call`, npm validates the calling workflow, not the called one.

## Decision

- **One workflow, `release.yml`, four stages:** check the version, build six platforms, publish, verify. A pushed tag `v<version>` runs all four. A run started by hand runs the first two and publishes nothing, as before.
- **No token, no secret.** The publish job has `id-token: write`; every other job has `contents: read` only. A test fails if the workflow mentions `secrets.`, an npm token variable, or gives the OIDC permission to a second job.
- **The publish job is in `release.yml` itself**, not in a reusable workflow, because npm validates the workflow the run started from.
- **Publish order: the six platform packages, then `@firescale/sdk`, then `firescale`.** `firescale` depends on the others at the exact version, so it goes last and never points at a package that is not there.
- **Safe to run again.** Before each publish the script asks the registry whether that exact version exists (`GET /<name>/<version>`: 200 means yes, 404 means no, anything else stops the job). A version that is there is skipped. A half-finished release is completed with "Re-run failed jobs". If a publish fails, the script looks once more before failing, in case another run got there first.
- **The tag must match the version.** The first job fails, before any build, unless `Cargo.toml`, `Cargo.lock` and every `package.json` agree and the tag is `v` plus that version.
- **One script sets the version everywhere:** `node scripts/bump-version.mjs <version>`. `--check` verifies, and `--check --tag` is what the workflow runs. It edits the version text in place, so formatting and line endings stay.
- **The platform-independent tarballs come from the `linux-x64` build.** Every build job packs its own copy of `@firescale/sdk` and `firescale`; one is published.
- **Verify after publishing** on Linux, macOS and Windows: `npx --yes firescale@<version> --version` in an empty temp folder with an empty npm cache, up to 20 attempts 15 seconds apart, because a new version can take a few minutes to become available.
- **Node 24 in the publish job, with the npm it ships.** No `npm install -g npm`: one less thing downloaded during a release. The script checks both minimum versions and stops with a clear message if a runner image ever falls below them.
- **No GitHub environment and no approval step.** The owner pushes the tag, which is the approval. An environment can be added later; it must then also be entered in each package's trusted publisher.
- **`actions/download-artifact` is a new action**, pinned to a commit like the others (ADR 0009).
- **0.1.1 is the first version meant for the workflow.** The version is bumped in the same pull request. The tag is the owner's to push, after setting up the trusted publishers.

## Consequences

- A release is: bump, merge, tag, push. `RELEASING.md` has the steps and the one-time setup for the eight packages.
- The workflow's publish and verify stages have **never run**. They cannot be tested without publishing. What is tested: the scripts, against a stand-in registry and a stand-in for `npx`; and the workflow file's structure. The first tag is the real test, and the workflow is built so that a failure there can be fixed and the run repeated.
- The first run also exercises the `linux-arm64` runner and the two cross-compiled targets (`darwin-x64`, `win32-arm64`), unless a manual run did so before.
- Renaming `release.yml`, or moving the repository, breaks publishing until the eight trusted publisher entries are made again.
- The registry check and the publish are two steps. Two runs for the same tag cannot overlap (the workflow's concurrency group), and a publish of an existing version is refused by npm anyway.
- A release costs about six build jobs, one publish job and three verify jobs of Actions time, the macOS and Windows ones among them.
