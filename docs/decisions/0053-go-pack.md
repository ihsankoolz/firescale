# 0053 — The `go` stack pack

Status: accepted (2026-11-14, M33). Follows the stack-pack shape of PLAN.md (M15, M16) and ADRs 0029, 0049 and 0050; builds on the Go project support of ADR 0051.

## Context

The owner asked for other languages "as deep as the options go": project support (M31, ADR 0051), the tamper guard (M32, ADR 0052), then starter packs. Go comes first of the packs: its tools are in the build environment and its module proxy is reachable, so the starter can be built and run here, not only written.

M31 already gives a Go module its commands (`go build ./...`, `go vet ./...`, `go test ./...`) and `fscale doctor` checks the Go version. What a pack adds is what makes `fscale init --packs core,go` produce a module those commands pass on, and the hygiene a Go repo needs.

## What the pack assumes (read on 2026-11-14)

go.dev could not be reached from the build environment. The facts come from the Go source at go1.27.4, the Go documentation's source (`golang/website`, `_content/ref/mod.md`), and the toolchain list of proxy.golang.org, and were **checked by running** the starter with Go 1.24.7 and the 1.26.0 toolchain it downloads.

- **Versions.** The newest release is go1.27.4; Go supports its two newest releases (1.27 and 1.26). A new module gets `go 1.26.0`, which both build, and which an older local Go builds by downloading the 1.26.0 toolchain (`GOTOOLCHAIN=auto`, the default: run here, Go 1.24.7 fetched go1.26.0 and built). `go mod init` writes the exact local version instead (`gover.Local()` in `modload`), which a teammate on an older patch release would have to download.
- **`go.mod` without a `go` line** means Go 1.16 ("If the `go` directive is missing, `go 1.16` is assumed", the module reference).
- **The module path** of a new module is `example.com/app`, fixed rather than the folder's name, so a dry run in a scratch copy writes the same file, as the other packs do; the skill says to change it.
- **`go build ./...` with a root `main` package writes the program at the root**, named after the last element of the module path (run here: `app`), and a major-version suffix is not the name (`…/tool/v2` builds `tool`).
- **`go.work`**: "It is generally inadvisable to commit go.work files into version control" (the module reference, with exceptions the skill points to).
- **What the go tool ignores**: folders whose name starts with `.` or `_`, `testdata`, and `vendor` for a module's own packages.

## The rules

| Rule                    | Checks                                                                                               | Fix                                                                                                                 |
| ----------------------- | ---------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| `go/go-mod` (high)      | the root has a `go.mod` with a `module` line and a `go` line                                         | creates `go.mod`; adds `go 1.26.0` after the module line in the file's line endings; no module line is for a person |
| `go/entry` (high)       | the module has Go code where the go tool looks                                                       | writes `main.go` (a greeting) and `main_test.go`, as `gofmt` writes them, when there is no Go file at all           |
| `go/tests` (medium)     | a module with Go code has a `_test.go` file                                                          | none: an agent writes a test                                                                                        |
| `go/go-sum` (high)      | a module that requires other modules has a `go.sum`                                                  | none: `go mod tidy` needs the network, and `go.sum` is never written by hand                                        |
| `go/gitignore` (medium) | `.gitignore` ignores the root program (`/app`), `*.exe`, `*.test`, `*.out`, `go.work`, `go.work.sum` | appends the missing lines under `# Go`                                                                              |

**Where it applies**: a repo with a root `go.mod`, or with no Go module anywhere and no other stack's manifest at the root (a new module). A repo whose modules are in subfolders gets one manual finding from `go/go-mod` naming them, and nothing written.

**Left out on purpose**: a linter config (golangci-lint is not part of Go; `go vet` is the lint step), `gofmt` checks (a rule cannot run tools; the starter is formatted), a CI step to set up Go (GitHub's runners have Go, and `GOTOOLCHAIN=auto` fetches 1.26.0), and modules in subfolders.

## Proof

- **Run for real**: in an empty folder, `fscale init --packs core,go` leaves no violations; then `go build ./...`, `go vet ./...` and `go test ./...` pass, `gofmt -l .` lists nothing, `go run .` prints `Hello, world!`, the built `app` is ignored (`git status` shows nothing new), and `fscale verify` passes (check, `go vet`, `go test`).
- **Tests**: each rule's `tests.rs` (passing, failing, fixed, the fix run twice, CRLF), the pack's tests (an empty folder built by the fixes, a second run changes nothing, another stack left alone, modules in subfolders, a doc comment's `package main`), and `packages/cli/src/go-pack.test.ts` (`init` with snapshots, `sync` changes nothing, the facts of the starter, `adopt --dry-run` on a set-up module and on one missing its `go` line).
- **Real repos**, shallow clones, read-only, nothing of theirs run, the pack alone: spf13/cobra and gin-gonic/gin get only `go/gitignore` (they do not ignore `*.test`, `*.out`, `go.work`), and GoogleCloudPlatform/microservices-demo one finding naming its four modules. Both other results were wrong in the first pass and are tests now: the pack would have created a root `go.mod` in microservices-demo, and asked gin to ignore `/gin` because its `doc.go` shows `package main` in a comment.

## Not checked, and limits

- Modules in subfolders, and multi-module repos, are not checked.
- `go/tests` asks for one test file in the module, not one per package.
- The module path is never checked against the repo's remote.
- The `go` line is not checked against Go's supported releases: a module on `go 1.21` passes.
- Windows and macOS: the starter was built on Linux only; the rules are text, and CI runs the tests on all three.

## Consequences

- `fscale init --packs core,go` starts a Go module that builds, vets and tests, with `fscale verify` and an agent's stop hook running Go's own commands.
- `fscale adopt` brings an existing Go module the `.gitignore` lines it lacks, and names a missing `go.sum` or test file for a person or an agent.
