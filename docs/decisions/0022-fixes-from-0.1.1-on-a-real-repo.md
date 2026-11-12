# 0022 — Fixes from running 0.1.1 on a real repo

Status: accepted (0.1.2). Changes ADR 0020 (facts: the Python interpreter and an `install` command per project; verify: projects are independent, a new status `setup-needed`, schema version 3) and ADR 0012 (`core/env-example`: test-runner config files are test code).

## Context

The owner ran 0.1.1 on the course project that M10 was built for: five Python services under `backend/`, each with its own `.venv`, and a Vite app under `frontend/`. Five things went wrong:

1. `fscale verify` ran `python -m pytest` with the system Python and failed with `ModuleNotFoundError: No module named 'jwt'`.
2. After `backend/audit_service` failed, every other project was "not run".
3. That failure was reported as a test failure, although no code change could fix it. An agent behind the stop hook would try to edit code.
4. The repo uses ruff with no `ruff.toml`, and ruff was not found.
5. `core/env-example` treated variables read by `playwright.config.ts` as variables of the app.

## Decision

### The Python interpreter

Each Python project's commands use one interpreter, chosen in this order. The first match wins:

1. **A virtual environment in the project's folder**: `.venv`, then `venv`, if it holds an interpreter for this platform (`Scripts\python.exe` on Windows, `bin/python` elsewhere). A folder without one is passed over, and `detail` says so.
2. **The project's own manager**: `uv run`, `poetry run` or `pipenv run`, from the same markers as before (`uv.lock` or `[tool.uv`; `poetry.lock` or `[tool.poetry`; `Pipfile`).
3. **An active virtual environment**: the interpreter in the folder named by `VIRTUAL_ENV`, by its full path, quoted when it has a space or a shell character. On Windows a path with `%` is skipped, because `cmd` expands `%NAME%` even inside quotes.
4. **`python` from the PATH**, as before. `fscale facts` says "no virtual environment found", and `fscale verify` prints a `note:` naming the projects.

Why this order:

- The project's own `.venv` is the most specific answer, and it is what the owner's repo has. Running its interpreter directly never installs anything, which `uv run` and `poetry run` can.
- So a uv or poetry project that has a `.venv` in its folder also runs it directly. That is the environment uv and poetry create there.
- uv and poetry choose the environment of a project themselves and prefer the project's own over an activated one. So they come before `VIRTUAL_ENV`.
- An activated environment belongs to the shell, not to one project. In a repo with five services, one activated venv is probably right for at most one of them. It only fills in for projects that name nothing.

The choice is a new `interpreter` object on each Python project in `fscale facts` (`source`, `command`, `detail`), and the human output has an `interpreter:` line. It is additive, so the facts schema stays at version 2.

**Windows paths in commands use backslashes.** `cmd` reads `.venv/Scripts/python.exe` as the program `.venv` with a switch, so the command is `.venv\Scripts\python.exe -m pytest`. This is the one exception to "forward slashes in all output": it is a command for `cmd`, not a path for people. A Windows-only Rust test pins it, and a CLI test runs a real `.venv\Scripts\python.exe` on the Windows runner.

`VIRTUAL_ENV` is read when the process starts. The tests pass it in (`detect_facts_with`), and the CLI test helper removes it from the environment of every test that does not set it, so a developer's activated venv cannot change a test's result.

### An install command per project

Verify needs a command to name when the environment is not set up, so `commands` of a project gained `install`:

- Python: `<interpreter> -m pip install -r <file>` with the first development requirements file found (`requirements-dev.txt` and five other spellings, then `requirements.txt`), else `<interpreter> -m pip install -e .`; `uv sync`, `poetry install` or `pipenv install --dev` for those managers.
- Node: `<pm> install`, except for workspace members (the root installs them) and for a `package.json` that cannot be read.
- A development requirements file that does not include `requirements.txt` with `-r` is installed together with it, so the app's own dependencies are there (the owner's `jwt` case).
- A uv, poetry or Pipfile project keeps its manager's install command even when its `.venv` runs the commands: a venv made by uv has no pip.
- None for Rust and Go, whose build tools fetch what they need.

