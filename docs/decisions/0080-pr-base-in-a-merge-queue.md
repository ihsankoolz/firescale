# 0080 — `--pr-base` in a merge queue run reads the merge_group event

Status: accepted (2026-11-18, Track A of the run after 0.5.4; the owner's brief). Adds to ADR 0045 (`--pr-base`).

## Context

`docs/research/research-build-tools.md`, idea 9, first step. GitHub's merge queue builds a temporary branch for each group of pull requests and sends a `merge_group` event. A workflow that runs `fscale verify --affected --pr-base` for that event failed: `--pr-base` knew only `pull_request` and `pull_request_target`. A repo that turned the queue on had to drop `--affected`, or write its own base.

## Decision

**In a GitHub Actions run for a `merge_group` event, `verify --pr-base` takes the base from the event's payload: `merge_group.base_sha`.** That is the commit the group is built on: the base branch, with the groups ahead of this one in the queue.

- **Never from a ref.** The payload also has `base_ref` and `head_ref`. They are not read. A step of the job can move a ref, and the queue's own branches (`refs/heads/gh-readonly-queue/...`) need not be in the clone.
- The commit to check is `merge_group.head_sha`. `GITHUB_SHA` must be that commit, and so must the checkout.
- Both must be full commit ids that are in the clone, and the base must be part of the head's history and not the head itself. Otherwise the run fails with exit code 2 and says which. It never goes on with no base.
- A `pull_request` or `pull_request_target` run is as it was.
- **`tamper --pr-base` still refuses a `merge_group` event.** The guard needs the pull request, its author and its approvals. That event carries none of them.

`merge_group` and `is_merge_group` in `crates/core/src/tamper/github.rs`; `with_pr_base` in `crates/cli/src/main.rs`. The payload's field names are from GitHub's changelog for the event and its API model (`base_sha`: "the SHA of the merge group's parent commit"; `head_sha`: the commit to validate).

## Consequences

- A workflow with `on: merge_group` can run `fscale verify --affected --pr-base` as it does for a pull request. Blame-aware verify compares with the queue's base.
- The shared cache is read and not written in such a run, as for a pull request (it writes only on a push to the default branch).

## Needs your decision

Nothing blocks. Decided on your behalf: tamper stays refused for this event; the base is the group's parent, not the base branch's tip without the groups ahead.

## Not done

- No run in a real merge queue. The test uses a fixture event file with the documented fields.
- The starter workflow `fscale adopt` writes has no `merge_group` trigger.
- `docs/ci.md` does not show a merge queue job.

## Later

- `merge_group` in the starter workflow, behind a rule option.
- The tamper guard for a merge group: each pull request in the group judged by its own event.

## Tests

`packages/cli/src/pr-base-merge-group.test.ts`, written first and seen failing in WSL: 4 of 5 (`--pr-base needs a pull_request or pull_request_target event`). The fifth passed before and after: tamper refuses the event.

- The base is the payload's, and the plan runs only what the group's commit changed.
- The branches the event names are missing or at another commit: the same base.
- Refused: no `base_sha`, a `base_sha` that is a branch name, the base and head swapped, `GITHUB_SHA` at another commit, the checkout at another commit.
