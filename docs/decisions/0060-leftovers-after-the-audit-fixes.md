# 0060 — Leftovers after the audit fixes

Status: accepted (2026-11-15, for 0.5.1). Refines ADR 0056 (bug 6: `hooks install`; bug 8: protected globs; bug 17: writes through a symbolic link), ADR 0047 (what a protected path is), ADR 0041 (the shared cache folder) and ADR 0046 (the JSON of the tamper guard).

## Context

The log of the audit fixes (`DECISIONS.md`, "Audit bug fixes (run of 2026-11-15)") lists things found while fixing that were not on the owner's list, and one test that timed out on Windows. The owner asked for five of them. As before: each first gets a test that fails because of it, then the smallest fix.

This ADR records the choices made where an item left more than one fix open.

## Decision

### 1. `fscale hooks install --all` tries every agent's file

The command stopped at the first file it refused. The files before it were written, the ones after it were not tried, and nothing said so.

With `--all`, a refusal no longer ends the run. Each of the three files is tried. Stdout lists each file as `installed`, `already installed` or `refused`, then says in words which were refused and which hold the hook. Each refusal's own message (why, and the item to add by hand) goes to stderr, as it did. The exit code is 2 when any file was refused, the code one refusal already gave.

- **`--agent <name>` is unchanged**: one file, and a refusal is the command's error, with nothing on stdout. An existing test pins that, and there is nothing to carry on with.
- Only a refusal a person can fix is carried past (a file that is not valid JSON, one the hook cannot go into as new lines, one that cannot be read or written). An internal error still ends the run.
- The closing line "Each agent now runs …" is printed only when no file was refused. Otherwise it names the files the hook is in.

### 2. A protected glob that can match no path is a config error

The guards compare a changed file's path as git prints it: from the repo root, with `/` between parts (`infra/main.tf`). Three kinds of `protectedPaths` entry compile and protect nothing. Each got its own answer.

- **A glob with an empty, `.` or `..` part**: `./infra/**`, `/infra/**`, `infra/`, `infra//x`, `../infra/**`. No path git prints has such a part, so the glob can match nothing. It is now a config error for every command that loads the config, as a glob that does not compile is since bug 8. The message names the glob and the form to write (`./infra/**` → `"infra/**"`, `infra/` → `"infra/**"`); for a glob with `..` it gives the rule and an example, since no form can be worked out.
- **A plain name that is a folder**: `infra`, where `infra/` exists. A name with no wildcard names one file (`Makefile`, `LICENSE`), so it must stay valid; it never names a folder's content, because a folder is not a path in a diff. Where the name is a folder on disk when the config is loaded, it is a config error that says to write `infra/**`.
- **A plain name that is nothing yet** stays accepted. It protects a file of that name if one is made, and nothing in a folder of that name until the next command sees the folder and stops.

**Why an error and not normalising.** Dropping `./` would be safe, but `infra/` and `infra` would have to become `infra/**`, and then a name would mean a file in one repo and a tree in another. An error changes what no glob matches: it cannot widen or narrow protection, and the person writes the form they mean. The cost is that a config that ran before now stops until one line is fixed.

**Rules from `.claude/settings.json`.** The agent runner also takes `Edit(…)`, `Write(…)` and `Read(…)` rules under `deny` and `ask` as protected paths (ADR 0056, bug 8).

- A `./` or `/` in front was already dropped there, so `Edit(./src/**)` and `Edit(/src/**)` protect `src/**`. A test now pins it.
- A rule that still can match no path (`Edit(src/)`, a `.` or `..` part) stops `fix --agent` before any agent starts, with the form to write. It is not a `fscale check` error: that file is not firescale's config.
- **A plain name there is left as it was**: `Edit(dist)` protects the one file `dist` in the diff guard and nothing in a folder `dist`. Claude Code itself reads such a rule more widely, and the rule is still handed to Claude Code as written, so its own tools refuse the folder. Refusing the run for a rule that is valid for Claude Code seemed the larger surprise. `docs/fix.md` says to write `Edit(dist/**)`.

Two helpers were added to `agentrun.rs` beside bug 8's (`unmatchable_glob`, `is_plain_path`). The folder check sits in `load_config`, the one place that has both the config and the repo root.

### 3. Nothing under `.firescale` is read or written through a symbolic link

Every writer under `.firescale/` was listed first:

