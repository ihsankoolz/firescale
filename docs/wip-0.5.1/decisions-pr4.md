**Pull request 4: leftovers from "Found while fixing"**

Five things the audit fixes found and left. For each: a test that failed because of it, then the smallest fix. Choices are in ADR 0060.

- **`hooks install --all`, fixed.** Failing test first: `--all tries every file: it writes the ones it can, and says which it refused` (hooks.test.ts: "ENOENT … .codex/hooks.json", the file after the refused one was never tried). Fix: with `--all` a refusal no longer ends the run. Every file is tried; stdout lists each as `installed`, `already installed` or `refused` and says which is which; each reason, with the item to add by hand, is on stderr; the exit code is 2 when any was refused.
- **A protected glob that can match no path, fixed.** Failing tests first: `rejects the protectedPaths glob ./infra/**` (and `/infra/**`, `infra/`, `../infra/**`) and `rejects a protectedPaths name that is a folder` in config.test.ts (`fscale check` exited 0), and `a protected glob that can match no path stops the run` in agent-runner.test.ts (the stand-in agent edited `src/app.js` and the run PASSED). Fix: a config error that names the glob and the form to write.
- **Writes through a linked `.firescale`, fixed where reproduced.** Failing tests first: `a_rule_result_is_never_stored_or_read_through_a_symlinked_folder` and `the_ledger_is_never_written_through_a_symlink` (ownfolder.rs), `a_base_result_is_never_kept_or_read_through_a_symlinked_folder` (blame.rs: `k2.json` appeared in the other repo), `nothing_is_read_written_or_pruned_through_a_symlinked_firescale_folder` (sharedcache.rs), and end to end `writes nothing through a .firescale that is a link to a folder outside the repo` (step-cache.test.ts: `.gitignore` and `cache` appeared outside) and `a .firescale that is a link stops the run before the agent starts` (agent-runner.test.ts: the agent ran). Fix: one check in a new small module, `ownfolder.rs`, called by the four writers.
- **Tamper kinds in the shipped schema, fixed.** Failing tests first: `every_kind_the_guard_can_report_is_in_the_shipped_schema` (five kinds in the schema, eleven in `Kind`) and `every_field_of_the_report_is_in_the_shipped_schema` (`/approvals` not described). Fix: the six kinds and `approvals` added to `verify-output.schema.json`.
- **The two slow tests, made faster.** No assertion was dropped, and every case still starts the real binary. Measured here on Linux, three runs each, on a shared machine: the sync test took 2.3 to 3.9 s before and 1.5 to 1.7 s after; the projects test took 0.8 to 1.4 s before and 0.16 to 0.22 s after.

**Decided on your behalf** (ADR 0060):

- `hooks install --agent <name>` is unchanged: a refusal is still the command's error with nothing on stdout. Only `--all` carries on.
- **A protected glob that can match no path is an error, not rewritten.** Rewriting `./infra/**` would be safe, but `infra/` and a bare `infra` would have to become `infra/**`, and a name would then mean a file in one repo and a tree in another. **One visible cost:** a config that ran before with `./infra/**`, `/infra/**` or `infra/` now stops every command until that line is fixed.
- **A bare name stays valid and means one file** (`Makefile`). It never protected a folder's content. Where the name is a folder on disk it is now a config error that says to write `infra/**`. Where nothing of that name exists yet it is accepted.
- Path rules from `.claude/settings.json`: `./` and `/` in front were already dropped, so those forms protect (now pinned by a test). A rule that still can match no path (a `.` part, `a//b`) stops `fix --agent` with the form to write (see "After the review" for `Edit(src/)` and `..`). A bare name there (`Edit(dist)`) is left as it was: one file in the diff guard, and Claude Code still gets the rule as written. `docs/fix.md` says so.
- A linked `.firescale` is refused, not followed, also when it points inside the repo, as in bug 17. `fscale check` and `verify` still run and pass or fail as before, without that cache, and `check` warns once. `fix --agent` is refused before the agent starts, since its ledger line could not be written.
- On Windows a junction counts as a link. The two end-to-end tests make a junction there, so they run on Windows too; the Rust tests are unix only.
- The schema test also checks every key of a full tamper report, which is how the missing `approvals` was found. It was added to the schema: same file, same block, same kind of gap.
- The sync test keeps its 60 s limit. Its four cases now run side by side, each in a copy of the freshly built repo. The projects test builds one fixture, not seven, and writes each case's config into it; it now also asserts that stdout is empty, that no step ran in any case and that no `.firescale` folder was made.

