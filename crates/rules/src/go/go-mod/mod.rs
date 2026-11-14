//! `go/go-mod`: the root has a `go.mod` that names its module and the Go
//! version it needs, as `go mod init` writes it.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{applies, create, finding, module_path, GO_VERSION, MODULE};

#[cfg(test)]
mod tests;

pub const ID: &str = "go/go-mod";

pub struct GoMod;

/// The `go.mod` of a new module.
#[must_use]
pub fn new_go_mod() -> String {
    format!("module {MODULE}\n\ngo {GO_VERSION}\n")
}

/// True when `go.mod` has a `go` directive.
fn has_go_line(text: &str) -> bool {
    text.lines().any(|line| {
        line.trim()
            .strip_prefix("go")
            .is_some_and(|rest| rest.starts_with([' ', '\t']))
    })
}

impl BuiltinRule for GoMod {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::High,
            category: Category::Consistency,
            description: "The root has a go.mod with a module path and a go line.".to_owned(),
            inputs: super::inputs(&[]),
            has_fix: true,
            after: Vec::new(),
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        if !applies(ctx)? {
            return Ok(Vec::new());
        }
        let Some(text) = ctx.read("go.mod")? else {
            return Ok(vec![finding(
                "go.mod",
                format!("There is no go.mod: the go tool builds nothing outside a module. `fscale sync` creates one for the module {MODULE} with `go {GO_VERSION}`; change the path to where the code will live."),
                Fixable::Deterministic,
            )]);
        };
        let mut found = Vec::new();
        if module_path(&text).is_none() {
            found.push(finding(
                "go.mod",
                "go.mod has no module line: add `module <path>`, the import path of the module (for example github.com/you/project).".to_owned(),
                Fixable::Manual,
            ));
        }
        if !has_go_line(&text) {
            found.push(finding(
                "go.mod",
                format!("go.mod has no go line: the go tool then assumes Go 1.16 and turns off newer language features. `fscale sync` adds `go {GO_VERSION}`."),
                Fixable::Deterministic,
            ));
        }
        Ok(found)
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        if !applies(ctx)? {
            return Ok(result);
        }
        let Some(text) = ctx.read("go.mod")? else {
            create(ctx, "go.mod", &new_go_mod(), &mut result)?;
            return Ok(result);
        };
        if has_go_line(&text) {
            return Ok(result);
        }
        let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
        // After the module line and a blank line, as `go mod init` places
        // it; with no module line, at the end.
        let mut lines: Vec<String> = text.split_inclusive('\n').map(str::to_owned).collect();
        let module = lines.iter().position(|line| module_path(line).is_some());
        let at = module.map_or(lines.len(), |index| index + 1);
        if let Some(before) = at.checked_sub(1).and_then(|index| lines.get_mut(index)) {
            if !before.ends_with('\n') {
                before.push_str(eol);
            }
        }
        let go_line = if module.is_some() {
            format!("{eol}go {GO_VERSION}{eol}")
        } else {
            format!("go {GO_VERSION}{eol}")
        };
        lines.insert(at, go_line);
        ctx.write("go.mod", &lines.concat())?;
        result.changed_files.push("go.mod".to_owned());
        Ok(result)
    }
}
