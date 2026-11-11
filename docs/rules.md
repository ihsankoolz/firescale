# Built-in rules

The `core` pack is on by default. Every rule is report-only (`evaluated`) until you set it to `enforced` in your config. `fscale explain <rule>` prints the full description and how to fix a violation; ADR 0012 records exactly what each rule treats as a violation.

| Rule                        | Checks                                                                      | Severity | `fscale fix` |
| --------------------------- | --------------------------------------------------------------------------- | -------- | ------------ |
| `core/no-committed-env`     | No `.env` files are tracked by git, except examples such as `.env.example`. | blocker  | yes          |
| `core/no-secrets-in-source` | No well-known key or token formats in tracked files.                        | blocker  | no           |
| `core/single-lockfile`      | Exactly one JavaScript lockfile, matching `packageManager`.                 | high     | no           |
| `core/gitignore-basics`     | `.gitignore` exists and covers the detected languages.                      | medium   | yes          |
| `core/ci-exists`            | A CI workflow exists and runs on pull requests.                             | medium   | yes¹         |
| `core/env-example`          | Env vars read in code are listed in `.env.example`.                         | medium   | yes          |
| `core/scripts-present`      | `package.json` has `lint`, `test` and `build` scripts.                      | medium   | no           |
| `core/readme-setup`         | The README shows how to install and how to run.                             | low      | yes¹         |

¹ Only creates a file that is absent: a starter `.github/workflows/ci.yml` when the repo has no CI config at all, and a skeleton `README.md` when the repo root has no README of any kind. The new file is staged with `git add`. A file that exists on disk is never overwritten or edited, so a workflow without a pull request trigger, or a README without install and run steps, is left for a person or an agent.

## The `agent-ready` pack

Not on by default. Add `"agent-ready"` to `packs` in your config. It checks that a coding agent can work in the repo and prove its work (ADR 0014).

| Rule                              | Checks                                                                            | Severity | `fscale fix` |
| --------------------------------- | --------------------------------------------------------------------------------- | -------- | ------------ |
| `agent-ready/agents-md`           | `AGENTS.md` exists at the repo root and is not blank.                             | medium   | no           |
| `agent-ready/single-test-command` | One command runs the tests: a `test` script, Cargo, pytest config or `make test`. | medium   | no           |
| `agent-ready/verify-hook`         | An agent stop hook runs `fscale verify` (see [hooks.md](hooks.md)).               | medium   | no           |
| `agent-ready/commands-documented` | `AGENTS.md` mentions the detected build, test, lint and typecheck commands.       | low      | no           |

## Options

| Rule                        | Option         | Meaning                                                                 |
| --------------------------- | -------------- | ----------------------------------------------------------------------- |
| `core/no-committed-env`     | `allow`        | Env file names that may be committed. Default `[".env.example"]`.       |
| `core/no-secrets-in-source` | `allowFiles`   | Repo-relative files to skip.                                            |
| `core/env-example`          | `ignore`       | Extra variable names that need not be listed.                           |
| `core/env-example`          | `excludePaths` | Repo-relative path prefixes that are not scanned, for example fixtures. |
| `core/scripts-present`      | `required`     | Script names to require instead of `lint`, `test`, `build`.             |
| `agent-ready/verify-hook`   | `agents`       | Agents that must each have the hook. Default: any one of the three.     |

To silence one line for the secrets rule, put `firescale:allow-secret` in a comment on that line.
