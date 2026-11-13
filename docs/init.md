# `fscale init`, `fscale adopt` and `fscale sync`

These three commands are the same thing: the fix loop of [`fscale fix`](fix.md), run over every rule that is on. firescale has no project templates. A new repo gets its files because the rules that ask for them also know how to create them.

| Command               | Use it                           | What it does                                                                   |
| --------------------- | -------------------------------- | ------------------------------------------------------------------------------ |
| `fscale init`         | in an empty folder               | `git init` if needed, writes `firescale.config.json`, then runs the fixes.     |
| `fscale adopt`        | in a project that already exists | Writes the config if there is none, then runs the fixes. Adds what is missing. |
| `fscale sync`         | in a firescale repo, any time    | Runs the fixes again, to repair drift.                                         |
| `fscale sync --check` | in CI                            | Changes nothing. Exit code 1 when `fscale sync` would change something.        |

Every command prints the repo root it works in (`Repo root: ...`, which can be a parent of the folder you are in), what it changed, and what is left for a person or an agent.

## What these commands will never do

- **Overwrite or delete a file.** The fixes that ship with firescale create a file only when it is absent, or add lines to the end of one (`.gitignore`, `.env.example`). The config file, the starter workflow and the README skeleton are created with a call that fails if anything is at that path.
- **Write outside the repo.** A fix does not write to a symbolic link or below one. With a linked `.github` no workflow is created; with a linked `.gitignore` the run stops and says so.
- **Replace what is in the git index.** If git tracks a README or a workflow that is missing from disk, nothing is created in its place: the index may hold work you staged. The rule tells you to `git restore` the file or commit its deletion.
- **Edit a README or a CI workflow that you wrote, or AGENTS.md outside firescale's block.** If your README has no install steps, or your workflow does not run on pull requests, that is reported as left for you. It is not rewritten.
- **Put a second file beside yours.** With a `ReadMe.MD`, a `README.adoc` or an untracked `README.md` in the repo, no `README.md` is created. With any workflow file in `.github/workflows`, tracked or not, or another CI system's config, no workflow is created.
- **Commit.** Two fixes run `git add` on the one file they have just created (see "Staging" below). Nothing else is staged, and nothing is committed. If that `git add` fails, the new file is removed again.

One fix changes the git index of a file you wrote: `core/no-committed-env` untracks a committed `.env` file (`git rm --cached`). The file stays on disk, and the change is printed.

Custom TypeScript rules are yours, and their fixes do what you wrote. Use `fscale adopt --dry-run` to see everything before it happens.

## `fscale init`

```
mkdir my-app && cd my-app
fscale init                    # packs: core and agent-ready
fscale init --packs core       # choose the packs
fscale init --packs core,group-project   # a repo for a team: template, guide, CI, branch names
```

In an empty folder (a folder that holds nothing, or only `.git`), `init`:

1. checks the packs and the rules' order, then runs `git init` if the folder is not in a git repo,
2. writes `firescale.config.json` with the chosen packs,
3. runs every fix until nothing is left to fix,
4. prints the changes, then the violations that no fix can repair.

With the `core` pack that gives you a `.gitignore`, a CI workflow that runs `npx firescale verify` on pull requests (only what each one changes) and on pushes, with a shared step cache ([ci.md](ci.md)), and a README skeleton. The README is yours from then on. The workflow is yours as soon as you edit it: until then, `fscale sync` replaces it when firescale has a newer one, and after your first edit it never touches it again.

`--packs` takes built-in pack names, separated by commas. Add a custom pack by editing `packs` in the config afterwards.

### When the folder is not empty

`init` refuses, changes nothing, and tells you to use `adopt`:

```
fscale: This folder is not empty (2 entries: notes.txt, src) and is not a firescale repo, so `fscale init` will not touch it.
```

- `fscale init --force` runs anyway. It is exactly as careful as `adopt`; the only difference is that the new config gets init's packs (`core` and `agent-ready`, or `--packs`).
- In a folder that already has a firescale config, `init` keeps the config and runs the fixes, like `sync`. `--packs` is an error there, because it would be ignored.
- In an empty folder inside another git repo or firescale repo, `init` refuses: the files would land in a subfolder of that repo and be staged in its index. `--force` overrides this too.

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

Drift is anything a fix would repair again: a generated file that is gone from disk and from git, a `.gitignore` that lost a required line. A file that is deleted from disk but still tracked is not recreated (see above); `git rm` it, or `git restore` it. `sync --check` is a dry run of `sync` whose exit code is 1 when there is at least one change, and it prints the changes as a diff.

## Staging

`core/ci-exists` and `core/readme-setup` only count files that git tracks: an untracked workflow does not run in CI, and an untracked README is not in anyone's clone. So when their fixes create a file, they also run `git add` on that one file, and the output says so:

```
core/readme-setup
  created README.md (staged with git add)
```

If such a file exists but is untracked, nothing is created or staged. The rule reports it as a manual fix: run `git add` on it yourself. The same holds when a `.gitignore` rule ignores `README.md` or `.github/`: the rule says so and creates nothing.

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
- The preview of `adopt --dry-run` and `sync --check` runs in a copy of the repo that holds tracked files, files git does not ignore, folders and symbolic links. Files that git ignores are not in it, and neither are the repo's own git settings (`.git/info/exclude`, sparse checkout), so in such repos the preview can differ from the real run.
- Files are written in place, not through a temporary file. If the disk fills up or the process is killed while a fix appends to `.gitignore`, that file can be left cut short.
- The workflow calls `npx firescale verify --shared-cache …`, which needs a firescale on npm that knows `--shared-cache` (0.3.0 or later). With an older one it fails at that step.
- The workflow installs only npm, pnpm and pip projects. For anything else it says so under "To review".
- These commands print text only. There is no `--json` output yet.

## Agent files (the `agent-ready` pack)

With `agent-ready` on (the default for `init`), the loop also writes what coding agents need (ADR 0027):

- `AGENTS.md` with a **managed block**: the repo's commands from `fscale facts` and the definition of done. The block runs from `<!-- BEGIN firescale ...` to `<!-- END firescale -->`. Write your own notes outside it; `fscale sync` replaces only what is inside, and `fscale sync --check` fails after a hand edit inside it.
- In an existing `CLAUDE.md` at the root, a three-line block at the end with `@AGENTS.md`, because Claude Code reads only `CLAUDE.md` when there is one. Nothing else in `CLAUDE.md` changes, and nested `CLAUDE.md` files are not touched.
- The stop hooks for Claude Code, Codex and Cursor (see [hooks.md](hooks.md)).
- Safe permissions: Claude Code and Cursor may not read env files (`.env`, `.env.local`, ...), keys or a `secrets/` folder, and Claude Code may run the repo's checks without asking; Codex gets a permission profile in a new `.codex/config.toml`. A Codex config you wrote is never edited.
- The SKILL.md of every enabled rule in `.claude/skills/`, `.agents/skills/` (Codex) and `.cursor/skills/`.

A settings file you already have keeps every line: firescale adds its entries as new lines and never re-formats the file. If it cannot do that (a list written on one line, invalid JSON), it leaves the file and tells you what to add.
