# firescale

`firescale` checks a repo against a set of rules, fixes what code can fix safely, and gives people, CI and coding agents one command that means "done".
It is one native binary (`fscale`), started by a small Node launcher. It runs on Windows, macOS and Linux, and it works on any repo.

## Quick start

You need Node.js 22.18 or newer and git. The commands are the same on Windows, macOS and Linux. Run them in the root of a repo you already have:

```
npx firescale@latest doctor
npx firescale@latest check
npx firescale@latest adopt --dry-run
npx firescale@latest verify
```

- `doctor` says whether this machine is ready for the repo.
- `check` runs the rules and lists what they find. It changes none of your files; it keeps a cache in `.firescale/`.
- `adopt --dry-run` shows, as a diff, what firescale would add. It changes nothing.
- `verify` runs the rules, then the repo's own lint, typecheck and tests.

Write `npx firescale`, not `npx fscale`: `fscale` on npm is another package. After `npm install --save-dev firescale` the command is `npx fscale`.

## Usage: the five ideas

Each block below is what `fscale` printed on a test fixture of this repo. `node scripts/readme-output.mjs` prints them all again (build first). They were made on Windows 11 with a debug build of 0.5.3, on 2026-11-18. Commit ids change from run to run. The tool versions and Windows paths in the doctor block are that machine's.

### 1. A rule is a check, a fix and a page that explains it

Every rule can find a problem. Some can fix it, the same way every time. Each has a `SKILL.md` that a person or an agent can read (`fscale explain <rule>`). There are ten rule packs; `core` is on by default. A rule only reports until you set it to `enforced`.

```
$ fscale check
core/gitignore-basics  (evaluated, medium)
  .gitignore
    .gitignore does not ignore "*.log". Add "*.log" to .gitignore so generated files are not committed. [deterministic fix available]
  .gitignore
    .gitignore does not ignore ".venv". Add ".venv/" to .gitignore so generated files are not committed. [deterministic fix available]
  .gitignore
    .gitignore does not ignore "__pycache__". Add "__pycache__/" to .gitignore so generated files are not committed. [deterministic fix available]
  .gitignore
    .gitignore does not ignore "node_modules". Add "node_modules/" to .gitignore so generated files are not committed. [deterministic fix available]
  How to fix: fscale explain core/gitignore-basics

core/readme-setup  (evaluated, low)
  README.md
    "README.md" does not show how to install the project. Add an "Install" section with the install command (a heading or a command in a code block). [agent-fixable]
  README.md
    "README.md" does not show how to run the project. Add a "Usage" section with the command that runs it (a heading or a command in a code block). [agent-fixable]
  How to fix: fscale explain core/readme-setup

OK: 8 rules checked, 6 violations (0 enforced, 6 report-only).
Report-only violations do not fail the run. Set a rule to "enforced" to make it fail.
```

`fscale fix` applies the fixes. `init`, `adopt` and `sync` are the same loop: run the fixes until `check` has nothing left that a fix can repair. There are no templates.

### 2. One command means "done"

`fscale verify` runs the rules, then lint, typecheck and tests, for every project it finds in the repo. With `--affected` it runs only the projects a change can reach. This is a dry run after one file changed in one of six projects:

```
$ fscale verify --affected --dry-run
verify --affected (dry run): 1 of 6 projects would run, 5 not affected. Nothing was run.
  check      would run  fscale check --affected
  in backend/audit_service: not affected
  in backend/auth_service:
    lint       would run  python -m ruff check .
    typecheck  skipped    no type checker was found (looked for mypy)
    test       would run  python -m pytest
  in backend/ledger_service: not affected
  in backend/market_service: not affected
  in backend/realtime_service: not affected
  in frontend: not affected

Affected only (uncommitted and untracked changes; 1 changed file). Why each ran:
  backend/auth_service: changed: backend/auth_service/main.py
```

`fscale hooks install --all` makes Claude Code, Codex and Cursor run `verify` when they try to stop, and sends them back while it fails. After three blocks in a row the agent may stop. A step that passed is kept in a local cache and is not run again until its inputs change.

### 3. Blame the change, not the repo

When a step fails, verify finds out whether it fails on the base commit too. It runs the step there in a separate git worktree, unless it already knows the result. A failure that was already there is labelled `pre-existing` and does not block. A `new` failure does. So does a failure the base run could not judge: the base run has a budget of 5 minutes. Here the change broke lint, and the test was already failing:

