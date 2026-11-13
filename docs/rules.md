# Built-in rules

The `core` pack is on by default. Every rule is report-only (`evaluated`) until you set it to `enforced` in your config. `fscale explain <rule>` prints the full description and how to fix a violation; ADR 0012 records exactly what each rule treats as a violation.

| Rule                        | Checks                                                                      | Severity | `fscale fix` |
| --------------------------- | --------------------------------------------------------------------------- | -------- | ------------ |
| `core/no-committed-env`     | No `.env` files are tracked by git, except examples such as `.env.example`. | blocker  | yes          |
| `core/no-secrets-in-source` | No well-known key or token formats in tracked files.                        | blocker  | no           |
| `core/single-lockfile`      | Exactly one JavaScript lockfile, matching `packageManager`.                 | high     | no           |
| `core/gitignore-basics`     | `.gitignore` exists and covers the detected languages.                      | medium   | yes          |
| `core/ci-exists`            | A CI workflow exists and runs on pull requests.                             | medium   | yes¹         |
| `core/env-example`          | Env vars read in code are listed in the nearest `.env.example`.             | medium   | yes          |
| `core/scripts-present`      | `package.json` has `lint`, `test` and `build` scripts.                      | medium   | no           |
| `core/readme-setup`         | The README shows how to install and how to run.                             | low      | yes¹         |

¹ Only creates a file that is absent: a starter `.github/workflows/ci.yml` when the repo has no CI config at all, and a skeleton `README.md` when the repo root has no README of any kind. The new file is staged with `git add`. A file that exists on disk is never overwritten or edited, so a workflow without a pull request trigger, or a README without install and run steps, is left for a person or an agent.

## The `agent-ready` pack

Not on by default. Add `"agent-ready"` to `packs` in your config. It checks that a coding agent can work in the repo and prove its work (ADR 0014).

| Rule                              | Checks                                                                                                                                  | Severity | `fscale fix` |
| --------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------- | -------- | ------------ |
| `agent-ready/agents-md`           | `AGENTS.md` has firescale's managed block of commands and the definition of done, up to date; a root `CLAUDE.md` imports it (ADR 0027). | medium   | yes          |
| `agent-ready/agent-permissions`   | Claude Code, Codex and Cursor settings deny reading env files and keys and allow the repo's check commands (ADR 0027).                  | medium   | yes          |
| `agent-ready/agent-skills`        | The SKILL.md of every enabled rule is in `.claude/skills/`, `.agents/skills/` and `.cursor/skills/` (ADR 0027).                         | low      | yes          |
| `agent-ready/single-test-command` | One command runs the tests: a `test` script, Cargo, pytest config or `make test`.                                                       | medium   | no           |
| `agent-ready/verify-hook`         | An agent stop hook runs `fscale verify` (see [hooks.md](hooks.md)).                                                                     | medium   | yes          |
| `agent-ready/commands-documented` | `AGENTS.md` mentions the detected build, test, lint and typecheck commands.                                                             | low      | no           |

## The `group-project` pack

Not on by default. Add `"group-project"` to `packs`, or start a repo with `fscale init --packs core,group-project`. It gives a team the files and checks that let several people work on one repo (SPEC §17, ADR 0023).

| Rule                         | Checks                                                                                                                                         | Severity | `fscale fix`                                            |
| ---------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- | -------- | ------------------------------------------------------- |
| `group-project/pr-template`  | A pull request template exists: `pull_request_template.*` or a `PULL_REQUEST_TEMPLATE/` folder in the root, `docs/` or `.github/`.             | low      | creates `.github/pull_request_template.md`              |
| `group-project/contributing` | A contributing guide exists: `CONTRIBUTING` or `CONTRIBUTING.*` in the root, `docs/` or `.github/`.                                            | medium   | creates `CONTRIBUTING.md` from `fscale facts`           |
| `group-project/ci-verify`    | A GitHub Actions workflow that runs on pull requests has a `run:` step whose command is `fscale verify`.                                       | medium   | creates the starter workflow when there is no CI at all |
| `group-project/branch-name`  | The current branch's name matches the team's patterns. In a pull request run on GitHub Actions, the pull request's branch (`GITHUB_HEAD_REF`). | low      | no                                                      |

