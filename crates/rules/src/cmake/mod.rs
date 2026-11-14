//! The `cmake` stack pack (ADR 0055): rules whose fixes create and repair
//! what a C or C++ project built with CMake needs, so `fscale init --packs
//! core,cmake` builds a working starter that `ctest` passes. Opt-in: add
//! "cmake" to `packs`.
//!
//! What the pack assumes was read on 2026-11-14 from CMake's documentation
//! and checked by running CMake 3.28.3 and 4.4.4 with GCC 13, Clang 18 and
//! clang-format 18 on the starter and on small projects.
//!
//! A fix only creates a file that is missing, or adds a missing line. It
//! never replaces a file a person wrote.

use firescale_core::rule::{BuiltinPack, RuleContext};
use firescale_core::Result;

pub(crate) use crate::stackkit::{any_exists, create, finding, other_stack, other_stack_inputs};

pub mod entry;
pub mod lists;
#[path = "minimum-version/mod.rs"]
pub mod minimum_version;
pub mod standard;
pub mod syntax;
#[path = "tests/mod.rs"]
pub mod test_files;

#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod pack_tests;

/// The oldest CMake the starter needs: 3.20 brought `ctest --test-dir`,
/// which `fscale verify` runs (ADR 0051).
pub const STARTER_MINIMUM: &str = "3.20";

/// The C++ standard a new project is built with.
pub const CXX_STANDARD: &str = "20";

/// C and C++ source and header extensions.
pub const C_CODE: [&str; 13] = [
    "c", "h", "cc", "cpp", "cxx", "c++", "hh", "hpp", "hxx", "h++", "ipp", "ixx", "cppm",
];

/// The C++ ones: a `.h` header may be C.
pub const CXX_CODE: [&str; 11] = [
    "cc", "cpp", "cxx", "c++", "hh", "hpp", "hxx", "h++", "ipp", "ixx", "cppm",
];

/// Root files of C and C++ builds other than CMake: a repo with one of
/// these is built another way and gets no starter.
const OTHER_C_BUILDS: [&str; 10] = [
    "Makefile",
    "makefile",
    "GNUmakefile",
    "configure.ac",
    "configure",
    "SConstruct",
    "xmake.lua",
    "premake5.lua",
    "BUILD",
    "meson_options.txt",
];

/// True for a folder a CMake build writes into: `build`, CLion's
/// `cmake-build-*`, Visual Studio's `out`.
fn build_folder(folder: &str) -> bool {
    folder == "build" || folder == "out" || folder.starts_with("cmake-build-")
}

/// True for a path the pack reads: not in a build folder, nor in a folder
/// no build looks in (`stackkit::set_aside`).
pub(crate) fn read_path(path: &str) -> bool {
    !crate::stackkit::set_aside(path)
        && !path
            .rsplit_once('/')
            .is_some_and(|(folders, _)| folders.split('/').any(build_folder))
}

/// The extension of a path.
fn extension(path: &str) -> &str {
    path.rsplit_once('/')
        .map_or(path, |(_, name)| name)
        .rsplit_once('.')
        .map_or("", |(_, extension)| extension)
}

/// The C or C++ files the pack reads, and whether any is C++.
pub(crate) fn code(ctx: &RuleContext) -> Result<(Vec<String>, bool)> {
    let files: Vec<String> = firescale_core::walk::walk_files(ctx.root())?
        .into_iter()
        .filter(|path| read_path(path) && C_CODE.contains(&extension(path)))
        .collect();
    let cxx = files.iter().any(|path| CXX_CODE.contains(&extension(path)));
    Ok((files, cxx))
}

/// The CMake files the pack reads: every `CMakeLists.txt` and `*.cmake`.
/// The root `CMakeLists.txt` first, then in path order.
pub(crate) fn cmake_files(ctx: &RuleContext) -> Result<Vec<String>> {
    let mut files: Vec<String> = firescale_core::walk::walk_files(ctx.root())?
        .into_iter()
        .filter(|path| {
            read_path(path)
                && (path == "CMakeLists.txt"
                    || path.ends_with("/CMakeLists.txt")
                    || extension(path) == "cmake")
        })
        .collect();
    files.sort_by_key(|path| path != "CMakeLists.txt");
    Ok(files)
}

