# 0049 — The `supabase` stack pack

Status: accepted (2026-11-14, M29). Follows the stack-pack shape in PLAN.md (M15/M16) and ADR 0029 (the nextjs pack).

## What the pack assumes (read on 2026-11-14)

Sources: the Supabase CLI's source at tag `v3.1.0` (commit `d4fc0e5b`; v3 is a TypeScript rewrite, so the templates are in `apps/cli/src/shared/init/project-init.templates.ts` and the commands' `SIDE_EFFECTS.md` files); Supabase's docs as MDX in `supabase/supabase` at `33c8ab1d` (`apps/docs/content/guides`); the database linter `supabase/splinter` at `cd238c83`; versions from the npm registry. Then **checked with the CLI 3.1.0 itself** (`npm install supabase@3.1.0` in a scratch folder), for every command that runs without Docker.

- **Versions.** `supabase` (the CLI) 3.1.0 as an exact dev dependency, so `npx supabase` runs one version everywhere (the docs install it with `npm install supabase --save-dev`); `@supabase/supabase-js` 2.119.0, whose `engines` says Node `>=22` (it dropped Node 20 in 2.110), so a new project says `"engines": { "node": ">=22" }`.
- **`supabase init`** writes only `supabase/config.toml` and, inside a git repo, `supabase/.gitignore`. The only key the CLI insists on is `project_id`; every other setting has a default. **Checked:** CLI 3.1.0 loads a config of one line, `project_id = "…"`, and goes on to the database connection.
- **`[inbucket]` is refused by v3**: `supabase migration list` with an `[inbucket]` table fails with `[inbucket] in supabase/config.toml is no longer supported. Use [local_smtp] instead.` (checked). The old keys (`enabled`, `port`, `smtp_port`, `pop3_port`, `admin_email`, `sender_name`) load under `[local_smtp]` with no warning (checked). The docs' v3 migration guide says configs from `supabase init` before 2.108 have the old table.
- **Migrations** are `supabase/migrations/<digits>_<name>.sql` (`MIGRATE_FILE_PATTERN = /^([0-9]+)_(.*)\.sql$/`); any other file is skipped with only a warning; `supabase migration new` uses a 14-digit UTC timestamp. **Checked:** `npx supabase migration new create_notes` in the starter wrote `supabase/migrations/20261114041604_create_notes.sql`.
- **Row level security**: the docs say "A table in an exposed schema without RLS is readable and writable by any role with a grant on it. Enable RLS on every table in an exposed schema." The exposed schemas are `[api] schemas` (default `public`, `graphql_public`). splinter's `0013_rls_disabled_in_public` checks a running database for the same.
- **Keys**: the publishable key (`sb_publishable_…`) is for clients, the secret key (`sb_secret_…`, and the legacy `service_role`) never ("Never put one in a browser, a shipped application, or source control"; "Server-only. Never prefix these, or your bundler will ship the key."). The quickstarts name them `NEXT_PUBLIC_`, `VITE_`, `EXPO_PUBLIC_`, `PUBLIC_` + `SUPABASE_URL`/`SUPABASE_PUBLISHABLE_KEY`.
- **Tests**: `supabase test db` runs pgTAP files under `supabase/tests` (`**/*.sql`, `**/*.pg`); the testing guide keeps them in `supabase/tests/database/`.
- **Left out on purpose:** Edge Functions (their template and `deno.json` change often, and a function needs a decision about auth); `seed.sql` (a missing seed is a warning, and `supabase init` does not create one, so a project the CLI made would be flagged); type generation (needs a running database); the remote project (`supabase link`).

## The rules

| Rule                                | Checks                                                                                                                                 | Fix                                                                                                                                                                                                                                       |
| ----------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `supabase/package-json` (high)      | the CLI is a dev dependency                                                                                                            | creates package.json when missing; adds the CLI as a new line when there is no lockfile; with a lockfile, an agent finding with the install command (the version belongs to the lockfile)                                                 |
| `supabase/config` (high)            | `supabase/config.toml` exists and sets `project_id`                                                                                    | creates it with `project_id` only; adds `project_id` before the first table. The id is package.json's name cleaned as the CLI cleans names, or `supabase-app`, never the folder's name (a dry run in a scratch copy writes the same file) |
| `supabase/local-smtp` (high)        | no `[inbucket]` table                                                                                                                  | renames the header(s), keys and comments untouched; manual when package.json allows a CLI older than 2.108                                                                                                                                |
| `supabase/gitignore` (medium)       | `supabase/.branches` and `supabase/.temp` ignored (by `supabase/.gitignore` or the root one)                                           | creates `supabase/.gitignore` as `supabase init` writes it, byte for byte; or appends the missing lines in the file's own line endings                                                                                                    |
| `supabase/migration-names` (high)   | every `.sql` file in `supabase/migrations` matches the CLI's pattern; no version used twice                                            | none: a rename sets the order migrations run in, so an agent (SKILL.md) picks the timestamp                                                                                                                                               |
| `supabase/rls` (high)               | every table the migrations create in an exposed schema has RLS on after the last migration                                             | none: an agent adds a migration with `enable row level security` **and the policies the app needs**. A deterministic `enable` alone would cut the app off its own data                                                                    |
| `supabase/db-tests` (low)           | at least one test under `supabase/tests`                                                                                               | creates `supabase/tests/database/rls_test.sql`, a pgTAP test that every table in `public` has RLS on                                                                                                                                      |
| `supabase/public-secret-key` (high) | no tracked source, `.env*`, `app.json` or `eas.json` names a variable with a client prefix, `SUPABASE`, and `SECRET` or `SERVICE_ROLE` | none: rotate the key, move the code to the server                                                                                                                                                                                         |

