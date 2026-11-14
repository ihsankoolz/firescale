//! `go/entry`: the module has Go code. A new module gets a `main` package
//! with one test, so `go build`, `go vet` and `go test` have something to
//! build and run.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{applies, create, finding, go_files};

#[cfg(test)]
mod tests;

pub const ID: &str = "go/entry";

pub struct Entry;

/// The program of a new module, formatted as `gofmt` writes it.
pub const MAIN: &str = "// Command app is the starting point of this module.
package main

import \"fmt\"

// Greeting returns the message the program prints.
func Greeting(name string) string {
\treturn fmt.Sprintf(\"Hello, %s!\", name)
}

func main() {
\tfmt.Println(Greeting(\"world\"))
}
";

/// Its test.
pub const MAIN_TEST: &str = "package main

import \"testing\"

func TestGreeting(t *testing.T) {
\tif got, want := Greeting(\"world\"), \"Hello, world!\"; got != want {
\t\tt.Errorf(\"Greeting(%q) = %q, want %q\", \"world\", got, want)
\t}
}
";

impl BuiltinRule for Entry {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::High,
            category: Category::Consistency,
            description: "The module has Go code; a new one gets main.go and main_test.go."
                .to_owned(),
            inputs: super::inputs(&["**/*.go"]),
            has_fix: true,
            after: vec!["go/go-mod".to_owned()],
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        if !applies(ctx)? || !go_files(ctx)?.is_empty() {
            return Ok(Vec::new());
        }
        Ok(vec![finding(
            "main.go",
            "The module has no Go code, so `go build` and `go test` have nothing to build. `fscale sync` writes a main package (main.go) and its test (main_test.go).".to_owned(),
            Fixable::Deterministic,
        )])
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        if !applies(ctx)? || !go_files(ctx)?.is_empty() {
            return Ok(result);
        }
        create(ctx, "main.go", MAIN, &mut result)?;
        create(ctx, "main_test.go", MAIN_TEST, &mut result)?;
        Ok(result)
    }
}
