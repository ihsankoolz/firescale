# Ponytail audit of firescale

Date: 2026-11-14. Commit audited: `6c77f66` (`main`, after M35 was merged). No code was changed; this file is the only change.

## How it was run

- Skill: `ponytail-audit` from `DietrichGebert/ponytail` (skill hash `20dca08a…cde4c`), installed for one session only and kept out of this repo. It asks for a read-only, whole-repo audit: bugs, risks, behaviour under load, risky code without tests, slow paths, and "lean" items (delete, reuse, merge, split). Every finding needs a concrete case, and code is called unused only after grepping the whole tree.
- The owner's format replaces the skill's ranking: simplification items are ordered by how much code each would remove. Each item lists the files, what would be removed or merged, what could break, and whether a test covers it. Bugs found along the way are listed after the items.
- The repo was read in five parts, side by side, each by a fresh reader: the rule packs and project detection; the tamper guard; verify and what it runs; facts, the graph, the rule engine and the CLI crate; and everything that is not Rust. The highest-ranked claims were then checked against the code by hand (marked **checked** under "Bugs found").
- Line counts are `wc -l` at `6c77f66`. "Lines removed" are estimates of net lines: the copies counted, minus the shared code that would replace them. Items overlap in places, so the totals are rough.

**What this repo does:** `fscale` is one Rust binary, shipped through npm, with a small TypeScript side. It checks a repo against rules and fixes what it can, and it builds new repos the same way. `verify` runs check, lint, typecheck and tests, and holds a change only to the failures it introduced. A tamper guard catches changes that pass by weakening the tests. **Load assumed:** one person or one coding agent at a time, on a laptop or a CI runner, on repos of up to about 50,000 files (`bench/`). Nothing is shared between machines except the CI step cache.

## The marker

**⛔ Do not simplify without my say** marks every item that touches a safety behaviour. The letter says which one:

- **(a)** Never overwriting a person's file. Fixes create missing files, merge into structured files, append, or edit inside a managed block (SPEC §15, §16).
- **(b)** Setup versus failure: `SETUP NEEDED` is never reported as a code failure, and the reverse.
- **(c)** The cache's trust rules:
  - A key covers at least what `graph::affected` covers.
  - Only passes are cached.
  - A step that needs a service outside the repo is never cached.
  - Pull requests never write the shared cache.
  - `pre-existing` needs `signature_within`, and `fixed` comes only from `confirm_fixed`.
  - The rule-result cache's inputs cover what a rule reads.
- **(d)** The tamper guard's base-only policy: policy, `testPaths` and `allow` are read from the base commit only, and approvals made by the change itself do not count.
- **(g)** Not on your list, but marked the same way: the agent runner's guardrails (SPEC §7) and keeping secrets out of output.

## Items, ordered by how much code each would remove

### 1. A shared reader layer for the ten tamper-guard languages: about −340

- **Files:** `crates/core/src/tamper/{js,python,rust,java,kotlin,swift,dart,cfamily,csharp,go}.rs` (18,176 lines in all: 11,368 of code, 6,808 of tests), `tamper/reach.rs` (205).
- **Removed or merged:** every `read()` repeats the same steps:
  - the parse prologue: parse, fall back to `has_errors`, `reach::Dead::of`, build the `TestFile` (16–23 lines × 10);
  - the stack that pops closed scopes (5–8 × 10);
  - the `Assertion { … }` literal plus the "innermost test" lookup (about 15 × 8);
  - the `::` id join (about 6 × 6);
  - the identifier walk for `uses` (6–9 × 7);
  - small helpers copied word for word: `squeezed` (java.rs:178, dart.rs:288, swift.rs:250), `children` (python.rs:128 = java.rs:164), `arguments` (go.rs:274, cfamily.rs:249).

  The fix is a `Spec` beside `reach::Grammar`, with `open()`, `Scopes`, `TestFile::add_assertion` and `identifiers()`. Each language keeps what is its own: what a test is, skips, weights, which calls assert, imports and suppression keys.

- **Could break:** parse-error strictness. JS, Python and Rust mark a file "not checked" on any parse error. The M32 readers ignore harmless errors (ADR 0052, for the eight new languages only). A shared `open()` must take a flag, or JS, Python and Rust silently change. JS weights assertions by the suite, and Dart always uses weight 1.
- **Test:** yes. There are about 240 reader unit tests (each `a_file_that_does_not_parse_says_so`, `deep_nesting_does_not_overflow`, …). `tamper.test.ts` runs the 458-case labelled corpus (0 missed, 0 false today).

### 2. One table-driven test for the nextjs pack's six rule test files: about −270

- **Files:** `crates/rules/src/nextjs/{app-router,eslint,gitignore,next-config,package-json,tsconfig}/tests.rs` (46 lines each). The `meta_names_the_rule_and_its_fix` test is in all six `python/*/tests.rs`.
- **Removed or merged:** the six nextjs files differ in 4 lines each: the dropped file and the expected finding. One table test in `nextjs/tests.rs` covers passing, failing, fixed and fixed twice. The six python meta tests repeat `python/pack_tests.rs` `every_rule_has_a_fix_and_a_skill`.
- **Could break:** only the per-rule test names in the output. AGENTS.md's "passing, failing, fixed" stays true.
- **Test:** these are the tests. `nextjs/tests.rs` `an_empty_folder_misses_everything_and_each_finding_is_deterministic` remains.

### 3. Copied helpers in the TypeScript pack tests → `test-helpers.ts`: about −200 ⛔ (a)

- **Files:** `packages/cli/src/{supabase,expo,go,maven,cmake,nextjs,python}-pack.test.ts`, `generators.test.ts`, `agent-files.test.ts`, `scaffold.test.ts`, `group-project.test.ts`, `test-helpers.ts` (211).
- **Removed or merged:** these copies go into one shared version each:
  - `snapshot(root)` (17 lines) is byte-identical in 8 files;
  - `diagnostics(root)` (4 lines) is identical in 10 files, and `group-project` adds an `env` parameter;
  - `interface Diagnostic` is in 11 files.

  `scaffold.test.ts` keeps its own `snapshot`, because it skips `.firescale`.

- **Could break:** these snapshot comparisons are the proof that `adopt --dry-run`, `sync` and a refused `add` leave a person's files alone. One wrong shared `snapshot` weakens all of them at once.
- **Test:** the tests themselves. No test checks the helper.

### 4. One "append lines to an ignore file" helper and one gitignore-lines rule: about −180 ⛔ (a)

- **Files:** `crates/rules/src/core/gitignore-basics/mod.rs` (201), `core/no-committed-env/mod.rs` (226), `{nextjs,python,expo,supabase,go}/gitignore/mod.rs` (98/106/116/117/166). Also `cmake/mod.rs:298` `line_ending` and `agent-ready/managed.rs:107` `newline_of`, which are the same function.
- **Removed or merged:**
  - Seven copies of about 20 lines (choose the line ending, separator, header, `ctx.write`, `FixResult`) become `stackkit::append_lines` plus one `eol()`, about −100.
  - nextjs, python and expo then differ only in data. One `GitignoreLines { id, entries, header, applies }` rule replaces their three rule bodies, about −80.
- **Could break:** today go and core write their header once, while the others repeat it on every append, so one shared behaviour changes a second append's output. CRLF handling must stay byte-exact. The fix must stay append-only.
- **Test:** yes. Each pack's `gitignore/tests.rs`, `nextjs/tests.rs` `the_gitignore_lines_are_appended_in_its_own_line_endings`, and `core/no-committed-env/tests.rs`.

### 5. One Rust test harness for the stack packs: about −160

- **Files:** `crates/rules/src/{nextjs,expo,supabase}/tests.rs` (279/112/169), `{python,go,maven,cmake}/pack_tests.rs` (208/184/143/211).
- **Removed or merged:**
  - `rules()`, `findings()` and `fix_all()` are copied 7 times (117 lines).
  - `the_pack_has_N_rules_with_skills` is copied 6 times (78 lines).

  The fix is a `#[cfg(test)] stackkit::testkit` and one test in `lib.rs` over `builtin_packs()`, with a one-line count per pack.

- **Could break:** python's variant also asserts `has_fix` and the `fscale check --rules <id>` line. Keep those.
- **Test:** these are the tests; a mistake is a compile error.

### 6. Other copied TypeScript test helpers: about −110 ⛔ (d)

- **Files:** `packages/cli/src/`: `tamper-ci.test.ts`, `tamper-approvals.test.ts`, `agent-runner.test.ts`, `agent-adapters.test.ts`, `doctor.test.ts`, `facts.test.ts`, `step-cache.test.ts`, `verify.test.ts`, `graph.test.ts`, `python-env.test.ts`, `blame.test.ts`, `stop-hook-blame.test.ts`, `bridge.test.ts`, `launcher.test.ts`, `release.test.ts`, `pack.test.ts`, `scaffold.test.ts`, `verify-run.test.ts`.
- **Removed or merged:**
  - `git()` with a fixed identity: 13 lines, in tamper-ci and tamper-approvals.
  - `git()`: 5 lines, in agent-runner and agent-adapters.
  - A schema check with Ajv: 7 copies.
  - `closedPort` × 2, `linkPackages` × 2, `exists` × 3, `step()` × 2.
  - launcher's `launch()` and release's `node()` re-implement `runCli`, whose unused `bin` and `fullEnv` options exist for exactly this.