```
$ fscale verify
verify: FAILED at lint (new); pre-existing (fails on the base commit 767f501 too, does not block): test
  check      passed   8 rules, no enforced violations
  lint       FAILED   [new] node lint.js (exit 1)
  typecheck  off      turned off in your firescale config
  test       FAILED   [pre-existing] node test.js (exit 1)

New: lint passes on the base commit 767f501, so this change broke it.

Fix the failure, then run again: node lint.js

Pre-existing: test fails on the base commit 767f501 too, with the same output, so it does not block this change.

Compared with the base commit 767f501 (main): 1 step from an earlier base run, 1 step run in a worktree of it.

note: test: its runner gives no per-test results, so it is compared as a whole step
```

### 4. A tamper guard

A change can pass by weakening the tests. The guard compares the change with the base commit. It reports, among other things, tests skipped or deleted, assertions removed, new lint suppressions, lowered coverage thresholds and weakened CI. Here one test was changed to `it.skip`:

```
$ fscale tamper
tamper: 1 report-only finding

Tamper guard (report only; verify.tamper on the base commit decides, and "enforced" makes this fail): compared with the base commit 9229bc0, this change takes away tests or assertions, adds lint suppressions, lowers coverage thresholds, weakens CI, skips git hooks, turns rules off, or changes files it cannot read.
  test skipped: src/math.test.ts:9: math > subtracts
  In the changed test files: 2 tests on the base, 2 now; 0 skipped on the base, 1 now; 3 assertions on the base, 3 now.
```

It only reports, until the base commit sets `verify.tamper` to `enforced`. It misses a lot: see the numbers below.

### 5. Is this machine ready?

`fscale doctor` checks the tools the repo declares, the variables its `.env.example` files name, the installed dependencies and the local services. Each problem comes with the next step. A variable that is set nowhere is a warning, not a problem, unless the config says otherwise. It never prints the value of a variable, a password or a connection string. The fixture has six projects and nothing installed. The lines for four of its services are cut here and marked `...`:

```
$ fscale doctor
Tools
  ok       node: node 22.19.0 (the repo declares no version)
  ok       npm: npm 10.9.3

Environment variables
  warning  .env.example: 1 of 1 variables are not set in the environment, and there is no .env: EXAMPLE_DATABASE_URL.
           next: copy .env.example to .env and fill in the values (or set the variables in your shell)
  warning  backend/audit_service/.env.example: 1 of 1 variables are not set in the environment, and there is no backend/audit_service/.env, nor a .env in a folder above it: EXAMPLE_SERVICE_URL.
           next: copy backend/audit_service/.env.example to backend/audit_service/.env and fill in the values (or set the variables in your shell)
  ...

Dependencies
  PROBLEM  backend/audit_service/.venv: backend/audit_service has no virtual environment (.venv or venv), so its packages would go into the Python on the PATH.
           next: in backend/audit_service: python -m venv .venv, then .venv\Scripts\python.exe -m pip install -r requirements-dev.txt
  ...
  PROBLEM  frontend node_modules: the dependencies of frontend/package.json are not installed.
           next: in frontend: npm ci

doctor: 6 problems, 6 warnings - run the `next:` commands above, then `fscale doctor` again.
```

## One real project

A course repo with six projects: five Python services and a web app.
A full `fscale verify`, three projects at a time, took 1h 41m on a laptop. One test suite took 1h 36m of that.
`--budget 10m` leaves out the two suites known to take longer than that. The steps that are left took about 5 minutes in that run, added up.
A run with a budget never says `OK`, and it names each step it did not run.
`--budget` is refused in CI and in the agent stop hook, where a run must stand for the whole repo.

## Measured, with limits

Every number is from [bench/RESULTS.md](bench/RESULTS.md) or [DECISIONS.md](DECISIONS.md). Read the limit before you trust the number.

