# 0093 — The release audit, and cargo-deny

Status: accepted (2026-11-26, M40). Part of ADR 0092 (step 2 of the release pipeline). A new tool, so it has its own ADR (AGENTS.md: "Record any significant new dependency in an ADR").

## Context

Nothing in the release checked the dependencies for known vulnerabilities. The owner asked for `pnpm audit --audit-level high` and cargo-deny before the build.

- `pnpm audit` reads `pnpm-lock.yaml` and asks the npm registry's advisory service. It needs nothing installed. On `main` on 2026-11-25: one moderate advisory, none high, exit 0.
- cargo-deny (EmbarkStudios, MIT or Apache-2.0) checks a Cargo workspace for RustSec advisories, licenses, banned or duplicate crates, and sources. 0.22.1 was released on 2026-11-24. Without a `deny.toml` it rejects every license (its allow list starts empty). With the `deny.toml` below, on this workspace on 2026-11-26: `advisories ok, bans ok, licenses ok, sources ok`, no warning.
- `cargo install cargo-deny` builds it from source, several minutes on a runner. Its GitHub release has a prebuilt `x86_64-unknown-linux-musl` binary, and GitHub's API gives the asset's SHA-256 (`13f7e825…364d` for 0.22.1).

## Decision

- **A job `audit`, after `check version` and before every build job** (`build` needs it). It runs `pnpm audit --audit-level high`, then `cargo-deny check`. Either failing fails the run before any build minutes are spent, and nothing is published.
- **cargo-deny is downloaded, not built, and not through a third-party action**: `curl` fetches the 0.22.1 musl tarball from the project's GitHub release, `sha256sum --check --strict` compares it with the SHA-256 written in the workflow, and only then is it unpacked and run. A change of version is a change of two lines in `release.yml`, and a test pins both.
- **`deny.toml`**, at the root:
  - `[graph]` the six release targets and all features, so a dependency that only one platform uses is checked.
  - `[advisories]` every RustSec vulnerability fails, whatever its severity; a yanked crate fails; an unmaintained advisory fails for a direct dependency only (`unmaintained = "workspace"`).
  - `[licenses]` an allow list of exactly the licenses the tree uses today: Apache-2.0, Apache-2.0 WITH LLVM-exception, CC0-1.0, MIT, MIT-0, Unicode-3.0, Unlicense, Zlib. `r-efi` also offers LGPL-2.1-or-later; it is covered by its MIT option, so LGPL is not allowed.
  - `[bans]` a version `*` fails; two versions of one crate only warn.
  - `[sources]` crates.io only.
- **Each workspace crate says `publish = false`.** cargo-deny treats a path dependency of a crate that could be published to crates.io as a wildcard. None of them is published there: the binary ships through npm (ADR 0013).
- **Only in the release**, not in every pull request's CI: a new advisory against an old dependency should stop a release, not every unrelated pull request. Anyone can run `cargo deny check` and `pnpm audit --audit-level high` locally (AGENTS.md).

## Consequences

- A release can now fail on a vulnerability published after the last merge. The fix is a dependency update in a pull request, then a new tag; or, for an advisory that does not apply, an `ignore` entry in `deny.toml` with its reason, through review.
- A new dependency under another license fails the release until `deny.toml` allows it. That is a review point on purpose.
- The audit job fetches the RustSec database from GitHub and the advisories from npm on every release. If either is down, the release waits; "Re-run failed jobs" retries it.
- The two audits use different thresholds: npm high and critical only, Rust every severity. RustSec advisories are few for this tree, so no severity filter is set.

## What the sums prove

`SHA256SUMS` is written by the build job and uploaded in the same artifact as the tarballs. A check against it catches a tarball changed or mixed up between the build and the job that uses it. It does not catch an artifact replaced as a whole, sums and all: whoever can replace one can replace the other. The registry check of ADR 0092 (the registry's integrity against the tarball in hand) builds on it and has the same limit.

## From the fresh review

Fixed: the stages table in `RELEASING.md` was broken by this change; `release-sums.mjs check` kept the last of two lines for one name, and passed an empty `SHA256SUMS` (both now fail, with a test written first and seen failing). Stated above: what the sums prove.

## Not done

- The two cross-compiled tarballs (`darwin-x64`, `win32-arm64`) are never installed and run, and `linux-arm64` is smoked only with npm in its build job. That was so before this change; arm64 and Intel macOS runners would close it at more Actions minutes.
