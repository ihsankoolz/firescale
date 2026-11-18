# 0068 — In doctor, a variable that is set nowhere is a warning; `doctor.missingEnv` makes it a problem

Status: accepted (2026-11-18, 0.5.3 run; the owner's decision). Amends ADR 0026 (doctor) and ADR 0059 (env files in the folders above).

## Context

`fscale doctor` reads the names in each `.env.example` and looks for each name in the environment and in the `.env` and `.env.local` files at or above the project (ADR 0059). Until now a name found nowhere was a `PROBLEM`, and doctor exited 1.

That was too strong. A `.env.example` lists every name the project can read, and many are needed only for some work: a mail server, a payment key, a tracing endpoint. On the six-project course repo of the 0.5.1 review, doctor reported problems for names that no test and no local run needed, on a machine that could build and test everything. A check that says "not ready" on a ready machine teaches people to ignore it. Doctor cannot tell which names are needed: that is in the project's code, which it does not read.

## Decision

- **A name that a `.env.example` lists and that is set nowhere is a `warning`.** The line and its `next:` step are the same as before; only the label changes. With no other problem, doctor ends with `doctor: OK with N warnings - read them above.` and exits 0. In the JSON the check has `"status": "warning"` and counts in `warnings`, not in `problems`.
- **A new config section, `doctor`, with one key: `missingEnv`**, `"warning"` (the default) or `"problem"`. With `"problem"` the check is a `PROBLEM` as before and doctor exits 1. A team that keeps its `.env.example` to the names every machine needs sets it.
- **Any other value, another key in `doctor`, or a `doctor` that is not an object is a config error** (exit 2), in `fscale doctor` and in every command that loads the config, like the other sections. `null` means the default.
- **Doctor reads the setting from `firescale.config.json` only.** It reads that one section and checks nothing else in the file. Doctor runs no code from the repo, only version commands, and a `firescale.config.ts` is code: it would need Node, which doctor may be run to find missing. So with a `firescale.config.ts` the default stands, and when a name is missing doctor adds one more warning that says so: `firescale.config.ts: doctor does not run firescale.config.ts (it is code), so doctor.missingEnv in it is not read: a variable that is set nowhere is a warning.`
- **The setting is in no step's cache key** (ADR 0058: `out`). Verify does not read it.
- The shape of the JSON output does not change, so its schema version stays 2.

## Consequences

- **This changes the exit code of `fscale doctor` on repos with a missing name: 1 becomes 0.** A script that used doctor's exit code to stop on a missing variable must set `doctor.missingEnv` to `"problem"`. The release notes say so.
- **A repo whose config is `firescale.config.ts` cannot make a missing name a problem.** The two config files cannot be kept together, so the only way is to move the config to JSON. Doctor says that the setting was not read, but only when a name is missing. Loading the TypeScript config from doctor (through Node, and falling back to the default when Node is missing) would lift this limit; it was left out to keep doctor's promise that it runs no code from the repo.
- A `firescale.config.json` that is not valid JSON now stops doctor with the same error as the other commands (exit 2). Before, doctor never read the file.
- A name that really is needed and is missing no longer fails doctor by default. The line is still printed with its next step, and the summary line sends the reader to the warnings.
- The services check is not changed: a local database that a `.env.example` names and that is not reachable is still a problem.
- Tests: `doctor and a variable that is set nowhere` in `packages/cli/src/doctor.test.ts` (default, `"warning"`, `"problem"`, the refused values, the TypeScript config), `a_variable_set_nowhere_is_a_warning_unless_the_config_makes_it_a_problem` in `crates/core/src/doctor.rs`, `checks_the_doctor_settings` in `crates/core/src/config.rs`, and the row `doctor.missingEnv` in the table of ADR 0058 (`crates/core/src/configkey.rs`). The older doctor tests that expected `PROBLEM` for a missing name now expect `warning`; where they expected exit 1 they now expect the exit code to follow the problems that are printed.
