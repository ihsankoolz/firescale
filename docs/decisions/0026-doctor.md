# 0026 — `fscale doctor`

Status: accepted (M12). Builds on ADR 0025 (`services.rs`) and ADR 0022 (the Python interpreter of each project).

## Context

SPEC §17 asks for `fscale doctor`: check a teammate's machine (tool versions against what the repo declares, every variable named in `.env.example` with names only, installed dependencies), each problem with the command that fixes it. The owner added three things after running 0.1.3 on the course project: local services from Compose files and env examples (reachable or not, with the start command, never a password or connection string), per-project checks (virtual environment, dependencies matching the lockfile or requirements, tool versions), and a hint from `verify` when a step needs setup.

## Decision

- **Rust, in `crates/core/src/doctor.rs`**, as SPEC §9 says (`doctor` is Rust-owned). No new dependency. It reads `fscale facts` for the projects, their kinds, install commands and interpreters, so doctor and verify agree on what a project is.
- **Four sections, fixed order:** tools, env, dependencies, services. Each check is `ok`, `problem` or `warning`; doctor exits 1 when there is a problem, 0 otherwise (warnings included). Usage errors exit 2 like every command.
- **It changes nothing.** It runs version commands only, through the platform shell so `pnpm.cmd` is found on Windows, with `COREPACK_ENABLE_DOWNLOAD_PROMPT=0`. `rustup toolchain list` runs in the repo's parent folder, because rustup reads `rust-toolchain.toml` from the current folder and may download a toolchain there.
- **Version rules.** npm ranges and Python specifiers are read by one small matcher (`satisfies`): `>=`, `>`, `<=`, `<`, `=`, `!=`, `^`, `~`, `~=`, `x`/`*` wildcards, `||`, hyphen ranges, spaces and commas as "and". A bare version (`22`, `3.12`) matches by prefix. A range it cannot read is a warning, never a problem. For `packageManager`, a different major version is a problem and the same major version is a warning, because corepack or a global install of the same major version usually works and a hard failure there would push people to edit the field.
- **Env variables:** names only. A local env file (`.env`, `.env.local` beside the example) is read line by line and only the part before `=` is kept. A name counts as set when the environment has it with a non-empty value, or the local file lists it (even empty: a person chose that). One check per `.env.example`, listing the missing names.
- **Dependencies:**
  - Node: each direct dependency resolved through `node_modules` up to the root (as Node does), then the lockfile's time against the install marker the package manager writes. All missing is "not installed"; some missing are listed.
  - Python: the venv (`.venv` or `venv`; for a uv workspace member, the workspace root's `.venv`), its interpreter must run, and the packages of the requirements files named by the project's install command (else every `requirements*.txt`, else `[project] dependencies`) must be in its `.dist-info` folders, with `==` pins matching. The fix is the project's install command; with no venv, the commands that make one and install into it. A repo with several services, each with its own requirements and `.venv`, gets one set of checks per service, and doctor never suggests a root `pyproject.toml`.
  - Rust and Go: not checked (their tools fetch on build).
- **Services:** from Compose files (with their override files) and `.env.example`, via `services.rs` (ADR 0025). A Compose service counts only when it is a known database, cache, queue or search service, or an env example names its port. Found on `fastapi/full-stack-fastapi-template`: without that filter, doctor listed the proxy, the backend, adminer, mailpit and playwright containers as services that must run. Only local hosts. One TCP connection with a 400 ms limit per service. Not reachable is a problem, because tests that need the service cannot pass; the fix is `docker compose up -d <service>`, else a `docker run` line with a `<password>` placeholder.
- **JSON schema version 1**, `packages/cli/schemas/doctor-output.schema.json`, snapshot-tested.
- **Verify's hint:** when any step is `setup-needed`, the human summary ends with one line telling the reader to run `fscale doctor`. The JSON does not change.
- **Found on a real repo and fixed in facts:** a Python project that is a member of a uv workspace (`[tool.uv.workspace]` in a `pyproject.toml` above it, `uv.lock` beside that) is a uv project. It was a pip project with `python` from the PATH.

## Consequences

- The publishing checkpoint the plan names for this milestone is met: 0.1.0 to 0.1.3 are on npm, so a teammate can run `npx firescale doctor`. Its second box (a generated repo passing its own CI on GitHub) is still open, as before.
- Listed under "Needs your decision": unreachable services are problems (exit 1), the `packageManager` major-version rule, and `.env`/`.env.local` as the only local env files.
- Limits are in `docs/doctor.md`: no `volta`, `.tool-versions` or lockfile-level version comparison; the Compose reader is a line reader.
