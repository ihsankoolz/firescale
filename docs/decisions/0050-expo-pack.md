# 0050 — The `expo` stack pack

Status: accepted (2026-11-14, M30). Follows the stack-pack shape in PLAN.md (M15/M16) and ADRs 0029 (nextjs) and 0049 (supabase).

## What the pack assumes (read on 2026-11-14)

docs.expo.dev could not be reached from the build environment. The facts come from Expo's docs source in `expo/expo` at `696b087d` (`docs/pages`, what docs.expo.dev renders), the CLI at branch `sdk-58` (`8a5196f9`), the npm tarballs of `expo@58.0.25` and `expo-template-blank-typescript@58.0.34`, and the npm registry. Then **checked by running the starter**: `npm install`, `tsc --noEmit`, `expo lint`, `expo install --check` (offline) and `expo export --platform android` (offline) all succeed (below).

- **Versions.** SDK 58: `expo` ~58.0.25 (Node `^22.13.0 || ^24.3.0 || …`, so `engines` `>=22.13.0`); from `expo/bundledNativeModules.json`: react 19.3.0, react-native 0.88.1, expo-status-bar ~58.0.5, eslint-config-expo ~58.0.4; from the blank TypeScript template: typescript ~6.0.3, @types/react ~19.3.0; ESLint `^9.0.0`, what `expo lint` installs. **Not npm's latest**: TypeScript 7, ESLint 10 and React 19.3.1 do not match the SDK, and `expo install --check` would flag them.
- **The starter is the blank TypeScript template**, not the default (Expo Router) one: four runtime dependencies, no plugins, no generated route types, and no image files. The default template needs about 20 packages and 30 images.
- **No image files.** The template's `app.json` names an icon, an adaptive icon and a favicon; `expo-doctor` checks every image a config names, and a fix writes text. The starter's `app.json` is the template's without those three keys (they are optional in the SDK 58 schema). SDK 58 has no `newArchEnabled` and no top-level `splash`.
- **ESLint is shipped, not left to `expo lint`**: with no config, `npx expo lint` installs `eslint` and `eslint-config-expo`, writes `eslint.config.js` and adds a script, which needs the network and changes the repo during a check. The starter has the same file, byte for byte (`@expo/cli/static/template/eslint.config.js`).
- **The entry**: `"main": "index.ts"`, which calls `registerRootComponent(App)` (the template's `index.ts` and `App.tsx`, exactly). With no `main`, Expo's `expo/AppEntry` loads `App` from the root; Expo Router's is `expo-router/entry` with `app/_layout` or `src/app/_layout`.
- **`EXPO_PUBLIC_` variables are inlined into the bundle**: "Do not store sensitive info, such as private keys, in `EXPO_PUBLIC_` variables. These variables will be visible in plain-text in your compiled application."
- **`.gitignore`**: the template ignores `.expo/`, `dist/`, `.env*.local`, the signing files (`*.jks`, `*.p8`, `*.p12`, `*.key`, `*.mobileprovision`) and more; `expo-doctor` warns when `.expo/` or `.env*.local` is not ignored.
- **Left out on purpose:** Expo Router, EAS (`eas.json` comes from `eas build:configure`), native folders and Continuous Native Generation, and `expo-doctor` itself (its schema, version and directory checks need the network).

## The rules

| Rule                            | Checks                                                                                                                                 | Fix                                                                                                                                                          |
| ------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `expo/package-json` (high)      | expo, react, react-native; `start`, `android`, `ios` scripts; `web` with react-native-web; `lint` with eslint-config-expo              | creates package.json; adds missing scripts as new lines; a missing react or react-native is for `npx expo install` (an agent finding), never typed by hand   |
| `expo/app-config` (high)        | an app config exists; a static one sets `expo.name` and `expo.slug`, not `expo.sdkVersion`                                             | creates app.json; adds a missing name or slug from package.json's name; `sdkVersion` is an agent finding. A dynamic `app.config.*` is the app's own business |
| `expo/entry` (high)             | `main` resolves (as Metro would: extensions, `index`); `expo-router/entry` has a root layout; no `main` has `App`                      | creates `App.tsx` (or `App.js`) and `index.ts` for a new app, or a missing `index.ts`; a missing other entry or layout is for an agent                       |
| `expo/tsconfig` (medium)        | a TypeScript app has tsconfig.json                                                                                                     | creates the template's; never edits one                                                                                                                      |
| `expo/eslint` (medium)          | an ESLint config, unless the app lints another way (`biome.json`, a `lint` script that is not ESLint)                                  | creates `eslint.config.js` as `expo lint` would, when eslint and eslint-config-expo are dependencies; otherwise an agent finding with the install command    |
| `expo/gitignore` (medium)       | `.expo/`, `dist/`, `.env*.local` (`.env*`, `.env.*` or `*.local` count), the five signing-file patterns                                | appends the missing lines in the file's own line endings                                                                                                     |
| `expo/public-env-secret` (high) | no tracked source, `.env*`, `app.json` or `eas.json` names `EXPO_PUBLIC_…` with SECRET, PRIVATE, PASSWORD, SERVICE_ROLE or SERVICE_KEY | none: rotate it and move the code to a server. Not `TOKEN` or `KEY` alone: many public client keys are meant to ship                                         |

The pack applies at the root when there is no package.json yet or it depends on `expo`; a workspace root with the app in a subfolder gets one manual finding and nothing written. In the pack list it comes before `supabase`, so `init --packs core,expo,supabase` writes Expo's package.json and the supabase pack adds its CLI to it.

## Proof

- `fscale init --packs core,expo` in an empty folder: the expo pack reports nothing (test, `expo-pack.test.ts`, with snapshots of the seven files), a second `sync` changes nothing; with `core,expo,supabase` neither pack reports anything.
- **Run by hand on 2026-11-14 in that starter** (output in the pull request): `npm install` (683 packages, exit 0); `npx tsc --noEmit` exit 0; `npx expo lint` exit 0, writing nothing; `EXPO_OFFLINE=1 npx expo install --check` "Dependencies are up to date" (offline, so checked against the SDK's bundled list only); `EXPO_OFFLINE=1 npx expo export --platform android` bundled `index.ts` (594 modules) into `dist/`. That run showed `dist/` was not ignored, and `expo/gitignore` now requires it.
- **Not run**: the app on a device or simulator, an iOS bundle (`expo export --platform ios` needs no Mac but was not tried), the web (needs react-native-web), and `expo-doctor` (needs the network).
- `adopt --dry-run` proposes no change on an app that is set up, and exactly `tsconfig.json` on one that lacks it (tests).
- **Real repo**, read-only: `streamyfin/streamyfin` at `1e3062a2` (shallow clone; the pack switched on in a copy; nothing of theirs run). First pass: three findings, two of them false (it lints with Biome; it is a phone and TV app with no web script). Both fixed with tests (an app that lints another way passes; `web` only with react-native-web). Now one finding, true: its `.gitignore` covers `.env` and `.env.local` but not `.env.development.local` and the like, which `expo-doctor` also flags.

## Consequences

- Opt-in: `"packs": [..., "expo"]` or `fscale init --packs core,expo`.
- Not meant to be combined with the nextjs pack (two React versions in one package.json).
- Listed under "Needs your decision": the blank template rather than Expo Router; no images in the starter; SDK 58 pins.