**After the review** (a fresh reader of the diff; each with a test that failed first, except the last):

- **Claude Code rules were treated too strictly.** `Read(../**)` and `Edit(../shared/**)` stopped `fix --agent` with no form to write, and `Edit(src/)` was refused though Claude Code reads it as a folder. Test: the settings cases of `a protected glob that can match no path stops the run` ("Edit(src/): expected 2 to be 1"). Now a settings rule with a `..` part is not a protected path and does not stop the run (the agent still gets it as written), and one that ends with `/` protects the folder and all in it. `protectedPaths` in the firescale config stays strict.
- **A nested shared folder passed the link check** (`--shared-cache .firescale/shared/linux`). Test: three more cases in `nothing_is_read_written_or_pruned_through_a_symlinked_firescale_folder` (the record was written through the link). Every part from `.firescale` down is now checked.
- **The folder check depended on the system.** `protectedPaths: ["Infra"]` beside a folder `infra/` was accepted on Linux and an error on macOS and Windows. Test: two more cases in `rejects a protectedPaths name that is a folder` (`fscale check` exited 0). The name is now compared without regard to case, as the guard compares paths. A symbolic link of that name is not called a folder.
- **`fscale ledger` read a linked ledger** while the docs said nothing is read through one. Test: `reads no ledger through a .firescale that is a link` (ledger.test.ts: exit 0). It now stops with the same refusal.
- A test for `hooks install --all` with all three files refused was added (`--all with every file refused says so, and claims no hook`). It passed as written; no code changed for it.

**Known limits, not fixed here:**

- A linked entry file inside a real cache folder (`.firescale/cache/<rule>.json`, `.firescale/base/<key>.json`, `.firescale/steps/<key>.json`) is still read, and a step-cache hit sets its modified time. Nothing is written through it.
- `C:/infra/**`, `~/infra/**` and a bad part inside braces (`{./infra,src}/**`) are still accepted in `protectedPaths`, and protect nothing or less than meant.
- A submodule named by a plain `protectedPaths` name is called a folder, though it is one path in a diff.
- A bare `protectedPaths` name whose folder is made later is not seen until the next command loads the config.
- The step cache and the stop hook keep their own copies of the link check and their own wording; they were not moved to `ownfolder.rs`.
- The schema tests compare lists and keys. They are not a JSON Schema validator: a wrong type in the schema would not be found.

**Not reproduced:**

- **The run records (`runs.rs`) do not write under `.firescale`.** They are in the temp folder (`firescale-runs`), as are the base worktrees and the private git index. Read, and no test could show a write through a linked `.firescale`. No code was changed for them.

**Found while fixing, not fixed:**

- `hooks install` still prints "the Cursor hook format could not be checked", also beside a refusal (already in the log).
- The verify schema's description of `tamper` still says "JS/TS, Python and Rust"; the guard reads more languages since M32.
- In a repo whose `.firescale` is a link, `FSCALE_LOG=cache` shows the step cache's older wording (".firescale or .firescale/steps is a symbolic link; replace it with a real folder") beside the new one.
- `fix --agent` strips every leading `/` of a Claude Code path rule, so `Edit(//abs/path)` (an absolute path in Claude Code) is read as a path in the repo.
