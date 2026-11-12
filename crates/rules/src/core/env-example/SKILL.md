---
name: env-example
description: Fixes violations of the firescale rule core/env-example by listing every env var the code reads in .env.example. Use when fscale check reports core/env-example.
---

# core/env-example

## What this rule enforces and why

If tracked source code reads an environment variable, a `.env.example` at or above the reading file lists it. The rule scans `.js`, `.jsx`, `.mjs`, `.cjs`, `.ts`, `.tsx`, `.mts`, `.cts`, `.py`, `.rs`, `.go` and `.rb` files (not `node_modules`, `dist`, `build`, `target`, `vendor` or `.git`) for reads such as `process.env.NAME`, `import.meta.env.NAME`, `os.environ["NAME"]`, `os.getenv("NAME")`, `env::var("NAME")`, `os.Getenv("NAME")` and `ENV["NAME"]`. Only upper-case names count.

Without an example file, a new developer finds out which variables are needed only when the app crashes.

### Which `.env.example` counts

A read is fine when any `.env.example` in the reading file's folder, or in a folder above it up to the repo root, lists the name. A read in `frontend/src/api.ts` is satisfied by `frontend/src/.env.example`, `frontend/.env.example` or the root `.env.example`. It is not satisfied by `backend/.env.example`. So the same name can be fine in one folder and a finding in another.

A missing name belongs in the nearest `.env.example` at or above the reading file. If there is none, it belongs in the root `.env.example`. Each finding names that file. Names read in several folders that share the same nearest file give one finding per name.

Example files are looked up on disk, so one that git does not track yet still counts. This is the same for the root file and for nested ones.

If the root `.env.example` does not exist, all names that belong in it share one finding on `.env.example`. If the root `.env.example` is a folder, the rule reports only that, and the fix does nothing. A nested `.env.example` that is a folder counts as absent.

### Test-only variables

A name that only test code reads is reported at low severity, and the message says so. A name read by test code and by other code is a normal finding that points at the read outside the tests. This is decided per example file: only the reads that belong to that file count.

Test code is:

- any file below a folder named `test`, `tests`, `__tests__`, `unit_test`, `unit_tests`, `integration_test`, `integration_tests`, `e2e` or `cypress`;
- a file named `conftest.py`;
- Python files named `test_*.py` or `*_test.py`;
- Go files named `*_test.go`;
- Ruby files named `*_spec.rb` or `*_test.rb`;
- files named `*.test.*` or `*.spec.*`, for example `api.test.ts`;
- the config or setup file of a test runner: a name that starts with `playwright.config.`, `playwright-ct.config.`, `vitest.config.`, `vitest.workspace.`, `vitest.setup.`, `jest.config.`, `jest.setup.` or `cypress.config.`, or a named config such as `vitest.e2e.config.ts`, in any letter case.

When the root `.env.example` does not exist, the one shared finding has low severity only if every name in it is test-only.

### Variables that are always ignored

The operating system, the shell, CI services and development tools set these. A project does not define them, so the rule never asks for them.

Exact names: `PATH`, `HOME`, `PWD`, `OLDPWD`, `USER`, `LOGNAME`, `SHELL`, `SHLVL`, `TERM`, `TERM_PROGRAM`, `COLORTERM`, `COLUMNS`, `LINES`, `DISPLAY`, `EDITOR`, `VISUAL`, `PAGER`, `LANG`, `LANGUAGE`, `TZ`, `TMPDIR`, `TEMP`, `TMP`, `SSH_AUTH_SOCK`, `USERPROFILE`, `APPDATA`, `LOCALAPPDATA`, `PROGRAMDATA`, `PROGRAMFILES`, `SYSTEMROOT`, `WINDIR`, `COMSPEC`, `PATHEXT`, `HOMEDRIVE`, `HOMEPATH`, `COMPUTERNAME`, `DOCKER_HOST`, `DOCKER_TLS_VERIFY`, `DOCKER_CERT_PATH`, `DOCKER_BUILDKIT`, `DOCKER_CONFIG`, `DOCKER_CONTEXT`, `CI`, `CONTINUOUS_INTEGRATION`, `GITLAB_CI`, `NO_COLOR`, `FORCE_COLOR`, `CLICOLOR`, `CLICOLOR_FORCE`, `DEBUG`, `HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY`, `ALL_PROXY`, `SSL_CERT_FILE`, `SSL_CERT_DIR`, `NODE_ENV`, `NODE_OPTIONS`, `NODE_PATH`, `NODE_DEBUG`, `NODE_NO_WARNINGS`, `NODE_EXTRA_CA_CERTS`, `JEST_WORKER_ID`, `PYTHONPATH`, `PYTHONHOME`, `PYTHONUNBUFFERED`, `PYTHONDONTWRITEBYTECODE`, `PYTHONIOENCODING`, `PYTHONHASHSEED`, `VIRTUAL_ENV`, `RUST_BACKTRACE`, `GOPATH`, `GOROOT`, `JAVA_HOME`.

Names that start with: `LC_*`, `XDG_*`, `npm_*`, `GITHUB_*`, `RUNNER_*`, `FSCALE_*`, `CARGO_*`, `RUSTC_*`, `RUSTUP_*`, `PYTEST_*`, `VITEST*`.

### Options

- `ignore`: a list of extra variable names to skip, for example `["EXTRA_VAR"]`. It adds to the list above.
- `excludePaths`: a list of repo-relative paths (forward slashes) that are not scanned, for example `["tests/fixtures"]`. Matching is by whole path segment: an entry, with or without a trailing `/`, excludes the path equal to it and everything under it. `["src/a"]` excludes `src/a/x.ts` but not `src/ab/y.ts`.

### Limits

- Lines whose trimmed text starts with `#`, `//`, `*` or `/*` are skipped as comments. This filter is cheap, so reads inside multi-line strings or docstrings can still be picked up.
- Destructuring (`const { FOO } = process.env`) and computed names (`process.env[name]`) are not detected.
- The always-ignored list cannot be switched off. A name on it is never asked for, even if the project wants it listed. You can still list it by hand.
- Test code is found by path only. A test in an unusual folder counts as other code.

## Manual fix

1. Read the finding. It names the `.env.example` that should list the variable.
2. Create that file if it does not exist.
3. Add one line per missing variable, for example `EXAMPLE_API_URL=`. Leave the value empty or use an obvious placeholder. Never put a real value in this file.
4. Commit the `.env.example`. Do not commit a real `.env` file.

## Agent fix instructions

Never read, print or copy the contents of a real `.env` file. Only `.env.example` files may be read or edited.

### DETERMINISTIC

Run `fscale fix --rules core/env-example`, or do the same by hand:

1. For each diagnostic, open the `.env.example` its message names. That is the nearest one at or above the reading file. Create the root `.env.example` only when no example file exists above the read.
2. Append one `NAME=` line for each variable the file does not list yet, sorted by name, with no values. When the file already has content, put them under the comment line `# Variables read by the code. Add example values or leave empty.`
3. Put names that only test code reads after those, under their own comment line `# Test-only variables: read only by test code.`
4. Keep existing lines and their line endings. Do not add a name twice. A name the file already lists stays where it is, also when it was added as test-only and other code reads it now.

### REPORT

- The names that were added, and to which file.
- Which of them are test-only.
- Any variable that probably needs a documented default or format; tell the user, and do not invent values.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules core/env-example
```