GitHub's rules were read from its documentation's source on 2026-11-12: both files may live in the root, `docs/` or `.github/`, and their names are not case sensitive. In a git repo a file only counts when git tracks it, because teammates get it through a commit.

- A file a person wrote is never changed. A workflow without verify is left for an agent or a person, with the one step to add.
- `group-project/ci-verify` runs after `core/ci-exists`: when both are on, `core/ci-exists` writes the starter workflow, and it already runs verify.
- `group-project/branch-name` always allows the repo's own main branch (`init.defaultBranch`, `origin/HEAD`), and is never cached, since the branch is not a file. Outside git, or with a detached HEAD outside a pull request run, it reports nothing.
- The guide's commands come from `fscale facts`. A Python command that names this machine's `.venv` is written as `python -m ...`, so it works for every teammate.

## Options

| Rule                         | Option         | Meaning                                                                                                                                                                                                                       |
| ---------------------------- | -------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `core/no-committed-env`      | `allow`        | Env file names that may be committed. Default `[".env.example"]`.                                                                                                                                                             |
| `core/no-secrets-in-source`  | `allowFiles`   | Repo-relative files to skip.                                                                                                                                                                                                  |
| `core/env-example`           | `ignore`       | Extra variable names that need not be listed.                                                                                                                                                                                 |
| `core/env-example`           | `excludePaths` | Repo-relative path prefixes that are not scanned, for example fixtures.                                                                                                                                                       |
| `core/scripts-present`       | `required`     | Script names to require instead of `lint`, `test`, `build`.                                                                                                                                                                   |
| `agent-ready/verify-hook`    | `agents`       | Agents that must each have the hook. Default: any one of the three.                                                                                                                                                           |
| `group-project/branch-name`  | `branches`     | Branch name patterns, `*` is any text. Default: `main`, `master`, `develop`, and `feat/*`, `fix/*`, `docs/*`, `test/*`, `refactor/*`, `chore/*`, `ci/*`, `build/*`, `perf/*`, `style/*`, `release/*`, `hotfix/*`, `revert/*`. |
| `group-project/contributing` | `branches`     | The patterns written into the guide. Set it to the same list as `branch-name`'s.                                                                                                                                              |

To silence one line for the secrets rule, put `firescale:allow-secret` in a comment on that line.

## `core/env-example` in detail

`fscale explain core/env-example` prints the full text. In short:

- **Nearest example file.** A read is fine when a `.env.example` in the reading file's folder, or in any folder above it up to the repo root, lists the name. A sibling folder's file does not count. A missing name belongs in the nearest `.env.example` at or above the reading file, or in the root one when there is none. The finding names that file and `fscale fix` adds the name there.
- **Test-only variables.** A name that only test code reads is reported at low severity. Test code is anything below a folder named `test`, `tests`, `__tests__`, `unit_test`, `unit_tests`, `integration_test`, `integration_tests`, `e2e` or `cypress`, plus `conftest.py`, `test_*.py`, `*_test.py`, `*_test.go`, `*_spec.rb`, `*_test.rb`, `*.test.*` and `*.spec.*` files, and the config and setup files of test runners (`playwright.config.*`, `playwright-ct.config.*`, `vitest.config.*`, `vitest.workspace.*`, `vitest.setup.*`, `jest.config.*`, `jest.setup.*`, `cypress.config.*`, and named configs such as `vitest.e2e.config.ts`), in any letter case. `fscale fix` writes these names under their own comment, `# Test-only variables: read only by test code.`
- **Example files are read from disk.** A `.env.example` that git does not track yet still counts, in the root and in nested folders.

### Variables that are always ignored

The operating system, the shell, CI services and development tools set these, so the rule never asks for them. The list lives in `crates/rules/src/core/env-example/well_known.rs`, and a test keeps this section equal to it. The `ignore` option adds names. There is no option to remove one.

