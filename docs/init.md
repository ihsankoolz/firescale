# `fscale init`, `fscale adopt` and `fscale sync`

These three commands are the same thing: the fix loop of [`fscale fix`](fix.md), run over every rule that is on. firescale has no project templates. A new repo gets its files because the rules that ask for them also know how to create them.

| Command               | Use it                           | What it does                                                                   |
| --------------------- | -------------------------------- | ------------------------------------------------------------------------------ |
| `fscale init`         | in an empty folder               | `git init` if needed, writes `firescale.config.json`, then runs the fixes.     |
| `fscale adopt`        | in a project that already exists | Writes the config if there is none, then runs the fixes. Adds what is missing. |
| `fscale sync`         | in a firescale repo, any time    | Runs the fixes again, to repair drift.                                         |
| `fscale sync --check` | in CI                            | Changes nothing. Exit code 1 when `fscale sync` would change something.        |

Every command prints two things: what it changed, and what is left for a person or an agent.

## What these commands will never do

- **Overwrite or delete a file.** The fixes that ship with firescale create a file only when it is absent, or add lines to the end of one (`.gitignore`, `.env.example`). The config file is created with a call that fails if the file exists.
- **Edit a README, a CI workflow or an AGENTS.md that you wrote.** If your README has no install steps, or your workflow does not run on pull requests, that is reported as left for you. It is not rewritten.
- **Put a second file beside yours.** With a `ReadMe.MD`, a `README.adoc` or an untracked `README.md` in the repo, no `README.md` is created. With any workflow file in `.github/workflows`, tracked or not, or another CI system's config, no workflow is created.
- **Commit.** Two fixes run `git add` on the one file they have just created (see "Staging" below). Nothing else is staged, and nothing is committed.

One fix changes the git index of a file you wrote: `core/no-committed-env` untracks a committed `.env` file (`git rm --cached`). The file stays on disk, and the change is printed.

Custom TypeScript rules are yours, and their fixes do what you wrote. Use `fscale adopt --dry-run` to see everything before it happens.

## `fscale init`

```
mkdir my-app && cd my-app
fscale init                    # packs: core and agent-ready
fscale init --packs core       # choose the packs
```

In an empty folder (a folder that holds nothing, or only `.git`), `init`:

1. runs `git init` if the folder is not in a git repo,
2. writes `firescale.config.json` with the chosen packs,
3. runs every fix until nothing is left to fix,
4. prints the changes, then the violations that no fix can repair.

With the `core` pack that gives you a `.gitignore`, a starter CI workflow that runs `npx firescale verify` on pull requests, and a README skeleton. The workflow and the README are yours from then on: firescale created them and will not rewrite them.

`--packs` takes built-in pack names, separated by commas. Add a custom pack by editing `packs` in the config afterwards.

### When the folder is not empty

`init` refuses, changes nothing, and tells you to use `adopt`:

```
fscale: This folder is not empty (2 entries: notes.txt, src) and is not a firescale repo, so `fscale init` will not touch it.
```

- `fscale init --force` runs anyway. It is exactly as careful as `adopt`; the only difference is that the new config gets init's packs (`core` and `agent-ready`, or `--packs`).
- In a folder that already has a firescale config, `init` keeps the config and runs the fixes, like `sync`. `--packs` is an error there, because it would be ignored.
- In an empty folder inside a firescale repo, `init` refuses, so a second repo is not started by accident. `--force` overrides this too.

## `fscale adopt`

```
fscale adopt --dry-run     # show every change as a diff, change nothing
fscale adopt               # apply
```

Run it at the root of an existing project. If the project has no firescale config, `adopt` writes one with the `core` pack only (`agent-ready` is opt-in for an existing repo; add it to `packs` when you want it). Every rule starts as `evaluated`, so adopting firescale does not fail anyone's build.

`--dry-run` copies the repo to a scratch folder, runs the real fixes there and prints the result, exactly as [`fscale fix --dry-run`](fix.md#dry-run) does. Your working tree and git index are not touched.

`adopt` ends with the list of what is left:

```
Left for you: 2 violations that a fix cannot repair. A person or an agent has to.
  core/scripts-present
    package.json: package.json has no "lint" script. ... [agent-fixable]
    How to fix: fscale explain core/scripts-present

To review:
  core/ci-exists: Review .github/workflows/ci.yml: it runs `npx firescale verify`. ...
```

"To review" lists notes from the fixes that created a file: what a person should look at in it.

## `fscale sync`

```
fscale sync            # repair drift
fscale sync --check    # CI: exit 1 if sync would change something
```

`sync` needs a firescale config. Without one it tells you to run `adopt` or `init`.

Drift is anything a fix would repair again: a generated file that was deleted, a `.gitignore` that lost a required line. `sync --check` is a dry run of `sync` whose exit code is 1 when there is at least one change, and it prints the changes as a diff.

## Staging

`core/ci-exists` and `core/readme-setup` only count files that git tracks: an untracked workflow does not run in CI, and an untracked README is not in anyone's clone. So when their fixes create a file, they also run `git add` on that one file, and the output says so:

```
core/readme-setup
  created README.md (staged with git add)
```

If such a file exists but is untracked, nothing is created or staged. The rule reports it as a manual fix: run `git add` on it yourself.

## Exit codes

| Code | Meaning                                                                                           |
| ---- | ------------------------------------------------------------------------------------------------- |
| 0    | The fixes ran and settled. Violations that need a person or an agent may remain; they are listed. |
| 1    | `sync --check` only: `fscale sync` would change something.                                        |
| 2    | Bad usage, a bad config, `init` refused a folder that is not empty, or the fixes did not settle.  |

A bad config, an unknown pack and a bad `after` declaration are all found before anything is written.

## Limits

- `init` and `adopt` only write `firescale.config.json`, never `firescale.config.ts`.
- In a dry run, what custom TypeScript rules would still report afterwards is not listed, because the Node host reads the real repo, not the scratch copy. Their fixes are previewed.
- A generated file that was committed and then deleted from disk is created again by `sync`. That holds for a README or workflow you wrote yourself and then deleted without committing the deletion: `sync` creates the starter version. `git restore <file>` brings yours back.
- The starter workflow calls `npx firescale verify`, which needs firescale on npm. Until 0.1.0 is published it fails at that step.
- The starter workflow has an install step only for npm and pnpm. For anything else it says so under "To review".
- These commands print text only. There is no `--json` output yet.
