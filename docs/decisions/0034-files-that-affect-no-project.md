# 0034 — Files that cannot affect a build or a test

Status: accepted (follow-up to M17). Narrows one rule of ADR 0032.

## Context

ADR 0032 made every changed file that no project owns affect every project, so that `verify --affected` never runs too little. On a repo with no root project that includes the README: fixing a typo in it ran every project's lint, typecheck and tests. The owner decided that files which cannot change what a build or a test does should affect no project.

## Decision

A changed file affects **no project** when no project and no narrow shared node holds it (ADR 0032) and it is one of these:

| Kind          | Exactly                                                                                                                                                                     |
| ------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Markdown      | a name ending in `.md` or `.markdown`, any letter case, in any folder                                                                                                       |
| Text          | a name ending in `.txt`, any letter case, in any folder, **except** names that contain `requirements` or `constraints` (pip reads them) and `CMakeLists.txt` (a build file) |
| Licence       | a name starting with `license` or `licence`, any letter case and extension: `LICENSE`, `LICENSE.md`, `LICENCE-MIT.txt`                                                      |
| Documentation | any file under the `docs/` folder at the repo root                                                                                                                          |

- The rules of ADR 0032 that widen still come first: a CI file (`.github/workflows/README.md` is under `.github/workflows/`), a Compose file, a root lockfile and the firescale config affect every project.
- A file that a project owns is still that project's (`frontend/README.md` runs the frontend), and a file in a narrow shared node still affects its users (an OpenAPI spec under `docs/` that projects read).
- `check` always runs, so the rules still see the change (`core/readme-setup`, for example).
- When such files are the only change, the headline says `0 of N projects ran`, and a note names them: `1 changed file (README.md) belongs to no project and cannot change a build or a test ...`.
- Every other file no project owns keeps affecting every project: a root script (`deploy.sh`), a `Makefile`, a dotfile, an image outside `docs/`.

Why these and not more: each kind on the list is read by people, not by compilers, test runners or package managers. `.rst` and `.adoc` outside `docs/`, images, and `.mdx` (which some site builds compile) are left out; a repo that wants them quiet can move them under `docs/`.

## Consequences

- A README, licence or docs change on a branch runs only the check.
- A test that reads a `.txt` or `.md` file outside every project (a fixture at the root) is not run when that file changes. That is the cost of the rule; such fixtures belong inside the project that reads them.
- Tests changed on purpose: two tests of ADR 0032 used `README.md` as "a file no project owns"; they use `deploy.sh` and `Makefile` now, and new tests prove the README case.