Exact names: `PATH`, `HOME`, `PWD`, `OLDPWD`, `USER`, `LOGNAME`, `SHELL`, `SHLVL`, `TERM`, `TERM_PROGRAM`, `COLORTERM`, `COLUMNS`, `LINES`, `DISPLAY`, `EDITOR`, `VISUAL`, `PAGER`, `LANG`, `LANGUAGE`, `TZ`, `TMPDIR`, `TEMP`, `TMP`, `SSH_AUTH_SOCK`, `USERPROFILE`, `APPDATA`, `LOCALAPPDATA`, `PROGRAMDATA`, `PROGRAMFILES`, `SYSTEMROOT`, `WINDIR`, `COMSPEC`, `PATHEXT`, `HOMEDRIVE`, `HOMEPATH`, `COMPUTERNAME`, `DOCKER_HOST`, `DOCKER_TLS_VERIFY`, `DOCKER_CERT_PATH`, `DOCKER_BUILDKIT`, `DOCKER_CONFIG`, `DOCKER_CONTEXT`, `CI`, `CONTINUOUS_INTEGRATION`, `GITLAB_CI`, `NO_COLOR`, `FORCE_COLOR`, `CLICOLOR`, `CLICOLOR_FORCE`, `DEBUG`, `HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY`, `ALL_PROXY`, `SSL_CERT_FILE`, `SSL_CERT_DIR`, `NODE_ENV`, `NODE_OPTIONS`, `NODE_PATH`, `NODE_DEBUG`, `NODE_NO_WARNINGS`, `NODE_EXTRA_CA_CERTS`, `JEST_WORKER_ID`, `PYTHONPATH`, `PYTHONHOME`, `PYTHONUNBUFFERED`, `PYTHONDONTWRITEBYTECODE`, `PYTHONIOENCODING`, `PYTHONHASHSEED`, `PYTEST_ADDOPTS`, `VIRTUAL_ENV`, `RUST_BACKTRACE`, `GOPATH`, `GOROOT`, `JAVA_HOME`.

Names that start with: `LC_*`, `XDG_*`, `npm_*`, `GITHUB_*`, `RUNNER_*`, `FSCALE_*`, `CARGO_*`, `RUSTC_*`, `RUSTUP_*`, `PYTEST_*`, `VITEST*`.

## The `nextjs` pack

Opt-in: add `"nextjs"` to `packs`, or `fscale init --packs core,nextjs`. Its fixes build a Next.js App Router starter and repair a project's missing pieces, never replacing a file a person wrote (ADR 0029).

| Rule                  | What it checks                                                                               | Severity | Fix |
| --------------------- | -------------------------------------------------------------------------------------------- | -------- | --- |
| `nextjs/package-json` | `next`, `react`, `react-dom` are dependencies; `dev`, `build`, `start` (and `lint`) scripts. | high     | yes |
| `nextjs/app-router`   | The App Router has a root layout; a new app gets `app/layout.tsx` and `app/page.tsx`.        | high     | yes |
| `nextjs/next-config`  | A `next.config.{ts,mjs,js,cjs}` is at the root.                                              | low      | yes |
| `nextjs/tsconfig`     | A TypeScript project has `tsconfig.json`.                                                    | medium   | yes |
| `nextjs/eslint`       | An ESLint config with `eslint-config-next`.                                                  | medium   | yes |
| `nextjs/gitignore`    | `.gitignore` ignores `.next/` and `next-env.d.ts`.                                           | medium   | yes |

## The `python` pack

Opt-in: add `"python"` to `packs`, or `fscale init --packs core,python`. It works on each Python project `fscale facts` finds, so a repo of several services, each with its own requirements files, `pytest.ini` and `.venv`, is checked service by service; nothing in it creates or asks for a `pyproject.toml` (ADR 0030).

| Rule                   | What it checks                                                                                        | Severity | Fix |
| ---------------------- | ----------------------------------------------------------------------------------------------------- | -------- | --- |
| `python/requirements`  | Each project declares its dependencies; a new one gets `requirements.txt` and `requirements-dev.txt`. | high     | yes |
| `python/test-tools`    | A project with tests declares pytest.                                                                 | medium   | yes |
| `python/tests`         | Each project has tests; a new one gets `app/` and `tests/test_main.py`.                               | medium   | yes |
| `python/pytest-config` | A project with tests has a pytest config in its folder or above.                                      | medium   | yes |
| `python/ruff-config`   | A project that declares ruff has a ruff config in its folder or above.                                | low      | yes |
| `python/gitignore`     | `.gitignore` ignores `__pycache__/`, `.venv/`, `venv/`, `.pytest_cache/` and `.ruff_cache/`.          | medium   | yes |
