# 0056 — Fixes for the bugs of the ponytail audit

Status: accepted (2026-11-15, for 0.5.0). Refines ADR 0034 and 0039 (what `--affected` and the stop hook keep), ADR 0035 (what a progress line may show) ADR 0047 (where the agent limits and protected paths come from), ADR 0027 (existing JSON settings get new lines only, now for `hooks install` too), ADR 0049 (which repos the supabase pack gives a `package.json`) and ADR 0021 (the release build).

## Context

The audit of 2026-11-14 (`docs/reviews/ponytail-audit.md`) lists 30 bugs. The owner asked for numbers 1 to 14, 17 and 20 to be fixed, and nothing else: each bug first gets a test that fails because of it, then the smallest fix. None of the shared pieces the audit proposes (its items 1 to 79 and parts A, B and C) is built here. A bug that no test could show is not fixed; it is logged in `DECISIONS.md` with what was tried.

This ADR records the choices made where a bug's entry left more than one fix open.

## Decision

### Bug 1: a finding on a repo-level file is kept when the change is one its rule reads

`--affected` (and so the stop hook) kept a finding only if its file changed or its owner project was affected. A finding on `AGENTS.md`, caused by a new script in `web/package.json`, was dropped.

`narrow_report` now also keeps a finding when both hold:

- its file belongs to no project below the repo root (it has no owner, or its owner is the `.` project), and
- a changed file matches the cache `inputs` of the finding's rule.

Why both conditions:

- Keeping every finding whose rule reads a changed file brings back the findings of unaffected sibling projects (a new `tools/.env.local` brought back every `packages/*/.env` finding), which is the narrowing `--affected` exists for.
- Keeping every repo-level finding would report them all whatever changed.

An `inputs` glob that does not compile counts as matching, so the finding is kept. A rule with no inputs (`group-project/branch-name`) is not kept by this clause.

**Seen change:** an old, unchanged repo-level finding now shows under `--affected`, and blocks the stop hook, once any file its rule reads has changed. Two cases:

- In a repo with no root project: a tracked root `.env` shows once another env file changes. One test changed for it (`works when the firescale root is a folder inside a larger git repo`); it now has a project in its fixture, so that it still proves paths are relative to the firescale root.
- In a repo with a root project, when the change is in a sub-project: a finding of a rule with wide inputs, such as `core/no-secrets-in-source` (`**/*`) in `scripts/deploy.sh`, shows on every change. Before, it showed only when the root project was affected.

No narrower rule was found that still keeps the finding of bug 1: the engine does not know which of a rule's inputs caused a finding.

**Rules that do not declare what they read.** A rule that is not built in was never cached, so its `inputs` meant nothing until now and may be empty. Such a rule counts as reading every change. A change to `firescale.config.json` or `firescale.config.ts` counts as read by every rule, since it can turn any rule on.

**Limit, left open:** a finding inside a project that is not affected, caused by a change elsewhere, is still dropped. Closing it needs the graph (audit item 23), which was not asked for. `docs/affected.md` lists it. The clause is also only as good as a rule's `inputs`: where a rule reads a file it does not list (bug 10), a change to that file still drops the finding.

### Bug 2: verify runs nx and turbo through `npx --no`

`exec_prefix` in `verify.rs` returned a bare `npx`, which downloads a missing tool when stdin is closed. It now returns `npx --no`, the form facts already uses. The one literal was changed; the two functions were not merged (audit item 69). With bun, `bunx` can still fetch a missing tool; `docs/facts.md` already says so for facts.

The test `nx_and_turbo_take_precedence_and_honour_affected` asserted `npx turbo run lint`. It pinned the bug, so its two lines now expect `npx --no turbo run …`.

### Bug 3: the heartbeat hides what the report hides

The progress line's own `redact` knew only `scheme://user:password@` and stopped at the first `/`. The heartbeat now calls the existing `verify::hide_secrets` (ADR 0035), and `redact`, which had no other caller, is removed. No new redactor was written. The file name a known test runner's markers belong to (`(in tests/test_a.py)`) goes through `hide_secrets` too: a line that is only a connection string was read as that file name. `Activity::recent()` still returns lines as printed; its callers only search them for a refused service and print nothing from them.