With no virtual environment, the install goes into the same `python` the steps use. Suggesting `python -m venv .venv` first would be better practice, but the command after it would name a path that differs by OS; the `note:` line suggests creating one instead.

### Independent projects

The repo root and each project are units. A failure stops the rest of its own unit, as before, and the other units still run. A failing `check` still stops everything: it is the gate in front of the rest, and an existing test pins it. The headline names every failure: `verify: FAILED at test in backend/audit_service, lint in frontend`. With one failure it reads exactly as before. `failedStep`, `failedProject` and `rerun` keep naming the first.

### Setup needed

A step whose output shows that the environment is not ready gets the status `setup-needed`, shown as `SETUP`. The summary says it is not a problem in the code, says not to change code for it, and names the install command. The result is `setup-needed` when nothing else failed, `failed` when something did. Either way the exit code is 1.

The detection is a fixed list of messages, in `crates/core/src/setup.rs` and `docs/verify.md`: Python's `No module named`, the Windows "Python was not found" stand-in, cmd's "is not recognized as an internal or external command", sh's "not found" with exit code 127, `npx --no`'s "canceled due to missing packages", pnpm's `Command "x" not found`, poetry's `Command not found:`, uv's `Failed to spawn:`, and Node's `Cannot find package` with `ERR_MODULE_NOT_FOUND`.

When unsure it is a normal failure:

- Output that reports failed tests (a line starting with `FAILED `, or a count such as `2 failed`) is a normal failure, even if a test failed on a missing module: the tests ran, and an agent must still fix the ones that failed for real. A pytest collection error stops pytest before any test runs, so it stays setup.
- A missing Python module whose top-level name is a folder or `.py` file of the project (also under `src/`) or of a folder above it is a normal failure: it may be a mistake in the code.
- A Node package given as a path is a normal failure.
- "not found" from sh counts only with exit code 127, the shell's own code for a missing command.

A missing program that an install does not bring back (`python`, `node`, `pnpm`, `uv` and the like) gets no install command; the summary says to install that program.

The stop hook still blocks on a setup problem: the work is not verified. Its message starts with `SETUP NEEDED:` and tells the agent not to edit code, tests or config to get past it, and to run the setup command or tell the user. When other steps failed in the code as well, the message says those still need code fixes. The loop guard still lets the agent go after 3 blocks.

`fscale verify --json` is now **schema version 3**: the status and result `setup-needed`, the step field `fix`, and `notes`. ADR 0020 had kept version 3 for blame-aware verify; that milestone now takes version 4.

### Ruff and mypy from dependency lists

A Python project's tools are also read from its dependency lists: every requirements file in its folder (any `.txt` with `requirements` in the name, and a `requirements/` folder), following `-r` includes inside the repo; `pyproject.toml` dependency tables, optional dependencies, dependency groups and the uv, pdm and poetry dev lists; `Pipfile` packages and dev-packages. Only the project's own folder is read: a requirements file above it is not the project's. `docs/facts.md` lists the forms.

### Test-runner config files are test code

`core/env-example` treats the config and setup files of test runners as test code, in any letter case: names that start with `playwright.config.`, `playwright-ct.config.`, `vitest.config.`, `vitest.workspace.`, `vitest.setup.`, `jest.config.`, `jest.setup.` or `cypress.config.`, and named configs such as `vitest.e2e.config.ts`. A variable read only there is a low-severity, test-only finding. `vite.config.*` and other build config stay app code.

## Consequences

- On the owner's repo, each service runs with its own `.venv`, and a missing package is reported as setup, with the pip command, while the other services and the frontend still run.
- Verify's JSON consumers must accept schema version 3. The stop hook is the only consumer in this repo.
- The facts output of every Python project, and of every Node project that is not a workspace member, gained lines (`install`, `interpreter`). Snapshots were updated on purpose.
- The detection list will need new entries as other tools are met. Each one needs a caught case and a not-a-finding case in `setup.rs`.
- Detection reads the whole output of a failed step, not only the last 30 lines.