- **Could break:** test setup only. The `git()` copies build the fixtures for the tests of the base-only `tamper-guard` workflow, but a wrong identity fails loudly. doctor's validator uses `allErrors`.
- **Test:** the tests themselves.

### 7. Build `#if` regions one way (Swift, C, C#): about −110

- **Files:**
  - `tamper/swift.rs` `Directives` (687–797);
  - `tamper/cfamily.rs` `Regions` (556–653);
  - `tamper/csharp.rs` `Cond`/`condition_of`/`Compiled` (247–290, 636–664) and the scope pushes in `read` (734–786);
  - `tamper/recover.rs` `first_branches` (94–160).
- **Removed or merged:** `never` and `sometimes_within` are verbatim copies in Swift and C. The fix is one `Regions` type with one line-based builder for Swift and C#. C keeps its tree builder (include guards, `__cplusplus`) but returns the shared type.
- **Could break:** C#'s "return under an `#if`" rule (csharp.rs:885–893). The two copies already disagree (see bug 27).
- **Test:** yes: `csharp` `preprocessor_conditions_turn_tests_off`, `swift` `compiler_directives_gate_tests_and_assertions`, `cfamily` `code_that_never_runs_is_not_counted`, `an_include_guard_is_not_a_condition`. The corpus does not cover every branch shape.

### 8. One repo-relative path module: about −95 ⛔ (a)(c)

- **Files:**
  - `graph.rs` 228–326 (`abs`, `join`, `parent`, `inside`, `resolve`, `read`);
  - `facts.rs` (`read_text`, `exists`, `in_dir`, `is_inside`, `resolve_include`);
  - `stepcache.rs` 114–120, 349–359;
  - `services.rs` 516–522, `scratch.rs` 33–36, `hooks.rs` 174–177, `verify.rs` 1487–1492, `doctor.rs` 418–423, `blame.rs` 821–828;
  - inline `split('/').fold(root…)` in `cache.rs:167`, `main.rs:776/818` and `ecosystems.rs:34`.
- **Removed or merged:** one `paths` module; `resolve_include(f, t)` is `resolve(parent(f), t)`.
- **Could break:** the copies differ in small ways:
  - `services::join` treats `""` as the root, `facts::in_dir` treats `"."` as the root, and `graph::join` accepts both.
  - `facts::is_inside` is strict, while `graph::inside` includes equality.

  Every call site needs checking. `hooks::resolve` is where the hook file is written. `stepcache::abs`/`inside` feed cache keys. `rule.rs` `resolve`/`validate_relative`, the write path, must stay as it is.

- **Test:** broad (graph, facts, stepcache unit tests; `graph.test.ts`, `facts.test.ts`, `step-cache.test.ts`). None checks the empty and `"."` cases across call sites.

### 9. Merge Kotlin into Java's shared pieces: about −95

- **Files:** `tamper/java.rs` (1929), `tamper/kotlin.rs` (1918; it already imports 18 items from java.rs).
- **Removed or merged:** these Kotlin copies are replaced by functions in java.rs over annotation names, arguments and call chains:
  - `Marks` and the 30-line `marks` match;
  - the AssertJ `cannot_fail` arm, which is identical;
  - the why-ladder of `test_case`;
  - `Scope`/`Around` (56/59 lines);
  - `in_declared_class` and `not_run`.
- **Could break:** Kotlin counts change on purpose, because this fixes bugs 5 and 18. The corpus has no Kotlin `assertThat`-alone case and no skip-exception case, so add them.
- **Test:** yes: `parameterized_tests_weigh_their_written_rows`, `assertions_that_cannot_fail_are_not_counted`, `a_body_that_does_nothing_or_assumes_false_first_is_a_skip` in both files.

### 10. `StepReport::new` instead of five 25-field literals: about −85 ⛔ (c)

- **Files:** `crates/core/src/verify.rs` (6,329) at 2055, 2441, 2474, 2689 and 2781.
- **Removed or merged:** a constructor that defaults the 17 rarely-set fields. The `NotAffected` branch (2439–2469) reuses the `idle` closure.
- **Could break:** nothing in behaviour. The cache-hit report must keep `cache: Some(CacheHit)`.
- **Test:** yes: verify.rs `everything_passing_is_ok`, `a_skipped_step_makes_the_run_partial_not_ok`; `step-cache.test.ts` "runs no step again on a second run with no change".

### 11. One starter-and-adopt suite for the five newest TypeScript pack tests: about −85 ⛔ (a)

- **Files:** `packages/cli/src/{supabase,expo,go,maven,cmake}-pack.test.ts` (135/114/104/116/117).
- **Removed or merged:** the starter test (13 lines), `setUp*()` (10) and "proposes no change" (6) are md5-identical in four files once pack names are normalised; supabase adds one or two lines. One `packStarterSuite(pack, files)` replaces them, and each file keeps only its gap test and facts test. Do not merge the files: the `__snapshots__` would move.
- **Could break:** as item 3, these tests prove that `adopt --dry-run` writes nothing.
- **Test:** the tests themselves.

### 12. Fill the gaps in `reach::Grammar`: about −85

- **Files:** `tamper/reach.rs`; `kotlin.rs` 365–402, 976–982; `swift.rs` 140–176, 810–815; `go.rs` 304–350; and "the first statement that does something", re-derived in java, kotlin, csharp, go, swift, cfamily, python and dart.
- **Removed or merged:** add a body-finder hook and a false-literal hook to `Grammar`, plus `reach::first_statement`. Kotlin and Swift then drop their own dead-code range code.
- **Could break:** Swift's `withKnownIssue`/`XCTExpectFailure` ranges stay Swift's own. Labelled returns in Kotest lambdas must keep working.
- **Test:** yes: `assertions_that_never_run_are_not_counted` and `code_that_never_runs_is_not_counted` (kotlin, swift, dart), and go `every_kind_of_skip`.

### 13. One "starter files" rule shape: about −80 ⛔ (a)

- **Files:** `crates/rules/src/nextjs/next-config/mod.rs` (64), `nextjs/tsconfig/mod.rs` (86), `expo/tsconfig/mod.rs` (58), `go/entry/mod.rs` (86), `maven/entry/mod.rs` (109), `cmake/entry/mod.rs` (154).
- **Removed or merged:** all six follow one pattern: "if wanted and missing, one deterministic finding; the fix calls `create()` for each file". cmake already holds its files as a table. One generic `StarterFiles { wanted, files, finding_file, message }`.
- **Could break:** nextjs's JS/TS variants, and the order in which `applies` and `uses_typescript` run. Everything must keep going through `stackkit::create`, which never overwrites.
- **Test:** yes: each rule's `tests.rs`, and each pack's "empty folder is built…" test.

### 14. One Rust fixture builder for the pack tests: about −75

- **Files:** `crates/rules/src/{expo,supabase,go,maven,cmake,nextjs}/fixtures.rs` (43/37/40/40/55/31).
- **Removed or merged:** `fixture`, `with`, `*_without` and `files` are copied verbatim (114 generic lines). They become one `testkit` function set.
- **Could break:** nothing at run time. python's fixtures have another shape and stay.
- **Test:** every per-rule and pack test uses them.

### 15. stackkit leftovers: copies of `finding`, `create` and the JSON merge: about −75 ⛔ (a)

- **Files:**
  - `stackkit::finding` copied exactly in `agent-ready/agent-permissions/mod.rs:171`, `agent-ready/verify-hook/mod.rs:49` and `agent-ready/agents-md/mod.rs:34`, with a variant at `python/gitignore/mod.rs:36`;
  - `python/mod.rs:250` `create`, the same as `stackkit::create`;
  - the pass-throughs `stackkit.rs:91` `package_json_with` and `nextjs/mod.rs:83` `declared_major`;
  - the "parse, merged, holds, `insert_merge`" sequence, three times: `stackkit.rs:76`, `agent-permissions/mod.rs:143`, `verify-hook/mod.rs:72`.
- **Removed or merged:** keep one of each. The merge sequence moves into `jsonedit`.
- **Could break:** the never-overwrite check in `create`, and the line-preserving JSON merge. Both are kept by construction, but they are the core of (a).
- **Test:** yes: `python/pack_tests.rs`, `agent-permissions/tests.rs`, `verify-hook/tests.rs`, nextjs and expo `package-json/tests.rs`.

### 16. Shared git output helpers: about −70 ⛔ (c)(g)

- **Files:**
  - `crates/cli/src/agentfix.rs` `git_lines` (111–124), the same body as the `lines` closure in `crates/core/src/agentrun.rs` `RepoState::capture` (819–829);
  - `affected.rs` `names` (27–41);
  - the "could not run git" closure, twice in `git.rs` (167–176, 195–205);
  - the NUL-split parsing in `blame.rs:746/1802`, `tamper/mod.rs:655/715/733`, `stepcache.rs:238/1590` and `git.rs:347`;
  - "changed vs HEAD plus untracked", in `affected.rs:77–89` and `blame.rs:1805–1827`.
- **Removed or merged:** `git::lines`, `git::z_paths`, one `not_started(error)`, and one `changed_since_head`.
- **Could break:** some copies treat a non-zero exit as "nothing" and others as an error; keep each caller's choice. `RepoState` is the runner's outside-the-worktree check (guardrail 2), and the changed-file list decides what is affected.
- **Test:** `git.rs` tests, `affected.test.ts`, and `agent-runner.test.ts` (guardrail 2).

### 17. nextjs and expo `package-json` rules are near-copies: about −70 ⛔ (a)

