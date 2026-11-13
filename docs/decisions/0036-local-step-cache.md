# 0036 — The local step cache

Status: accepted (2026-11-13, M18). Builds on ADR 0031 (why the cache comes before blame-aware verify) and ADR 0032 (the project graph the key walks).

## Context

`fscale verify` runs every lint, typecheck and test command each time, even when nothing those commands read has changed. On the course project a full run takes over ten minutes. M18 remembers passes, so a run with no change runs nothing again, and so the base run of blame-aware verify (M19) can skip every project whose inputs are the same on the base and the head.

A cache that serves a stale pass is worse than no cache: it tells an agent its work is done when it is not. Every choice below leans towards a miss.

## Decision

### What is stored

- Only a step whose status is `passed`. Failed, setup needed, timed out, stopped and skipped steps are never stored, so a failure always runs again.
- Entries live in `.firescale/steps/<key>.json`: the key, the step, the project's folder, the command, how long it took, when it ran (UTC seconds), the line about its outcome and the test counts. No output, no environment values.
- `.firescale/` is in the `.gitignore` that `core/gitignore-basics` writes, and the cache never hashes `.firescale/` or `.git/` itself, also when a repo does not ignore them.

### The key

A blake3 hash of:

1. A format number of this cache and the firescale version.
2. The step, the project's folder and kind, and the command.
3. The files the step can read, each by its path and the hash of its **bytes** as they are in the working tree. The file list is `git ls-files --cached --others --exclude-standard`: tracked files (with uncommitted changes as they are on disk, and a deleted tracked file as "absent") and untracked files git does not ignore. Outside git, the gitignore-aware walk of `check`. A CRLF checkout on Windows is therefore a different key, never a wrong hit. A symbolic link counts by its target. Which files:
   - every file in the project's folder, deeper projects included (a test runner may collect their tests);
   - every file of the projects and shared folders or files it depends on in the project graph, followed transitively (ADR 0032);
   - the files directly in each folder above those, up to the repo root, where tools find their settings (`pyproject.toml`, `tsconfig.base.json`, `ruff.toml`);
   - every file that belongs to no project and to no narrow shared node, including Markdown and `docs/`. `--affected` treats those as affecting nothing (ADR 0034), but a test may read them, and a miss is cheap.
   - The repo root's own steps, a project at the repo root, and a folder only named in `verify.projects` cover the whole repo.
4. The versions of the tools the command runs. The first word of the command decides: a Node tool (`npm`, `npx`, `pnpm`, `yarn`, `bun`, `deno`, `node`, `corepack`) adds its own `--version` and `node --version`; a Python interpreter (any `python*`, `py`, including a `.venv` path) or a Python manager (`uv`, `poetry`, `pipenv`, `pdm`, `hatch`, `tox`, `nox`) adds its `--version`; `cargo` adds `cargo --version` and `rustc --version`; `go` adds `go version`. Any other program (a script of the repo, `make`) is **never** run with `--version`, because that could run the script; its own file is in the key when it is in the repo. Each probe runs once per run, through doctor's `Machine`, with corepack and Go downloads turned off. A probe that fails means the step is not cached.
5. What is installed: the package manager's marker file in `node_modules` (`.package-lock.json`, `.modules.yaml`, `.yarn-integrity`, `.yarn-state.yml`, `.bun-tag`) and `.yarn/install-state.gz`, from the step's folder up to the root; and the name and version of every package in a `.venv` or `venv` of the step's folder. These folders are ignored by git, so the file list cannot see them.
6. The OS and CPU architecture (`std::env::consts`), and whether verify sets `CI=1`.
7. The values of 19 environment variables, hashed together and never written down: `CI`, `NODE_ENV`, `NODE_OPTIONS`, `TZ`, `LANG`, `LC_ALL`, `PYTHONPATH`, `PYTHONHASHSEED`, `VIRTUAL_ENV`, `PYENV_VERSION`, `RUSTFLAGS`, `RUSTUP_TOOLCHAIN`, `CARGO_BUILD_TARGET`, `GOFLAGS`, `GOOS`, `GOARCH`, `CGO_ENABLED`, `GOTOOLCHAIN`, `JAVA_HOME`. **No other variable is read.** The list is a constant (`ENV_ALLOWLIST`); there is no config key for it yet.

### When a step is not cached

- **It may need a service the repo does not start**: `services_for_folder` (ADR 0025) finds a service in an `.env.example` of its folder or a folder above that no Compose file in the repo starts. A Compose service counts as the repo's own, and the Compose file is in the key.
- **Its project, or a project or shared node it depends on, has a dynamic import**: what it loads is not in the graph.
- **An input could not be read**: a file that exists and cannot be read, a file list that git could not give, or a tool version that could not be read.
- A step is run, not served from the cache, when the check failed or an earlier step of its project blocked; that has not changed.

### Writing and cleaning

- The working tree is hashed once before the steps run. A pass is kept in memory and stored only at the end of the run, after the tree is hashed again (reusing a file's hash when its size and modification time are unchanged, within the run only) and the step's files are found unchanged. A step that writes a file it depends on, or a person who edits one while verify runs, gets no entry.
- An entry is written to a temporary file and renamed. A hit touches the entry's modification time.
- At the end of every run that stored something, entries older than 30 days are removed, then the least recently used beyond 500. There is no command to clean the cache; deleting `.firescale/steps/` is always safe.

### How a hit is shown

- In `--json`, the step's `status` stays `passed`, `durationMs` is `0` (it did not run now) and `cache` gives `durationMs` and `ranAt` of the run that passed. The report gets `cache: {read, hits, savedMs}`.
- In the summary the status column says `cached`, the detail says when it passed and how long it took then, and the headline says how many steps passed from the cache, not run now. A line under the steps says what the cache saved.
- `FSCALE_LOG=cache` prints, per step, `hit`, `miss`, `not cached ... (reason)`, `stored` or `not stored`.
- `--no-cache` runs every step and still stores the passes.

### Schema

The verify JSON stays at schema version 5: `cache` on a step and on the report are new optional fields, which is not a breaking change. Blame-aware verify (M19) takes version 6.

## Consequences

- A second `fscale verify` with no change runs no lint, typecheck or test command.
- The key is only as good as the graph: an import the graph misses is a dependency whose change does not invalidate a pass. ADR 0032 errs towards more edges for that reason, and a dynamic import turns the cache off for that project.
- A file git ignores is not in the key. A `.env` (never read, by the hard rules) or a generated file the tests read can change without a miss. A test that needs a database declared only in an ignored `.env` is cached when it passes. `--no-cache` is the way out, and the summary names it.
- Every verify run hashes the files in the keys and asks the tools for their versions. Measured in `bench/RESULTS.md`.
