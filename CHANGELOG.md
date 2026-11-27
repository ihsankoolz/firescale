# Changelog

What changes for a user in each release. The reasons are in `docs/decisions/` (ADR numbers below) and `DECISIONS.md`. Releasing is in `RELEASING.md`.

## 0.6.0 (pre-release 0.6.0-rc.1)

Compared with 0.5.3, the last release on npm. 0.5.4 was never released; what it held is here. A pre-release is published under the npm dist-tag `next` only: `npx firescale` keeps giving 0.5.3, `npx firescale@next` or `npx firescale@0.6.0-rc.1` gives this one.

### What changes for a user

Each line ends with the command that shows it.

**Verify: what passes and what fails**

- **A failing lint no longer stops the tests of its project** (ADR 0070). A failing `typecheck` still stops them when there is no base to compare with. See it: `fscale verify` on a branch with a lint error.
- **A lint command fscale guessed only reports** (ADR 0070): `cargo clippy`, `go vet`, `dotnet format`, `dart analyze`, `flutter analyze` and Gradle's Android `lint`, unless the repo declares them (in its CI, or for some a config file). A failing guessed lint shows as `report-only`, does not fail the run, and the summary gives the config line that makes it block. See it: `fscale verify`.
- **A Maven build with modules is tested with `mvn -B verify`** (ADR 0071): a `pom.xml` with a `<modules>` tag holding a `<module>`, outside comments. Any other stays `mvn -B test`. See it: `fscale verify --dry-run` in such a repo.
- **A test command that exits 0 while its results hold failing tests is `FAILED`**, and never cached (ADR 0072): pytest, Vitest, Jest and `cargo test`, as with `|| exit 0` at the end of a test script. See it: `fscale verify`.
- **A new failing test in a file the change added or edited is never labelled `flaky`**, and its step is not run again (ADR 0073). See it: `fscale verify` on a branch that adds a failing test.

**Verify: the step cache and `--affected`**

- **Every step runs once more after the upgrade**: the version is part of the step cache key. See it: `FSCALE_LOG=cache fscale verify`.
- **`.env` and `.env.local`, at the repo root and in the project's folder, are in every step's key**, as a hash; no value is ever printed or stored (ADR 0074). Edit one and the steps that can read it run again. See it: `FSCALE_LOG=cache fscale verify` after editing `.env`.
- **On a cache miss, verify says which projects and shared folders differ from the step's last pass** (ADR 0075). A step over a minute says so in the normal output (`Not from the cache: …`). See it: `FSCALE_LOG=cache fscale verify`.
- **A project can add inputs to its step keys** (ADR 0076): `verify.projects.<folder>.inputs`, or `inputs` on one step, with `{ "ignored": "<glob>" }` and `{ "env": "NAME" }`. Inputs can only be added, never removed. See it: `FSCALE_LOG=cache fscale verify` after changing the named file or variable.
- **A step with no kept time can carry an expected time**, `verify.projects.<folder>.<step>.expected` (such as `"1h 40m"`), which `--budget` reads only until a time is kept (ADR 0078). See it: `fscale verify --dry-run --budget 30m`.
- **Verify's JSON is schema version 9** (8 in 0.5.4, 7 in 0.5.3). Version 8 adds the step status `report-only` and a step's `reportOnly`; version 9 gives a step left out for its expected time `expectedMs`, not `lastKnownMs`. See it: `fscale verify --json`, the first field.
- **With a project at the repo root, a change to a file in no project below it runs every project** under `--affected` and in the stop hook (ADR 0077). The root's own steps run when a project below it changed. See it: `fscale verify --affected --dry-run`.
- **A Python import that more than one place could hold is an edge to each**, and `fscale graph` warns with the import and the places (ADR 0077). See it: `fscale graph`.
- **With `--affected`, projects that hold a changed file start first**; the printed order is unchanged (ADR 0078). See it: `fscale verify --affected`.
- **`fscale verify --affected --dry-run --json` prints the affected plan with typed reasons** (ADR 0079; `docs/schemas/affected-plan.schema.json`). Nothing runs. See it: that command.
- **`verify --pr-base` in a merge queue run takes the base from the `merge_group` event** (ADR 0080), and `fscale --help` says so. See it: `fscale --help`.
- **Base runs of a Rust repo build in an empty `target/`, and never get `CARGO_TARGET_DIR`** from the environment (ADR 0081, addendum 1), so a base build never writes into the head's build folder. See it: `fscale verify` on a branch with a failing Rust step.

**The stop hook, the AGENTS.md block and doctor**

- **The stop hook has a time limit**, and doctor warns when the kept step times exceed it (ADR 0083, ADR 0090). See it: `fscale hooks install --agent claude` (prints the limit), `fscale doctor` (`Agent hooks`).
- **An agent may stop and say a test is wrong** instead of editing it: the AGENTS.md block and the hook's block message say so (ADR 0084). `fscale sync` updates an existing block. See it: `fscale sync --check`.
- **The stop hook says when it gave up, and what the agent said**: after three blocks, Claude Code and Codex show `verify still fails after 3 attempts` and the agent's last message (ADR 0087). See it: the next time an agent is let go on a failing branch.
- **Doctor checks every program the verify plan starts**, looked up as the step would start it (ADR 0085). See it: `fscale doctor`.

**The tamper guard**

- **Two new findings, neither of which fails verify on a laptop:** `runner-weakened`, a test script or `conftest.py` made unable to fail, report only (ADR 0086); `assertion-weakened`, an assertion moved under an `if`, swallowed by a `try` or made always true, which needs a person's approval in CI when the guard is `enforced` (ADR 0088). See it: `fscale tamper --base main` on a branch.
- **Evidence from the old tests on the new code** (ADR 0089, ADR 0090): on a finding of a changed expected value, snapshot or assertion, verify runs the base's test file against the new code once, within `verify.baseBudget`, and lists the failing tests under the finding. It never fails verify and says `not judged` when it cannot tell. See it: `fscale verify` on a branch that changes code and its test's expected value together.

**Releases**

- **A release now reaches npm's `latest` only after staging and an approval** (ADR 0092): every version is first published under `next`, installed and tried on Linux, macOS and Windows, then moved to `latest` and tried again, and moved back if that fails. One release runs at a time. A pre-release such as this one stops at `next`. See it: `npm view firescale dist-tags`.

### For CS464

- **Run `fscale sync` once after upgrading, or `fscale sync --check` fails**: the AGENTS.md block changed (ADR 0084). This is a pre-release, so upgrade by version: `npx --yes firescale@0.6.0-rc.1 sync`. It also moves the stop hooks' pin to `firescale@0.6.0-rc.1`; the next `fscale sync` of a later version moves it again.
- **Every step runs once more after the upgrade**: the version is in the step key, and so are `.env` and `.env.local`.
- **The slow suite can carry an expected time**, so a fresh worktree's `--budget` run leaves it out: add `"expected": "1h 40m"` to `verify.projects["backend/ledger_service"].test`, then `fscale verify --dry-run --budget 30m`.
- **A branch whose test edits hit the tamper guard runs its pytest step once more** in a worktree, within `verify.baseBudget` (ADR 0089). It never fails verify.
- **A script that reads `fscale verify --json` sees `schemaVersion: 9`.**
- **Nothing else changes for it**: it has no Rust and no project at the root, so the base run and `--affected` are as before. Check with `fscale verify --affected --dry-run`.
