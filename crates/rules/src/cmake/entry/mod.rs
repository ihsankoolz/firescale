//! `cmake/entry`: the starter's `CMakeLists.txt` has the code it builds. A
//! new project gets a small library, a program using it and a test of it,
//! in Google's C++ style, with the `.clang-format` its `lint` target checks.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{applies, code, create, finding, syntax};

#[cfg(test)]
mod tests;

pub const ID: &str = "cmake/entry";

pub struct Entry;

/// The files of a new project, by path.
pub const FILES: [(&str, &str); 5] = [
    (".clang-format", "---\nBasedOnStyle: Google\n"),
    (
        "src/greeting.hpp",
        "#pragma once

#include <string>
#include <string_view>

namespace app {

// Returns the message the program prints.
std::string greeting(std::string_view name);

}  // namespace app
",
    ),
    (
        "src/greeting.cpp",
        "#include \"greeting.hpp\"

#include <string>
#include <string_view>

namespace app {

std::string greeting(std::string_view name) {
  return \"Hello, \" + std::string(name) + \"!\";
}

}  // namespace app
",
    ),
    (
        "src/main.cpp",
        "#include <iostream>

#include \"greeting.hpp\"

int main() {
  std::cout << app::greeting(\"world\") << '\\n';
  return 0;
}
",
    ),
    (
        "tests/greeting_test.cpp",
        "#include \"greeting.hpp\"

#include <cstdlib>
#include <iostream>

int main() {
  const auto got = app::greeting(\"world\");
  if (got != \"Hello, world!\") {
    std::cerr << \"greeting(\\\"world\\\") was \\\"\" << got << \"\\\"\\n\";
    return EXIT_FAILURE;
  }
  return EXIT_SUCCESS;
}
",
    ),
];

/// True when the root `CMakeLists.txt` builds the starter's program
/// (`add_executable(… src/main.cpp)`) and the repo has no C or C++ code:
/// a `CMakeLists.txt` a person wrote without code (a superbuild) is theirs.
fn wants_starter_code(ctx: &RuleContext) -> Result<bool> {
    if !applies(ctx)? || !code(ctx)?.0.is_empty() {
        return Ok(false);
    }
    let Some(commands) = ctx
        .read("CMakeLists.txt")?
        .and_then(|text| syntax::commands(&text))
    else {
        return Ok(false);
    };
    Ok(commands
        .iter()
        .any(|command| command.name == "add_executable" && command.has_arg("src/main.cpp")))
}

impl BuiltinRule for Entry {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::High,
            category: Category::Consistency,
            description:
                "The starter's CMakeLists.txt has its code: src/ and tests/, and .clang-format."
                    .to_owned(),
            inputs: super::inputs(&[".clang-format"]),
            has_fix: true,
            after: vec!["cmake/lists".to_owned()],
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        if !wants_starter_code(ctx)? {
            return Ok(Vec::new());
        }
        Ok(vec![finding(
            "src/main.cpp",
            "CMakeLists.txt builds src/main.cpp and the repo has no C or C++ code, so CMake stops (\"Cannot find source file\"). `fscale sync` writes the starter's code: a greeting library (src/greeting.hpp, src/greeting.cpp), the program (src/main.cpp), its test (tests/greeting_test.cpp) and the .clang-format the lint target checks.".to_owned(),
            Fixable::Deterministic,
        )])
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        if wants_starter_code(ctx)? {
            for (path, content) in FILES {
                create(ctx, path, content, &mut result)?;
            }
        }
        Ok(result)
    }
}