- **Files:** `crates/rules/src/nextjs/package-json/mod.rs` (150), `expo/package-json/mod.rs` (164).
- **Removed or merged:** both have the same skeleton:
  - invalid JSON is a manual finding;
  - a missing framework dependency is an agent finding;
  - missing scripts are deterministic when `json_with` works;
  - the fix creates the file or merges scripts.

  One function over a spec (package, required dependencies, script filter, messages) replaces both.

- **Could break:** expo's workspace early return, and the dependency sets (react / react-dom for nextjs, react / react-native for expo). The merge must stay new-lines-only (`jsonedit`).
- **Test:** yes: both `package-json/tests.rs`, nextjs `a_workspace_root_with_the_app_in_a_subfolder_gets_nothing_written`, expo `a_workspace_listing_expo_at_its_root_gets_nothing_written`.

### 18. One suppression walk for the M32 readers: about −65

- **Files:** `tamper/java.rs:1142–1167`, `kotlin.rs:1231–1255`, `swift.rs:1103–1124`, `dart.rs:809–830`, `go.rs:1435–1458`, `csharp.rs:1166–1195`, `cfamily.rs:1813–1865`, `suppress.rs:35–117`.
- **Removed or merged:** each driver parses, walks the tree, stops at strings, maps a node kind to keys and pushes a `Suppression`. One `suppress::collect(source, grammar, classify)` replaces the walking, and each language keeps only its node-kind `match`.
- **Could break:** the text shown for a suppression. C# shows the attribute's parent line, and C walks the masked text.
- **Test:** yes: each reader's suppression tests and the 7 tests in `suppress.rs`; the corpus has a suppression case per language.

### 19. One "cut to 120 characters" helper and one `keyed`: about −57

- **Files:** `tamper/csharp.rs:1046–1069` and `tamper/go.rs:1325–1349` (private copies of `suppress.rs:121–164` `shown`/`keyed`), `imports.rs` `shorten` (292–303), `references.rs` `snippet` (94–102).
- **Removed or merged:** keep `suppress::shown`/`keyed`, with `keyed` taking `impl IntoIterator<Item = impl AsRef<str>>`, and use them everywhere.
- **Could break:** nothing; the behaviour is identical by construction.
- **Test:** yes: csharp and go `suppressions_are_read_…`, `look_alikes_are_not_suppressions`; `suppress.rs` `a_long_comment_is_shown_cut`; the imports and references unit tests.

### 20. Use one TOML reader in the supabase pack: about −50 ⛔ (a)

- **Files:** `crates/rules/src/supabase/mod.rs:211–289` (`config_value`, `strip_comment`, `string_list`), `crates/core/src/facts.rs` `strip_comment`, `parse_toml`, `array_strings`.
- **Removed or merged:** supabase's `strip_comment` is the same logic as facts', with the arms reordered. `config_value` is a lookup over what `parse_toml` returns. Keep `table_header` for line-level edits.
- **Could break:** `parse_toml` does not strip a BOM (bug 21) and does not unquote `[auth."email"]`. A missed `project_id` would make the `supabase/config` fix insert a second one into a person's `config.toml`.
- **Test:** yes: `supabase/config/tests.rs`, `supabase/local-smtp/tests.rs`, facts `toml_reader_handles_multiline_arrays_and_comments`. None covers a BOM or a quoted header.

### 21. One starter `package.json` writer and one `uses_typescript`: about −45 ⛔ (a)

- **Files:** `crates/rules/src/nextjs/mod.rs:89` (28 lines), `expo/mod.rs:141` (29), `supabase/mod.rs:134` (29); `uses_typescript` at `nextjs/mod.rs:74` and `expo/mod.rs:129`; the dev-dependency split in `supabase/package-json` `cli_dependency`.
- **Removed or merged:** a `stackkit::starter_package_json` and one split function.
- **Could break:** key order: `serde_json` keeps insertion order, and the starters are compared byte for byte. The writer is used only through `create`.
- **Test:** yes: nextjs `fixing_an_empty_folder_…`, expo and supabase `an_empty_folder_is_built_by_the_fixes…`.

### 22. Use `services::folders_up` instead of twelve hand-written walk-ups: about −45 ⛔ (b)(c)

- **Files:** `setup.rs` 74–85, 522–540, 945–981, 1171–1183, 1265–1273, 1310–1318; `pertest.rs` 1052–1065; `stepcache.rs` 776–786, 908–916; `doctor.rs` 1140–1148, 1474–1488, 1503–1515.
- **Removed or merged:** each loop becomes `folders_up(..)` with `any`, `find` or `enumerate`.
- **Could break:** `setup::start_services` needs the depth. `stepcache::installed` and `outside_service` feed the key and the never-cache decision for outside services.
- **Test:** yes: setup.rs `a_missing_module_of_a_src_layout_package_is_a_normal_failure`, services.rs `a_folder_sees_its_own_files_and_those_above`, `step-cache.test.ts` "never caches a step that needs a service the repo does not start".

### 23. One "affected" computation for `check --affected` and the stop hook: about −40 ⛔ (c)

- **Files:** `crates/core/src/affected.rs` (223) `owner` and `affected_projects` (94–136), `graph.rs` `affected` (1705–1838), `crates/cli/src/main.rs:1081`.
- **Removed or merged:** `narrow_report` uses the graph's affected set, and keeps any finding whose rule's inputs match a changed file. That fixes bug 1.
- **Could break:** `check --affected` must build the graph, so it gets slower. Findings on root files stay in, which is the point.
- **Test:** partly: affected.rs `dependents_of_a_changed_project_are_affected_transitively`, `affected.test.ts`. No test has a finding on an unchanged root file that the change caused.

### 24. One project-type table that packs, facts and cache inputs all read: about −40 ⛔ (c)

- **Files:**
  - `crates/rules/src/stackkit.rs` `MANIFESTS` (28) and `CODE` (29);
  - `supabase/package-json` `OTHER_STACKS` (7);
  - `crates/core/src/ecosystems.rs` `MARKER_GLOBS` (22) and each entry's `marks`;
  - `cmake/mod.rs` `VENDORED`, `OTHER_C_BUILDS`, `C_CODE`, `CXX_CODE`, `OTHER_NATIVE`;
  - `facts.rs` `DISCOVERY_SKIPPED_DIRS` (32);
  - `stackkit::depends_on` (reads `peerDependencies`) against `facts::has_node_dependency` (does not).
- **Removed or merged:** see "Duplication across the milestones of 14 November", part C.
- **Could break:** where packs write starters. The lists differ today: `.cppm`/`.ixx` are C++ only to cmake, and `thirdparty` is vendored only to cmake. The inputs feed the rule-result cache.
- **Test:** every pack's "another stack" tests, `ecosystems/tests.rs`, and the core gitignore tests.

### 25. Dead code in several places: about −37

- **Files and what goes:**
  - `verify.rs` `run_verify` (2761–2770): only tests call it.
  - `graph.rs` `project_of` (1957–1964): nothing calls it.
  - `tamper/go.rs` `holds_tests` (1316–1324): always false; fold it into the `mod.rs` match arm.
  - `runs.rs` `State.root`: written, never read.
  - `agentrun.rs:667` `LEDGER`: unused; the path is spelled out again at `main.rs:1290/1301`.
- **Could break:** nothing (whole-tree grep, scripts and docs included).
- **Test:** the unit tests that use them compile against the new form.

### 26. One `inputs()` helper for the stack packs: about −36 ⛔ (c)

- **Files:** `crates/rules/src/{go,maven,expo,supabase,cmake}/mod.rs` (12/12/12/9/36 lines). nextjs has none.
- **Removed or merged:** five copies of "own inputs plus shared, deduplicated" become `stackkit::inputs(own, shared)`. cmake's five extra manifests are already in `stackkit::MANIFESTS`.
- **Could break:** a dropped glob makes a cached check result go stale. Bugs 4, 12 and 13 are exactly that.
- **Test:** yes: `every_rule_reads_what_decides_where_the_pack_applies` in expo, supabase, go, maven and cmake. nextjs has none.

### 27. One benchmark statistics module: about −35

- **Files:** `bench/startup.mjs` (48), `launcher-bench.mjs` (87), `check-bench.mjs` (119), `verify-bench.mjs` (102), `blame-bench.mjs` (192).
- **Removed or merged:** the warm-up, sort and percentile code (×5) and the Markdown table printer (×2) become `bench/stats.mjs`.
- **Could break:** the numbers in `bench/RESULTS.md` stay comparable only if the rounding is kept.
- **Test:** none.

### 28. One `split_path` for `is_test_path`: about −35

- **Files:** `tamper/java.rs:1006–1011` (`split_path`), and inline copies at `swift.rs:1022`, `dart.rs:753`, `csharp.rs:1008`, `rust.rs:558` and `cfamily.rs:1600`.
- **Removed or merged:** one `split_path` next to `Language`.
- **Could break:** which files count as tests: an empty stem (`.swift`), and extension case (cfamily lowercases). The base-read `testPaths` policy is separate and untouched.
- **Test:** yes: each reader's `test_paths` test.

### 29. CI: run the macOS/Windows job from the same steps as Linux: about −33

