//! `maven/entry`: the project has Java code. A new project gets a class
//! with a `main` method and a JUnit test of it, so `mvn -B test` has
//! something to compile and run.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{aggregates, applies, create, finding, java_files};

#[cfg(test)]
mod tests;

pub const ID: &str = "maven/entry";

pub struct Entry;

/// Where a new project's code goes, from [`super::PACKAGE`].
pub const APP_PATH: &str = "src/main/java/com/example/app/App.java";
pub const TEST_PATH: &str = "src/test/java/com/example/app/AppTest.java";

/// The class of a new project, in Google's Java style, which the
/// starter's Checkstyle checks.
pub const APP: &str = "package com.example.app;

/** The starting point of this project. */
public final class App {
  private App() {}

  /** Returns the message the program prints. */
  static String greeting(String name) {
    return \"Hello, \" + name + \"!\";
  }

  /** Prints the greeting. */
  public static void main(String[] args) {
    System.out.println(greeting(\"world\"));
  }
}
";

/// Its test.
pub const APP_TEST: &str = "package com.example.app;

import static org.junit.jupiter.api.Assertions.assertEquals;

import org.junit.jupiter.api.Test;

class AppTest {
  @Test
  void greetsByName() {
    assertEquals(\"Hello, world!\", App.greeting(\"world\"));
  }
}
";

/// True when the project should have code of its own and has none: not an
/// aggregator POM, and no Java file under `src/main` or `src/test`.
fn has_no_code(ctx: &RuleContext) -> Result<bool> {
    if !applies(ctx)? {
        return Ok(false);
    }
    if ctx.read("pom.xml")?.is_some_and(|pom| aggregates(&pom)) {
        return Ok(false);
    }
    let (main, test) = java_files(ctx)?;
    Ok(main.is_empty() && test.is_empty())
}

impl BuiltinRule for Entry {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::High,
            category: Category::Consistency,
            description: "The project has Java code; a new one gets App.java and AppTest.java."
                .to_owned(),
            inputs: super::inputs(&["src/**/*.java"]),
            has_fix: true,
            after: vec!["maven/pom".to_owned()],
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        if !has_no_code(ctx)? {
            return Ok(Vec::new());
        }
        Ok(vec![finding(
            APP_PATH,
            "The project has no Java code under src/main/java or src/test/java, so `mvn -B test` compiles and runs nothing. `fscale sync` writes a class with a main method (App.java) and its JUnit test (AppTest.java).".to_owned(),
            Fixable::Deterministic,
        )])
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        if has_no_code(ctx)? {
            create(ctx, APP_PATH, APP, &mut result)?;
            create(ctx, TEST_PATH, APP_TEST, &mut result)?;
        }
        Ok(result)
    }
}
