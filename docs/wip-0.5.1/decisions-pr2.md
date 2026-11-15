**Pull request 2: what a config change affects** (branch `pr2-config-affects`, ADR 0058)

Your case: an untracked `firescale.config.json` that only set two test timeouts made `verify --affected` run all six projects and changed every step's cache key. Now a step is affected, and its key changes, only when a setting changed that can change its result.

- **The config is compared by value, not as a file.** `firescale.config.json` is no longer a file of any step's key, and no longer "a file no project owns" for `--affected`. Instead:
  - a step's command, and whether it is on, come from the plan; the command was already in the key;
  - `packs`, `rules`, `verify.ci`, `verify.failOnSkipped`, `verify.stopWhenServiceDown`, `verify.stopAfterErrors` and any setting this version does not know are in every step's key, and a change to one affects every project;
  - time limits (`verify.timeout`, `verify.idleTimeout`, every step and project `timeout`), `verify.baseBudget`, `verify.labelFixed`, `verify.baseInstall`, `verify.tamper`, `protectedPaths`, `agent` and `$schema` are in no key and affect nothing.
- **Failing tests first** (`packages/cli/src/config-affects.test.ts`, committed before the code; 9 of its 16 failed):
  - `runs no project for an untracked config that only sets two test timeouts`: your case on the course fixture. It failed with `Expected: "verify: OK (affected only) - 0 of 6 projects ran, 6 not affected"`, `Received: "verify: PARTIAL (affected only) - 6 of 6 projects ran; 10 of 19 steps ran, 9 skipped"`. It also checks that a full run afterwards takes every pass from the cache, and that every other "nothing" setting runs nothing.
  - `writing a default out in full, or reformatting, changes no key`: failed, all nine commands ran again.
  - `runs only the project whose command the config changes`: failed, 6 of 6 projects ran.
  - `runs nothing for a project or a step that the config turns off, and runs it when it is turned on again`: failed, seven commands ran.
  - `runs the root step, and no project step, when the config gives the whole repo a command`: failed, three project steps ran too.
  - `compares a committed config with the base commit, setting by setting`, `runs every project when the base commit has a config this version cannot read`, `keeps the whole-file rule for a firescale.config.ts`, and the note of `runs every step again when a setting every step shares changes, with one note`: failed on the reasons and notes, which did not exist.
  - The seven `runs every step again, and every project under --affected, when <setting> changes` tests (one per setting that every step shares) passed before and pass now: they pin what must not be lost.
- **Every setting is placed, in code.** `configkey::shared_parts` takes `VerifyConfig` apart with no `..`, so a new field does not compile until it is put on a side. `configkey::tests::every_setting_lands_on_the_side_the_adr_gives_it` walks the table of ADR 0058 (31 rows), changes each setting and checks where it lands; `the_table_names_every_setting_and_nothing_else` checks the table against the struct's fields, the top-level keys the reader accepts and the verify keys its error message lists. These unit tests came with the code, not before it: they could not compile without it.
- **Unknown settings stay in the key.** The working tree's config cannot have one (the reader refuses it, exit 2, as before). The base commit's can: then every project is affected (`a_base_config_this_version_cannot_read_affects_everything`, and end to end).
- **A `firescale.config.ts` keeps the old rule**: its bytes are in every key and any change to it affects every project.
- **The base run of blame-aware verify**: a step counts as "the same on the base" after a config change only when the change cannot reach it (`a_changed_config_is_the_same_on_the_base_only_for_steps_it_does_not_reach`; `blame.test.ts` "counts a step as the same on the base after a config change that cannot reach it, and runs the base after one that can").
- **The shared CI cache** uses the same keys: `shared-cache.test.ts` "serves a record to a pull request that changes a time limit, and not to one that changes a setting every step shares".
- **Your second report, the untracked `image.png` that was not listed: reproduced and fixed.** Cause: `graph::affected` stopped at the first file that runs every project. Your untracked `firescale.config.json` sorts before `image.png`, so the image was never looked at. Not the cause: untracked or tracked, the mode, the extension list, the root, the Windows path form. Failing test first: `graph::tests::inert_files_are_listed_whatever_else_changed` (`left: ["README.md"]`, `right: ["README.md", "image.png"]`), and end to end `names an inert file even when another changed file runs every project`. Fix: the walk goes to the end and lists every inert file. Which projects run did not change.

