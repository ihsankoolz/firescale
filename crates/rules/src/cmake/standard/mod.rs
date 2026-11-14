//! `cmake/standard`: a project with C++ code says which C++ standard it is
//! written in. Without one, each compiler builds with its own default
//! (C++17 for GCC 11 and later and for Clang 16 and later, C++14 for MSVC),
//! so code that builds on one machine can fail on another.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::syntax::{project_languages, Command};
use super::{all_commands, applies, code, finding, CXX_STANDARD};

#[cfg(test)]
mod tests;

pub const ID: &str = "cmake/standard";

pub struct Standard;

/// True when an argument of a command sets the C++ standard: the
/// `CMAKE_CXX_STANDARD` variable, a target's `CXX_STANDARD` property, a
/// `cxx_std_*` compile feature, or a `-std=c++…`, `-std=gnu++…` or
/// `/std:c++…` flag.
#[must_use]
pub fn sets_standard(command: &Command) -> bool {
    command.args.iter().any(|arg| {
        let lower = arg.to_ascii_lowercase();
        lower == "cmake_cxx_standard"
            || lower == "cxx_standard"
            || lower.starts_with("cxx_std_")
            || ["-std=c++", "-std=gnu++", "/std:c++", "-std:c++"]
                .iter()
                .any(|flag| lower.contains(flag))
    })
}

/// True when the build compiles C++: the root `project()` enables CXX (by
/// name or by default), or a CMake file calls `enable_language(CXX)`. C++
/// files a C project's build never compiles (zlib's `contrib/`, curl's
/// examples; found on real repos) need no standard.
fn builds_cxx(all: &[(String, Vec<Command>)]) -> bool {
    let root = all
        .iter()
        .find(|(path, _)| path == "CMakeLists.txt")
        .and_then(|(_, commands)| commands.iter().find(|command| command.name == "project"))
        .is_none_or(|project| project_languages(project).iter().any(|l| l == "CXX"));
    root || all.iter().any(|(_, commands)| {
        commands
            .iter()
            .any(|command| command.name == "enable_language" && command.has_arg("CXX"))
    })
}

/// True when a `CMakePresets.json` sets `CMAKE_CXX_STANDARD`.
fn presets_set_it(ctx: &RuleContext) -> Result<bool> {
    for file in ["CMakePresets.json", "CMakeUserPresets.json"] {
        if ctx
            .read(file)?
            .is_some_and(|text| text.contains("\"CMAKE_CXX_STANDARD\""))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

impl BuiltinRule for Standard {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Consistency,
            description: "A CMake project with C++ code sets the C++ standard it is written in."
                .to_owned(),
            inputs: super::inputs(&["CMakePresets.json", "CMakeUserPresets.json"]),
            has_fix: false,
            after: Vec::new(),
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        if !applies(ctx)? || !ctx.root().join("CMakeLists.txt").is_file() {
            return Ok(Vec::new());
        }
        let (files, cxx) = code(ctx)?;
        if !cxx || files.is_empty() {
            return Ok(Vec::new());
        }
        let all = all_commands(ctx)?;
        if !builds_cxx(&all) {
            return Ok(Vec::new());
        }
        let set = all
            .iter()
            .any(|(_, commands)| commands.iter().any(sets_standard));
        if set || presets_set_it(ctx)? {
            return Ok(Vec::new());
        }
        Ok(vec![finding(
            "CMakeLists.txt",
            format!(
                "The project has C++ code and its CMake files do not set a C++ standard, so each compiler uses its own default (C++17 for recent GCC and Clang, C++14 for MSVC) and code that builds on one machine can fail on another. Set the standard the code is written in after project(): `set(CMAKE_CXX_STANDARD <17, 20, …>)` and `set(CMAKE_CXX_STANDARD_REQUIRED ON)`, or `target_compile_features(<target> PUBLIC cxx_std_<n>)` (the starter uses {CXX_STANDARD})."
            ),
            Fixable::Manual,
        )])
    }

    fn fix(&self, _ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        Ok(FixResult::default())
    }
}
