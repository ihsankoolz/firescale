//! `maven/properties`: the POM says how to read the sources (UTF-8) and
//! which Java version to build for. Without the first, Maven reads them in
//! the machine's own encoding and warns that "the build is platform
//! dependent"; without the second, the compiler plugin picks its own
//! default, which differs between plugin versions.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{applies, finding, xml, JAVA_RELEASE};

#[cfg(test)]
mod tests;

pub const ID: &str = "maven/properties";

pub struct Properties;

/// The element a POM without a source encoding gets.
pub const ENCODING: &str = "<project.build.sourceEncoding>UTF-8</project.build.sourceEncoding>";

/// True when the POM sets how to read sources: the project's property, or
/// the compiler plugin's own `<encoding>` (another plugin's does not count;
/// found in review).
#[must_use]
pub fn has_encoding(pom: &str) -> bool {
    let Some(elements) = xml::elements(pom) else {
        return false;
    };
    xml::find(&elements, "project/properties/project.build.sourceEncoding").is_some()
        || xml::plugin_settings(&elements, pom, "maven-compiler-plugin")
            .iter()
            .any(|name| name == "encoding")
}

/// True when the POM says which Java version to build for: a project
/// property, or the compiler plugin's own setting (an antrun `<target>` or
/// a javadoc `<source>` is not one; found in review).
#[must_use]
pub fn has_java_version(pom: &str) -> bool {
    let Some(elements) = xml::elements(pom) else {
        return false;
    };
    [
        "maven.compiler.release",
        "maven.compiler.source",
        "maven.compiler.target",
    ]
    .iter()
    .any(|name| xml::find(&elements, &format!("project/properties/{name}")).is_some())
        || xml::plugin_settings(&elements, pom, "maven-compiler-plugin")
            .iter()
            .any(|name| matches!(name.as_str(), "release" | "source" | "target"))
}

/// True when the POM has a `<parent>`, which may set both.
#[must_use]
pub fn has_parent(pom: &str) -> bool {
    xml::elements(pom).is_some_and(|elements| xml::find(&elements, "project/parent").is_some())
}

/// The white space before byte `at` on its line, when nothing else is
/// there: the element starts its line.
fn indent_before(text: &str, at: usize) -> Option<&str> {
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let indent = &text[line_start..at];
    indent
        .chars()
        .all(|c| c == ' ' || c == '\t')
        .then_some(indent)
}

/// The POM with the encoding added: as the first child of the project's
/// `<properties>`, or in a new `<properties>` block before the project's
/// `<dependencyManagement>`, `<dependencies>` or `<build>` (the first of
/// them in the file), else before `</project>`; indented one step deeper
/// than its parent, the step being the file's own (from where the
/// project's first child sits), in the file's line endings. `None` when the
/// POM cannot be read, or the project's `<properties>` closes itself.
#[must_use]
pub fn with_encoding(pom: &str) -> Option<String> {
    let elements = xml::elements(pom)?;
    let project = xml::find(&elements, "project")?;
    let eol = if pom.contains("\r\n") { "\r\n" } else { "\n" };
    let project_indent = indent_before(pom, project.start).unwrap_or("");
    // The file's step: how far the project's first child sits in.
    let step = elements
        .iter()
        .find(|e| e.path.len() == 2)
        .and_then(|child| indent_before(pom, child.start))
        .and_then(|indent| indent.strip_prefix(project_indent))
        .filter(|step| !step.is_empty())
        .unwrap_or("  ")
        .to_owned();
    if let Some(properties) = xml::find(&elements, "project/properties") {
        if properties.close_start == properties.open_end
            && properties.end == properties.open_end
            && pom[properties.start..properties.open_end].ends_with("/>")
        {
            return None;
        }
        let parent = indent_before(pom, properties.start)
            .map_or_else(|| format!("{project_indent}{step}"), str::to_owned);
        let at = properties.open_end;
        return Some(format!(
            "{}{eol}{parent}{step}{ENCODING}{}",
            &pom[..at],
            &pom[at..]
        ));
    }
    let anchor = elements
        .iter()
        .filter(|e| {
            matches!(
                e.at().as_str(),
                "project/dependencyManagement" | "project/dependencies" | "project/build"
            )
        })
        .min_by_key(|e| e.start);
    let child = format!("{project_indent}{step}");
    let block = format!("<properties>{eol}{child}{step}{ENCODING}{eol}{child}</properties>");
    match anchor {
        Some(anchor) => match indent_before(pom, anchor.start) {
            Some(indent) => Some(format!(
                "{}{block}{eol}{eol}{indent}{}",
                &pom[..anchor.start],
                &pom[anchor.start..]
            )),
            None => Some(format!(
                "{}{block}{}",
                &pom[..anchor.start],
                &pom[anchor.start..]
            )),
        },
        None => {
            let close = project.close_start;
            match indent_before(pom, close) {
                Some(indent) => {
                    let line_start = close - indent.len();
                    Some(format!(
                        "{}{eol}{child}{block}{eol}{}",
                        pom[..line_start].trim_end_matches([' ', '\t']),
                        &pom[line_start..]
                    ))
                }
                None => Some(format!("{}{block}{}", &pom[..close], &pom[close..])),
            }
        }
    }
}

impl BuiltinRule for Properties {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Consistency,
            description:
                "pom.xml sets project.build.sourceEncoding and the Java version to build for."
                    .to_owned(),
            inputs: super::inputs(&[]),
            has_fix: true,
            after: vec!["maven/pom".to_owned()],
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        if !applies(ctx)? {
            return Ok(Vec::new());
        }
        let Some(pom) = ctx.read("pom.xml")? else {
            return Ok(Vec::new());
        };
        // A parent may set both: what it sets is not read here. A POM this
        // cannot read is maven/pom's to report.
        if has_parent(&pom) || xml::elements(&pom).is_none() {
            return Ok(Vec::new());
        }
        let mut found = Vec::new();
        if !has_encoding(&pom) {
            let fixable = with_encoding(&pom).is_some();
            found.push(finding(
                "pom.xml",
                format!(
                    "pom.xml does not set project.build.sourceEncoding, so Maven reads the sources in each machine's own encoding and warns that the build is platform dependent. {}",
                    if fixable {
                        "`fscale sync` adds UTF-8."
                    } else {
                        "Add <project.build.sourceEncoding>UTF-8</project.build.sourceEncoding> to the project's <properties> (this POM's layout is not one `fscale sync` edits)."
                    }
                ),
                if fixable {
                    Fixable::Deterministic
                } else {
                    Fixable::Manual
                },
            ));
        }
        if !has_java_version(&pom) {
            found.push(finding(
                "pom.xml",
                format!("pom.xml does not say which Java version to build for, so the compiler plugin picks its own default, which changes between plugin versions. Set <maven.compiler.release> in <properties> to the version the project supports ({JAVA_RELEASE} for a new project)."),
                Fixable::Manual,
            ));
        }
        Ok(found)
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        if !applies(ctx)? {
            return Ok(result);
        }
        let Some(pom) = ctx.read("pom.xml")? else {
            return Ok(result);
        };
        if has_parent(&pom) || has_encoding(&pom) {
            return Ok(result);
        }
        if let Some(updated) = with_encoding(&pom) {
            ctx.write("pom.xml", &updated)?;
            result.changed_files.push("pom.xml".to_owned());
        }
        Ok(result)
    }
}
