# CI

`fscale init` (through `core/ci-exists`, and `group-project/ci-verify`) writes `.github/workflows/ci.yml`. It is made to be fast on pull requests and safe to trust (ADR 0041).

## What it does

| Job                                 | Runs on                                    | Verifies                                                        | Caches                                       |
| ----------------------------------- | ------------------------------------------ | --------------------------------------------------------------- | -------------------------------------------- |
| `pull-request`                      | every pull request                         | `--affected --pr-base`: only what it reaches                    | restores; can never save                     |
| `default-branch`                    | pushes to the default branch               | everything                                                      | restores; saves from the default branch only |
| `tamper-guard` (`tamper-guard.yml`) | every pull request (`pull_request_target`) | `fscale tamper --pr-base`: what merging the change would weaken | none                                         |

- **Only what a pull request reaches.** `fscale verify --affected --pr-base` runs the projects the change can affect ([affected.md](affected.md)) and judges each failure against the base branch ([verify.md](verify.md)): only new failures block. The base is the one GitHub's event names (the merge commit's first parent), not a ref in the clone that the change's install scripts could move.
- **A tamper guard the pull request cannot change.** `.github/workflows/tamper-guard.yml` runs only on `pull_request_target`, so GitHub takes it from the default branch's copy: a pull request cannot edit or delete it, and no skipped job of the same name passes the check in another run. It checks out the base branch, fetches the pull request's commits as git objects, and runs `npx --yes firescale@<version> tamper --pr-base`, which compares what merging them would change with the base branch's tip, under the tip's policy. It never checks out, installs or runs the pull request's files, has no cache and a read-only token (ADR 0045). Mark it as a required check, and set `verify.tamper` to `"enforced"` on the default branch, to make it block. A changed snapshot or expected value blocks there until someone other than the author, with write access, approves the pull request's last commit; the workflow reads the reviews with its own read-only token (`pull-requests: read`). After approving, re-run the workflow (ADR 0046).
- **Full history.** Every job checks out with `fetch-depth: 0`, so the merge base is in the clone. In a shallow clone that lacks it, verify runs every project and says to fetch the history.
- **`--jobs 2`** runs two projects at once. If your projects share something that cannot be shared (one test database), edit the workflow.
- **A superseded run is cancelled.** A new push to a pull request cancels its run in progress. Each push to the default branch has a concurrency group of its own, so its run is never cancelled.
- **The push trigger** names your default branch (from `origin/HEAD`, or the branch of a new repo), else `main` and `master`.
- **Download caches.** npm, pnpm and pip keep their download folders in the GitHub Actions cache, keyed on the lockfile or requirements lists. Not `node_modules`.
- **Every action is pinned** to a commit, with its tag in a comment.

## The shared step cache

A lint, typecheck or test step that passed on the default branch, and whose inputs a pull request did not change, does not run again on the pull request.

- On each push to the default branch, `fscale verify --shared-cache .firescale/shared` writes one small record per passing step: its key, `passed`, the time, how long it took, a digest of its output, and who wrote it (the workflow, the ref and the commit). Never a build output. The workflow saves that folder to the GitHub Actions cache, and only when it holds something new.
- On a pull request, the job restores the folder and verify uses a record only when its key is exactly the step's key ([verify.md](verify.md#the-step-cache): the step's files and what it depends on, the lockfile, tool versions, OS, the allowed variables, and in CI the workflow file and the runner image), it says `passed`, it is under 30 days old, and it was written from the default branch.

### Why a pull request cannot plant a result

- The pull request job has `cache-mode: read`: its token cannot save any cache. It also has no save step at all, only `actions/cache/restore`, and the setup actions' own caches (which save in a post step) are off there.
- GitHub scopes a cache saved by a pull request to its merge ref; the default branch never restores it.
- firescale writes records only on a push to the default branch, and uses only records whose writer is the default branch. A run that writes never takes a pass from the machine's own step cache, so a runner kept between jobs cannot carry a pull request's pass into a record.
- The writer field is a label, not a proof: the guarantee is GitHub's scoping and the read-only token. A pull request that adds its own workflow could save into its own merge ref and fool only its own check, in plain sight in its diff.
- `--shared-cache` removes only record files, never folders or other files, and refuses the repo, a folder above it and firescale's other folders.
- `ci.yml` never uses `pull_request_target`; only `tamper-guard.yml` does, with `cache-mode: none` and no cache step.

### What it does not do

- **Laptops do not read the shared cache.** Outside GitHub Actions, `--shared-cache` does nothing but say so.
- Nothing secret is cached: anyone who can open a pull request can read a repository's caches (GitHub's docs say so). Records hold digests and refs, nothing else.

## Your workflow, or firescale's

The workflow is yours as soon as you edit it. Until then its last line, `# firescale-workflow: <digest>`, matches the rest of the file, and `fscale sync` replaces it when firescale has a newer one (`core/ci-exists` reports that with low severity). After one edit, it is never rewritten. A workflow you wrote yourself is never touched. The starter of firescale 0.1 to 0.3 counts as firescale's while it is byte for byte as written.
