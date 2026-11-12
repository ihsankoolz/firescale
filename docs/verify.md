# fscale verify

`fscale verify [--affected [--base <ref>]] [--json]` is the one command that says whether work is done. People, CI and agent hooks all call it. It runs these steps in order and stops at the first one that fails:

1. `check`: the firescale rules, exactly as `fscale check` runs them. It fails when an enforced rule has violations.
2. `lint`, `typecheck` and `test` at the repo root.
3. `lint`, `typecheck` and `test` in each project found below the root, in that project's folder.

| Exit code | Meaning                                                                                           |
| --------- | ------------------------------------------------------------------------------------------------- |
| 0         | No step failed. The headline says `OK` when every step ran, and `PARTIAL` when some were skipped. |
| 1         | A step failed, or steps were skipped and `verify.failOnSkipped` is on.                            |
| 2         | Bad usage or a bad config.                                                                        |

## A skipped step is never a clean pass

A step with no command cannot verify anything. It is **skipped**, and verify says so in three places:

- The headline. It reads `verify: OK` only when every step ran and passed. With a skipped step it reads `verify: PARTIAL - 1 of 4 steps ran, 3 skipped`.
- The step's own line, with the reason no command was found.
- A `warning:` line under the summary that names every skipped step and the config key that gives the first one a command.

A partial run still exits 0, so adopting firescale does not break a build on day one. To make skipped steps fail, turn on `failOnSkipped`:

```json
{ "verify": { "failOnSkipped": true } }
```

Then the headline reads `verify: FAILED - 3 of 4 steps skipped, and verify.failOnSkipped is on`, and the exit code is 1.

A step can also be **off**: you set it to `false` in the config, or another step covers it (`cargo clippy` already type-checks). An off step is listed as `off`, is not counted, and does not make a run partial.

## Which commands run

### At the repo root

For each of `lint`, `typecheck` and `test`, the first match wins:

| Order | Source                                             | Command                                                                                              |
| ----- | -------------------------------------------------- | ---------------------------------------------------------------------------------------------------- |
| 1     | `verify.<step>` in the firescale config            | The string you wrote, or `false` to turn the step off.                                               |
| 2     | Nx (`nx.json` exists)                              | `<exec> nx run-many -t <step>`, or `<exec> nx affected -t <step>` with `--affected`.                 |
| 3     | Turborepo (`turbo.json` exists)                    | `<exec> turbo run <step>` (plus `--affected`), only if `turbo.json` defines the task.                |
| 4     | A root `package.json`                              | The script of that name: `<pm> run <step>`. Without a script, the command from a tool's config file. |
| 5     | A Rust-only repo (`Cargo.toml`, no `package.json`) | `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace`.                |
| 6     | A Python or Go project at the root                 | The commands `fscale facts` shows for it, for example `python -m pytest`.                            |

`<exec>` is `pnpm exec`, `yarn`, `bunx` or `npx`, by package manager. `fscale facts` shows the package manager, scripts and tools that were detected, and [facts.md](facts.md) lists which config files give which command.

### In projects below the root

`fscale facts` finds projects in subfolders (see [facts.md](facts.md)). Each one whose `source` is `discovered` runs its own `lint`, `typecheck` and `test`, with the commands `fscale facts` lists for it, **in its own folder**. A project with no command for a step has that step skipped, with the reason.

- Workspace members are not run one by one: the root's commands cover them.
- When the root has no commands of its own (no `package.json`, `Cargo.toml`, Nx, Turborepo, Python or Go project there), it lists no steps, and `runner` is `projects`.
- Projects run in the order of their folders. They always run in full, also with `--affected`.

### Config

```json
{
  "verify": {
    "lint": "pnpm lint",
    "typecheck": false,
    "test": "pnpm test --run",
    "failOnSkipped": true,
    "projects": {
      "backend/auth_service": { "typecheck": false, "test": "python3 -m pytest -q" },
      "docs": false,
      "tools/deep/down/below/level4": { "test": "make test" }
    }
  }
}
```

| Key                                      | Meaning                                                                                                                                                                              |
| ---------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `lint`, `typecheck`, `test`              | A setting for the **whole repo**. A command runs once, from the repo root, and no project runs its own command for that step. `false` turns the step off everywhere.                 |
| `failOnSkipped`                          | `true` makes a skipped step fail the run. Default `false`.                                                                                                                           |
| `projects.<folder>`                      | Settings for one project, by its folder. `false` turns all its steps off. An object sets single steps to a command or to `false`, and wins over the repo-wide setting for that step. |
| `projects.<folder>` for any other folder | A folder `fscale facts` did not list as discovered runs exactly the commands given for it, in that folder. Use it for a project below the search depth.                              |

A folder named under `projects` must exist, or verify stops with exit code 2. The repo root is not a project here: use the top-level keys for it.

Commands run through the platform shell (`sh -c` on macOS and Linux, `cmd /C` on Windows) with `NO_COLOR=1`, and with no input. Root steps run from the repo root, project steps from the project's folder.