### Bugs 4, 12 and 13: the missing cache inputs, added by hand

The rule-result cache serves a rule's last result until a file matching its `inputs` changes. Three packs decided "is this repo mine" from files their rules did not list:

- **nextjs** (bug 4): `gitignore`, `next-config` and `app-router` now list `package.json`, which `applies()` reads.
- **maven** (bug 12): `entry` and `tests` now list `src/**/*.groovy`. Kotlin and Scala were already covered through the pack's other-stack inputs.
- **cmake** (bug 13): every rule now lists `binding.gyp`, and also `Sources/**`: the pack reads whether a `Sources/` folder exists to tell a Swift package from a CMake project, and a test showed the same stale result for it. An empty `Sources/` folder still cannot be seen, since the cache key is built from files.

No `inputs` helper and no project-type table was built (audit items 24 and 26, part C).

### Bug 10: the files that decide a command are inputs of `agents-md` and `agent-permissions`

`FACTS_INPUTS` grew from 28 globs to 44, by hand. Each family below has end-to-end cases that showed a cached pass where an uncached run reported a finding:

- the Maven and Gradle wrappers (`mvnw`, `mvnw.cmd`, `gradlew`, `gradlew.bat`);
- Node tool configs (`.oxlintrc.json`, `oxlint.config.*`, `biome.json`, `.eslintrc*`, `vitest.workspace.*`);
- lockfiles below the root;
- `conftest.py`, `manage.py`, `alembic.ini`, `.ruff.toml`, `.mypy.ini`, and requirements lists named `dev-requirements.txt` and the like;
- every `.gitignore`, since facts skips a folder that one names even when git still tracks its files.

This goes beyond the nine names of the audit's entry: the others sit in the same tables of `facts.rs` and `ecosystems.rs` and failed the same way. The list is still kept by hand, so it can fall behind again; the test `the_rules_that_list_commands_read_every_file_that_decides_one` pins what is there.

**Not reproduced:** the entry's related point, that `agent-skills` lists only `firescale.config.json` though it reads `firescale.config.ts`. No state was found where a cached pass survives: both files together stop with a usage error before any rule runs, and swapping one for the other removes a matched input, which changes the key. No code was changed for it.

**Still not inputs** (facts reads them; no test was written, so nothing was added): any `*.py` beside a `requirements.txt`; files a requirements list pulls in with `-r` under another name; `.swiftlint.yml` and `.swift-format`; `cmake/lint.cmake`, `conanfile.*` and `vcpkg.json`; Xcode's shared schemes; whether a Dart `test/` folder exists.

Since bug 1's fix, these inputs also decide what the stop hook keeps: a change to `mvnw`, a nested lockfile or a `.gitignore` keeps a stale-`AGENTS.md` finding under `--affected`.

### Bug 9: a `package.json` that starts with a byte order mark is read

`pertest::script_text` now strips the mark before parsing, as the repo's other readers do, in place. With the mark, the runner and its `--bail` were not found, and a failure of a run that stopped at its first failure was labelled `pre-existing`, which ADR 0039 forbids. The end-to-end test runs the real `npm`, because the case needs a package manager that itself accepts the mark.

### Bug 17: the step cache never reads, writes or cleans through a symbolic link

A committed `.firescale/.gitignore` that is a dangling link made the step cache create a file outside the repo. Tests showed the same for the config digest file, and, with `.firescale` or `.firescale/steps` itself a link, for storing a pass, touching an entry on lookup and removing old entries. All get the check `hooks.rs` already makes, written in place: a linked cache is refused, never followed, and verify says "`.firescale` or `.firescale/steps` is a symbolic link; replace it with a real folder". No shared writer was built (audit item 72).

The rule-result cache wrote the same ignore file with the same mistake (`Cache::put` in `cache.rs` asked `exists()`, which is false for a dangling link), so a plain `fscale verify` or `fscale check` still created the outside file. It is the same file and the same case as the bug's entry, so its one condition was fixed too, with its own failing test.

