# 0006 — Fixture repos are built at test time

Status: accepted (M1). Since ADR 0008 the same approach is used for Rust rule tests (`firescale_core::testing::Fixture`).

## Context

AGENTS.md says rules are tested with fixture repos: passing, failing and fixed. `core/no-committed-env` is about the git index, so its fixtures must be real git repos with tracked files.

Checked-in fixture folders do not work here:

- A nested `.git` folder cannot be committed.
- A tracked `.env` fixture in this repo would violate the very rule being tested once firescale checks itself (M3), and it is a bad habit next to the "no secrets" hard rule.

## Decision

`@firescale/sdk/testing` exports `createFixtureRepo` and `fixtureSet`. A test describes its files inline; the helper creates a temp folder, runs `git init`, writes the files and adds the chosen ones to the index. No commit is made, because "tracked" means "in the index", so no git identity is needed on CI machines. The "fixed repo" is the failing fixture after `fix()` has run.

The helper passes `core.autocrlf=false` and resolves the real path of the temp folder, so fixtures behave the same on Windows, macOS and Linux.

## Consequences

- Tests need `git` on PATH. All three CI images have it.
- Fixtures are a few lines next to the assertions, which keeps each case readable.
- Rules that only read files could use checked-in fixture folders later; the helper does not prevent that.
