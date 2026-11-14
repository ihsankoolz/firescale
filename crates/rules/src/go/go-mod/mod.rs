//! `go/go-mod`: the root has a `go.mod` that names its module and the Go
//! version it needs, as `go mod init` writes it.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{
    any_exists, applies, create, finding, module_path, modules_below, other_stack, GO_VERSION,
    MODULE,
};

#[cfg(test)]
mod tests;

pub const ID: &str = "go/go-mod";

pub struct GoMod;

/// The `go` line an existing `go.mod` without one gets: what the go tool
/// assumes when there is none ("If the go directive is missing, go 1.16 is
/// assumed", the Go module reference).
pub const EXISTING_GO_VERSION: &str = "1.16";

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
            // Modules in subfolders of a repo that is no other stack's:
            // said once, nothing written. In a Node or Java repo, a Go
            // module below is that repo's business (found on okhttp with
            // the maven pack).
            if !any_exists(ctx, &["go.mod"]) && !other_stack(ctx, &["go.mod"], &["go"])? {
                let below = modules_below(ctx)?;
                if !below.is_empty() {
                    return Ok(vec![finding(
                        &format!("{}/go.mod", below[0]),
                        format!(
                            "The go pack checks a Go module at the repo root, and this repo's modules are in {}. Nothing is checked or written; move the module to the root, or turn the pack off.",
                            below.join(", ")
                        ),
                        Fixable::Manual,
                    )]);
                }
            }
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
        if text.starts_with('\u{feff}') {
            found.push(finding(
                "go.mod",
                "go.mod starts with a byte order mark, which the go tool rejects (\"unexpected input character\"). Save it as UTF-8 without one.".to_owned(),
                Fixable::Manual,
            ));
            return Ok(found);
        }
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
                format!("go.mod has no go line, so the go tool assumes Go 1.16 and turns off newer language features. `fscale sync` writes `go {EXISTING_GO_VERSION}`, which keeps how the module builds today; raise it on purpose (`go mod tidy -go=1.26.0`) when the code is ready for the newer rules."),
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
        if has_go_line(&text) || text.starts_with('\u{feff}') {
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
        // What the go tool assumes today, never a newer version: moving an
        // existing module from 1.16 to a later `go` line prunes its module
        // graph and changes language rules, which can stop it building
        // (found in review).
        let mut go_line = if module.is_some() {
            format!("{eol}go {EXISTING_GO_VERSION}{eol}")
        } else {
            format!("go {EXISTING_GO_VERSION}{eol}")
        };
        if lines.get(at).is_some_and(|next| !next.trim().is_empty()) {
            go_line.push_str(eol);
        }
        lines.insert(at, go_line);
        ctx.write("go.mod", &lines.concat())?;
        result.changed_files.push("go.mod".to_owned());
        Ok(result)
    }
}
