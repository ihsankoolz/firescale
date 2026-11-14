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

pub(crate) use crate::stackkit::{any_exists, create, finding, other_stack, other_stack_inputs};

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

/// True for a path the go tool looks at: not in a folder whose name starts
/// with `.` or `_`, nor in `testdata` or `vendor`.
fn go_sees(path: &str) -> bool {
    let mut parts: Vec<&str> = path.split('/').collect();
    parts.pop();
    parts
        .iter()
        .all(|folder| !folder.starts_with(['.', '_']) && !matches!(*folder, "testdata" | "vendor"))
}

/// True when the pack applies to the repo root: it has a `go.mod`, or it
/// has no Go module anywhere and no manifest of another stack (a new
/// module). A repo whose modules are in subfolders gets nothing written:
/// `go/go-mod` names them (ADR 0053, found on a real repo).
pub(crate) fn applies(ctx: &RuleContext) -> Result<bool> {
    if any_exists(ctx, &["go.mod"]) {
        return Ok(true);
    }
    Ok(!other_stack(ctx, &["go.mod"], &["go"])? && modules_below(ctx)?.is_empty())
}

/// The folders below the root that hold a `go.mod`, as the go tool sees
/// them (not in `testdata`, `vendor`, or folders starting with `.` or `_`).
pub(crate) fn modules_below(ctx: &RuleContext) -> Result<Vec<String>> {
    Ok(firescale_core::walk::walk_files(ctx.root())?
        .into_iter()
        .filter(|path| go_sees(path))
        .filter_map(|path| path.strip_suffix("/go.mod").map(str::to_owned))
        .collect())
}

/// True when a Go file has a build constraint (`//go:build …` or the old
/// `// +build …`) before its package clause: it is built only sometimes,
/// or never (`ignore`, a generator's `package main`; found in review).
#[must_use]
pub fn build_constrained(source: &str) -> bool {
    for line in source.trim_start_matches('\u{feff}').lines() {
        let line = line.trim();
        if line.starts_with("//go:build") || line.starts_with("// +build") {
            return true;
        }
        if line.starts_with("package") {
            return false;
        }
    }
    false
}

/// The package a Go file declares: its first line of code, after comments,
/// blank lines and build constraints. A `package main` in a doc comment's
/// example is not it (found on a real repo: gin's doc.go).
#[must_use]
pub fn package_name(source: &str) -> Option<&str> {
    let mut in_block = false;
    for line in source.trim_start_matches('\u{feff}').lines() {
        let mut line = line.trim();
        if in_block {
            let Some(end) = line.find("*/") else {
                continue;
            };
            in_block = false;
            line = line[end + 2..].trim();
        }
        while let Some(rest) = line.strip_prefix("/*") {
            match rest.find("*/") {
                Some(end) => line = rest[end + 2..].trim(),
                None => {
                    in_block = true;
                    line = "";
                }
            }
        }
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        return line
            .strip_prefix("package")
            .filter(|rest| rest.starts_with([' ', '\t']))
            .map(|rest| rest.split("//").next().unwrap_or_default().trim())
            .filter(|name| !name.is_empty());
    }
    None
}

/// What every rule of the pack reads besides its own files, for the cache.
pub(crate) fn inputs(own: &[&str]) -> Vec<String> {
    let mut all: Vec<String> = own.iter().map(|s| (*s).to_owned()).collect();
    for shared in ["go.mod".to_owned(), "**/go.mod".to_owned()]
        .into_iter()
        .chain(other_stack_inputs())
    {
        if !all.contains(&shared) {
            all.push(shared);
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
    // A folder with its own `go.mod` is another module (found in review:
    // `tools/go.mod`'s files counted as the root module's).
    let nested: Vec<String> = modules_below(ctx)?
        .into_iter()
        .map(|folder| format!("{folder}/"))
        .collect();
    Ok(firescale_core::walk::walk_files(ctx.root())?
        .into_iter()
        .filter(|path| {
            let name = path.rsplit('/').next().unwrap_or(path);
            name.ends_with(".go")
                && !name.starts_with(['.', '_'])
                && go_sees(path)
                && !nested
                    .iter()
                    .any(|folder| path.starts_with(folder.as_str()))
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

    #[test]
    fn the_package_clause_is_the_first_line_of_code() {
        assert_eq!(
            package_name("// Command app.\npackage main\n"),
            Some("main")
        );
        assert_eq!(
            package_name("//go:build linux\n\npackage calc // why\n"),
            Some("calc")
        );
        assert_eq!(
            package_name("/*\nPackage gin is a framework.\n\n\tpackage main\n*/\npackage gin\n"),
            Some("gin")
        );
        assert_eq!(package_name("/* one line */ package x\n"), Some("x"));
        assert_eq!(package_name("func f() {}\n"), None);
    }
}