| What                                                   | Number                                                                                        | Limit                                                                                                                                                                                                                                                                                                                                                |
| ------------------------------------------------------ | --------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Cheating changes the tamper guard misses               | 43 of 98                                                                                      | 47 are caught and 8 more are only sent for a person's approval. One AI subagent wrote all 98 cases in one sitting, and 11 of them are of kinds the guard does not try to catch. They cover JavaScript, TypeScript, Python, Rust and config files only. Counted on Windows with a debug build; a Linux release build of an earlier version missed 42. |
| Tamper findings that are false, on real commits, 0.5.2 | 12.4% (105 of 844; 95% interval 10.4 to 14.8%)                                                | 2,045 commits of sixteen public repos in twelve languages. An AI labelled every finding, once: 51 of them now, the rest in earlier measurements. No second labeller and no human checked. These are the commits the fixes were made against, so it is optimistic. 306 of the findings come from one repo.                                            |
| The same for 0.5.1                                     | 11.6% (98 of 842; 95% interval 9.6 to 14.0%)                                                  | The same commits, labels and limits. The intervals overlap, so this shows no difference between the two versions.                                                                                                                                                                                                                                    |
| Binary size                                            | 30.0 to 32.4 MB, by platform                                                                  | Measured for 0.5.0 from npm's log lines, and not since. About 19 MB of it is the parse tables of eight language grammars.                                                                                                                                                                                                                            |
| Download size (the `.tgz` npm fetches)                 | 5.3 to 5.7 MB, by platform                                                                    | Measured for 0.5.0, and not since.                                                                                                                                                                                                                                                                                                                   |
| `fscale check` on a repo of 50,005 files               | 99 ms with a cold cache, 64 ms warm (p50), on Linux; 494 ms and 370 ms on a Windows CI runner | One rule on, 10 runs, on 2026-11-11, when the binary was smaller. With one rule the cache makes `check` slower: 33 ms with `--no-cache` on Linux.                                                                                                                                                                                                    |
| `fscale verify` with every step taken from the cache   | 36 ms (p50)                                                                                   | A small repo with three empty steps. Linux, 100 runs. A real test suite takes far longer.                                                                                                                                                                                                                                                            |
| What the base run adds to a failing `verify`           | about 100 ms with a cold cache                                                                | The steps were one-line scripts. A real base run costs what the project's own step costs. Not measured on macOS or Windows.                                                                                                                                                                                                                          |
| This repo's own `verify`                               | 2 min 07 s of steps, then 0.65 s for the next run with no change                              | From 2026-11-13, on Linux. How many runs is not recorded.                                                                                                                                                                                                                                                                                            |

## What it does not do

- It is not a task runner. Nx and Turborepo do that.
- It is not a general linter. The core rules cover repo hygiene and setup, not code style.
- It keeps no flaky-test history. A new failure is run once more, and that is all.
- It is not an agent benchmark. The referee only serves `verify`.
- It never runs an agent on a pull request from a fork.

## Known limits

- **Some languages' commands have run nowhere yet.** `dotnet test`, `flutter test` and `xcodebuild test` have never run through `fscale verify` on a real project. On 15 public repos the result was right in 8 and wrong in 6 in the first run, and right in 9 and wrong in 5 after three fixes (two repos could not be judged in the second run, because their downloads failed, and keep their first verdict). In four of the five a lint command that fscale guessed failed, and the repo's passing tests were then not run ([the review](docs/reviews/real-repos-0.5.3.md)).
- **Codex is unverified.** Its hook and settings files follow Codex's source code. `fscale fix --agent codex` has had no real run, and the Codex settings file was never loaded by a real Codex. Claude Code had a real run.
- **Slow suites need `--budget`.** Verify runs the whole test command of every project it runs. A suite that takes an hour takes an hour. `--budget` leaves it out only after one run has recorded how long it takes.
- **A test moved to another file is not followed.** The tamper guard compares expected values one file at a time. A test that moves to another file with its expected value changed is not reported.
- **Alpine Linux is not supported.** The Linux binaries need glibc.

## Install

```
npm install --save-dev firescale
npx fscale --version
```

pnpm and yarn work the same way. The package picks a prebuilt binary for Linux, macOS or Windows, x64 or arm64. No Rust toolchain is needed. More in [docs/install.md](docs/install.md).

To work on firescale itself you need Rust (through `rustup`), Node.js 22.18 or newer, pnpm 12 and git:

```
pnpm install
pnpm build
pnpm test
pnpm fscale --version
```

`pnpm build` must come before `pnpm test`: the tests run the built binary.

## Docs

- Commands: [verify](docs/verify.md), [doctor](docs/doctor.md), [fix](docs/fix.md), [init, adopt and sync](docs/init.md), [hooks](docs/hooks.md), [graph and affected](docs/graph.md), [generators](docs/generators.md)
- Rules: [the built-in rules](docs/rules.md), [config](docs/config.md), [writing your own](docs/writing-rules.md)
- Output for tools: [check JSON](docs/check-json.md), [SARIF](docs/sarif.md), [facts](docs/facts.md), [CI](docs/ci.md)
- Design and history: [SPEC.md](SPEC.md), [PLAN.md](PLAN.md), [DECISIONS.md](DECISIONS.md), [docs/decisions/](docs/decisions/), [RELEASING.md](RELEASING.md)

## Licence

MIT