`--affected` narrows the `check` step to changed files (see [affected.md](affected.md)) and is passed on to Nx and Turborepo. `--base <ref>` is also passed to `nx affected`; for Turborepo, set `TURBO_SCM_BASE` yourself. Plain package scripts, Cargo and the projects below the root always run in full.

On Windows the command line is handed to `cmd` exactly as written, so `cmd` quoting works (`node -e "..."`, `"C:\Program Files\tool\tool.exe" arg`).

## Output

The summary is short and written for an agent to act on: what failed, the end of its output, the command to run again, and what did not run.

```
verify: FAILED at test
  check      passed   8 rules, no enforced violations
  lint       passed   pnpm run lint
  typecheck  off      turned off in your firescale config
  test       FAILED   pnpm run test (exit 1)

Last lines of test:
  FAIL adds two numbers: expected 4, got 5

Fix the failure, then run again: pnpm run test
```

In a repo with projects below the root, each project's steps are listed under its folder:

```
verify: PARTIAL - 5 of 7 steps ran, 2 skipped
  check      passed   8 rules, no enforced violations
  in backend/auth_service:
    lint       passed   python -m ruff check .
    typecheck  skipped  no type checker was found (looked for mypy)
    test       passed   python -m pytest
  in frontend:
    lint       passed   npx --no oxlint
    typecheck  passed   npx --no tsc --noEmit
    test       skipped  package.json has no test script, and no vitest or jest config was found

warning: steps were skipped because no command was found: typecheck in backend/auth_service, test in frontend. Set verify.projects["backend/auth_service"].typecheck in your firescale config to a command, or to false if that project has none.
```

A failure in a project names it: `verify: FAILED at test in backend/auth_service`, and `Fix the failure, then run again in backend/auth_service: python -m pytest`.

At most the last 30 non-empty lines of the failed command are kept, with colour codes removed. Steps after the failure are listed as `not run`. When `check` is the step that failed, the lines are the violations of enforced rules, one per line, with the same cap; report-only rules are left out.

`--json` prints the format described by `packages/cli/schemas/verify-output.schema.json` (JSON Schema draft-07, published as `firescale/schemas/verify-output.schema.json`).

| Field           | Meaning                                                                                                                                                                                                                                 |
| --------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `schemaVersion` | Always `2`. Changes only on a breaking change. Version 2 added `result`, the counts, `skipped`, `failOnSkipped`, `failedProject`, the `project` of a step, and the step status `off`.                                                   |
| `version`       | The firescale version.                                                                                                                                                                                                                  |
| `ok`            | `true` when the exit code is 0: `result` is `ok` or `partial`.                                                                                                                                                                          |
| `result`        | `ok` (every step ran and passed), `partial` (nothing failed, at least one step skipped) or `failed` (a step failed, or steps were skipped with `failOnSkipped` on). **Read this, not `ok` alone.**                                      |
| `runner`        | How the root's commands were chosen: `config` (all three steps set in the config), `nx`, `turbo`, `scripts`, `cargo`, `python`, `go`, `projects` (only the projects below the root) or `none`.                                          |
| `stepsTotal`    | Steps that should run: every step except those that are `off`.                                                                                                                                                                          |
| `stepsRun`      | Steps that did run, passing or failing.                                                                                                                                                                                                 |
| `failOnSkipped` | The config setting.                                                                                                                                                                                                                     |
| `failedStep`    | The first failed step. Absent when no step failed.                                                                                                                                                                                      |
| `failedProject` | The folder of the project whose step failed. Absent for a root step.                                                                                                                                                                    |
| `rerun`         | The command to run again, from the folder in `failedProject` when that is present. Absent when no step failed.                                                                                                                          |
| `steps`         | `check` first, then the root's steps, then each project's. Each has `name`, `status` (`passed`, `failed`, `skipped`, `not-run`, `off`), `durationMs`, `detail`, and where they apply `project`, `command`, `exitCode` and `outputTail`. |
| `skipped`       | Every skipped step: `step`, `reason`, and `project` for a project's step. Empty when nothing was skipped.                                                                                                                               |
| `warnings`      | One sentence naming the skipped steps. Absent when there are none.                                                                                                                                                                      |

## The stop hook

`fscale hooks run` blocks an agent only when verify fails. On a partial run the agent may stop, and the hook prints a line on stderr that says the run was not a clean pass and names the skipped steps. With `verify.failOnSkipped` on, a partial run is a failure and the hook blocks. ADR 0020 explains the default.

## Limits

- There is no time limit on a step. A command that hangs makes `verify` hang.
- Output is captured, not streamed, so nothing is printed until a step ends.
- With Nx, the task names must be `lint`, `typecheck` and `test`. Use the `verify` config key for other names.
- A project's commands run with the tools found on the PATH. A Python service needs its virtual environment active, or its own command under `verify.projects`.
- Projects run one after another, not in parallel.
- A step that is skipped in every run stays in the summary until it gets a command or is set to `false`.
- `verify` is as fast as the repo's own commands. firescale's share is a few milliseconds (see `bench/RESULTS.md`).