| Path                                   | Writer                     | Before                                                                               |
| -------------------------------------- | -------------------------- | ------------------------------------------------------------------------------------ |
| `.firescale/steps/`, the config digest | `stepcache.rs`             | refused a link (bug 17)                                                              |
| `.firescale/.gitignore`                | `stepcache.rs`, `cache.rs` | a linked file was left alone (bug 17); a linked folder was not checked by `cache.rs` |
| `.firescale/hooks/`                    | `hooks.rs`                 | refused a link                                                                       |
| `.firescale/cache/`                    | `cache.rs`                 | wrote through a link                                                                 |
| `.firescale/base/`                     | `blame.rs`                 | wrote through a link                                                                 |
| `.firescale/ledger.jsonl`              | `agentrun.rs`              | appended through a link                                                              |
| `.firescale/shared/` (when given)      | `sharedcache.rs`           | wrote and pruned through a link                                                      |

Not under `.firescale`: the run records of `runs.rs` (the temp folder's `firescale-runs`), the base worktrees and the private git index (the temp folder). Nothing was changed for those.

One small module, `ownfolder.rs`, holds the check (`linked`: is `.firescale`, or a named part below it, a symbolic link) and the one refusal text. The four writers that had no check call it. The step cache and the stop hook keep their own copies of the same check: moving them was not needed for the fix.

What each does when it finds a link:

- **Rule results** (`cache.rs`): nothing is read or stored. Every rule runs, and `fscale check` prints one warning with the refusal. The check does not fail: its result is already known.
- **Base results** (`blame.rs`): nothing is read or kept, without a note, as when the folder cannot be written. The base runs again next time.
- **The ledger**: `fix --agent` is refused before the agent starts, so no run's work ends in an error about its last line. `append_ledger` refuses too.
- **Shared records**: with `.firescale` or the records folder inside it a link, nothing is read, written or pruned; the cache log says why. A `--shared-cache` folder outside `.firescale` is the caller's choice and is used as given.

The text: "`.firescale` is a symbolic link, and firescale reads and writes nothing through one. Remove the link: a real folder or file is made in its place."

A link is refused, never followed, also when it stays inside the repo, as in bug 17. On Windows a junction counts as a link (Rust's `is_symlink` is true for it).

### 4. The shipped schema lists what the tamper guard emits

`verify-output.schema.json` listed five kinds for a finding and for an allowed file; `Kind` has eleven. The six missing ones are added. A Rust test reads the shipped schema file and compares both lists with every `Kind`, in order; its list of kinds holds a `match` with no wildcard, so a new variant does not compile until it is added there, and then fails until the schema has it.

A second test fills a whole `TamperReport` and checks that the schema describes every key. It found one more gap: `approvals` (ADR 0046) was not in the schema, which forbids unknown keys, so `fscale tamper --pr-base --approvals --json` output did not validate. It is added.

No new dependency: the test compares lists and keys; it is not a JSON Schema validator. No other shipped schema describes tamper output.

### 5. Two tests that reached their limit on Windows

Both kept every assertion, and every case still starts the real binary.

- `scaffold.test.ts`, "`--check` exits 1 after a generated file is deleted, and 0 after sync": the four files were handled one after the other in one repo. Each now has its own copy of the freshly built repo, and the four run side by side. The 60 s limit stays, for a runner with few cores.
- `verify.test.ts`, "rejects a project folder that does not exist, and a malformed projects section": it built seven repos and seven folders of stand-in tools. It now builds one and writes each case's config into it. The cases cannot affect each other: each ends at the config error, and the test now also asserts that no step ran in any case, that stdout is empty, and that no `.firescale` folder was made.

## Consequences

- `fscale hooks install --all` on a repo with one hand-made, minified settings file installs the other two hooks and exits 2. Before, what it did depended on which file was the minified one.
- A config with `./infra/**`, `/infra/**` or `infra/` in `protectedPaths`, or a plain folder name, stops every command until the entry is rewritten. Before, the run went on with that path unprotected.
- `fix --agent` in a repo whose Claude Code settings hold `Edit(src/)` is refused until it reads `Edit(src/**)`.
- A repo whose `.firescale` is a symbolic link gets no cache of any kind and no agent fix run, and is told to remove the link. Someone who linked `.firescale` to another disk on purpose loses that.
- JSON with any tamper kind, or with approvals, validates against the shipped schema.
- **Known limits.** A linked entry file inside a real cache folder is still read (`.firescale/cache/<rule>.json`, `.firescale/base/<key>.json`, `.firescale/steps/<key>.json`), and a step-cache hit sets its modified time; a stored entry replaces the link and never writes through it. Such a file is used only when it holds the key the run worked out itself. `fscale ledger` still reads a linked ledger. A plain `protectedPaths` name whose folder is made later, in the same agent run, is not seen until the next command.
