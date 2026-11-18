# 0085 — Doctor follows the verify plan

Status: accepted (2026-11-18, Track B run). Amends ADR 0026 (doctor). Verification idea 10, part (i).

## Context

`fscale doctor` checks the tools of its own list, per ecosystem. `fscale verify` runs the commands of its plan. The two can disagree: a step runs a program doctor's list does not name, and doctor says "this machine is ready" while verify then fails with `SETUP NEEDED`. The research report's bug 3 was one case (a linter verify ran and doctor missed; fixed then for SwiftLint only). Before this change, a repo whose config says `"verify": { "lint": "zzlint --strict" }`, on a machine without `zzlint`, got `doctor: OK - this machine is ready for this repo.` (seen in WSL on `main`).

## Decision

- **Doctor builds the plan of a full `fscale verify`**, as `verify --dry-run` builds it (the config, the facts, no `--affected`), and takes the first word of each step's command, at the root and in each project.
- **It looks the program up as the step would start it**, with the step cache's lookup (`stepcache::locate`, ADR 0043): on the PATH, only in absolute folders, on Windows with the `PATHEXT` extensions as `cmd` tries them; a path relative to the step's folder.
- **Not found: a problem** in `Tools`, subject the program, that names the step and its command; the fix is the install hint of doctor's own list when it knows the program, else to install it or change the step's command under `verify`.
- **Found, and a tool doctor's own list knows: its version is asked**, with the same arguments that list uses, and a failure is a problem. Found and unknown: OK, never run (as the step cache never runs a program it does not know).
- **Doctor's own tool list gives way where the plan covers it**: a tool of the list whose program a step of the same folder runs is not checked again by the list. The JDK under a Maven project is still checked by the list: no step's program is `java`.
- **Not judged**: a command whose first word is a shell builtin or keyword (`cd`, `set`, `call`, `if`, ...), sets a variable (`FOO=1 cmd`), or starts with shell syntax (`$`, `%`, `(`). Several steps that run one program in one folder give one line that names them all.
- If the config cannot be read, doctor runs as before, without the plan.

## Consequences

- **Doctor can only get stricter.** Where the plan covers a tool, the plan's check asks the same version with the same arguments the list did; everywhere else the list is unchanged; a program the list never knew is now looked up. No check is dropped without one at least as strict in its place.
- A repo whose steps run programs doctor's list never named gets new lines in `Tools`, OK or problem. The full suite needed no snapshot change: its doctor fixtures either have no plan or plans whose programs the list already reported.
- **Only the first program** of a command is looked up: `npm run lint && cargo test` checks `npm`. A wrapper (`npx`, `pnpm exec`, `uv run`) is looked up, not the tool it starts.
- A program found may still fail when the step runs it (a broken install, a missing runtime under a script); for a program doctor does not know, nothing runs it to find out.
- Doctor reads the config once more (`verify`); a broken config was already reported by doctor's own settings.

## Later

- Doctor warns when the default branch does not require the tamper-guard and verify checks, when `gh` is signed in (idea 10, part ii).
- A core rule that checks firescale's generated workflows for a top-level read-only `permissions` (idea 10, part iii).
- Every program of a compound command; the tool a wrapper starts (`npx eslint`: is `eslint` in `node_modules/.bin`).

## Tests

- `doctor.test.ts`, "doctor follows the verify plan (ADR 0085)": a missing program a step runs is a problem naming the step (exit 1); a present unknown one is OK; Maven's `mvn` gets one line, from the plan, with the list's install hint, and `java` stays the list's; a step that starts with `cd` or `FOO=1` is not judged. The first two were seen failing on `main` in WSL; the third guards the not-judged case and passed before and after.
