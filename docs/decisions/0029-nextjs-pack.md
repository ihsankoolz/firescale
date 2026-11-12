# 0029 — The `nextjs` stack pack

Status: accepted (M15). Follows the stack-pack shape in PLAN.md (M15/M16) and ADR 0019 (the fix loop builds the starter).

## What the pack assumes (read on 2026-11-12)

nextjs.org was blocked from the build session. The facts come from the docs sources and create-next-app's `app-empty/ts` template in `vercel/next.js` at tag `v16.4.6` (commit `54b876b6`, 2026-11-09), and from the npm registry (`npm view`).

- **Versions.** next 16.4.6 (needs Node `>=20.9.0`), react and react-dom 19.3.1, eslint-config-next 16.4.6. Like create-next-app, the rest are ranges that the plugins support: `typescript ^5`, `eslint ^9`, `@types/node ^20`, `@types/react ^19`, `@types/react-dom ^19`. The newest TypeScript (7) and ESLint (10) are newer than eslint-config-next's plugins declare support for.
- **Scripts.** `dev: next dev`, `build: next build`, `start: next start`, `lint: eslint` (Next.js 16 removed `next lint`).
- **Files.** `app/layout.tsx` (the root layout with `<html>` and `<body>`) and `app/page.tsx`; `next.config.ts`; the template's `tsconfig.json` exactly (with `jsx: react-jsx`), because `next build` rewrites a tsconfig that lacks a required option, and a rewrite would make the fix and the build undo each other; a flat `eslint.config.mjs` with `eslint-config-next/core-web-vitals` and `/typescript`; `.next/` and `next-env.d.ts` in `.gitignore`.
- **Left out on purpose:** fonts from `next/font/google` (downloaded at build time, so a build would need the network), Tailwind, and create-next-app's `--yes` extras (Cache Components, `agentFeedback`). The layout types its props as `{ children: ReactNode }` rather than the template's `LayoutProps<"/">`, which only exists after `next build` or `next typegen`.

## The rules

| Rule                         | Checks                                                                                                                | Fix                                                                                                                                                                                                                                            |
| ---------------------------- | --------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `nextjs/package-json` (high) | next, react, react-dom are dependencies; dev, build, start scripts (and lint once eslint-config-next is a dependency) | creates package.json when missing; adds missing scripts as new lines; a missing dependency is for a person (its version belongs to the lockfile)                                                                                               |
| `nextjs/app-router` (high)   | an `app/` or `src/app/` has a root layout; with neither `app/` nor `pages/`, a first page                             | creates `layout.tsx`, and `page.tsx` for a new app; a Pages Router app passes                                                                                                                                                                  |
| `nextjs/next-config` (low)   | `next.config.{ts,mjs,js,cjs}`                                                                                         | creates `next.config.ts`                                                                                                                                                                                                                       |
| `nextjs/tsconfig` (medium)   | a TypeScript project (or a new one) has `tsconfig.json`                                                               | creates the template's; never edits one                                                                                                                                                                                                        |
| `nextjs/eslint` (medium)     | an ESLint config                                                                                                      | creates `eslint.config.mjs` only when eslint and eslint-config-next are dependencies (or package.json is about to be written with them); otherwise an agent finding with the install command, so an app is never given a config it cannot load |
| `nextjs/gitignore` (medium)  | `.next/` and `next-env.d.ts` ignored                                                                                  | appends them in the file's own line endings                                                                                                                                                                                                    |

Every fix only creates a missing file or appends; none replaces a file a person wrote. The package name of a new app is `next-app`, never the folder's name, so a dry run in a scratch copy writes the same file.

## Proof

- `fscale init --packs core,nextjs` in an empty folder: the nextjs pack reports nothing (test). Run by hand on 2026-11-12 in that folder: `npm install` (347 packages), `npm run build` (Next.js 16.4.6, two static routes) and `npm run lint` all exit 0, and the build leaves every generated file unchanged (`git status` shows only `package-lock.json` and the gitignored output). The output is in the pull request.
- `adopt --dry-run` proposes no change on an app that is set up, and exactly `app/layout.tsx` on one that lacks it (tests).
- On a copy of `vercel/commerce` with the pack on, the only finding is the missing ESLint setup, as an agent finding; nothing is proposed for its files.

## Consequences

- Opt-in: `"packs": [..., "nextjs"]` or `fscale init --packs core,nextjs`.
- Listed under "Needs your decision": exact pins for next and react versus ranges; the app name `next-app`; that `core/scripts-present` still asks a new app for a `test` script (Next.js has no test runner by default).
- The versions are a snapshot. A later firescale updates them; an app made earlier keeps its own.
