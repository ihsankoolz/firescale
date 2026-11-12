# 0030 — The `python` stack pack

Status: accepted (M16). Follows the stack-pack shape in PLAN.md and ADR 0019; reads Python projects the way ADR 0020 and 0022 find them.

## What the pack assumes (read on 2026-11-12)

The docs sites were blocked from the build session; the facts come from the sources they are built from (pytest, pip, packaging.python.org and ruff on GitHub, pinned to commits) and from PyPI's JSON API.

- **Tools.** pytest 9.2.0 (needs Python 3.10 or later) and ruff 0.17.5, pinned with `==` in `requirements-dev.txt`, as the packaging guide pins.
- **pytest.** `pytest.ini` with `[pytest]` is still read (only pytest 9's `pytest.toml` takes precedence). `pythonpath = .` (pytest 7 and later) lets both `pytest` and `python -m pytest` import the project's own packages from any working folder; `testpaths = tests`; `addopts = -ra`. pytest looks for its config in the folders above, so a config there counts.
- **requirements.** `requirements-dev.txt` starts with `-r requirements.txt`, which pip resolves relative to the file itself.
- **venv.** `python -m venv .venv`; the interpreter is `.venv/bin/python`, or `.venv\Scripts\python.exe` on Windows. fscale's verify already uses it (ADR 0022).
- **ruff.** `ruff.toml` with `line-length = 100` and `[lint] select = ["E", "F", "W", "I"]`. Ruff uses the nearest config, so one per service works.
- **Several services.** Each service runs its own tests with its own `.venv`; one pytest run over two services with the same package or test names fails with "import file mismatch". So the pack works service by service.

## Decision

- **Per project.** Every rule works on each Python project `fscale facts` finds. A repo of services with `requirements.txt`, `requirements-dev.txt`, `pytest.ini` and their own `.venv`, and no `pyproject.toml`, passes as it is, and a gap in one service is fixed in that service's folder. **Nothing in the pack creates or asks for a `pyproject.toml`.** Only a repo with no Python project at all (or a root project with no Python code yet) gets a starter at its root.
- **The starter** (`fscale init --packs core,python`): `requirements.txt` (empty, with a comment), `requirements-dev.txt`, `pytest.ini`, `ruff.toml`, `app/__init__.py`, `app/main.py`, `tests/test_main.py`, and `.gitignore` lines. Plain pip and a venv; no uv or Poetry needed.
- **Rules.**

| Rule                            | Checks                                                                                   | Fix                                                                                                                       |
| ------------------------------- | ---------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| `python/requirements` (high)    | each project declares its dependencies (requirements, pyproject, Pipfile, setup.py/cfg)  | the starter's two files; an existing project with none is an agent finding                                                |
| `python/test-tools` (medium)    | a project with tests declares pytest                                                     | creates `requirements-dev.txt` when only `requirements.txt` exists; a list a person wrote is never edited (agent finding) |
| `python/tests` (medium)         | each project has tests                                                                   | the starter's app and test; otherwise an agent finding                                                                    |
| `python/pytest-config` (medium) | a project with tests has a pytest config here or above                                   | creates `pytest.ini` in the project's folder                                                                              |
| `python/ruff-config` (low)      | a project that declares ruff has a ruff config here or above                             | creates `ruff.toml` in the project's folder                                                                               |
| `python/gitignore` (medium)     | `.gitignore` ignores `__pycache__/`, `.venv/`, `venv/`, `.pytest_cache/`, `.ruff_cache/` | appends them in the file's line endings                                                                                   |

- **Found on real repos and fixed:** on `GoogleCloudPlatform/microservices-demo`, services with code and no tests were offered a `pytest.ini` and a `requirements-dev.txt`, which only added files. Those two rules now wait until a project has tests; the tests finding comes first.

## Proof

- `init --packs core,python` in an empty folder leaves no violation of any rule (tests). Run by hand on 2026-11-12 (Linux, Python 3.13.18): `python3 -m venv .venv`, `.venv/bin/python -m pip install -r requirements-dev.txt`, `.venv/bin/python -m pytest` (1 passed), `.venv/bin/python -m ruff check .` (All checks passed!), all exit 0; `fscale verify` there runs check, lint and test, all passed.
- `adopt --dry-run` on two services of the course project's shape proposes no change, and exactly `backend/payments_service/pytest.ini` when that is missing, with nothing at the root (tests).

## Consequences

- Opt-in: `"packs": [..., "python"]`.
- `fscale verify` in the starter is `PARTIAL`: no type checker is declared, so typecheck is skipped. Listed under "Needs your decision" (add mypy to the starter, or not).
- Projects that use uv or Poetry pass when they declare their dependencies and pytest; the pack does not create their lockfiles.