**Found, not fixed** (not on the owner's list): with `.firescale` itself a link, the rule-result cache (`cache.rs`), the base-result store (`blame.rs`), the run records (`runs.rs`) and a shared cache folder under it (`sharedcache.rs`) still create folders and write through it. Those were read, not tested. With a linked digest file whose target differs, the "config changed" note can print on every run.

### Bug 5: a lone `assertThat(x)` in Kotlin cannot fail

The Kotlin reader of the tamper guard now has the rule the Java reader has: an `assertThat` with one argument and nothing chained, or whose last argument is Hamcrest's `anything()`, is not counted as an assertion. So `assertThat(total()).isEqualTo(3)` cut down to `assertThat(total())` is reported as `assertion-removed`. The rule was written into `kotlin.rs`; Kotlin was not merged into Java's pieces (audit item 9), and bug 18 was left alone. The labelled corpus gained three Kotlin cases (461 cases, none missed, no false finding).

The rule applies only where the call stands as a statement (its parent is a block or a lambda body). Elsewhere the check can be real though nothing is chained: `with(assertThat(x)) { isEqualTo(3) }`, `assertThat(x)!!.isEqualTo(3)`, the infix `assertThat(x) isEqualTo 3`, and an expression body `fun sum() = assertThat(x)` whose caller checks. Those still count, so a refactor into them is not reported. The cost is a missed cheat in the rarer forms: `@Test fun t() = assertThat(x)`, and a brace-less `if (r) assertThat(x)`. Java's rule has no such condition and was not touched.

### Bug 7: the agent limits come from the loaded config

`fix --agent` read `maxTurns`, `maxCostUsd` and `timeoutSeconds` again from `firescale.config.json`, so a `firescale.config.ts` got the defaults (30 turns, $2, 900 s). The config check now hands back the three numbers it validates (`ResolvedConfig.agent`), and the run uses those. This corrects a line under "Known limits" in ADR 0047.

The two clamps still differ and were both kept, so nothing changes for a JSON config: the config accepts any value of 1 or more, and the run caps turns at 1,000 and time at 86,400 s.

### Bug 8: a protected glob that does not compile is an error; a finding's file is a path

- **`protectedPaths`.** A glob that does not compile was dropped, and the paths it was meant to protect were open to the agent. It is now a config error (exit 2) for every command that loads the config, `fscale check` included, from a JSON or a TS config. The glob is compiled for the check exactly as it is for matching.
- **The repo's Claude Code settings.** The runner also takes `Edit(…)`, `Write(…)` and `Read(…)` rules under `deny` and `ask` in `.claude/settings.json` and `.claude/settings.local.json` as protected paths. One that does not compile now stops `fix --agent` before any worktree or agent is started. It is not a `fscale check` error: that file is not firescale's config.
- **The files a finding names.** They were compiled as globs, so a finding on `app/[id]/page.tsx` allowed `app/i/page.tsx` and refused the real file. They now go through `globset::escape` and match only themselves.

Two one-line helpers were added to `agentrun.rs` for this (`glob_error`, `literal_glob`), because the CLI crate does not depend on globset. No dependency was added.

**Left as it was:** a rule's `inputs` that does not compile is still dropped from the set the agent may change, which fails safe (it allows less). A `protectedPaths` glob that compiles but can match no path (`./infra/**`, `/infra/**`, a bare `infra`) is still accepted and protects nothing; the owner's rule covers globs that do not compile.

### Bug 6: `fscale hooks install` adds lines, or refuses

`hooks install` parsed a person's settings file and wrote it back whole: 4-space indentation became 2, `1e3` became `1000.0`. It now adds the hook through `jsonedit::insert_merge`, the function the agent-ready fix uses (ADR 0027), so every line a person wrote stays byte for byte. Nothing moved between crates.

Where the hook cannot go in as new lines, the file is left alone and the command fails with the item to add by hand. That is the case for a file written on one line, for `"hooks": {}` or `"Stop": []` on one line, and for `"hooks": null`.

Seen changes:

- **A minified settings file is refused**, where it was rewritten. The audit named this cost. With `--all`, the command stops at the first refused file, as it already did for a file that is not valid JSON; the files before it are written and the ones after it are not tried.
- **`"hooks": null` is refused**, where it was treated as missing and replaced. Replacing it would change a line the person wrote.
- **The hook goes first in the stop list**, where it went last. Appending would need a comma on the person's last line.
- Four existing tests fed one-line files and so had to change: their inputs got line breaks, the two assertions on the list's order were swapped, and `a_null_hooks_key_is_treated_as_missing` became `a_null_hooks_key_is_left_for_a_person`. No assertion on a kept value was dropped.
- The `agent-ready/verify-hook` message and skill said "run `fscale hooks install`" for the files the fix cannot edit, which relied on the rewrite. They now say that command tells what to add.

### Bug 11: the supabase pack gives no `package.json` to a repo of any other stack

`supabase/package-json` knew seven manifests of other stacks. With a `Pipfile`, `setup.py`, `pom.xml`, `build.gradle.kts` or `Gemfile`, `fscale sync` wrote a root `package.json`. The rule now asks the list that already exists for this question, `stackkit::MANIFESTS` (28 names, used by the go, maven and cmake packs), and its cache inputs are that same list. This is audit item 57; it was the smaller change, it adds nothing new, and the alternative was typing 21 names into a second list.

A repo that has a `package.json` is not affected. A repo of another language with no manifest at all (only `.py` files) still gets one.

### Bug 14: the release build restores no cache

`cache: pnpm` and `Swatinem/rust-cache` are gone from the `build` job of `release.yml`. A test (`restores no cache in any job`) fails on any line of the four jobs that names a cache, other than a key that turns one off, and fails when a job is added, until someone reads it.

setup-node can also cache with no `cache:` input (ADR 0041). At the pinned commit its `action.yml` does that only when npm is the package manager, so nothing was cached that way here; each of the four jobs still sets `package-manager-cache: false`, pinned by a second test, so the rule holds whatever the package manager. pnpm/action-setup's own `cache` input is off by default. `ci.yml` keeps its caches.

Release builds are now cold on all six targets, so a release takes longer.

### Bug 20: the test fixture is not in the npm package

`packages/cli/src/course-fixture.ts` is excluded from the build, and from `pack-npm.mjs`'s list, since `tsc` does not delete an older `dist/course-fixture.js`. A test lists every file of the packed `firescale` and `@firescale/sdk` packages and rejects test-only names. `@firescale/sdk/testing` is a public export and stays.

## Consequences

- The stop hook blocks on a stale `AGENTS.md`, a missing permission or any other repo-level finding that the change itself caused.
- A fresh clone of an Nx or Turborepo repo that uses npm gets `SETUP NEEDED`, never a download.
- `hide_secrets` is now the one place that decides what output may be shown. It hides `password=`, `pwd=` and `passwd=` values; `token=` and `secret=` are still shown, in the report as in the heartbeat.
- Adding `next` to a `package.json`, a Spock test to a Maven project, a `binding.gyp` to a CMake project or `mvnw` to a Maven project ends the cached result that the file makes wrong.
- `agents-md` and `agent-permissions` miss the cache more often (any `.gitignore` or requirements list ends their cached pass). That costs time only.
- A repo whose `.firescale` or `.firescale/steps` is a symbolic link gets no step cache.
- A config with a mistyped `protectedPaths` glob stops every command until it is fixed. Before, it ran with that path unprotected.
- `fix --agent` in a repo whose Claude Code settings hold a path rule globset cannot compile is refused, where before it ran without that protection.
- `fscale hooks install` on a hand-made settings file changes no line of it. On a minified file it now fails and prints the item to add.
- A Supabase project in a Python, Java, Ruby or any other listed stack is told it is not a JavaScript project, and gets no `package.json`.
- What a release publishes is built from the tagged commit and the lockfiles only.