- **Files:** `.github/workflows/ci.yml` (203). Lines 89–122 copy lines 39–74, under a comment saying "Keep the two lists identical".
- **Removed or merged:** a YAML anchor, or one job with a matrix over the OS and Node versions. Check GitHub's current docs on anchors first, as AGENTS.md requires.
- **Could break:** a mistake could quietly stop macOS and Windows tests on pull requests.
- **Test:** none. No test reads `ci.yml`; `release.test.ts` pins actions only in `release.yml`.

### 30. Let supabase use the shared walker: about −30

- **Files:** `crates/rules/src/supabase/mod.rs` `elsewhere` (35 lines, its own `read_dir`, depth 3), against the `walk::walk_files` filters in go, maven and cmake.
- **Removed or merged:** an 8-line `walk_files` filter.
- **Could break:** supabase would start honouring `.gitignore` and lose its depth cap, so a gitignored `tmp/a/supabase/config.toml` stops counting.
- **Test:** yes: supabase `a_workspace_with_supabase_in_a_subfolder_gets_nothing_written_at_the_root`.

### 31. `SetupProblem { reason, ..Default::default() }`: about −30 ⛔ (b)

- **Files:** `crates/core/src/setup.rs` (2,701) at 161, 1386, 1402, 1423 and 1464; `verify.rs:1999`.
- **Removed or merged:** six 7-line literals that set only `reason`.
- **Could break:** nothing (it derives `Default`). It is in the setup-versus-failure code, so it is marked.
- **Test:** yes: setup.rs `a_missing_toolchain_is_setup_not_a_failure`, `a_command_that_is_not_found_is_a_setup_problem`.

### 32. One shell builder and one quiet `Watch`: about −30 ⛔ (b)

- **Files:** `doctor.rs` 194–212 and 147–156; `verify.rs` 1450–1485 (`shell`, `windows_wrapper`) and 2177–2186; `agentrun.rs:469`.
- **Removed or merged:** `process::shell(command)` and `Watch::quiet(..)`.
- **Could break:** doctor would get `windows_wrapper`, which only rewrites commands starting `./`.
- **Test:** yes: `wrapper_tests::a_wrapper_in_the_folder_runs_with_cmd`, `verify.test.ts` "runs a command with quoted arguments as written, on every platform", `doctor.test.ts` "never lets a version command download anything".

### 33. Read each folder once during discovery: about −30

- **Files:** `facts.rs` `file_names` (494–511) and `list_subdirs` (386–414), against `ecosystems.rs` `Folder::read` (28–53). `discovered_projects` reads every folder twice.
- **Removed or merged:** use `Folder::read` everywhere.
- **Could break:** `file_names` follows symlinked files and `Folder::read` does not, so a symlinked `package.json` would stop marking a project unless that is kept.
- **Test:** facts discovery tests, `ecosystems/tests.rs`. None covers a symlinked manifest.

### 34. Shared helpers for the tamper bench scripts: about −26

- **Files:** `bench/tamper-rates.mjs:30–57`, `bench/tamper-misses.mjs:28–55` (identical `git` and `put`), and an async `put` in `packages/cli/src/tamper.test.ts:59–71`.
- **Could break:** the bench runs only.
- **Test:** none for the scripts.

### 35. `run_again` and `run_base_step` as one function: about −25 ⛔ (c)

- **Files:** `verify.rs` 2579–2608 and 2612–2641.
- **Removed or merged:** one function taking `base: Option<&Path>`.
- **Could break:** `run_again` records the command in the run record and `run_base_step` does not; decide which is wanted. Both must keep `cache: None`: a rerun's pass and a base result must never reach the step cache.
- **Test:** yes: `stop-hook-blame.test.ts` "labels a new failure that passes when run again flaky…", `blame.test.ts` "passes, labelled pre-existing, when the test already fails on the base commit".

### 36. One way to add the stop hook, the one that keeps a person's lines: about −25 ⛔ (a)

- **Files:** `crates/core/src/hooks.rs` `merge_hook`/`install_hook` (140–207) re-serialise the whole file; `crates/rules/src/agent-ready/verify-hook/mod.rs` `with_hook` (72–95) edits only the lines it adds (ADR 0027).
- **Removed or merged:** `install_hook` uses `with_hook`, and says what to add by hand when no line-preserving edit is possible. This fixes bug 6.
- **Could break:** `fscale hooks install` would refuse minified files instead of rewriting them.
- **Test:** partly: `hooks.test.ts` "installs for one agent, keeps existing settings…" and hooks.rs `existing_settings_and_hooks_are_kept` compare values, not lines, so neither catches the rewrite today.

### 37. One "the project is only in a subfolder" finding: about −25

- **Files:** `crates/rules/src/go/go-mod/mod.rs:61–78`, `maven/pom/mod.rs:154–172`, `cmake/mod.rs` `only_below` with `cmake/lists/mod.rs`.
- **Removed or merged:** `stackkit::only_below(ctx, manifest, own_code, below)`. This fixes bug 15.
- **Could break:** cmake also excludes other C builds; keep that.
- **Test:** yes: go `modules_in_subfolders_get_one_finding_and_nothing_written`, maven `a_pom_below_another_stack_is_that_stacks_business`, cmake `a_project_in_a_subfolder_is_named_once_and_nothing_is_written`.

### 38. `go/tests` and `maven/tests` rules as one helper: about −25

- **Files:** `crates/rules/src/go/tests/mod.rs` (59), `maven/tests/mod.rs` (59); a normalised diff is 28 lines.
- **Could break:** the messages.
- **Test:** yes: both `tests/tests.rs`.

### 39. One secret redactor: about −24 ⛔ (g)

- **Files:** `crates/core/src/activity.rs` `redact` (99–122), against `verify.rs` `hide_secrets`/`hide_user` (1248–1325).
- **Removed or merged:** delete `redact` and call `hide_secrets` from the heartbeat. This fixes bug 3.
- **Could break:** heartbeat text for lines holding `password=` or `user "x"`, which is the intent.
- **Test:** yes: activity.rs `a_connection_string_in_the_output_is_shown_without_its_secret`; verify.rs `secrets_are_hidden_in_every_form_the_review_found`, `a_failed_login_s_user_name_is_never_shown`.

### 40. One list of local hosts and Compose files: about −22 ⛔ (b)(c)

- **Files:** `setup.rs` 209 and 242–247 (identical to `services.rs` 107–113), `setup.rs` 226–231 and `stepcache.rs` 897–902 (the same names as `services.rs` 31–36). `graph.rs:1392` is a fourth, wider rule.
- **Could break:** nothing today. `stepcache::outside_service` decides whether a step is never cached.
- **Test:** yes: setup.rs `node_econnrefused_is_caught_on_localhost_only`; `step-cache.test.ts` "caches a step whose service the repo starts with Compose", "never caches a step that needs a service the repo does not start".

### 41. Derive "the files facts reads" from the readers: about −20 ⛔ (c)

- **Files:** `crates/rules/src/agent-ready/managed.rs` `FACTS_INPUTS` (18–47), `ecosystems.rs` `MARKER_GLOBS` (157–180), and the hand list in `core/gitignore-basics/mod.rs` (about 110–130).
- **Removed or merged:** one constant exported from core, plus an `inputs` field per ecosystem entry. This fixes bug 10.
- **Could break:** wider inputs mean more cache misses for agents-md, agent-permissions and gitignore-basics. That costs speed only.
- **Test:** none checks that inputs cover what facts reads.

### 42. One requirements-file finder: about −20 ⛔ (c)

- **Files:** `graph.rs` `requirement_files` (1183–1205), `facts.rs` `python_dependencies` (1221–1233).
- **Could break:** the copies differ only in case folding. Graph edges feed what is affected.
- **Test:** yes: graph.rs `a_requirements_list_in_a_requirements_folder_can_affect_builds`; facts python tests.

### 43. Two small copies in the engine: about −20

- **Files:**
  - `explain.rs:71–80` re-reads the SKILL.md instead of calling `rule_skill`.
  - `fix_hint` and `plural` are copied from `report.rs:77–87` into `crates/cli/src/scaffold.rs:302–308` and `main.rs:347–349`.
- **Could break:** nothing; the texts are identical.
- **Test:** yes: `explain.test.ts`, `check.test.ts`, `scaffold.test.ts` output assertions.

### 44. One service name-and-port table: about −17 ⛔ (b)

- **Files:** `verify.rs` `usual_port` (1328–1339) and 1404–1410, against `services.rs` `service_for` (132–163).
- **Could break:** nothing for the five service kinds verify knows.
- **Test:** yes: verify.rs `a_service_is_named_by_its_address_its_name_or_its_client`; `verify-run.test.ts` "makes passes plus errors that all name an unreachable service setup needed".

### 45. Small TypeScript and script leftovers: about −16

- **Files:**
  - `scripts/pack-npm.mjs` `workspaceVersion()` (59–65) re-reads Cargo.toml's version; `bump-version --check` already keeps the two equal.
  - `packages/sdk/src/types.ts:152` `RULE_STATUSES` is dead.
  - `createContext`, `FixMode`, `toPosix`, `normalizeRelative` and `SKILL_*` are exported but used nowhere else.
- **Could break:** the published `@firescale/sdk` API, a 0.x change.
- **Test:** `pack.test.ts` "puts a working binary in the platform package" compares versions.

### 46. Remove the unused dry-run path of a TypeScript fix: about −15 ⛔ (a)