**Where the pack applies.** At the repo root, when the root has `supabase/`, or no Supabase project is in a subfolder (three levels, skipping dependency, build and hidden folders). Dependencies do not count: a workspace often lists the CLI at its root while its project is in a subfolder. So `init --packs core,nextjs,supabase` gives one package.json with both stacks, and a workspace with `apps/db/supabase/` gets one manual finding naming that folder and nothing written at the root.

**The RLS reader** (`supabase/sql.rs`) splits statements outside comments (nested `/* */` too), strings, quoted names and dollar-quoted bodies, and follows `create [unlogged] table [if not exists]`, `alter table [if exists] [only] … enable|disable|force row level security`, `rename to`, `set schema` and `drop table`, through the migrations in the CLI's order. Unqualified names are in `public`; quoted names keep their case.

## Proof

- `fscale init --packs core,supabase` in an empty folder: the supabase pack reports nothing (test, `supabase-pack.test.ts`, with snapshots of the four files), and a second `sync` changes nothing. With `core,nextjs,supabase` neither pack reports anything.
- **Run by hand on 2026-11-14 in that starter** (output in the pull request): `npm install` (17 packages, exit 0); `npx supabase --version` 3.1.0; `npx supabase migration new create_notes` wrote the migration (exit 0); `npx supabase migration list` loaded the config and stopped at the database connection (no Docker); `@supabase/supabase-js` imports.
- **Not run**: `supabase start`, `supabase test db` and `supabase db lint` need Docker images, which could not be pulled in the build session. The pgTAP test in the starter has never run against a database.
- `adopt --dry-run` proposes no change on a project that is set up, and exactly the `[inbucket]` rename on one from an older CLI (tests). A table without RLS is an agent finding, and adopt writes nothing for it.
- **Real repo**, read-only: `supabase-community/supabase-mcp` at `9092ab90` (shallow clone; the pack switched on in a copy; nothing of theirs run). Two findings, both true: its `config.toml` still has `[inbucket]` (line 70), which CLI v3 refuses — manual, since its package.json allows `supabase ^2.1.1`; and it has no database tests (low). Its two migrations (a table, then RLS and policies) pass.

## After the review

A fresh subagent reviewed the pack on fixtures and on three real repos (supabase-mcp, chatgpt-your-files, basejump) and found, all fixed with tests:

- **A workspace listing the CLI at its root with its project in a subfolder got a second project written at the root**: dependencies no longer decide where the pack applies.
- **Cached runs hid findings**: what decides where the pack applies (package.json, `supabase/config.toml`, any `**/supabase/config.toml`) was not in the rules' inputs. Every rule now lists them (test: "every rule reads what decides where the pack applies"); package-json and local-smtp also list the lockfiles.
- **A config starting with a byte order mark** made the `project_id` fix land inside a table and repeat on every run: the mark is skipped, and the fix writes only when the key then reads back at the top level.
- **The SQL reader took every backslash as an escape**, so `like 'a\_%' escape '\'` swallowed the next statements: only `E'…'` strings escape.
- **Dollar-quoted bodies took quadratic time** (a 1.6 MB dump took seconds): the closing tag is found in place.
- **A `[api] schemas` array over several lines** turned the RLS rule off: arrays are read to their closing bracket, and a `#` comment after a value is dropped.
- **`force row level security` counted as RLS on**, and so did `no force`: only `enable` does.
- **The `[inbucket]` rename could make a second `[local_smtp]`** or rename text inside a multi-line string: a config with both is a manual finding, and multi-line strings are skipped. The update command names the repo's package manager.
- **One migration in another encoding stopped the whole check**: migrations are read lossily.
- **A Supabase project in a Flutter, Python, Go, Rust or Deno repo** was given a package.json (basejump would have been): with another stack's manifest and no package.json, the finding is manual and nothing is created.
- Smaller: a mixed-case table is quoted in the suggested SQL; `…_SUPABASE_SERVICE_KEY` under a client prefix is found too.

Left as limits (in the rls SKILL.md): tables made or RLS switched on in a `DO` block or a function, and unqualified names after `set search_path`, are not followed. The gitignore check reads patterns by name, not as git does (`/.temp` at the root counts; `supabase/*` does not).

## Consequences

- Opt-in: `"packs": [..., "supabase"]` or `fscale init --packs core,supabase`.
- Listed under "Needs your decision": exact pins; RLS left to an agent rather than fixed; Edge Functions and the seed file left out.
- The versions are a snapshot; a later firescale updates them.
