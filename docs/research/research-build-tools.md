# What firescale can take from build systems, task runners and CI tools

Research date: 2026-11-18. Read-only pass over the firescale repo (0.5.3) plus primary-source web reading.

## How to read this

- Each idea has six parts: (a) the mechanism and its source, (b) what firescale does today, (c) the proposed change and its smallest first step, (d) who benefits, (e) the risk under the rule "a stale pass is worse than no cache", (f) effort (S = days, M = one to two weeks, L = a month or more).
- **Verification marks.** "Verified" means I read the claim on the linked primary page in this session. "Search only" means a search summary quoted the primary page but I did not open it. "From memory" means I did not check it at all and it must be confirmed before use.
- Repo facts come from `README.md`, `SPEC.md`, `docs/verify.md`, `docs/graph.md`, `docs/affected.md`, the scratchpad `arch_facts.md`, and spot reads of `crates/core/src/stepcache.rs`, `sharedcache.rs`, `blame.rs` and `graph.rs`.

## The main finding

The largest gap is in what the step cache key leaves out, not in speed.
firescale runs each step with the whole environment but keys only 23 named variables, and it leaves ignored files such as `.env` out of the key (`docs/verify.md`, "The step cache": "No other variable is read" and "a `.env` a test reads can change without a miss").
Turborepo calls that arrangement Loose Mode and warns that it "can produce incorrect cache hits".
Ideas 1 and 2 close that gap cheaply and only ever add inputs, so they cannot create a stale pass.

The second finding is a warning. Pants can infer fine-grained dependencies safely because it runs every process in a sandbox, so a missed edge fails instead of caching a wrong result.
firescale has no sandbox, so a missed edge is a stale pass or a wrong `not affected`. Every idea that narrows scope (file-level edges, lockfile pruning, per-file test caching) carries that risk and is ranked low or rejected.

## Top 10 by value for effort

