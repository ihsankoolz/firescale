# fscale verify

`fscale verify [--affected [--base <ref>]] [--json]` is the one command that says whether work is done. People, CI and agent hooks all call it. It runs four steps in order and stops at the first one that fails:

1. `check`: the firescale rules, exactly as `fscale check` runs them. It fails when an enforced rule has violations.
2. `lint`
3. `typecheck`
4. `test`

Exit code 0 means no step failed. Exit code 1 means a step failed. Exit code 2 means bad usage or a bad config.

## Which commands run

For each of `lint`, `typecheck` and `test`, the first match wins:

| Order | Source                                             | Command                                                                               |
| ----- | -------------------------------------------------- | ------------------------------------------------------------------------------------- |
| 1     | `verify.<step>` in the firescale config            | The string you wrote, or `false` to skip the step.                                    |
| 2     | Nx (`nx.json` exists)                              | `<exec> nx run-many -t <step>`, or `<exec> nx affected -t <step>` with `--affected`.  |
| 3     | Turborepo (`turbo.json` exists)                    | `<exec> turbo run <step>` (plus `--affected`), only if `turbo.json` defines the task. |
| 4     | A root `package.json`                              | The script of that name: `<pm> run <step>`.                                           |
| 5     | A Rust-only repo (`Cargo.toml`, no `package.json`) | `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace`. |

`<exec>` is `pnpm exec`, `yarn`, `bunx` or `npx`, by package manager. A step with no command is **skipped**, with the reason in the summary. A skipped step is not a failure: a repo with nothing to run passes. `fscale facts` shows the package manager and scripts that were detected.

```json
{
  "verify": {
    "lint": "pnpm lint",
    "typecheck": false,
    "test": "pnpm test --run"
  }
}
```

Commands run from the repo root through the platform shell (`sh -c` on macOS and Linux, `cmd /C` on Windows) with `NO_COLOR=1`, and with no input.

`--affected` narrows the `check` step to changed files (see [affected.md](affected.md)) and is passed on to Nx and Turborepo. `--base <ref>` is also passed to `nx affected`; for Turborepo, set `TURBO_SCM_BASE` yourself. Plain package scripts and Cargo always run in full.

On Windows the command line is handed to `cmd` exactly as written, so `cmd` quoting works (`node -e "..."`, `"C:\Program Files\tool\tool.exe" arg`).

## Output

The summary is short and written for an agent to act on: what failed, the end of its output, and the command to run again.

```
verify: FAILED at test
  check      passed   8 rules, no enforced violations
  lint       passed   pnpm run lint
  typecheck  skipped  package.json has no typecheck script
  test       FAILED   pnpm run test (exit 1)

Last lines of test:
  FAIL adds two numbers: expected 4, got 5

Fix the failure, then run again: pnpm run test
```

At most the last 30 non-empty lines of the failed command are kept, with colour codes removed. Steps after the failure are listed as `not run`. When `check` is the step that failed, the lines are the violations of enforced rules, one per line, with the same cap; report-only rules are left out.

`--json` prints the format described by `packages/cli/schemas/verify-output.schema.json` (JSON Schema draft-07, published as `firescale/schemas/verify-output.schema.json`).

| Field           | Meaning                                                                                                                                                         |
| --------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `schemaVersion` | Always `1`. Changes only on a breaking change.                                                                                                                  |
| `version`       | The firescale version.                                                                                                                                          |
| `ok`            | `true` when no step failed.                                                                                                                                     |
| `runner`        | `config` (all three steps set in the config), `nx`, `turbo`, `scripts`, `cargo` or `none`.                                                                      |
| `failedStep`    | The first failed step. Absent when `ok` is true.                                                                                                                |
| `rerun`         | The command to run again. Absent when `ok` is true.                                                                                                             |
| `steps`         | Four entries: `name`, `status` (`passed`, `failed`, `skipped`, `not-run`), `durationMs`, `detail`, and where they apply `command`, `exitCode` and `outputTail`. |

## Limits

- There is no time limit on a step. A command that hangs makes `verify` hang.
- Output is captured, not streamed, so nothing is printed until a step ends.
- With Nx, the task names must be `lint`, `typecheck` and `test`. Use the `verify` config key for other names.
- `verify` is as fast as the repo's own commands. firescale's share is a few milliseconds (see `bench/RESULTS.md`).
