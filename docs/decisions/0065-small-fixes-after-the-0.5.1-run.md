# 0065 — Small fixes after the 0.5.1 run on the course project

Status: accepted (2026-11-16, 0.5.2 run). Refines ADR 0012 (`core/env-example`) and ADR 0026 (doctor). Findings 7, 8 and 9 of `docs/reviews/cs464-0.5.1.md`. Finding 10 of the same review updates ADR 0048 itself.

## Context

The owner ran 0.5.1 on the course project and on the SPM project on Windows. Three small findings are here: `core/env-example` asked the SPM project for Vite's `MODE`, doctor's line for installed Python packages read wrongly, and `npx firescale@0.5.1` failed 7 times with `'fscale' is not recognized`, then worked. As in the earlier runs: a test that fails first, then the smallest fix; a cause outside firescale is not worked around in code.

## Decision

### 1. `core/env-example` skips Vite's own names, only through `import.meta.env`

Vite sets `MODE`, `BASE_URL`, `PROD`, `DEV` and `SSR` on `import.meta.env` itself ("Built-in Constants" in Vite's "Env Variables and Modes", vite.dev, read on 2026-11-16). They are replaced at build time; no env file sets them. So a read of one of them through `import.meta.env.NAME`, `import.meta.env["NAME"]` or `import.meta.env['NAME']` is not asked for.

- The five names are a separate list in the rule (`VITE_BUILT_INS`), not part of the well-known list in `well_known.rs`. That list is skipped for every way of reading. `process.env.MODE` or `os.getenv("MODE")` is a project's own variable, and is still asked for.
- The names are skipped whether or not the project uses Vite. Any `import.meta.env` read is a bundler's, in practice Vite or a tool built on it, and those tools set the same five. Astro's further names (`SITE`, `ASSETS_PREFIX`) are not skipped; a project can list them in `ignore`.
- Only the exact names are skipped. `MODE_NAME` or `VITE_MODE` through `import.meta.env` is still asked for.

Not changed: names read only by `frontend/` still go into the root `.env.example` when there is no `frontend/.env.example`, though Vite reads env files from its own folder. The review saw this and did not call it a bug; it is how ADR 0012's "nearest example file" works.

### 2. Doctor's line for installed Python packages

The line now reads as the Node line does: `all 16 packages in backend/audit_service/requirements-dev.txt are installed`, and `the 1 package in backend/one_service/requirements.txt is installed` for one. It read `all 16 packages are in ... installed`. No snapshot held the old text, so no snapshot changed. The JSON format and schema are unchanged; only `detail` changed.

### 3. `'fscale' is not recognized` from `npx firescale`: a cause outside firescale gives the same message

A cause that gives the exact message and exit code was reproduced; that it is what happened at 09:07 is not proven. Nothing in firescale was changed for it. What was found, all on the owner's machine (Windows 11, Node 22.19.0, npm 10.9.3, the same as the review):

- **Why `fscale`, not `firescale`.** npm's `getBinFromManifest` (libnpmexec) takes the first `bin` key when every bin points at the same file. The `firescale` package lists `fscale` and `firescale`, both `./bin/fscale.js`, so `npx firescale@x` runs `fscale`. npx runs it through `cmd.exe /d /s /c`.
- **Not two npx processes on one folder.** The review's npx folder, `_npx/8cf7aca27e22ba4e`, was still as installed at 08:56:39: every file under it, `.bin/fscale.cmd` and `node_modules/.package-lock.json` included, had that time. Every install npx makes there (an Arborist reify) writes `node_modules/.package-lock.json` (`await this.actualTree.meta.save()` in `@npmcli/arborist/lib/arborist/reify.js`, npm 10.9.3). So nothing reinstalled it at 09:07, when the failures were seen. npx installs a package with an exact version only when its folder does not have it (`missingFromTree` in libnpmexec). 38 runs of `npx --yes firescale@0.5.1 --version` at once or staggered, after removing only that folder, all printed `0.5.1`.
- **`cmd.exe` ignores a `PATH` longer than about 8,191 characters.** With the npx bin folder first in `PATH`, `cmd /d /s /c "fscale --version"` found `fscale` with a `PATH` of 8,020 characters and printed `'fscale' is not recognized as an internal or external command, operable program or batch file.` with exit 1 at 8,196 and above.
- **npx makes `PATH` longer**: its bin folder, then `node_modules\.bin` of the current folder and of every folder above it, then npm's `node-gyp-bin`, then the caller's `PATH` (`@npmcli/run-script/lib/set-path.js`).
- **On this machine `PATH` grows on each `cd`.** The owner's `~/.bashrc` runs `eval "$(mise activate bash)"`. In Git Bash each `cd` to another folder adds about 2,000 characters to `PATH` (3,567, then 5,544, 7,619, 9,792, 12,063, 14,432 for five `cd`s). In one shell: after one `cd` into a scratch folder, `npx --yes firescale@0.5.1 --version` printed `0.5.1`; after two more `cd`s, ending in the same folder, the same command printed the reported message and exited 1. In a folder where it failed, the same command with a short `PATH` printed `0.5.1`.

The review's own commands are not kept, so it is not proven that this is what happened at 09:07: the cause is a candidate, not a finding. It gives the exact message and exit code, on the same machine, with the npx folder untouched, and it clears with a new shell. That fits "failed 7 times, then worked".

`docs/install.md` ("Problems") now names the message and the limit. No launcher change: the launcher never runs when `cmd.exe` cannot find it, and renaming or reordering the bins would only change the name in the message.

## Consequences

- A Vite project is no longer asked for `MODE=`, `DEV=`, `PROD=`, `SSR=` or `BASE_URL=` in `.env.example`. A project that already listed one keeps it; firescale removes nothing.
- Doctor's ok line for Python packages reads as the Node one.
- `npx firescale` on Windows still fails when the `PATH` npx builds passes `cmd.exe`'s limit; the docs say why and what to do.

## Not done

- Doctor's Node line still says `all 1 packages in package.json are installed` for a project with one package, and the Python and Node problem lines say `1 of 1 packages in ... are not installed`.
- `core/env-example` does not read which bundler a project uses; `import.meta.env` in a project that is not built by Vite (or a tool on it) would have its five names skipped too.
