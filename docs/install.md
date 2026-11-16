# Installing firescale

> Not published yet. These instructions describe how it works once the packages are on npm. Until then, see "From a checkout" below.

## With npm, pnpm or yarn

```
npm install --save-dev firescale
npx fscale --version
```

`firescale` is a small package. Its dependency list includes one prebuilt binary package per platform, and your package manager installs the one that matches your machine. No Rust toolchain is needed.

Without installing: `npx firescale check`. Use the package name here: `npx fscale` would fetch an unrelated npm package that happens to be called `fscale`.

Supported platforms: Linux x64 and arm64 (glibc), macOS x64 and arm64, Windows x64 and arm64. Alpine and other musl systems are not supported yet.

## What runs when you type `fscale`

`fscale` is a short Node launcher that starts the native binary and passes everything through. The binary does the work and, for built-in rules, starts no further Node process.

If you call `fscale` very often, for example from an agent hook, you can skip the launcher and run the binary directly:

```
node_modules/@firescale/cli-<os>-<arch>/bin/fscale check
```

## Environment variables

| Variable           | Meaning                                                                                   |
| ------------------ | ----------------------------------------------------------------------------------------- |
| `FSCALE_BINARY`    | Path of a binary for the launcher to run in place of the installed one.                   |
| `FSCALE_NODE`      | The Node executable the binary uses for TypeScript rules. The launcher sets it to itself. |
| `FSCALE_NODE_HOST` | Path of the Node host script. The launcher sets it.                                       |
| `FSCALE_LOG=cache` | Print cache hits and misses on stderr.                                                    |

## Problems

- **"the package @firescale/cli-… is not installed"**: optional dependencies were skipped. Reinstall without `--omit=optional` or `--no-optional`.
- **"there is no prebuilt fscale binary for …"**: your platform has no package yet. Build from source and set `FSCALE_BINARY`.
- **`'fscale' is not recognized as an internal or external command` from `npx firescale` on Windows**: npx runs the command through `cmd.exe`, and `cmd.exe` does not search a `PATH` longer than about 8,191 characters. npx adds one folder to `PATH` for each folder above the one you are in, so a deep folder or a long `PATH` can pass that limit. Open a new terminal, or shorten `PATH`. A shell hook that adds to `PATH` on every `cd` (for example `mise activate bash` in Git Bash) makes `PATH` grow with each change of folder.

## From a checkout

```
pnpm install
pnpm build
pnpm fscale --version
```

To build installable tarballs for your own platform:

```
cargo build --release -p firescale-cli
pnpm -r run build
node scripts/pack-npm.mjs
node scripts/smoke-npm.mjs --pm npm
```

The tarballs land in `dist-npm/`.