/// The commands of every CMake file the pack reads, by file. A file the
/// reader cannot follow is left out (`cmake/lists` names the root one).
pub(crate) fn all_commands(ctx: &RuleContext) -> Result<Vec<(String, Vec<syntax::Command>)>> {
    let mut out = Vec::new();
    for path in cmake_files(ctx)? {
        if let Some(commands) = ctx.read(&path)?.and_then(|text| syntax::commands(&text)) {
            out.push((path, commands));
        }
    }
    Ok(out)
}

/// The folders below the root that hold a `CMakeLists.txt`.
pub(crate) fn lists_below(ctx: &RuleContext) -> Result<Vec<String>> {
    Ok(cmake_files(ctx)?
        .into_iter()
        .filter_map(|path| path.strip_suffix("/CMakeLists.txt").map(str::to_owned))
        .collect())
}

/// True when the root has a C or C++ build other than CMake, or a Visual
/// Studio solution or project.
fn other_c_build(ctx: &RuleContext) -> Result<bool> {
    if any_exists(ctx, &OTHER_C_BUILDS) {
        return Ok(true);
    }
    let Ok(entries) = std::fs::read_dir(ctx.root()) else {
        return Ok(false);
    };
    Ok(entries.flatten().any(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        matches!(extension(&name), "sln" | "slnx" | "vcxproj")
    }))
}

/// True when the pack applies to the repo root: it has a `CMakeLists.txt`,
/// or it is new to C and C++ (no code at all, no other stack's manifest, no
/// other C build, and no `CMakeLists.txt` in a subfolder). A repo whose
/// code is built another way is never given a `CMakeLists.txt`.
pub(crate) fn applies(ctx: &RuleContext) -> Result<bool> {
    if any_exists(ctx, &["CMakeLists.txt"]) {
        return Ok(true);
    }
    Ok(!other_stack(ctx, &["CMakeLists.txt"], &[])?
        && !other_c_build(ctx)?
        && code(ctx)?.0.is_empty()
        && lists_below(ctx)?.is_empty())
}

/// True when a CMake project is in a subfolder of a repo that is no other
/// stack's and has no other C build at its root: `cmake/lists` says so
/// once.
pub(crate) fn only_below(ctx: &RuleContext) -> Result<Option<Vec<String>>> {
    if any_exists(ctx, &["CMakeLists.txt"])
        || other_stack(ctx, &["CMakeLists.txt"], &C_CODE)?
        || other_c_build(ctx)?
    {
        return Ok(None);
    }
    let below = lists_below(ctx)?;
    Ok((!below.is_empty()).then_some(below))
}

/// What every rule of the pack reads besides its own files, for the cache.
pub(crate) fn inputs(own: &[&str]) -> Vec<String> {
    let mut all: Vec<String> = own.iter().map(|s| (*s).to_owned()).collect();
    let shared = ["CMakeLists.txt", "**/CMakeLists.txt", "**/*.cmake"]
        .iter()
        .map(|s| (*s).to_owned())
        .chain(OTHER_C_BUILDS.iter().map(|s| (*s).to_owned()))
        .chain([
            "*.sln".to_owned(),
            "*.slnx".to_owned(),
            "*.vcxproj".to_owned(),
        ])
        .chain(C_CODE.iter().map(|extension| format!("**/*.{extension}")))
        .chain(other_stack_inputs());
    for input in shared {
        if !all.contains(&input) {
            all.push(input);
        }
    }
    all
}

/// The line ending a file uses.
pub(crate) fn line_ending(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

#[must_use]
pub fn pack() -> BuiltinPack {
    BuiltinPack {
        name: "cmake",
        rules: vec![
            Box::new(lists::Lists),
            Box::new(minimum_version::MinimumVersion),
            Box::new(standard::Standard),
            Box::new(entry::Entry),
            Box::new(test_files::Tests),
        ],
    }
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn build_folders_and_extensions() {
        assert!(read_path("src/a.cpp"));
        assert!(read_path("CMakeLists.txt"));
        assert!(!read_path("build/_deps/x/CMakeLists.txt"));
        assert!(!read_path("cmake-build-debug/x.cmake"));
        assert!(!read_path("out/build/x64/CMakeLists.txt"));
        assert!(!read_path(".cache/x.cpp"));
        assert!(read_path("src/build.cpp"));
        assert_eq!(extension("a/b.c++"), "c++");
        assert_eq!(extension("a.b/c"), "");
    }
}
