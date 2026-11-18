# 0075 — On a cache miss, verify says which files differ from the step's last pass

Status: accepted (2026-11-18, Track A of the run after 0.5.4; the owner's brief). Adds to ADR 0036 (the step cache) and ADR 0043 (what verify says about the cache without being asked).

## Context

`docs/research/research-build-tools.md`, idea 3, first step. `FSCALE_LOG=cache` printed `hit`, `miss`, `not cached`, `stored` and `not stored`. A miss gave no cause. A person who wonders why a 4 minute test step ran again, or an agent that cannot tell a useful rerun from a cache that never hits, had nothing to go on.

## Decision

**Beside each stored pass, verify keeps a small manifest: one digest per node of the project graph whose files the step's key covers, and one for the covered files that belong to no project.** On a miss, it compares the manifest of the key it just made with the one of the step's last pass, and names the nodes that differ.

- **Where it is kept:** `.firescale/steps/last-<id>.json`, where the id is a hash of the step, its project folder and its kind. One file per step, replaced each time a pass of that step is stored. It is written like a pass: to a temporary file first, never through a link. The cache's cleaning counts and removes it like a pass.
- **What it holds:** a format number, when the pass ran, and the digests (16 hex characters each) by name. The names are `files in <project or shared folder>`, the path of a file that is a shared node by itself (`docker-compose.yml`), and `files that belong to no project`. **Digests only:** no content, no file's path beyond a node's name, no value of a variable or of an env file.
- **Under `FSCALE_LOG=cache`**, one line per step that was looked up and not found:
  - `cache: why test in backend/auth_service ran again: these differ from its last pass (2026-11-18 10:00 UTC): files in backend/auth_service; files in backend/shared`
  - `cache: why lint ran: no pass of it is kept to compare with`
  - `cache: why test ran again: no file it reads differs from its last pass (…), so something else in its key does (the command, a tool, what is installed, a variable, an env file or a setting), or that pass is no longer kept`
  - At most six names, then `; and N more`.
- **In the normal output, one line** for a step that took one minute or more and was not served from the cache: `Not from the cache: test in backend/auth_service took 1m 23s and ran again: these differ from its last pass (…): files in backend/shared.` A quicker step says nothing there. The line is not in `--json`.
- With `--no-cache` nothing is looked up, so nothing is said.
- **It never decides a hit.** The key is made as before; the manifest is only read after a miss. No cache input is removed or added.

## Consequences

- One more small file per step in `.firescale/steps/`, and one read of it per miss.
- The comparison is with the step's **last pass**, not with the entry the key would have matched. After switching between two branches, the names are what differs from the branch that passed last.
- A step that is not cached at all (`cache: not cached …`) already says why and gets no such line.

## Not done

- A pass taken from the shared cache of CI writes no manifest on this machine.
- The names are not counted: `files in backend/shared`, not "2 files".
- The line in the normal output is tested through the summary's printer with a made-up 83 second step, not with a real step that takes a minute.

## Later

- The other parts of the key, each with its own digest and name: the command, each tool's version, the installed dependencies, each allowlisted variable, the env files, the config's settings, the CI workflow file. The idea's own example is "NODE_OPTIONS changed". The first version of this change did that and was cut back to the first step the brief names; the `why_text` sentence already says "something else in its key" for those.
- A count of the files that differ per node.
- The reason in `--json`, with a schema change.

## Tests

- `packages/cli/src/step-cache-why.test.ts`, written first and seen failing in WSL before the change: 6 of 7 failed (no `cache: why` line, no `last-*.json`). The seventh is a guard: `--no-cache` says nothing.
- `a_miss_names_the_parts_that_differ_and_at_most_six_of_them` and `the_manifest_of_a_key_has_a_digest_per_folder_of_the_graph_and_no_path` (`crates/core/src/stepcache.rs`), and `a_step_over_a_minute_that_ran_again_says_why_in_one_line` (`crates/core/src/verify.rs`), with the change.
