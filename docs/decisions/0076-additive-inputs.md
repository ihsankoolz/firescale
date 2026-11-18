# 0076 — A project can name more inputs of its steps' keys, and can only add

Status: accepted (2026-11-18, Track A of the run after 0.5.4; the owner's brief). Adds to ADR 0036 (what a step's key holds) and ADR 0058 (what a config change affects).

## Context

`docs/research/research-build-tools.md`, idea 2, first step. What a step's key covers is worked out, not declared: the project's folder, what it depends on in the graph, the files in the folders above, and the files that belong to no project. There was no way to say "this step also reads the ignored `dist/`" or "this step's result depends on `API_URL`". After such a thing changed, the step was served from the cache.

Nx and Turborepo let a task declare inputs, and both also let it take inputs away, which is how their configurations go stale.

## Decision

**`verify.projects.<folder>.inputs` is a list of more inputs for the key of every step of that project. `verify.projects.<folder>.<step>.inputs` is the same for one step.** Two kinds of entry, and no other:

```json
{
  "verify": {
    "projects": {
      "backend/api": {
        "inputs": [{ "ignored": "generated/**" }],
        "test": { "inputs": [{ "env": "API_URL" }] }
      }
    }
  }
}
```

- **`{ "ignored": "<glob>" }`**: the bytes of every file the glob names under the project's folder, also a file git ignores. `*` stays in one folder, `**` crosses folders. The glob is relative to the project's folder and cannot leave it: no `..`, no `/` in front, no `\`. A glob that names a folder names every file in it (`dist` is all of `dist`). A link counts by where it points and is not followed, also a link among the folders on the way to the files. `.git` is never entered.
- **`{ "env": "NAME" }`**: the value of that environment variable as verify sees it, as a hash, or that it is not set. Set to nothing and not set are different.
- **They can only add.** There is no `exclude`, no `!` in front of a glob, no plain glob that would replace the worked-out set. Anything that is not one of the two kinds is an error, and the error says: `Inputs can only add to what a step's key holds`.
- **No value is printed, logged or stored.** A variable's value and a file's bytes are hashed; the hash goes into the key's hash.
- A step with no named input has the same key as before this change.
- A pass is not stored when a named file changed while the step ran, as for the step's other files (ADR 0039). So a step that **writes** into a folder its own glob names is never cached, and the summary says so. Name the inputs on the steps that read them.
- A step is not cached, with the reason in the summary, when one glob names more than 5,000 files, when the folder it starts in holds more than 50,000 files and folders, or when a named file cannot be read.
- **A change to a project's `inputs` in the config** reaches the steps of that project that have a command, and no other project, like a change to their commands (ADR 0058). Under `--affected` those steps run.
- `inputs` under `verify.<step>` at the top level is an error: the repo root's own steps already cover the whole repo.
- **Inputs for a folder verify runs no project in are an error**, not a setting that does nothing: a folder above the project (`backend` for `backend/api`), or the wrong letter case.

Schema: `VerifyInput`, `VerifyProjectStepSetting` and `inputs` on `VerifyProjectSettings` in `packages/sdk/src/types.ts`. Docs: `docs/verify.md` ("Config", "The step cache", "What a config change affects") and `docs/config.md`.

## Consequences

- The named files are read and hashed each time a key of the step is made; nothing of them is reused between runs as the tree's snapshot reuses a file's hash. A glob over a large build output slows every verify of that project.
- A variable that changes on every run (a timestamp, a per-session token) named as an input means the step never hits. The cache log shows the miss; it does not yet name the variable (ADR 0075, "Later").

## Not done

- `--affected` and the stop hook do not see a change to a named ignored file or to a named variable: they look at what git says changed. A project whose only change is in its ignored `dist/` is not affected. This is as before for every ignored file; the key is right, so a full `fscale verify` runs the step.
- The base run of blame-aware verify gets no copy of the named ignored files in its worktree.
- The "why a step ran again" note (ADR 0075) does not name a changed input.
- **A kept base result** of blame-aware verify is not keyed on the named inputs. A base failure kept while `API_URL` was wrong is reused after it is put right. It was so before for every variable outside the fixed list.
- A glob with no plain first part (`**/*.snap`, `*.json`) walks the whole project folder, `node_modules` and `target` included, and so can reach the limit of 50,000 entries; the step is then not cached and says why. Start the glob with the folder: `tests/**/*.snap`.
- The named variables are read from the process, not through the reader the cache's unit tests inject.
- No test for a file name that is not UTF-8 (the step is then not cached).
- `"."` under `verify.projects`: inputs named there are read for a project at the repo root, whose key covers the whole repo already. Not tested.

## From the fresh review

It found nothing that removes or weakens an input, and no leak. It found places where a named input did nothing without saying so. Fixed, each with a test seen failing first: a glob that names a folder (`dist`) named no file; a link among the folders on the way was walked through, out of the repo; inputs for a folder that is not a project were dropped. The error for an unknown key of a project's step now lists `inputs`. The rest is above.

## Later

- `{ "runtime": "<command>" }`: a command whose output is hashed. It needs a time limit and must not run in `--dry-run`.
- Plain extra paths and globs for tracked files outside the worked-out set.
- A named edge that replaces the "never cached" verdict for a tsconfig `paths` alias or a dynamic import.
- Never: a way to remove an input.

## Tests

- `packages/cli/src/step-cache-inputs.test.ts`, written first and seen failing in WSL before the change: 9 of 9 failed (the config reader refused `inputs` as an unknown key).
- `the_line_of_named_inputs_holds_digests_and_changes_with_what_it_names` (`crates/core/src/stepcache.rs`), and two rows of the table of ADR 0058 (`crates/core/src/configkey.rs`), with the change. After the review: `a_glob_that_names_a_folder_counts_every_file_in_it`, `a_link_on_the_way_to_named_files_counts_by_its_target_and_is_not_followed` and two more CLI tests, each seen failing before its fix.