- **Files:** `packages/sdk/src/context.ts` (22, 64–75), `packages/sdk/src/run.ts` (76, 85), `packages/cli/src/host.ts:216`, and the `dry_run` parameter of `crates/core/src/node.rs` `fix_rule`. The binary always sends `false` (`main.rs:529/533`); dry runs use a scratch copy instead.
- **Removed or merged:** the TypeScript dry-run mode and the always-false Node parameter. Keep `RuleContext`'s record-only mode: ADR 0018 keeps it for tests.
- **Could break:** `fixRuleWithChanges` is exported by the published `@firescale/sdk` 0.4.0, so this is a 0.x API change. Its dry run is also wrong (bug 24).
- **Test:** none for the dry-run path; the real path is covered by `fix-loop.test.ts` and `bridge.test.ts`.

### 47. Dead features in the TypeScript generators: about −15

- **Files:** `packages/cli/src/generators/validate.ts:37–41`, `types.ts:17, 40–41`, `index.ts:11–12, 41`; `custom-rule.ts:104–107` and `workspace-package.ts:65–68` (the same folder pattern); `custom-rule.ts:76–77` (copies the SDK's `RULE_ID`).
- **Removed or merged:** the `boolean` input type, which no generator uses; `GeneratorContext.root`, which no generator reads; two unused re-exports; and one folder pattern and one rule-id pattern instead of two each.
- **Could break:** nothing (whole-tree grep, Rust included). Refusing to overwrite stays in Rust (`create_new`). `fscale schema` output is unchanged.
- **Test:** yes: `generators.test.ts` "rejects %s input %j with an error naming the field" and the schema snapshots.

### 48. One "test in backend" place text: about −15

- **Files:** `verify.rs` 3074–3079, 1915–1918, 2659–2662; `blame.rs` 870–876; `stepcache.rs` 1395–1398.
- **Could break:** the `FSCALE_LOG=cache` line for a root project changes from "in ." to "in the repo root".
- **Test:** `blame.test.ts` note texts; `step-cache.test.ts` "says on every run, without FSCALE_LOG, why a passed step is not cached".

### 49. blame: one status mapping and one flaky marker: about −15 ⛔ (c)

- **Files:** `blame.rs` 1521–1537 and 1739–1746 (StepStatus → BaseStatus); 1972–1979 and 2015–2021 (mark flaky).
- **Could break:** the base run also turns pnpm's refused link into `SetupNeeded` and uses a 50 ms deadline margin; keep those for the base run only. `fixed` comes only through `confirm_fixed`, which uses this mapping.
- **Test:** yes: `blame.test.ts` "labels the step fixed when it passes in a worktree of the head…", "falls back to blocking when the base run runs out of its budget".

### 50. One "read JSON that may start with a BOM": about −15 ⛔ (c)

- **Files:** `doctor.rs` 399–408, `setup.rs` 959–966 and 1149–1156, `verify.rs` 438–443, `hooks.rs:213`, `pertest.rs` 1084–1086 (which does not strip the BOM: bug 9).
- **Could break:** nothing. `pertest::script_text` decides fail-fast detection, which keeps a bail-stopped failure from being labelled `pre-existing`.
- **Test:** none has a BOM in `package.json` for `script_text`.

### 51. One set of dependency-section matchers: about −15 ⛔ (c)

- **Files:** `facts.rs` `pyproject_dependencies` (1189–1218) and `cargo_deps` (1759–1778), against `references.rs` `is_requirement_list`, `is_poetry_dependencies`, `is_cargo_dependencies` (519–618).
- **Could break:** facts' `dependsOn` gains target-specific dependencies (bug 19), a correct widening of the graph.
- **Test:** facts `ruff_in_pyproject_dependency_groups_counts`; references tests. None for `[target.*.dependencies]` in facts.

### 52. One file-walker setup: about −15 ⛔ (a)(c)

- **Files:** `walk.rs` `walk_entries` (48–101), `facts.rs` `searchable_dirs` (1831–1876), `graph.rs` `walk` (1419–1478).
- **Removed or merged:** one builder for "repo files as facts sees them", and one `is_skipped_dir`. This fixes bugs 22 and 23.
- **Could break:** the rule-cache snapshot and the dry-run scratch copy would include files ignored only by global excludes or `info/exclude`. The cache gets more inputs, which is safe, and the copy gets bigger.
- **Test:** yes: walk.rs `lists_files_with_forward_slashes_and_skips_ignored_ones`, scratch.rs tests, `cache.test.ts`.

### 53. Config detectors: one pair of path helpers and one workflow rule: about −15

- **Files:** `tamper/config.rs:190–198`, `coverage.rs:60–65`, `noverify.rs:29–31`, `lintconfig.rs:63–69`; workflow detection in `coverage.rs:100–104` (any depth) against `ci.rs:24–29` (root only).
- **Could break:** coverage stops reading nested workflows, which GitHub never runs (bug 28).
- **Test:** coverage.rs's reads-paths test; `command_lines_in_scripts_workflows_and_makefiles`.

### 54. Small near-copies in verify planning and the test-result parsers: about −15

- **Files:** `verify.rs` 468–491 (`only_on` checked twice), 501 and 746 (the clippy note twice); `pertest.rs` 680–689 and 813–822 (the "N passed" summary parsed twice).
- **Test:** yes: verify.rs `a_rust_only_repo_uses_cargo_and_config_overrides_win`; pertest `each_runner_gives_each_failing_test_its_reason`.

### 55. Read CMake's languages with the CMake reader: about −15

- **Files:** `ecosystems.rs` `cmake_languages` (834, text search) and `project_call`, against `crates/core/src/cmake.rs` `commands`/`project_languages`.
- **Could break:** `project_languages` defaults to C and C++ where ecosystems scans sources; keep the source fallback (it decides Swift against CMake). This fixes bug 29.
- **Test:** `ecosystems/tests.rs` (swift-argument-parser, nlohmann/json, swift-crypto layouts).

### 56. Build "Available rules: …" once: about −14

- **Files:** `explain.rs:24–32`, `select.rs:63–71`, `config.rs` `sorted_ids` (120–127).
- **Could break:** nothing; the rules are already sorted.
- **Test:** yes: `explain.test.ts` "lists the available rules when the rule is unknown"; select.rs tests.

### 57. Use `stackkit::MANIFESTS` in supabase instead of its own 7 names: about −14

- **Files:** `crates/rules/src/supabase/package-json/mod.rs:35–49` (and the same names in its `inputs`).
- **Could break:** more repos get the manual "not a JavaScript project" finding instead of a written `package.json`, which is the intent. This fixes bug 11. It decides whether a missing file is created and never overwrites one, so it is not marked.
- **Test:** yes: supabase `a_supabase_project_in_another_stack_gets_no_package_json`.

### 58. One list of finding kinds: about −13 ⛔ (d)

- **Files:** `tamper/policy.rs:70–82` (`KINDS`) and `:216` (the error message lists all 11 by hand), `tamper/mod.rs:892–904`, `bench/tamper-rates.mjs:58–70`.
- **Removed or merged:** `mod.rs` and the message read `policy::KINDS`.
- **Could break:** this table decides which `allow` entries the base commit's policy accepts. Today a new `Kind` added without a policy name cannot be allowed; the merge prevents that drift.
- **Test:** yes: policy.rs `mistakes_say_what_is_allowed`, `the_base_value_never_fails_the_run`; `tamper.test.ts` "allows only the kinds an allow entry names".

### 59. Read the agent limits once, from the parsed config: about −12 ⛔ (g)

- **Files:** `crates/cli/src/agentfix.rs` `limits` (90–109) re-reads `firescale.config.json`; `config.rs` `check_agent` (684–715) validates and discards.
- **Removed or merged:** `check_agent` returns the limits in `ResolvedConfig`. This fixes bug 7.
- **Could break:** the clamps differ (agentfix caps turns at 1000 and time at 86,400 s; config accepts any value ≥ 1). Pick one.
- **Test:** `agent-runner.test.ts` "guardrail 5, budget limits…" (JSON config only). Nothing covers a `.ts` config.

### 60. One `write_atomic`: about −12 ⛔ (c)

- **Files:** `stepcache.rs` 1057–1067, `blame.rs` 435–439, `sharedcache.rs` 293–298, `runs.rs` 186–193.
- **Could break:** `sharedcache::prune` and `stepcache::clean_with` recognise the temp name `.{key}.{pid}.tmp`; the helper must keep it.
- **Test:** yes: `verify-run.test.ts` "stores each pass as its step finishes…", sharedcache `pruning_keeps_only_usable_records_and_only_on_the_default_branch`.

### 61. Take the step cache's version probes from the ecosystems table: about −12 ⛔ (c)

- **Files:** `stepcache.rs` `probes` (605–617) lists mvn, gradle, swift, cmake, meson, dotnet, dart, flutter, xcodebuild and go by hand. The `ecosystems.rs` header says the step cache reads the table, but it does not.
- **Could break:** a new ecosystem's tool would not be probed, so an upgrade would not change the key. That gap exists today.
- **Test:** stepcache `a_repo_script_is_never_run_for_its_version`. None checks that every ecosystem tool is probed.

### 62. pnpm `minimumReleaseAgeExclude` may be dead: about −12 (low confidence)

- **Files:** `pnpm-workspace.yaml` (11 entries for typescript-eslint 8.72.2, published 2026-11-09).
- **Could break:** if pnpm 12's default minimum age is longer than five days, installs would refuse. Check pnpm's docs first.
- **Test:** none.

### 63. The `packages/generators` placeholder: about −11

- **Files:** `packages/generators/package.json` (8), `README.md` (3); `release.test.ts:93` expects the manifest.
- **Could break:** SPEC §9's layout. ADR 0028 keeps it on purpose, so removing it needs a new ADR.
- **Test:** `release.test.ts` "sets the version everywhere it is recorded…" (update its expectation, do not weaken it).

### 64. One `now_secs`: about −10 ⛔ (c)

- **Files:** `stepcache.rs` 1045–1050, `sharedcache.rs` 184–188, `runs.rs` 62–66.
- **Could break:** nothing. It drives the shared cache's 30-day age check.
- **Test:** yes: sharedcache `a_record_written_from_another_ref_or_too_old_is_not_used`.

### 65. One test-runner detector: about −10 ⛔ (b)

- **Files:** `activity.rs` `Runner::from_command` (35–49; "pytest" anywhere in the command), `pertest.rs` `TestRunner::of` (27–55).
- **Could break:** the live counts and the no-pass early stop. This fixes bug 25.
- **Test:** yes: activity `runners_are_known_by_their_command`, pertest `runners_are_found_in_commands_and_scripts`, verify `only_a_plain_pytest_command_gets_maxfail`.

### 66. Parse a step's output once: about −10 ⛔ (b)

- **Files:** `verify.rs` `final_tally` (1166–1172) and `run_step`'s `text()` (2081–2088, 2121, 2341, 2344); `process.rs` `Finished` (77–89).
- **Removed or merged:** carry the live tally in `Finished`, and build the text once. Today a failing step makes up to about seven copies of up to 32 MB.
- **Could break:** counts become the live counts, which the heartbeat already shows (bug 26). The tally feeds `erred` and `failed`.
- **Test:** `verify-run.test.ts` "gives a running count of tests in the heartbeat…".

### 67. Dead parts of `test-helpers.ts`: about −10

- **Files:** `packages/cli/src/test-helpers.ts`: `RunOptions.bin`/`fullEnv` (unused, or use them for item 6), `onlyEnvRule`'s `extra`, an orphan doc block, an unneeded `export`.
- **Test:** the tests compile against it.

### 68. Move the Maven XML reader to core and use it in the ecosystems table: about −10

- **Files:** `crates/rules/src/maven/xml.rs` (257), `ecosystems.rs` `maven_name` (353) and lint detection (374–388).
- **Removed or merged:** the substring search goes; this fixes bug 16. It decides whether verify has a lint step at all, not setup versus failure, so it is not marked.
- **Could break:** lint detection for plugins only under `<pluginManagement>`; decide that explicitly.
- **Test:** `ecosystems/tests.rs` (Spotless fixture). None has a commented-out plugin.

### 69. Use one Node "exec" prefix: about −8 ⛔ (b)

- **Files:** `verify.rs` `exec_prefix` (427–434, `npx`), `facts.rs` `node_exec` (542–549, `npx --no`).
- **Removed or merged:** delete `exec_prefix`. This fixes bug 2.
- **Could break:** verify.rs `nx_and_turbo_take_precedence_and_honour_affected` asserts `npx turbo run lint`. It pins the wrong behaviour, so update it and say why in the PR.
- **Test:** that test; setup.rs already maps npx's refusal to setup needed (setup.rs:1422).

### 70. Use one list of root paths for failure signatures: about −8 ⛔ (c)

- **Files:** `verify.rs` `failure_signature` (2528–2537), against `pertest.rs` `roots_of` (158–184) plus `head_root`.
- **Could break:** today Windows `\\?\` paths, Python's doubled backslashes and the head's linked `node_modules` are not normalised, so they fail toward _not_ `pre-existing`, which is safe. Merging normalises more, so more failures can match `pre-existing`. That is the trust rule ADR 0037 says every review found a way around. Do this only with new blame tests.
- **Test:** `blame.test.ts` "does not hide a new failure in a step that also failed on the base another way".

### 71. Use the shared `-r` parser in `python_interpreter`: about −8 ⛔ (b)

- **Files:** `facts.rs` `includes_base` (1407–1420), against `requirement_include` (1113) and `resolve_include`.
- **Could break:** the install command shown with setup-needed for `-r ./requirements.txt`.
- **Test:** facts `a_venv_in_the_project_folder_runs_its_steps`.

### 72. One `.firescale/.gitignore` writer, with the symlink check: about −7 ⛔ (a)

- **Files:** `stepcache.rs` `ignore_firescale_folder` (1072–1080, no symlink check), `hooks.rs` `write_blocks` (304–327, with it).
- **Removed or merged:** this fixes bug 17.
- **Test:** hooks.rs `the_counter_is_never_written_through_a_symlink`. None for the step cache's copy.

### 73. One table of lockfile-and-install-marker pairs: about −6 ⛔ (c)

- **Files:** `stepcache.rs` 749–755 (part of the key), `doctor.rs` 1496–1502.
- **Could break:** nothing today. The risk is drift: a package manager added only to doctor's list would leave its installs out of the key, and a stale pass would be served.
- **Test:** none checks that an install changes the key.

### 74. One list of `package.json` dependency fields: about −6 ⛔ (c)

- **Files:** `references.rs` `PACKAGE_FIELDS` (644–649), `facts.rs` `node_deps` (1669–1682).
- **Test:** facts and references unit tests.

### 75. Ask the graph which files are inert, in the step cache too: about −5 ⛔ (c)

- **Files:** `stepcache.rs` `covers` (486–496, `readme_like` + `is_document_or_media`), `graph.rs` `cannot_affect_builds` (1548–1568).
- **Could break:** if the graph ever stopped treating root `.md` files as inert, `--affected` would widen while the key still left them out. The key would then be narrower than `graph::affected`, which breaks the rule.
- **Test:** none for root `.md` files in a key.

### 76. Read GitHub's event once in `fscale tamper`: about −5 ⛔ (d)

- **Files:** `crates/cli/src/main.rs:1341` and `:1402`.
- **Could break:** "blocks until approved" in a `pull_request_target` run, the approval path of ADR 0046. Today it reads the same file twice, so behaviour is unchanged.
- **Test:** `tamper-approvals.test.ts` "blocks in the pull_request_target run when no reviews were read at all".

### 77. Split `verify.rs` by job: about 0 (a move) ⛔ (b)(c)

- **Files:** `crates/core/src/verify.rs`: 6,329 lines, about 4,020 of code. It mixes seven jobs:

  | Job                   | Lines                   |
  | --------------------- | ----------------------- |
  | Config and planning   | 60–818                  |
  | Report types          | 820–1128                |
  | Output-text helpers   | 1130–1325               |
  | Service-down evidence | 1327–1449 and 1528–1824 |
  | Running               | 1450–1527 and 1826–3070 |
  | Human text            | 3071–3858               |
  | Tamper text           | 3859–4013               |

  `run_step` alone is 480 lines (1906–2388), and its ending classification (2224–2387) is its own job.

- **Removed or merged:** nothing; the modules move. The service-down evidence goes beside `setup.rs`/`services.rs`, and the tamper text goes into `tamper/`.
- **Could break:** the public paths `verify::{format_tamper_human, tamper_annotations, strip_ansi}` (`main.rs:1421/1425`, `activity.rs`, `pertest.rs`). Setup-versus-failure and cache code move unchanged.
- **Test:** the verify.rs tests move with the code; `verify.test.ts`, `verify-run.test.ts`.

### 78. Move fix planning and `add`'s writing out of `main.rs`: about 0 (a move) ⛔ (a)

- **Files:** `crates/cli/src/main.rs` (1,804): `render_fixes`, `plan_fixes`, `FixPlan::run` and `loop_failure` (351–564, used by `scaffold.rs`), and `add`'s "create new files or nothing" (733–852).
- **Removed or merged:** move into `crates/core` next to `fixloop.rs` and `RuleContext::create_tracked`, so Rust tests can cover them.
- **Could break:** nothing if moved verbatim. This is the never-overwrite logic of `fscale add`.
- **Test:** `fix.test.ts`, `fix-loop.test.ts`, `scaffold.test.ts`, `generators.test.ts` ("would write over"). No Rust test.

### 79. Do not ship `course-fixture.ts` in the npm package: about 0 (+1 line)

- **Files:** `packages/cli/tsconfig.build.json`, `scripts/pack-npm.mjs:123`, `packages/cli/src/course-fixture.ts` (148).
- **Removed or merged:** exclude it from the build. This fixes bug 20.
- **Test:** none. `pack.test.ts` checks that files are present, never that a file is absent.

## Bugs found during the audit

Ordered by harm. **Checked** means it was confirmed by reading the code by hand after the reader reported it; the others were found by reading and not reproduced. A ⛔ letter means the bug sits in a safety behaviour.

1. **The stop hook can let an agent stop with a finding its change caused** ⛔ (c) — **checked**.
   - Where: `affected.rs:139–149` keeps a finding only if its own file changed or its owner project is affected; the hook turns `--affected` on at `main.rs:1041–1046`.
   - Case: an agent adds a `lint` script to `web/package.json`. `agents-md` now reports `AGENTS.md`, but `AGENTS.md` did not change and belongs to no affected project, so the finding is dropped. The agent stops with a stale AGENTS.md.
   - Fix: item 23.
2. **verify can download nx or turbo from the registry instead of saying SETUP NEEDED** ⛔ (b) — **checked**.
   - Where: `verify.rs:427–434` uses bare `npx`.
   - Case: a fresh clone with `nx.json` and no `node_modules`. verify runs `npx nx run-many …` with `CI=1` and stdin closed, so npx assumes yes and fetches an unpinned nx.
   - Fix: item 69 (`npx --no`, as facts already does).
3. **Heartbeat lines can show passwords and user names** ⛔ (g) — **checked**.
   - Where: `activity.rs:99–122` hides only `scheme://user:pass@` and stops at the first `/`.
   - Case: a step prints `password=hunter2`, `password authentication failed for user "app"`, or `postgres://app:pa/ss@…`, then goes quiet. The progress line shows it, against ADR 0035.
   - Fix: item 39.
4. **nextjs rules keep stale cached results** ⛔ (c) — **checked**.
   - Where: `nextjs/gitignore`, `next-config` and `app-router` list inputs without `package.json`, but `applies()` reads it.
   - Case: add `next` to an existing package.json. The three rules' cached "no finding" stays. Supabase was fixed for exactly this in M29; nextjs never got the same fix or test.
   - Fix: item 26.
5. **A Kotlin change can weaken a test unseen: `assertThat(x)` with nothing chained still counts** — **checked**.
   - Where: `tamper/kotlin.rs:281–320` has no rule for a lone `assertThat`; `java.rs:507–532` does.
   - Case: `assertThat(total()).isEqualTo(3)` becomes `assertThat(total())`. Java reports it; Kotlin does not.
   - Fix: item 9.
6. **`fscale hooks install` rewrites a person's whole settings file** ⛔ (a) — **checked**.
   - Where: `hooks.rs:181–207` re-serialises the JSON.
   - Case: a `.claude/settings.json` indented with 4 spaces, or holding `1e3`, gets every line rewritten. The agent-ready fix for the same hook keeps the person's lines.
   - Fix: item 36.
7. **Agent budget limits in `firescale.config.ts` are ignored** ⛔ (g) — **checked**.
   - Where: `agentfix.rs:90–109` reads only `firescale.config.json`.
   - Case: a `.ts` config sets `maxCostUsd: 0.5`; the run gets $2 and 900 s.
   - Fix: item 59.
8. **A mistyped `protectedPaths` glob protects nothing, silently** ⛔ (g) — **checked**.
   - Where: `agentrun.rs:541–556` drops globs that fail to compile, and `config.rs:290–297` does not check them.
   - Related: diagnostic file paths are compiled as globs too (`agentfix.rs:266`), so a finding on `app/[id]/page.tsx` allows `app/i/page.tsx` and refuses the real file. Fix with `globset::escape` and a config error.
9. **A BOM in `package.json` can let a `--bail` failure be labelled pre-existing** ⛔ (c) — **checked**.
   - Where: `pertest.rs:1084–1086` parses without stripping the BOM.
   - Case: `"test": "jest --bail"`. The runner is not found, the fail-fast mark is never added, and `blame.rs:1239–1241` can call the stopped failure `pre-existing`, which ADR 0039 forbids.
   - Fix: item 50.
10. **agents-md and agent-permissions can serve stale cached passes** ⛔ (c).
    - Where: their inputs (`managed.rs:18–47`) miss files facts reads: `mvnw`, `gradlew`, `.oxlintrc.json`, `biome.json`, `.eslintrc*`, `vitest.workspace.*`, nested lockfiles, `**/.gitignore`, and `conftest.py`/`manage.py`.
    - Related: `agent-skills` lists only `firescale.config.json` though it reads `.ts` too.
    - Case: add `mvnw` to a Maven project; the commands change and the key does not.
    - Fix: item 41.
11. **The supabase pack writes a `package.json` into Python, Java and Ruby repos** ⛔ (a) — **checked**.
    - Where: `supabase/package-json/mod.rs:35–49` knows only 7 other-stack manifests.
    - Case: a repo with `Pipfile`, `setup.py`, `pom.xml`, `build.gradle.kts` or `Gemfile` plus `supabase/config.toml`: `fscale sync` creates a root `package.json`.
    - Fix: item 57.
12. **Maven entry and tests rules miss Groovy sources in their cache inputs** ⛔ (c). `maven/entry/mod.rs:80` and `maven/tests/mod.rs:27` list `src/**/*.java`, while `JVM_CODE` counts `.groovy`. Adding a Spock test leaves "no test" cached.
13. **The cmake pack's cache inputs miss `binding.gyp`** ⛔ (c). `cmake/mod.rs:260–296`. Adding `binding.gyp` should silence the pack (`ecosystems.rs:585–603`), but cached findings stay.
14. **The release build restores caches before building what gets published** — **checked**. `release.yml` `build` job (116–130): `cache: pnpm` and `Swatinem/rust-cache`. The publish job says a release never restores files from an earlier run. Remove both caches from `build` to close a cache-poisoning path. No test asserts it.
15. **A Kotlin, Scala or Groovy Maven project only in a subfolder gets no finding at all.** `maven/pom/mod.rs:158` passes `["java"]` where `applies` uses `JVM_CODE`. Fix: item 37.
16. **facts reads commented-out Maven plugins as real.** `ecosystems.rs:374–388` searches `pom.xml` text. A commented-out Checkstyle plugin makes verify run Checkstyle with default rules and fail, where it should say "no linter was found". `maven_name` can pick a commented-out `<artifactId>`. Fix: item 68.
17. **A step-cache write can follow a symlink out of the repo** ⛔ (a). `stepcache.rs:1072–1080`. A committed `.firescale/.gitignore` that is a dangling symlink makes `fs::write` create the file outside the repo; `hooks.rs` guards this case and the step cache does not. Fix: item 72.
18. **Kotlin does not read `throw TestAbortedException(…)` or TestNG's `SkipException` as a skip.** `kotlin.rs:225–232` reads only calls. Java does. Fix: item 9.
19. **facts misses `[target.'cfg(..)'.dependencies]` in Cargo.toml** ⛔ (c). `facts.rs:1764–1776`. The graph loses the edge, and `check --affected` drops findings when the dependency changes. Fix: item 51.
20. **Test-only code ships in the `firescale` npm package** — **checked**: `dist/course-fixture.js` and `.d.ts` exist after `pnpm build`. Fix: item 79.
21. **facts' `read_text` does not strip a BOM** (`facts.rs:181–186`). A BOM'd `pyproject.toml` loses its first `[project]` header, and the project is not found.
22. **The graph skips vendored folders case-sensitively, while facts ignores case** (`graph.rs:1440` against `facts.rs:1846`). `Vendor/` gets parsed. This errs wider, so it is safe but slow. Fix: item 52.
23. **The rule-cache snapshot honours `.git/info/exclude` and global excludes; facts does not** ⛔ (c). A project in an excluded folder is in facts but not in the snapshot, so editing its `package.json` gives a stale hit. Fix: item 52.
24. **The SDK's dry-run fix previews a different result from the real fix.** `packages/sdk/src/context.ts:64–70`: writes are recorded but reads still go to disk, so a fix that writes a file twice loses its first edit in the preview. Only third-party callers reach it. Fix: item 46.
25. **`--maxfail=1` is added to commands that only contain "pytest"**, such as `make pytest`, so the rerun fails with make's usage error. `activity.rs:38–46`. Fix: item 65.
26. **Test counts are low for steps that print more than 16 MB.** `verify.rs:1167` re-parses only the kept half. Fix: item 66.
27. **Swift reads an `#else` after `#elseif true` as "sometimes compiled".** `swift.rs:767–772`. Only the reason text is wrong. Fix: item 7.
28. **The coverage detector reads workflows GitHub never runs** (`coverage.rs:100–104`). Fix: item 53.
29. **`cmake_languages` reads a commented-out `project(`** (`ecosystems.rs:834`). Fix: item 55.
30. **Small ones:**
    - `fscale add package` prints a wrong "add to pnpm-workspace.yaml" note when the workspace file has a comment at column 0 or uses the flow form (`generators/workspace-package.ts:25`).
    - `scripts/smoke-npm.mjs:255/295` fail where `commit.gpgsign` is on.

## Duplication across the milestones of 14 November

The eleven milestones of 14 November are M25 to M35:

| Milestone | What it built                              |
| --------- | ------------------------------------------ |
| M25       | Tamper guard: config and CI                |
| M26       | Tamper guard: approvals and measured rates |
| M27       | Guarded agent runner                       |
| M28       | Agent adapters and `fscale ledger`         |
| M29       | Stack pack: Supabase                       |
| M30       | Stack pack: Expo                           |
| M31       | Ecosystems                                 |
| M32       | Tamper guard in eight more languages       |
| M33       | Stack pack: Go                             |
| M34       | Stack pack: Maven                          |
| M35       | Stack pack: CMake                          |

Each was built in its own session from the previous one's code. Each copied the nearest earlier piece and changed what it needed. Three kinds of piece were copied most.

### A. The stack packs (M29, M30, M33, M34, M35; nextjs M15 and python M16 for comparison)

Lines of each piece, per pack:

| Piece                                                     | nextjs | python | supabase | expo |  go | maven | cmake |
| --------------------------------------------------------- | -----: | -----: | -------: | ---: | --: | ----: | ----: |
| gitignore append block (eol, separator, header, write)    |     20 |     19 |       18 |   19 |  22 |     — |     — |
| fixture builder (`fixture`, `with`, `*_without`, `files`) |      9 |      — |       15 |   20 |  20 |    20 |    30 |
| Rust test harness + skill-count test                      |     32 |     14 |       33 |   29 |  29 |    29 |    29 |
| `inputs()` merge                                          |      — |      — |        9 |   12 |  12 |    12 |    36 |
| starter `package.json` writer                             |     28 |      — |       29 |   29 |   — |     — |     — |
| "starter files" entry rule                                |    150 |      — |        — |   58 |  86 |   109 |   154 |
| "manifest below the root" walker                          |      — |      — |       35 |    — |  16 |     7 |     6 |
| "only in a subfolder" finding                             |      — |      — |        — |    — |  17 |    18 |    12 |
| TypeScript pack test: header, starter, `setUp`, no-change |    ~25 |    ~25 |      ~65 |  ~65 | ~65 |   ~65 |   ~65 |

Notes on the table:

- nextjs's entry-rule figure is two rules, `next-config` and `tsconfig`.
- The TypeScript pack-test files total 586 lines for the five newest packs; about 360 of those lines are copies.

Each pack also reads its own file format: supabase has a TOML reader (about 80 lines), maven has `xml.rs` (257), and cmake has `core/cmake.rs` (430). facts and the ecosystems table read the same TOML and POM files again, and they read CMake files with a plain text search instead.

The copies have already drifted, which caused bugs 4, 11, 12, 13 and 15. A missed input or a missed manifest in one pack is the same mistake fixed in another pack weeks earlier.

**One shared piece:** grow `stackkit` into a pack kit.

- Shared helpers: `append_lines`/`eol`, `inputs(own, shared)`, `only_below`, one `below` walker and `starter_package_json`.
- Two generic rules: `GitignoreLines` and `StarterFiles`. Each pack then lists data (files, lines, messages) and keeps only the checks that are truly its own: the Go version line, the POM encoding, `enable_testing()` and RLS.
- A `#[cfg(test)]` test kit for the Rust tests, and one `packStarterSuite` for the TypeScript tests.

Items 2, 4, 5, 11, 13, 14, 17, 21, 26, 30, 37 and 38 come to about **−1,000 lines**. The next pack (M36 Swift) would then be mostly data. Several of these touch (a). The kit must keep `create` and the `jsonedit` merge as the only ways a pack writes, which the tests in items 3 and 11 prove.

### B. The per-language readers of the tamper guard (M32, on top of M23/M24's JS, Python and Rust)

Seven readers were written on 14 November, one after another. Each copied the previous reader's skeleton:

| Reader     |  Code | Tests | Corpus cases |
| ---------- | ----: | ----: | -----------: |
| java.rs    | 1,168 |   761 |           23 |
| kotlin.rs  | 1,256 |   662 |           22 |
| swift.rs   | 1,126 |   574 |           30 |
| dart.rs    |   832 |   495 |           26 |
| cfamily.rs | 1,867 |   985 |           46 |
| csharp.rs  | 1,197 |   630 |           28 |
| go.rs      | 1,459 |   743 |           28 |

The older three, js, python and rust, have 2,463 lines of code. Here is what each reader repeats:

- the parse prologue, the scope stack and the assertion literal;
- the `uses` walk and the suppression walk;
- `shown`/`keyed`, `squeezed`, `children` and `split_path`;
- "the first statement that does something";
- `#if` regions (three ways);
- Kotlin also repeats Java's annotation handling.

Two pieces were shared from the start, `reach::Grammar` and `recover::first_harmful`. The later readers work around the gaps in `Grammar` rather than filling them. The drift caused bugs 5, 18 and 27.

**One shared piece:** a reader layer beside `reach.rs`.

- A `Spec` holds:
  - the tree-sitter language and the `Grammar`, with body-finder, false-literal and first-statement hooks added;
  - a `lenient_errors` flag, true only for ADR 0052's eight languages;
  - the comment and string kinds, the identifier kinds and the id separator.
- The layer adds `open()`, `Scopes`, `TestFile::add_assertion`, `identifiers()`, one `Regions` builder, one `suppress::collect` and one `split_path`.
- Each language keeps what is truly its own: what a test is, skip and weight markers, which calls assert, imports and suppression keys.

Items 1, 7, 9, 12, 18, 19 and 28 come to about **−775 lines**, roughly 7% of the reader code. This keeps ADR 0052's "a reader per language", so it needs no new ADR; a single table-driven reader would. The labelled corpus and about 240 unit tests guard the move. Add the Kotlin cases first (bugs 5 and 18), because the corpus would not notice them change.

M25's four config detectors (`coverage`, `ci`, `noverify`, `lintconfig`) share `formats.rs` well. They repeat only path helpers and the workflow rule (item 53).

### C. The project-type table (M31, used by M33 to M35)

M31 made `ecosystems.rs` "one table of ecosystems found by a marker file". Today the question "what kind of project is this folder, and which files decide that?" is answered in at least seven places:

1. `ecosystems.rs`: `ECOSYSTEMS` (10 kinds), plus a hand-kept `MARKER_GLOBS` (22 globs). The globs must track every `marks` function, and they miss files the `found` functions read (`mvnw`, `gradlew`, `.swiftlint.yml`, `cmake/lint.cmake`, …).
2. `facts.rs`: Node, Python and Rust are not in the table. They are hand-coded: `node_detail`, `NODE_TOOLS`, `rust_detail`, `python_interpreter`, `PYTHON_SIGNS`, `LOCKFILE_NAMES` and the per-kind branches of `discovered_projects`. `DISCOVERY_SKIPPED_DIRS` holds 32 names.
3. `stackkit.rs`: `MANIFESTS` (28) and `CODE` (29 extensions), which go and maven use to say "another stack".
4. `supabase/package-json`: its own `OTHER_STACKS` (7), which causes bug 11.
5. `cmake/mod.rs`: `VENDORED` (9), `OTHER_C_BUILDS` (10), and the C, C++ and other-native extension lists. They differ from facts' and ecosystems' lists.
6. The rule-result cache inputs: `managed.rs` `FACTS_INPUTS` (28), the hand list in `gitignore-basics`, and each pack's `inputs()`. This is where bugs 4, 10, 12 and 13 come from.
7. `stepcache.rs` `probes`, which hardcodes the ecosystems' programs (item 61), and the per-kind bits in `verify.rs` (500–521, 714–760), `graph.rs` (528–670) and `doctor.rs`.

The packs decide "is this repo mine" four ways:

- nextjs and expo, by dependency;
- go and maven, by `MANIFESTS` and `CODE`;
- cmake, through the ecosystems table;
- python, through facts.

**One shared piece:** make the ecosystems table the only answer.

- Every kind, Node, Python and Rust included, gets an entry.
- Each entry holds:
  - its marker files;
  - its code extensions;
  - its vendored folder names;
  - the readers it uses (cmake, xml and toml move into core);
  - an `inputs` list: every file its detection and commands read.
- Built from the table:
  - `MARKER_GLOBS` and `FACTS_INPUTS`;
  - every pack's cache inputs and its "another stack" answer;
  - the step cache's probes.
- Packs ask `ecosystems::matching(folder, kind)`, as cmake's `root_is_cmake` already does.

This removes items 24, 41, 61 and 68, and parts of 26 and 33: about **−150 lines**. More importantly, it removes the whole class of "a cache input was forgotten" bugs, because no list is kept by hand. This touches (c): the rule cache and step cache keys would be derived rather than written. It also reverses the line in `ecosystems.rs`'s module doc that Node, Python and Rust "stay in facts", so per AGENTS.md it needs an ADR first.

**If only one piece is built, build C first.** Parts A and B are copies that cost reading time. C is copies that already serve stale cache results (bugs 4, 10, 12 and 13). A pack kit built on top of C would get each pack's inputs right by construction.

## Verdict

The code is careful where it matters most. The trust rules are written down, tested and mostly kept. But copying between milestones has started to cost correctness:

- four stale-cache bugs and one wrong-stack write come from hand-kept lists;
- one dropped-finding bug in the stop hook;
- one download-instead-of-setup bug;
- one secret leak in the heartbeat;
- one tamper-guard gap in Kotlin.

Fix bugs 1, 2, 3 and 4 first (small, each with a test), then build the shared table of part C.

**Lean: about −3,000 lines possible (overlaps removed, about 2.6% of 116k), −0 dependencies.** No dependency was found that a few lines or the platform could replace.

## Not checked

- **Not built or run for this audit.** No bug was reproduced, except bug 24 (on a scratch copy of the SDK) and bug 20 (seen in `dist/`). The full test suite passed on this commit (`pnpm build`, then `pnpm fscale verify`).
- **Read only in part:**
  - test bodies of the large test files;
  - `verify.rs`'s test module and `format_verify_human`;
  - `setup.rs` 620–1140, `services.rs` 197–500 and `doctor.rs` 510–1645;
  - the JUnit and cargo parsers in `pertest.rs`;
  - `stepcache.rs`'s snapshot and key code (137–350, 800–896, 1460–1720);
  - `process.rs`'s platform modules;
  - `references.rs`'s Dockerfile and Compose readers;
  - the bodies of `formats.rs` and the config detectors' `measures`;
  - the tokenizers of `supabase/sql.rs`, `maven/xml.rs` and `core/cmake.rs`;
  - the core pack rules other than `gitignore-basics`, `no-committed-env` and `env-example`'s append code;
  - most of the agent-ready and group-project packs;
  - the testkit sources;
  - `pnpm-lock.yaml`, the snapshots, the corpus JSON files and most ADRs.
- **Not measured:** speed. The skill asks for slow paths; only item 66's output copies were found by reading.