| # | Idea | Taken from | Effort | Why it ranks here |
| - | ---- | ---------- | ------ | ----------------- |
| 1 | Put `.env` files and the variables the repo names into the step key | Turborepo env modes, wireit `env` | S | Closes a stale-pass hole the docs already admit; adds inputs only |
| 2 | Additive user-declared inputs (ignored files, extra paths, env, a command's output) | Nx `inputs`, Turborepo `inputs` | S-M | Removes the "use `--no-cache` after changing an ignored file" advice |
| 3 | Say which input changed on a cache miss | Turborepo `--summarize`, `turbo query` reasons, moon hash manifests | S-M | Diagnostic only, no correctness risk; makes the cache trusted |
| 4 | Never excuse a flake in a test the change added or edited; add a `flaky` policy | nextest `flaky-result`, Develocity safety nets | S | Tightens the one place firescale lets a failure through |
| 5 | Run the most likely failures first | Google TAP, bazel-diff distance | S | Skips nothing; shortens time to first failure with `--jobs 1` |
| 6 | A rule for cross-project imports with no manifest edge; all owners on an ambiguous import | `turbo boundaries`, Pants `python-infer` | S-M | Makes the graph that `--affected` and the cache rely on checkable |
| 7 | A JSON affected plan with typed reasons and an exit code | `turbo query affected` | S | `--dry-run` has no JSON form today; CI gating needs one |
| 8 | Confirm a new failure by running only the failing tests first | nextest retries, Bazel `flaky` | M | Saves a whole-suite rerun when the failure is real, which is the common case |
| 9 | Merge queue support in the generated workflow and `--pr-base` | GitHub merge queue | S | `DECISIONS.md` already records the missing `merge_group` trigger |
| 10 | A persistent build folder for Rust base runs | cargo fingerprints, sccache | S | Hypothesis: Rust base runs rebuild from scratch inside a 5-minute budget; measure first |

---

## The top 10 in detail

### 1. Put `.env` files and the variables the repo names into the step key

**(a) Mechanism.** Turborepo's default Strict Mode passes a task only the variables listed in `env` and `globalEnv`, and hashes their values. Loose Mode passes everything but hashes only the listed ones, and the docs warn this can restore outputs built with the wrong variables. Turborepo also infers prefix wildcards per framework (`NEXT_PUBLIC_*`, `VITE_*`, `REACT_APP_*`, `EXPO_PUBLIC_*`, `NUXT_*`, `PUBLIC_*` and others) and tells users to add `.env` files to `inputs` or `globalDependencies`, because it does not load them itself. wireit puts the values of variables declared in `env` into its fingerprint. Verified: [Turborepo: using environment variables](https://turborepo.dev/docs/crafting-your-repository/using-environment-variables), [turbo run reference](https://turborepo.dev/docs/reference/run), [wireit README](https://github.com/google/wireit). Pants goes further and strips the parent environment from every test process ([Pants test goal](https://www.pantsbuild.org/stable/docs/python/goals/test), verified).

**(b) firescale today.** The key holds a hash of 23 fixed variables (`ENV_ALLOWLIST` in `crates/core/src/stepcache.rs`), and the step runs with the full environment. Ignored files are not in the key: `docs/verify.md` says "a generated file or a `.env` a test reads can change without a miss. Use `--no-cache` after changing one." The base run already hashes `.env` and `.env.local` into a kept base result's key (`docs/verify.md`, "Only new failures block"), so the code to do this safely exists.

**(c) Change.** Keep running with the full environment (strict filtering would break real test suites), but widen the key:
1. the bytes of `.env` and `.env.local` at the repo root and in the unit's folder, hashed and never stored, as the base run does;
2. the value of every variable that an `.env.example` in scope names (doctor already parses those names);
3. the framework prefixes Turborepo infers, for the frameworks `fscale facts` detects.

First step: item 1 alone, reusing the base run's env-file digest.

**(d) Benefit.** Anyone who edits `DATABASE_URL` or a feature flag in `.env` and reruns verify, which includes every agent session that touches config. Today they get a cached pass for a configuration that was never tested.

**(e) Risk.** None toward staleness: this only adds inputs. The cost is more misses, and a volatile variable named in `.env.example` (a timestamp, a per-session token) would defeat the cache for that unit; the "not cached" line should name it. Values must stay hashed and unlogged, as now.

**(f) Effort.** S.

### 2. Additive user-declared inputs

**(a) Mechanism.** Nx lets a task declare extra inputs of several kinds: filesets (with `includeIgnored: true` to bring in gitignored files), `{ "runtime": "node --version" }` (the command's output is hashed), `{ "env": "API_KEY" }`, and `{ "json": ..., "fields": [...] }` to hash chosen fields of a JSON file. Turborepo has `inputs` with `$TURBO_DEFAULT$` to extend the default set instead of replacing it. Verified: [Nx inputs reference](https://nx.dev/reference/inputs), [Turborepo caching](https://turborepo.dev/docs/crafting-your-repository/caching).

**(b) firescale today.** The scope is computed, not declared: the unit's folder, its graph dependencies, files in folders above, and unowned files. There is no way to say "this step also reads the ignored `dist/`" or "this depends on `docker --version`". A project with a tsconfig `paths` alias or a dynamic import is never cached at all (`docs/verify.md`, "A step is never cached when").

**(c) Change.** Add `verify.projects.<folder>.inputs` (and per step) that can only add: extra paths or globs, `{ "ignored": "dist/**" }`, `{ "env": "NAME" }`, `{ "runtime": "<command>" }`. Do not offer a way to remove inputs. First step: `ignored` globs and `env` names only; they need no new process handling.

**(d) Benefit.** Projects whose tests read build outputs or generated files, and projects currently never cached because the graph cannot see what they load. A later step could let a declared edge replace the "never cached" verdict for tsconfig `paths`.

**(e) Risk.** Additive inputs cannot cause a stale pass. A `runtime` command is a new thing to run on every verify, so it needs a timeout and must be refused in `--dry-run`. The real risk is the follow-on: letting users subtract inputs, which is how Nx and Turborepo configurations go stale. Do not add that.

**(f) Effort.** S for the first step, M for all four kinds.

### 3. Say which input changed on a cache miss

**(a) Mechanism.** Turborepo's `--summarize` writes a JSON file per run with each task's hash inputs, and the docs say "Comparing two summaries will show why two task's hashes are different"; `--dry=json` gives the same without running. `turbo query affected` returns a typed `reason` per result (`FileChanged`, `LockfileChanged`, `DependencyChanged`, `GlobalDepsChanged`). moon keeps a hash manifest per task for debugging. Nx has `nx affected --explain`. Verified: [Turborepo caching](https://turborepo.dev/docs/crafting-your-repository/caching), [turbo query](https://turborepo.dev/docs/reference/query), [moon cache](https://moonrepo.dev/docs/concepts/cache), [Nx affected](https://nx.dev/ci/features/affected).

**(b) firescale today.** `FSCALE_LOG=cache` prints `hit`, `miss`, `not cached` with a reason, `stored` or `not stored`. A miss gives no cause; I found no code in `stepcache.rs` that compares a miss with the last pass. Two special cases are explained: a version upgrade and a config setting every key holds.

**(c) Change.** Alongside each stored pass, keep a small manifest of component digests under a stable id (step, project, command): one digest per graph node in scope, one for the env set, one per tool, one for config settings. On a miss, compare with the last manifest for that id and print the components that differ: "ran because: backend/shared changed (2 files), NODE_OPTIONS changed". First step: file components only, per graph node, shown under `FSCALE_LOG=cache`.

**(d) Benefit.** People who wonder why a step ran again, and agents, which otherwise cannot tell a useful rerun from a cache that never hits. It also gives firescale's own maintainers a way to see over-wide keys.

**(e) Risk.** None for correctness: it never decides a hit. It must store digests only, never env values. Per-file detail would make the manifest large on big repos, so keep it per node with a count.

**(f) Effort.** S for file components, M for all.

### 4. Never excuse a flake in a test the change added or edited

**(a) Mechanism.** nextest marks a test flaky when it fails and then passes on retry, and by default counts it as a pass; since 0.9.131 `flaky-result = "fail"` makes it fail the run. Develocity Predictive Test Selection always selects tests that are "recently new, recently changed, recently failed, or recently flaky". Nx Cloud calls a task flaky only when the same input hash has both failed and passed, and drops the flag after two weeks with no incident. Google's Flake Aware Culprit Finding paper states that plain reruns stop gaining accuracy while cost keeps rising. Verified: [nextest retries](https://nexte.st/docs/features/retries/), [Develocity PTS](https://docs.develocity.ai/predictive-test-selection/), [Nx flaky tasks](https://nx.dev/ci/features/flaky-tasks). Search only: [Flake Aware Culprit Finding, ICST 2023](https://research.google/pubs/flake-aware-culprit-finding/).

**(b) firescale today.** A `new` failure runs once more; a pass makes it `flaky`, which does not block (`rerun_new_failures` in `crates/core/src/blame.rs`, ADR 0039). No history is kept, by design (`SPEC.md` section 12). So a change that introduces a test failing one run in three is let through most times it fails.

**(c) Change.** Two parts.
1. A test that the change added or edited, or a failing test in a file the change touched, is never labelled `flaky`: it stays `new` with a note. The tamper guard already knows the changed test files and per-test results already name the failing tests.
2. `verify.flaky: "block" | "allow"` (default `allow`, as now), mirroring nextest.

First step: part 1 for runners with per-test results (pytest, Vitest, Jest, `cargo test`).

**(d) Benefit.** Reviewers and repos using the stop hook: an agent can no longer leave behind a test that only sometimes passes.

**(e) Risk.** This makes verify stricter, so there is no stale-pass risk. It will block some true flakes that happen to sit in a touched file; the note should say how to proceed. It does not need flake history, so it stays inside the spec.

**(f) Effort.** S.

### 5. Run the most likely failures first

**(a) Mechanism.** Google's TAP study found that very few tests ever fail and that those that do are generally "closer" to the changed code. bazel-diff exposes that as `target_distance`: the number of dependency hops from an impacted target to a directly changed one. Search only for the paper abstract: [Taming Google-Scale Continuous Testing](https://research.google/pubs/taming-google-scale-continuous-testing/). Verified: [bazel-diff README](https://github.com/Tinder/bazel-diff).

**(b) firescale today.** Units run in folder order (`plans.sort_by` on root and kind in `crates/core/src/verify.rs`), with `--jobs` defaulting to 1. The base run already orders by kept time, quickest first (ADR 0062).

**(c) Change.** With a known change set, order units by: holds a changed file, then graph distance from one, then kept time ascending. Print order stays as it is. First step: changed-first only.

**(d) Benefit.** Agents in the stop hook and people on slow repos. In the README's course repo the 1h 36m suite currently runs wherever its folder name sorts.

**(e) Risk.** None for correctness: every unit still runs. Units that share a database may interact differently in a new order, which is already true of `--jobs`.

**(f) Effort.** S.

### 6. A rule for undeclared cross-project imports, and all owners on an ambiguous import

**(a) Mechanism.** `turbo boundaries` (experimental) reports two things: an import of a file outside the package's folder, and an import of a package not declared in `package.json`. Pants' `python-infer` has `unowned_dependency_behavior` (`error`, `warning`, `ignore`; default `warning`) for imports with no owner, `ambiguity_resolution` (default `none`: no inference when several targets provide a module), `string_imports` for strings that look like module paths (Django settings), and inferred edges to `conftest.py` and `__init__.py`. Verified: [turbo boundaries](https://turborepo.dev/docs/reference/boundaries), [Pants python-infer](https://www.pantsbuild.org/stable/reference/subsystems/python-infer).

**(b) firescale today.** Edges carry a trust tier and evidence (`docs/graph.md`). A Python import that could resolve in more than one place takes the first match: "The first folder above the project that has the module ... wins". Dynamic imports widen to every Python project. Nothing reports an import edge (tier 3) that no manifest edge (tier 1) backs.

**(c) Change.**
1. A built-in rule, report-only by default: a cross-project import with no manifest dependency behind it. The data is already in the graph.
2. When an import has more than one candidate owner, add an edge to each and a warning, instead of first-wins.
3. Later: Pants-style string imports as tier-4 edges.

First step: item 2, because first-wins can miss a real edge.

**(d) Benefit.** Everyone using `--affected` or the step cache: both trust this graph. Teams also get a boundaries check without Nx or Turborepo.

**(e) Risk.** Item 2 only adds edges, so it is safe. Item 1 is a lint and cannot affect results. False positives are likely for Python layouts that rely on `PYTHONPATH`, hence report-only.

**(f) Effort.** S for item 2, M for the rule with fixtures and a SKILL.md.

### 7. A JSON affected plan with typed reasons and an exit code

**(a) Mechanism.** `turbo query affected` prints affected tasks or packages for a base and head, each with a typed reason, and `--exit-code` returns 1 when something is affected, 0 when nothing is, 2 on error. `turbo ls --affected --output json` does the same for packages. Verified: [turbo query](https://turborepo.dev/docs/reference/query).

**(b) firescale today.** `verify --affected --dry-run` prints "Why each ran" as text and "has no `--json` form" (`docs/verify.md`, "Dry run"). `fscale graph --json` carries nodes and edges but not what a change affects. I did not check whether the JSON of a real run carries the reasons.

**(c) Change.** `fscale verify --affected --dry-run --json` (or `fscale graph --affected --json`): units, status, and reasons as typed objects (`changed-file`, `depends-on` with the edge source, `root-lockfile`, `ci-file`, `unowned-file`, `config-setting`). Add `--exit-code`. First step: the JSON only.

**(d) Benefit.** CI authors who gate jobs on what changed. firescale's own workflow has a hand-written `changes` job (ADR 0066) that this would replace.

**(e) Risk.** None for correctness. It is one more schema to keep stable.

**(f) Effort.** S.

### 8. Confirm a new failure by running only the failing tests first

**(a) Mechanism.** nextest retries the individual failing test, not the suite, with optional backoff. Bazel's `flaky = True` reruns the test target up to three times, and `--flaky_test_attempts` caps attempts at 10. Nx retries a flaky task once, on a different agent. Verified: [nextest retries](https://nexte.st/docs/features/retries/), [Bazel common definitions](https://bazel.build/reference/be/common-definitions), [Bazel user manual](https://bazel.build/docs/user-manual), [Nx flaky tasks](https://nx.dev/ci/features/flaky-tasks).

**(b) firescale today.** The rerun runs the step's whole command again inside what is left of `verify.baseBudget` (5 minutes by default). A slow suite therefore gets no rerun and blocks unlabelled, or costs a second full run.

**(c) Change.** Use the rerun asymmetrically. Run only the failing tests first (pytest node ids, `vitest -t`, Jest `-t`, `cargo test <name>`). If they fail again alone, the failure is confirmed `new` at once. If they pass alone, that proves nothing (order dependence is a real bug), so fall back to the whole-step rerun before calling it `flaky`. First step: pytest only, where node ids are exact and firescale already adds flags for `--maxfail=1`.

**(d) Benefit.** Slow suites, which today cannot afford the rerun. The common case, a real failure, is confirmed in seconds.

**(e) Risk.** The danger is the opposite shortcut: treating "passes alone" as `flaky` would let order-dependent failures through. The design above never does that. Selecting tests by name is fragile for parametrised ids; on any selection error, fall back to the whole step.

**(f) Effort.** M.

### 9. Merge queue support

**(a) Mechanism.** GitHub's merge queue builds a temporary branch for each group and fires a `merge_group` event; a workflow without that trigger never reports and the queue waits. A group that fails is dropped and later groups are rebuilt without it. Search only (the docs URL I tried returned 404; behaviour is from search summaries of the GitHub Enterprise Server copy of ["Managing a merge queue"](https://docs.github.com/en/enterprise-server@3.17/repositories/configuring-branches-and-merges-in-your-repository/configuring-pull-request-merges/managing-a-merge-queue)).

**(b) firescale today.** `DECISIONS.md` line 370 and ADR 0066 both say the workflow has no `merge_group` trigger. The tamper guard's CI reader knows the event name (`crates/core/src/tamper/ci.rs`). `--pr-base` reads the base from pull request events only (`docs/verify.md`). The shared cache writes only on a push to the default branch, so a queue run would be read-only, which is right.

**(c) Change.** Add `merge_group` to the starter workflow (opt-in via a rule option), and teach `--pr-base` to take the base from the merge group event's base commit. First step: the `--pr-base` support with a test, since a user can add the trigger by hand.

**(d) Benefit.** Repos that turn the queue on after adopting firescale. Without this their required check hangs.

**(e) Risk.** Low. The base must come from the event payload, never from a ref in the clone, as for pull requests.

**(f) Effort.** S.

### 10. A persistent build folder for Rust base runs (hypothesis, measure first)

**(a) Mechanism.** Cargo decides freshness by file modification time by default; the unstable `-Zchecksum-freshness` replaces that with content checksums and is described as "most useful on systems with a poor mtime implementation, or in CI/CD". A fresh checkout or worktree therefore rebuilds. sccache caches compiler outputs across folders but cannot cache crates that invoke the linker (`bin`, `dylib`, `cdylib`, `proc-macro`) or incremental builds. Verified: [Cargo unstable features](https://doc.rust-lang.org/nightly/cargo/reference/unstable.html) (tracking issue 14136), [sccache README](https://github.com/mozilla/sccache).

**(b) firescale today.** The base worktree links in ignored `node_modules`, `.venv` and `venv` (`docs/verify.md`). I found no mention of `CARGO_TARGET_DIR` or `target/` anywhere in the repo, so a Rust base step appears to compile every dependency from nothing, inside a 5-minute budget. Not confirmed by a run.

**(c) Change.** First, measure a Rust base run on firescale's own repo. If it is as slow as expected, give base runs a persistent target folder of their own per repo (for example under the temp folder, keyed like `firescale-runs`), set through `CARGO_TARGET_DIR`, so the second base run is incremental. Do not share the head's `target/`.

**(d) Benefit.** Rust repos, firescale's own included: without this, most Rust failures probably end as "base ran over the budget", which blocks with no label.

**(e) Risk.** A separate folder cannot change head results. Whether cargo would reuse dependency builds across worktree paths in one target folder is from memory and unverified, which is why the proposal keeps the folders apart. Disk use grows; it needs pruning.

**(f) Effort.** S after the measurement.

---

## The long tail

### 11. Check that "affected" always covers "key differs", then consider merging the two

**(a)** bazel-diff computes a hash per target at two revisions (rule attributes, source content, and the hashes of its inputs) and reports the targets whose hash differs. Nx's `affected` is now task-based: it selects tasks whose declared inputs match a changed file, and project-based selection is the legacy mode. Turborepo has the same behind `futureFlags.affectedUsingTaskInputs`. Verified: [bazel-diff](https://github.com/Tinder/bazel-diff), [Nx affected](https://nx.dev/ci/features/affected), [turbo run](https://turborepo.dev/docs/reference/run).
**(b)** firescale has two definitions of reach: the graph walk for `--affected`, with a list of special cases (`docs/graph.md`), and the step key's file scope. Blame-aware verify already uses the key for "same inputs on base and head".
**(c)** First step: a property test in firescale's own suite that, over fixture repos and random changes, every unit whose key differs between base and head is in the affected set. Later, define affected as "key differs" and keep the graph for the explanation.
**(d)** Maintainers: one definition instead of two that can drift.
**(e)** The test adds no risk. The merge is risky only if the key is narrower than the graph anywhere, which is what the test finds.
**(f)** S for the test, L for the merge.

### 12. Lockfile-aware affected

**(a)** Nx's `projectsAffectedByDependencyUpdates` takes `"all"` (the default, described as "Safest"), `"auto"` (only projects whose resolved dependencies changed, by diffing the lockfile at both revisions; pnpm, npm, yarn and bun) or a list. Turborepo's package hash includes "lockfile changes that affect the package". Pants documents the opposite choice: a change to one third-party dependency counts as a change to every user of any dependency. Verified: [nx.json reference](https://nx.dev/reference/nx-json), [Turborepo caching](https://turborepo.dev/docs/crafting-your-repository/caching), [Pants advanced target selection](https://www.pantsbuild.org/stable/docs/using-pants/advanced-target-selection).
**(b)** A root lockfile affects every project (`docs/graph.md`); a lockfile inside a project is that project's (`graph.rs` test near line 2121).
**(c)** Smallest useful step: a root lockfile of one ecosystem does not affect a project of another ecosystem that has no edge to a project of the first. Per-importer pruning of `pnpm-lock.yaml` would be a second step.
**(d)** Mixed repos with root JS tooling beside Python or Go services.
**(e)** Real risk. Nx itself keeps `"all"` as the default. A Python test that shells out to a Node tool would be missed by the ecosystem rule, and the root lockfile's bytes are in every unit's key through "files in folders above", so affected and the cache would disagree (see idea 11). Blame also loses the base for a changed lockfile without `verify.baseInstall`.
**(f)** M for the ecosystem rule, L for pruning.

### 13. Cheaper file digests: reuse git's knowledge or keep a stat cache

**(a)** Turborepo reads committed files' object ids with `git ls-tree -r -z HEAD`, finds changed and untracked files with git status, and hashes only those, in process and in parallel with rayon; it falls back to manual hashing when git fails. Nx moved to a Rust hasher in 15.8 and reported 72 ms against 330 ms for the git-based one on Windows. Git's untracked cache and fsmonitor skip directory reads and `lstat` calls. BLAKE3 maps files of 16 KiB or more into memory for about a 10% single-thread gain, and its docs warn that threads hurt under 128 KiB or on spinning disks. Verified: Turborepo source ([package_deps.rs](https://github.com/vercel/turborepo/blob/main/crates/turborepo-scm/src/package_deps.rs), [ls_tree.rs](https://github.com/vercel/turborepo/blob/main/crates/turborepo-scm/src/ls_tree.rs), [hash_object.rs](https://github.com/vercel/turborepo/blob/main/crates/turborepo-scm/src/hash_object.rs)), [git update-index](https://git-scm.com/docs/git-update-index), [blake3 Hasher](https://docs.rs/blake3/latest/blake3/struct.Hasher.html). Search only: [Nx 15.8 blog](https://nx.dev/blog/nx-15-8-rust-hasher-nx-console-for-intellij-deno-node-and-storybook).
**(b)** Every verify lists files with `git ls-files --cached --others --exclude-standard`, then reads and BLAKE3-hashes every listed file on up to 16 threads (`Snapshot::capture_reusing`). Size and time stamps let a second capture in the same run reuse a hash, with a 2-second racy margin, but are "never kept across runs".
**(c)** First step: time the snapshot under `FSCALE_LOG=cache` and on the 50,000-file fixture. Only if it matters, persist the stamp cache across runs with size, modification time, and on Unix change time and inode.
**(d)** Large repos and the stop hook. On the small repos firescale targets, probably nobody.
**(e)** This is the idea most in tension with the rule. Any stat-based shortcut trusts timestamps, which is the weakness cargo is moving away from. Git object ids are of normalised content, while firescale deliberately hashes bytes so a CRLF checkout is a different key; and the tamper guard already distrusts `skip-worktree` and `assume-unchanged` bits. A file watcher adds a daemon whose missed events become stale passes; watchman's own docs call timestamps "prone to race conditions" ([clockspec](https://meta.github.io/watchman/docs/clockspec), verified).
**(f)** S to measure, M to build.

### 14. Committed duration hints, and a warning for over-generous limits

**(a)** Bazel tests declare a `size` (small, medium, large, enormous) that implies a timeout of 60, 300, 900 or 3,600 seconds, and `--test_verbose_timeout_warnings` warns when a test finishes far inside its timeout. Pants has `timeout_default` and `timeout_maximum`. Verified: [Bazel test encyclopedia](https://bazel.build/reference/test-encyclopedia), [Bazel user manual](https://bazel.build/docs/user-manual), [Pants test goal](https://www.pantsbuild.org/stable/docs/python/goals/test).
**(b)** Kept times live in `.firescale/step-times.json` on one machine. A step with no kept time always runs under `--budget`, so a fresh clone or a fresh worktree gets no protection from a 1h 36m suite on its first run.
**(c)** An optional committed hint, `verify.projects.<folder>.test.expected: "1h 40m"`, used only when no kept time exists. Separately, a note when a step's limit is more than ten times its kept time. First step: the hint.
**(d)** New contributors and agents working in fresh worktrees.
**(e)** A stale hint leaves a step out of a `--budget` run that is already `PARTIAL`, so nothing reads as a pass. No cache effect.
**(f)** S.

### 15. Name the commit that broke it

**(a)** bors tested pull requests in batches and split a failing batch in two to find the culprit; it is archived since April 2024 and points to GitHub's merge queue. Mergify splits a failing batch the same way and can test `PR1`, `PR1+PR2`, `PR1+PR2+PR3` in parallel. Google's culprit finder ranked the breaking change in the top 5 of thousands 82% of the time across 140 projects. Verified: [bors-ng README](https://github.com/bors-ng/bors-ng), [Mergify speculative checks](https://docs.mergify.com/merge-queue/speculative-checks/). Search only: [Who broke the build?, ICSE-SEIP 2017](https://research.google/pubs/who-broke-the-build-automatically-identifying-changes-that-induce-test-failures-in-continuous-integration-at-google-scale/).
**(b)** Blame compares the head with one base commit and gives a yes-or-no label.
**(c)** `fscale verify --bisect`: for a `new` failure on a branch with several commits, run that one step over the commits between base and head in worktrees, reusing `.firescale/base/` results. First step: one failing step, bisect order, no flake handling.
**(d)** People on long branches. Agents usually have one uncommitted change, so they gain little.
**(e)** No cache risk. A flaky test sends plain bisection to the wrong commit, which is the Flake Aware paper's whole point; the output must call the result a suspect.
**(f)** M.

### 16. Pass shard settings through in CI

**(a)** nextest offers `hash:m/n` partitions that stay stable when tests are added, and `slice:m/n` for even buckets; Bazel sets `TEST_SHARD_INDEX` and `TEST_TOTAL_SHARDS`; Nx Atomizer makes one cacheable target per test file plus an aggregate `e2e-ci` target. Verified: [nextest partitioning](https://nexte.st/docs/ci-features/partitioning/), [Bazel test encyclopedia](https://bazel.build/reference/test-encyclopedia), [Nx Atomizer](https://nx.dev/ci/features/split-e2e-tasks).
**(b)** A step is one command; `--jobs` runs units in parallel, not parts of a unit.
**(c)** At most: `fscale verify --shard k/n` in CI, mapped to each runner's own flag, with the shard in the key and the headline reading `PARTIAL` for a single shard.
**(d)** CI on repos with one very slow suite.
**(e)** Per-file caching, the real gain of Atomizer and Pants, needs per-file inputs that firescale cannot prove without a sandbox. Nx's own docs note that each file's inputs default to the whole app, so a small change reruns every file anyway.
**(f)** M. Low priority.

### 17. Signed shared-cache records

**(a)** Turborepo can sign remote artifacts with HMAC-SHA256 using `TURBO_REMOTE_CACHE_SIGNATURE_KEY`; an artifact that fails verification is treated as a miss. Nx's remote cache specification requires a 409 response on an attempt to overwrite an existing key. Verified: [Turborepo remote caching](https://turborepo.dev/docs/core-concepts/remote-caching), [Nx deprecated self-hosted cache packages](https://nx.dev/docs/reference/deprecated/self-hosted-cache-packages).
**(b)** A record names its own writer, and a lookup requires that writer to be the default branch (`sharedcache.rs`). That field is self-declared, so the real protection is GitHub's cache scoping plus the generated workflow's read-only pull request job. `write` replaces an existing record by rename.
**(c)** Optional `FSCALE_SHARED_CACHE_KEY`: records carry an HMAC, and unsigned records are ignored when the key is set.
**(d)** Repos with other workflows that run in the default branch's context.
**(e)** Low value now. GitHub made the Actions cache read-only for untrusted triggers on 2026-06-26 (search only: [GitHub changelog](https://github.blog/changelog/2026-06-26-read-only-actions-cache-for-untrusted-triggers/)). A signing key is also only as safe as the workflows that can read the secret.
**(f)** S-M.

---

## What firescale should not adopt

1. **Predictive (machine-learned) test selection.** Meta reports keeping more than 95% of individual test failures and more than 99.9% of faulty changes while halving cost, which means some failures are missed by design ([arXiv 1810.05286](https://arxiv.org/abs/1810.05286), search only). Develocity needs 14 days of history, recommends it for pull request builds, and tells users to run the remaining tests later ([Develocity PTS](https://docs.develocity.ai/predictive-test-selection/), verified). CloudBees Smart Tests (formerly Launchable) selects to a confidence target (search only; its docs and blog describe the method differently). firescale has no history service, and its headline must stand for the repo. `--budget` with `PARTIAL` is the honest form of this.

2. **Pull-request-scoped or laptop-written shared caches.** Nx Cloud lets feature branches write to their own isolated cache while only protected branches write the shared one ([CREEP post](https://nx.dev/blog/cve-2025-36852-critical-cache-poisoning-vulnerability-creep), verified). That works because a server records who wrote what. Nx deprecated its bucket-based packages on 2026-05-21, stating the flaw "is in their design and cannot be patched". For firescale it is worse than for a build cache: a record is the verdict itself. A pull request could plant a pass record for the key of its next, clean-looking commit. This last point is my own reasoning, not from a source. Keep the default-branch-only rule of ADR 0041.

3. **Laptops reading CI's cache.** Gradle and Bazel both recommend that CI writes and developers only read ([Gradle build cache](https://docs.gradle.org/current/userguide/build_cache.html), [Bazel remote caching](https://bazel.build/remote/caching), verified). It pays off for them because toolchains are pinned or hermetic. firescale's key holds the OS, the architecture, the runner image and each program's path, size and time, so a laptop would almost never match. Bazel's page lists tools outside the workspace as a known cause of wrong hits.

4. **Caching failures.** Bazel's `--cache_test_results=auto` reruns a test that failed; only `yes` may cache failures ([Bazel user manual](https://bazel.build/docs/user-manual), verified). ADR 0036 is the same choice. Keep it.

5. **A daemon or watch mode.** Nx's 22.x changelog includes a fix described as "skip stale recomputations and prevent lost file changes in daemon" (search only), which is the failure class a daemon adds. A fully cached verify already takes 36 ms.

6. **Sandboxing or file-access tracing, for now.** Nx 22.6 traces each task's real reads and writes and flags those outside its declared inputs and outputs; its Ultracache records real reads automatically (search only: [Nx sandboxing docs](https://nx.dev/docs/features/ci-features/sandboxing)). This is the principled fix for firescale's computed scopes, but it needs a tracer per operating system, and firescale states it does not sandbox. A Linux-only audit mode in CI could be revisited later.

7. **File-level narrowing of `--affected` or of the key.** Pants reports about 30% fewer dependencies per file from file-level inference, and states that failing to infer one "can never cause the wrong things to be cached" because processes run in sandboxes ([Pants dependency inference post](https://www.pantsbuild.org/blog/2020/10/29/dependency-inference), verified; from 2020). Without a sandbox the same miss is silent. Take Pants' diagnostics (idea 6), not its granularity.

8. **Early cutoff and "build without the bytes".** Buck2's DICE skips recomputing a node whose dependencies recompute to the same values ([Buck2 DICE](https://buck2.build/docs/insights_and_knowledge/modern_dice/), verified), and Bazel's `--remote_download_outputs` avoids fetching outputs ([Bazel command-line reference](https://bazel.build/reference/command-line-reference), verified). Both depend on build outputs with digests. firescale's steps produce a verdict, not artifacts. The one transferable piece, "same inputs on base and head means same result", is already in blame-aware verify.

9. **Remote execution and task distribution.** Nx's distributed execution and the Bazel remote execution API move work to other machines. firescale is not a task runner. I did not read primary sources on these.

10. **Timestamp freshness.** mise skips a task when the newest source is older than the newest output unless `task.source_freshness_hash_contents` is set ([mise task configuration](https://mise.jdx.dev/tasks/task-configuration.html), verified). Cargo is moving the other way. firescale already hashes bytes.

11. **Quarantining or muting flaky tests.** Buildkite Test Engine can set a test to muted or skipped (search only: [Buildkite quarantine docs](https://buildkite.com/docs/test-engine/quarantine)). firescale's tamper guard exists to report exactly that kind of change.

12. **Layer or function-call caching as in Dagger and Earthly.** Dagger caches module function calls by default since 0.19.4 with a seven-day default lifetime (search only: [Dagger function caching](https://docs.dagger.io/extending/function-caching/)); a time-based lifetime is the opposite of an input-keyed pass. Earthly ended active maintenance of its open-source project (search only: [Earthly announcement](https://earthly.dev/blog/shutting-down-earthfiles-cloud/)), so it is not a model to follow.

---

## Where firescale already matches the best practice

- **Only trusted branches write the shared cache.** This is the fix Nx Cloud describes for CREEP, and `SPEC.md` cites the CVE.
- **A pass is not stored when inputs changed during the step.** Bazel's remote caching page lists "an input file is modified during a build" as a known cause of invalid cache entries; firescale's `remember` refuses in that case.
- **Platform and runner image in the key.** wireit adds `ImageOS` to its fingerprint only under GitHub Actions caching; firescale does the same with `ImageOS` and `ImageVersion`.
- **Content hashes, not timestamps.** This is where cargo is heading with checksum freshness.
- **Shallow or unknown history widens to everything.** Turborepo documents the same: "If the checkout is too shallow, then all packages will be considered changed."
- **Fixed locale and time zone variables in the key.** Bazel requires `TZ=UTC` and unset `LANG` and `LC_*` for tests; firescale keys `TZ`, `LANG` and `LC_ALL` instead of forcing them.

## Not verified, or not covered

- **Not researched from primary sources:** `pre-commit`, `just`, Lage's cache key (the page I read only says source files and arguments), Graphite and Aviator queues, Buck2 and Please test caching, Bazel persistent workers, Gradle configuration cache and test distribution, Nx Agents distribution details, the Nx daemon's design, and Turborepo watch mode. Nothing in this report depends on them.
- **Nx Rust core in versions 20 to 22:** only the 15.8 hasher post and scattered 22.x changelog lines were found. I could not confirm a summary of the Rust core changes.
- **Cargo's fingerprint internals:** the rustc-dev module page returned 404. Idea 10's claim about reuse across worktree paths is from memory.
- **Pants caching only successful test results:** from memory; the page I read does not say it.
- **sccache's key recipe:** the README covers path handling and what is not cached, not the recipe.
- **Meta's "one third of tests" figure:** not found; only the 95%, 99.9% and halved-cost figures are supported by the abstract.
- **GitHub merge queue wording:** the docs URL returned 404; behaviour is from search summaries of the Enterprise Server copy.
- **Repo facts I inferred and did not run:** that a Rust base run compiles from scratch (idea 10), and that a real run's `--json` lacks affected reasons (idea 7).
