# 0023 — The group-project pack

Status: accepted (M11). Builds SPEC §6 "group-project" and the pack half of §17.

## Context

SPEC §6 and PLAN M11 ask for an opt-in `group-project` pack, in Rust, with four rules: a pull request template exists (fix creates it), a contributing guide exists (fix creates it, with the commands from `fscale facts` and the branch naming pattern), CI runs `fscale verify` on pull requests (after `core/ci-exists`; a hand-written workflow without verify is agent-fixable, not rewritten), and the current branch name matches a pattern from the rule's options (no fix).

GitHub's rules for these files were read on 2026-11-12. docs.github.com could not be reached from the build session, so they were read from the source of those pages in the `github/docs` repository:

- "You can store your pull request template in the repository's visible root directory, the `docs` folder, or the hidden `.github` directory. Pull request template filenames are not case sensitive, and can have an extension such as .md or .txt."
- Several templates can live in a `PULL_REQUEST_TEMPLATE` folder in any of those places, chosen with the `template` query parameter.
- Contributing guidelines go in the root, `docs` or `.github`; "Contributing guidelines filenames are not case sensitive."
- `GITHUB_HEAD_REF`: "The head ref or source branch of the pull request in a workflow run. This property is only set when the event that triggers a workflow run is either `pull_request` or `pull_request_target`."

## Decision

### The rules

| Rule                         | Severity | Fix                                                    |
| ---------------------------- | -------- | ------------------------------------------------------ |
| `group-project/pr-template`  | low      | creates `.github/pull_request_template.md`             |
| `group-project/contributing` | medium   | creates `CONTRIBUTING.md`                              |
| `group-project/ci-verify`    | medium   | creates the starter workflow, only when there is no CI |
| `group-project/branch-name`  | low      | none                                                   |

- **A tracked file of the kind that is missing from disk blocks the fix**, under any name and letter case, so the fix never adds `pull_request_template.md` beside a deleted `PULL_REQUEST_TEMPLATE.md` (two such names break checkouts on Windows and macOS).
- **`.gitignore` files, and a plain file named `.github` or `docs`, are cache inputs** of the rules whose fix they can block, so a cached result never goes stale when they change. `core/ci-exists` gained the same inputs.
- **Where files count.** Every place GitHub reads, in any letter case; a `PULL_REQUEST_TEMPLATE` folder with a file in it counts as a template. In a git repo a file counts only when tracked: a teammate gets it only through a commit. An untracked file is a manual finding that names the `git add` command, and the fix does not create a second file beside it.
- **Fixes follow M9's rules.** They create a file only where nothing is (`create_tracked`), never through a link, never where `.gitignore` would keep the file out of git, never over a tracked file missing from disk. The check uses the same precondition, so a finding is deterministic only when the fix will act.
- **The fixes write fixed text** for the template, and text built from `fscale facts` for the guide. Both are LF and are pinned by snapshot tests.
- **The guide's commands are portable.** A Python command that starts with this machine's `.venv` interpreter or active environment is written as `python -m ...`; uv, poetry and pipenv commands are kept. A `.venv` path in a team document would be wrong for every other machine, and on Windows it would hold backslashes.
- **The guide lists** each project's install command and its lint, typecheck and test commands, the one check to run (`npx firescale verify`), what `SETUP NEEDED` means, the branch patterns, and what a pull request needs. With nothing detected it says "Describe how to install the project here." instead of inventing commands.
- **`ci-verify` reads only `run:` steps** (one line, or the lines of a `|` block, or a `>` block folded into one) of a tracked workflow whose `on:` lists `pull_request` or `pull_request_target`, using `core/ci-exists`'s trigger reader. A command counts when, after variable assignments and a runner (`npx`, `pnpm exec`, `yarn`, `bunx` and their options), it starts `firescale` or `fscale` (also `@<version>` or a path) and its first word after options is `verify`. Commands joined with `&&`, `;` or `|` are read one by one. A step's name, an `echo` or a later command that mentions the words does not count. A package script that runs verify is not followed.
- **`ci-verify` has the same fix as `core/ci-exists`**, shared code: it creates the starter workflow only when there is no CI config at all. It runs after `core/ci-exists`, so with both on, the workflow is written once. Without `core`, the pack still builds a repo that passes. A hand-written workflow is never changed; the finding is agent-fixable and names the one step to add. CI of another system is a manual finding, since the rule cannot read it.
- **The repo's own main branch always passes `branch-name`**: `init.defaultBranch` and `origin/HEAD`. Without that, `fscale init` on a machine with `init.defaultBranch = trunk` would fail its own pack.
- **`branch-name` checks the checked-out branch**, also before the first commit (`git symbolic-ref`). With a detached HEAD it checks `GITHUB_HEAD_REF`, which is how a pull request run on GitHub Actions knows the branch. Outside git, or detached with no pull request, it reports nothing. It has no inputs, so it is never cached: the branch is not a file.
- **Branch patterns are globs**, from the option `branches`. `*` is any text, also `/`. The default list is `main`, `master`, `develop`, and `feat/*`, `fix/*`, `docs/*`, `test/*`, `refactor/*`, `chore/*`, `ci/*`, `build/*`, `perf/*`, `style/*`, `release/*`, `hotfix/*`, `revert/*`: the Conventional Commits types, plus release, hotfix and revert. `globset` is already a dependency.
- **`contributing` has its own `branches` option** with the same default, because a rule only sees its own options. The docs say to keep the two the same.
- **The finding of `branch-name` names `.git/HEAD` as its file**, since the branch lives there.

### Tests

The explain test of every SKILL.md now runs in a scratch repo with every built-in pack on. It used to run in this repo, whose config turns on `core` and `agent-ready` only; turning `group-project` on here would make this repo carry its own template and guide and judge the branch name of every session branch.

## Consequences

- `fscale init --packs core,group-project` builds a repo where the pack reports nothing: the template, the guide, and one workflow that runs verify.
- The pack is opt-in, like `agent-ready`. `adopt` still writes `core` only.
- A team that renames its main branch, or uses other kinds of branches, sets `branches` on both rules.
- The guide is written once. When the commands change later, the guide is the team's to update; `sync` does not rewrite it.
