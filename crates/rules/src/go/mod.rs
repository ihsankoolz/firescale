//! The `go` stack pack (ADR 0053): rules whose fixes create and repair what
//! a Go module needs, so `fscale init --packs core,go` builds a working
//! starter that `go build ./...`, `go vet ./...` and `go test ./...` pass.
//! Opt-in: add "go" to `packs`.
//!
//! What the pack assumes was read on 2026-11-14 from the Go source at
//! go1.27.4 (`go mod init`, `go/build`), the Go module reference, and the
//! toolchain list of proxy.golang.org; the version is [`GO_VERSION`].
//!
//! A fix only creates a file that is missing, or adds missing lines. It
//! never replaces a file a person wrote.

use firescale_core::rule::{BuiltinPack, RuleContext};
use firescale_core::Result;

pub(crate) use crate::stackkit::{any_exists, create, finding};

pub mod entry;
pub mod gitignore;
#[path = "go-mod/mod.rs"]
pub mod go_mod;
#[path = "go-sum/mod.rs"]
pub mod go_sum;
#[path = "tests/mod.rs"]
pub mod test_files;

#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod pack_tests;

/// The `go` line a new module gets. Go supports its two newest releases;
/// on 2026-11-14 they were 1.27 (1.27.4) and 1.26. `go 1.26.0` builds with
/// either, and an older local Go downloads the 1.26.0 toolchain itself
/// (`GOTOOLCHAIN=auto`, the default). `go mod init` writes the exact local
/// version instead (`gover.Local()`), which a teammate on an older patch
/// release would have to download.
pub const GO_VERSION: &str = "1.26.0";

/// The module path of a new module. Fixed, never the folder's, so a dry run
/// in a scratch copy writes the same file; `go/go-mod`'s skill says to
/// change it to where the code will live.
pub const MODULE: &str = "example.com/app";

/// Root files that mean the repo is another stack's: the pack applies to
/// them only when they also have a `go.mod`.
const OTHER_MANIFESTS: [&str; 10] = [
    "package.json",
    "pyproject.toml",
    "Cargo.toml",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "CMakeLists.txt",
    "Package.swift",
    "pubspec.yaml",
    "requirements.txt",
];

/// True when the pack applies to the repo root: it has a `go.mod`, or it
/// has no manifest of another stack (a new module). A repo whose Go module
/// is in a subfolder is not checked (ADR 0053).
pub(crate) fn applies(ctx: &RuleContext) -> Result<bool> {
    Ok(any_exists(ctx, &["go.mod"]) || !any_exists(ctx, &OTHER_MANIFESTS))
}

/// What every rule of the pack reads besides its own files, for the cache.
pub(crate) fn inputs(own: &[&str]) -> Vec<String> {
    let mut all: Vec<String> = own.iter().map(|s| (*s).to_owned()).collect();
    for shared in std::iter::once("go.mod").chain(OTHER_MANIFESTS) {
        if !all.iter().any(|a| a == shared) {
            all.push(shared.to_owned());
        }
    }
    all
}

/// The module path `go.mod` declares, if any.
#[must_use]
pub fn module_path(go_mod: &str) -> Option<&str> {
    go_mod.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("module")?;
        if !rest.starts_with([' ', '\t']) {
            return None;
        }
        let rest = rest.split("//").next().unwrap_or_default().trim();
        let path = rest.trim_matches(|c| c == '"' || c == '`');
        (!path.is_empty()).then_some(path)
    })
}

/// The name `go build` gives the root package's program: the last element
/// of the module path, without a major version suffix (`example.com/app/v2`
/// builds `app`), as `go build` names it.
#[must_use]
pub fn program_name(module: &str) -> &str {
    let mut parts = module.rsplit('/');
    let last = parts.next().unwrap_or(module);
    let is_major =
        last.len() > 1 && last.starts_with('v') && last[1..].bytes().all(|b| b.is_ascii_digit());
    if is_major {
        parts.next().unwrap_or(last)
    } else {
        last
    }
}

/// The module's Go files, as the go tool sees them: not in a folder whose
/// name starts with `.` or `_`, nor in `testdata` or `vendor`.
pub(crate) fn go_files(ctx: &RuleContext) -> Result<Vec<String>> {
    Ok(firescale_core::walk::walk_files(ctx.root())?
        .into_iter()
        .filter(|path| {
            let mut parts: Vec<&str> = path.split('/').collect();
            let Some(name) = parts.pop() else {
                return false;
            };
            name.ends_with(".go")
                && !name.starts_with(['.', '_'])
                && parts.iter().all(|folder| {
                    !folder.starts_with(['.', '_']) && !matches!(*folder, "testdata" | "vendor")
                })
        })
        .collect())
}

#[must_use]
pub fn pack() -> BuiltinPack {
    BuiltinPack {
        name: "go",
        rules: vec![
            Box::new(go_mod::GoMod),
            Box::new(entry::Entry),
            Box::new(test_files::Tests),
            Box::new(go_sum::GoSum),
            Box::new(gitignore::GoGitignore),
        ],
    }
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn module_paths_and_program_names() {
        assert_eq!(
            module_path("module example.com/app\n\ngo 1.26.0\n"),
            Some("example.com/app")
        );
        assert_eq!(
            module_path("module \"example.com/q\" // why\n"),
            Some("example.com/q")
        );
        assert_eq!(module_path("go 1.26.0\n"), None);
        assert_eq!(module_path("modules x\n"), None);
        assert_eq!(program_name("example.com/app"), "app");
        assert_eq!(program_name("github.com/acme/tool/v2"), "tool");
        assert_eq!(program_name("hello"), "hello");
        assert_eq!(program_name("example.com/v"), "v");
    }
}
