# 0061 — Small fixes found during the 0.5.1 run

Status: accepted (2026-11-16, after the version was set to 0.5.1). Refines ADR 0014 (the stop hook's exit codes), ADR 0027 (the Cursor hook format), ADR 0057 (`verify --budget`), ADR 0059 (doctor and env files) and ADR 0060 (nothing under `.firescale` through a link).

## Context

The five pull requests of the 0.5.1 run each listed things found while fixing that were not theirs to fix. The owner asked for seven of them, in one pull request after the version was set. As before: each first gets a test that fails because of it, then the smallest fix. All seven were reproduced.

## Decision

### 1. `hooks run` is the stop hook wherever its options are written

`is_stop_hook` decides whether a usage error exits with 1 (the hook's own errors never hold the agent, ADR 0014) or 2. It dropped every word that starts with `-` and took the next two, so `fscale --agent claude hooks run` was not seen as the hook (the value `claude` was counted as a word) and a usage error there exited 2, which Claude Code and Codex read as "do not stop". The other way round, `fscale --rules hooks run` was taken for the hook.

- `is_stop_hook` now reads the command line as `parse_args` does (`positionals_of`): it skips the value of every option that takes one, honours `--`, and never fails.
- The options are two lists, `FLAG_OPTIONS` and `VALUE_OPTIONS`. `parse_args` knows an option only when it is in one of them, so a new option that takes a value cannot work without being listed, and once listed its value is skipped. `--budget` (ADR 0057) is in the list: `fscale hooks --budget 30m run --agent claude` is refused with 1, as the hook's refusal of `--budget` always meant to be.
- Tests hold the lists: every option of both lists goes through `parse_args` and `is_stop_hook`, and the help text must agree with the lists about which options take a value.
- An option in a list that `parse_args` does not store is an internal error, not a silent default.

Not changed: a word after an unknown option is read as a word, since nothing can know whether an unknown option takes a value.

### 2. A run stopped during the check prints its report with `--json`

Ctrl+C during the `check` step printed `verify: STOPPED - interrupted before the check finished` in plain text and nothing at all with `--json`. A script that reads the JSON got an empty stdout.

- With `--json` the run now prints a report: `result: stopped`, one step `check` with the status `stopped` and the detail `interrupted before the check finished`, and `notRun: []`.
- No schema change: only fields and values of schema version 7.
- The run stopped before the config's commands were chosen, so `runner` is `none`, `failOnSkipped` is `false`, `stepsTotal` is 1 and `scope` is `all`, also for a run started with `--affected`. `docs/verify.md` says so.

### 3. `verify --dry-run --budget <time>` names the steps a run would leave out

ADR 0057 accepted `--budget` with `--dry-run` and printed the same plan as without it. The dry run now adds a paragraph:

```
With --budget 30m, 1 step would not be run:
  test in backend/api: it took 1h 40m when it last ran
A step whose pass is in the step cache is taken from it and not left out; a dry run does not look there.
```

or `With --budget 30m, no step would be left out: none is known to take longer.` It reads the kept times only. A dry run does not read the step cache (it would have to hash every file), so it says that a kept pass would win. With `--affected` only the units that would run are looked at. Kept times that cannot be read are said in the same paragraph.

### 4. `hooks install` no longer says the Cursor hook format could not be checked

ADR 0027 marks `.cursor/hooks.json` "confirmed (official examples)", from Cursor's own example repositories. The note told people to check something firescale had checked. It is removed, not reworded; `docs/hooks.md` says where the confirmation came from. The `agent-permissions` skill still says that `.cursor/cli.json` could not be checked: by ADR 0027 that one is still unconfirmed.

### 5. The schema's description of `tamper` names every language the guard reads

It said "JS/TS, Python and Rust". It now lists all eleven. A test builds the list from the code's own `Language` type with no catch-all, so a new language does not compile until it is listed and then fails until the schema names it.

### 6. An entry file that is a link is a miss

ADR 0060 refused a `.firescale` folder, or one of its cache folders, that is a link. One entry file in a real folder (`.firescale/steps/<key>.json`, `.firescale/base/<key>.json`, `.firescale/cache/<rule>.json`, a shared record) was still followed: a link to a file outside the repo let a planted record be served as a pass, and a hit set the modified time of a file outside the repo.

- Each reader asks whether the entry itself is a symbolic link before it reads. A link is a miss: not read, not touched.
- The link is left where it is. The next store renames a temporary file over it, which puts a real file in its place and writes nothing where the link pointed.
- The ledger already checked its file. The shared cache had the same gap and got the same check, in every shared folder.
- On Windows a file symbolic link needs a privilege or Developer Mode, and `is_symlink()` is true for one. A junction always stands for a folder.

### 7. Doctor reads no env file through a symbolic link

Doctor followed a `.env` or `.env.local` that is a link; `blame::env_files`, which copies env files into the base run's worktree, did not. They now agree: doctor does not read one either.

Blame's own reason (it copies bytes) is weaker for doctor, which reads names only. But a link breaks doctor's rule from ADR 0059 that an env file above the firescale root is never read, and lets doctor print the names and local ports of any file the link points to.

- A name then found nowhere is a PROBLEM that names the link: `svc/.env is a symbolic link, so it was not read.`, with the next step `replace svc/.env with a real file that sets ..., or set the variables in your shell`.
- When every name is set in the environment or in a real env file, the text is as before and the link is not mentioned.
- **One visible cost:** a project whose `.env` is a link on purpose (a shared file, or a git worktree that points at the main checkout) and whose variables are set nowhere else now gets PROBLEM and exit 1 from `fscale doctor`, where it got `ok`. The project's own tools still follow the link.

## Consequences

- A stop hook written with its options before the command no longer holds an agent on its own usage error.
- A script that reads `verify --json` gets a report for every stopped run.
- `hooks install` prints one line less for Cursor; three snapshots changed for it.
- Nothing kept under `.firescale` is read through a link, at the folder or at the entry.
- `fscale doctor` can fail where it passed, for a linked env file.

## Not done

- A linked `.env.example` is still read by doctor.
- The temporary file of each store is written with a plain write; a link planted under that exact name (it holds the process id) would be written through.
- `--budget`, like the other options of verify, is still accepted without a word by commands that do not use it (`fscale check --budget 30m`).
- The help shows `fscale add <generator> --json <input>`, though `--json` is a flag and the input is the next word.
