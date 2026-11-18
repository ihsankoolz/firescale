# 0085 — Doctor follows the verify plan

Status: accepted (2026-11-18, Track B run). Amends ADR 0026 (doctor). Verification idea 10, part (i).

## Context

`fscale doctor` checks the tools of its own list, per ecosystem. `fscale verify` runs the commands of its plan. The two can disagree: a step runs a program doctor's list does not name, and doctor says "this machine is ready" while verify then fails with `SETUP NEEDED`. The research report's bug 3 was one case (a linter verify ran and doctor missed; fixed then for SwiftLint only). Before this change, a repo whose config says `"verify": { "lint": "zzlint --strict" }`, on a machine without `zzlint`, got `doctor: OK - this machine is ready for this repo.` (seen in WSL on `main`).

## Decision

- **Doctor builds the plan of a full `fscale verify`**, as `verify --dry-run` builds it (the config, the facts, no `--affected`), and takes the first word of each step's command, at the root and in each project.
- **It looks the program up as the step would start it**, with the step cache's lookup (`stepcache::locate`, ADR 0043): on the PATH, only in absolute folders, on Windows with the `PATHEXT` extensions as `cmd` tries them; a path relative to the step's folder. On Windows a bare name is also looked for in the step's folder, as `cmd` does, unless `NoDefaultCurrentDirectoryInExePath` is set.
- **Not found: a problem** in `Tools`, subject the program, that names the step (never its command, which may hold a password); the fix is the install hint of doctor's own list when it knows the program, else to install it or change the step's command under `verify`.
- **Found, named by its bare name, and a tool doctor's own list knows: its version is asked**, with the same arguments that list uses, and a failure is a problem. Found and unknown, or named by a path (`./gradlew`, `tools/make`, `/opt/cmake/bin/cmake`): OK, never run, as the step cache never runs a path.
- **Doctor's own tool list gives way where the plan covers it**: a tool of the list whose program a step of the same folder runs **by its bare name** is not checked again by the list. A step that runs a path never takes a tool off the list: `tools/make test` leaves the list asking for `make` on the PATH. The JDK under a Maven project is still checked by the list: no step's program is `java`.
- **Not judged**: a command whose first word is a builtin or keyword of `sh` or `cmd` (`cd`, `set`, `read`, `call`, `copy`, `del`, `md`, `if`, ...), sets a variable (`FOO=1 cmd`), starts with shell syntax (`$`, `%`, `(`), or is a quoted or `~` path that the shell rewrites. Several steps that run one program in one folder give one line that names them all.
- **When the plan cannot be built** (a config verify refuses, or a `firescale.config.ts`, which doctor never runs), doctor runs its other checks and gives a warning, `verify plan`, that says the programs of verify's steps were not checked and why.

## Consequences

- **Doctor can only get stricter.** Where the plan covers a tool, it is a bare name, looked up where the list's shell would look and asked the same version with the same arguments the list did; everywhere else the list is unchanged; a program the list never knew is now looked up. No check is dropped without one at least as strict in its place. (The first version let a path such as `tools/make` take `make` off the list, and ran that file: the review of this pull request found both.)
- A repo whose steps run programs doctor's list never named gets new lines in `Tools`, OK or problem. The full suite needed no snapshot change: its doctor fixtures either have no plan or plans whose programs the list already reported.
- **Only the first program** of a command is looked up: `npm run lint && cargo test` checks `npm`. A wrapper (`npx`, `pnpm exec`, `uv run`) is looked up, not the tool it starts.
- A program found may still fail when the step runs it (a broken install, a missing runtime under a script); for a program doctor does not know, nothing runs it to find out.
- Doctor reads the config once more (`verify`). A repo with a `firescale.config.ts` now gets one more warning (the plan is not known); one test's expected output changed for it on purpose.

## Later

- Doctor warns when the default branch does not require the tamper-guard and verify checks, when `gh` is signed in (idea 10, part ii).
- A core rule that checks firescale's generated workflows for a top-level read-only `permissions` (idea 10, part iii).
- Every program of a compound command; the tool a wrapper starts (`npx eslint`: is `eslint` in `node_modules/.bin`).

## Tests

- `doctor.test.ts`, "doctor follows the verify plan (ADR 0085)": a missing program a step runs is a problem naming the step (exit 1); a present unknown one is OK; Maven's `mvn` gets one line, from the plan, with the list's install hint, and `java` stays the list's; a step that starts with `cd` or `FOO=1` is not judged. The first two were seen failing on `main` in WSL; the third guards the not-judged case and passed before and after.
- From the review, written first and seen failing in WSL: "never prints the command of a step, which may hold a password", "never runs a program a step names by its path, and never takes a tool of its own list away for it", "does not judge a step that starts with a builtin of cmd or sh", "says so when the verify plan cannot be built, instead of checking nothing". The first two tests' expected details lost the command. The Maven case runs with only `sh` and `git` from the system on the PATH: GitHub's runners have Maven installed.

## Not done

- The look in the step's folder on Windows has no test: CI runs on Linux. It was read against `cmd`'s documented search order only.
- A step whose program a version manager shims (asdf, mise) is found as the shim; whether the shim has the version is not asked for a program the list does not know.