**Decided on your behalf** (ADR 0058):

- **`packs` and `rules` stay in every key.** A step's command may run firescale on the repo (`"lint": "fscale check && eslint ."`; this repo's own tests do). Unsure, so kept. Turning a rule on still runs every step once.
- **`stopAfterErrors` and `stopWhenServiceDown` stay in every key.** I tried to move them out as "only an early stop" and found that a step stopped early can be reported `failed`, which a base run keeps.
- **`failOnSkipped` stays in, and a step that is no longer off but has no command counts as reached.** Under `--affected` the skipped steps of projects that do not run are not counted, so the run said OK where a full run fails. The second case I found while writing the table, after the first version of the code; it has its own row and was fixed in a later commit.
- **Settings are compared as this version reads them.** A default written out, a key set to `null`, reordered keys and white space are no change. A different order of `packs` is a change.
- **The cache format is 2.** The first run after the upgrade runs every step.
- **`--affected` reads the base commit's config with `git show`**, never by running code. With no base it compares with the last commit; an untracked or new config is compared with the defaults.
- **A note says when a config change runs nothing**: `firescale.config.json changed, but only in settings that cannot change a step's result (time limits, budgets and the like), so the change runs no project.`
- **The note of ADR 0043 changed its words** and is said only when a setting every key holds changed: `A setting of the firescale config that every step's result can depend on changed since the last run, ...`.
- **One existing test changed on purpose**: `step-cache.test.ts` "says once when a change to the config means no kept pass applies" wrote `"failOnSkipped": false` (the default) and expected every step to run again. It pinned the whole-file rule. It now checks that this and a time limit run nothing, then turns `verify.ci` off.
- `check --affected` is untouched: every rule still reads a change of the config (ADR 0056, bug 1). The rule-result cache is untouched.
- The worktree was made from an old commit (M23). The branch was created from `b43a548` (0.5.0, `origin/main`) instead, since the brief is about 0.5.0.

**Seen changes**

- After a time limit is lowered, a step that passed under the old limit is served from the cache. It is not run again to see whether it now times out. Test: "never keeps a step that a limit stopped, and serves a kept pass after the limit is lowered".
- After only `verify.baseInstall` changes, a failing step whose files are the same on the base is `pre-existing` with no base run. Before, any config change forced one base run.

**Known limits**

- A command that reads `firescale.config.json` itself for a setting that is in no key is not run again when only that setting changes: a test that asserts the file's text, or a step that runs `fscale verify --dry-run` on its own repo.
- A rule that reads every file (`core/no-secrets-in-source`) reads the config's text too. A step that runs `fscale check` inside is not run again for a secret typed into a setting that is in no key; verify's own `check` step runs fresh every time and reports it.
- `Firescale.config.json` with another letter case, on a disk that ignores case, keeps the whole-file rule. Read from the code, not run.
- Windows was not run here. What it depends on there: `git show <commit>:./firescale.config.json` (the same call the tamper guard makes) and `git ls-tree --name-only -z <commit> -- <names>`, both with forward slashes; a byte order mark and CRLF in the committed config are covered by a unit test. The new time-limit test waits on a 2 s script with 1 s limits, five runs in all (about 7 s here, under the 30 s test limit).

**Found while fixing, not fixed**

- Read from the code, not run: with a base, the base run executes the working tree's commands in the base worktree (`blame::compare` takes each step's command from the head's report, ADR 0037). So a step that fails only because the config gave it a failing command would fail on the base too and be labelled `pre-existing`. This was so before this change and is not changed by it. I found no tamper finding for a `verify.<step>` command in the firescale config (the tamper code names that file only to read its policy).

**Not reproduced**

- Nothing. Both reports were reproduced.
