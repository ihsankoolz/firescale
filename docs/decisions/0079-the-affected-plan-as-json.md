# 0079 — The affected plan as JSON, with typed reasons

Status: accepted (2026-11-18, Track A of the run after 0.5.4; the owner's brief). Adds to ADR 0032 (`verify --affected`).

## Context

`docs/research/research-build-tools.md`, idea 7, first step. `verify --affected --dry-run` said in words which projects a change would run and why. A CI job that wants to gate on that had to parse the words. `--dry-run --json` was refused.

## Decision

**`fscale verify --affected --dry-run --json` prints the affected plan as JSON. Nothing runs.**

- One object: `schemaVersion` (1), `base`, `changedFiles`, `everything`, `units`, `notes`, `warnings`.
- `units` has the repo root's own steps first (no `project`), then each project, in plan order. Each has `affected`, a `reason`, and the `steps` that have a command, with the command.
- **A reason is `{ "type": ..., "text": ... }`**, with more fields by type: `changed-file` has `files`; `depends-on` has `on` (the folder it depends on) and `sources` (the evidence for the edge); the types for a file that runs everything (`root-lockfile`, `ci-file`, `compose-file`, `config-file`, `unowned-file`, `root-file`, `agent-hook-file`) have `file`; `config-setting` has `steps`. The others: `reads-whole-repo`, `no-base`, `shallow-clone`, `nothing-to-compare`, `root-commands`, `undetected-project`, `not-affected`, `other`.
- `text` is the reason the plan for people prints. The type is set where the words are made, not read back out of them.
- When every unit runs, `everything` is the reason and each detected project carries it. A folder named only in the config keeps `undetected-project`.
- The exit code is 0 whatever is affected.
- Without `--affected`, `--dry-run --json` is still refused: there the plan has no reasons to give. Its message gained one sentence that names the new form.

`Why` in `crates/core/src/graph.rs` is the type. `affected_except` sets it beside each reason. `limit_to_affected`, `ConfigEffect::mark` and `mark_file_readers` carry it into `AffectedPlan.typed`. `format_affected_plan_json` in `crates/core/src/verify.rs` prints it. Schema: `docs/schemas/affected-plan.schema.json`. Types: `AffectedPlan`, `AffectedPlanUnit` and `AffectedReason` in `packages/sdk/src/types.ts`.

## Could this repo's own `changes` job use it?

**Not as it is. I did not change the job.** The `changes` job in `.github/workflows/ci.yml` (ADR 0066) decides from the changed files whether the test jobs run at all.

1. **It runs before fscale exists.** The job is a few lines of shell on a fresh checkout and takes seconds. The plan needs the `fscale` binary, which the test jobs build. Using the published package (`npx firescale`) would make this repo's CI depend on its own last release.
2. **Its rule is narrower than firescale's, on purpose.** Here `docs/rules.md` is read by a Rust test, `docs/schemas/` by a Vitest test, and `README.md` and `AGENTS.md` by the rules `fscale check` runs. firescale counts each of them as a document that runs nothing (ADR 0034). The plan would skip the tests for a change that can break them. That is known gap 1 of ADR 0077 seen from the other side.
3. **There is no `--exit-code`.** The job would have to read the JSON with `jq` to turn it into an output.

What would make it usable: a way to say that a document is an input of a project (the plain extra paths under "Later" in ADR 0076), `--exit-code`, and a decision on point 1.

## Consequences

- One more schema to keep stable. A new reason type may be added without a new `schemaVersion`; the schema's text and the docs say to treat an unknown type as `other`.
- The JSON of a real run (`verify --affected --json`) is as it was: `affected.projects[].reason` stays a string.

## Needs your decision

Nothing blocks. Decided on your behalf: the names of the types; `on` is a folder, not a node id; `steps` lists only steps with a command; the JSON form exists only with `--affected`.

## Not done

- `--exit-code`.
- The typed reason in the JSON of a real run.
- With `--budget`, the JSON does not name the steps a run would leave out; the plan for people does.
- `config-setting` with no steps stands for two things the words tell apart: a setting every step shares, and a base config that could not be read.
- A reason has one type. A unit that holds a changed file and also depends on a changed one is `changed-file`.
- When the repo root's own steps run for a workspace member, their reason is that member's (`changed-file` or `depends-on`), and the member's name is only in `text`.

## From the fresh review

It found no change to what is affected, to any reason's words, to a real run's output or to a key. It found three places where the new output or its docs were not exact: `config-setting` also stands for an unreadable base config; a folder named only in the config does not carry the reason for everything; the refusal without `--affected` has a longer message. The docs, the schema and the types now say each.

## Later

- `--exit-code`: 1 when something is affected, 0 when nothing is.
- The same reasons in `verify --affected --json` and in `fscale graph --affected --json`.
- Replace the hand-written `changes` job, once the three points above are answered.

## Tests

- `packages/cli/src/affected-plan-json.test.ts`, written first and seen failing in WSL: 6 of 6 (the flags were refused; there was no schema). It checks the reasons of a changed project, a dependent and an untouched one, the reason for everything, a document that runs nothing, the output against the schema with Ajv, and that the schema refuses a reason with no type.
