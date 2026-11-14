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

use super::{applies, finding, without_comments, JAVA_RELEASE};

#[cfg(test)]
mod tests;

pub const ID: &str = "maven/properties";

pub struct Properties;

/// The line a POM without a source encoding gets.
pub const ENCODING: &str = "<project.build.sourceEncoding>UTF-8</project.build.sourceEncoding>";

/// True when the POM sets how to read sources: the property, or the
/// compiler plugin's `<encoding>`.
#[must_use]
pub fn has_encoding(pom: &str) -> bool {
    let pom = without_comments(pom);
    pom.contains("<project.build.sourceEncoding>") || pom.contains("<encoding>")
}

/// True when the POM says which Java version to build for: a property, or
/// the compiler plugin's own setting.
#[must_use]
pub fn has_java_version(pom: &str) -> bool {
    let pom = without_comments(pom);
    [
        "<maven.compiler.release>",
        "<maven.compiler.source>",
        "<maven.compiler.target>",
        "<release>",
        "<source>",
        "<target>",
    ]
    .iter()
    .any(|element| pom.contains(element))
}

/// Byte offsets of `needle` in `text` that are outside comments and outside
/// `<profiles>`: the project's own elements.
fn project_level(text: &str, needle: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(found) = text[from..].find(needle) {
        let at = from + found;
        let before = &text[..at];
        let in_comment = before.matches("<!--").count() > before.matches("-->").count();
        let in_profiles =
            before.matches("<profiles>").count() > before.matches("</profiles>").count();
        if !in_comment && !in_profiles {
            return Some(at);
        }
        from = at + needle.len();
    }
    None
}

/// The start of the line holding byte `at`, and that line's indentation.
fn line_of(text: &str, at: usize) -> (usize, &str) {
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let indent_end = text[start..]
        .find(|c: char| c != ' ' && c != '\t')
        .map_or(text.len(), |i| start + i);
    (start, &text[start..indent_end])
}

/// The POM with the encoding line added in the project's `<properties>`,
/// or in a new `<properties>` block before the first of
/// `<dependencyManagement>`, `<dependencies>` and `<build>` (else before
/// `</project>`), in the file's own indentation and line endings.
#[must_use]
pub fn with_encoding(pom: &str) -> Option<String> {
    let eol = if pom.contains("\r\n") { "\r\n" } else { "\n" };
    if let Some(at) = project_level(pom, "<properties>") {
        let (_, indent) = line_of(pom, at);
        let line_end = pom[at..].find('\n').map_or(pom.len(), |i| at + i + 1);
        // One step deeper than `<properties>`: the next line's, if deeper.
        let (_, next_indent) = line_of(pom, line_end.min(pom.len().saturating_sub(1)));
        let child = if next_indent.len() > indent.len() && next_indent.starts_with(indent) {
            next_indent.to_owned()
        } else {
            format!("{indent}  ")
        };
        if line_end >= pom.len() && !pom.ends_with('\n') {
            return None;
        }
        let mut out = String::with_capacity(pom.len() + 80);
        out.push_str(&pom[..line_end]);
        out.push_str(&format!("{child}{ENCODING}{eol}"));
        out.push_str(&pom[line_end..]);
        return Some(out);
    }
    // `<properties/>`: a second block would make the POM invalid.
    if project_level(pom, "<properties/>").is_some()
        || project_level(pom, "<properties />").is_some()
    {
        return None;
    }
    let anchor = [
        "<dependencyManagement>",
        "<dependencies>",
        "<build>",
        "</project>",
    ]
    .iter()
    .find_map(|element| project_level(pom, element).map(|at| (at, *element)))?;
    let (start, indent) = line_of(pom, anchor.0);
    let indent = if anchor.1 == "</project>" {
        project_level(pom, "<modelVersion>").map_or("  ", |at| line_of(pom, at).1)
    } else {
        indent
    };
    // The element must start its line, as it does in any laid-out POM.
    if !pom[start..anchor.0].trim().is_empty() {
        return None;
    }
    let block = format!(
        "{indent}<properties>{eol}{indent}  {ENCODING}{eol}{indent}</properties>{eol}{eol}"
    );
    let block = if anchor.1 == "</project>" {
        format!(
            "{eol}{}",
            block.trim_end_matches(['\r', '\n']).to_owned() + eol
        )
    } else {
        block
    };
    Some(format!("{}{block}{}", &pom[..start], &pom[start..]))
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
        // A parent may set both: what it sets is not read here.
        if without_comments(&pom).contains("<parent>") {
            return Ok(Vec::new());
        }
        let mut found = Vec::new();
        if !has_encoding(&pom) {
            found.push(finding(
                "pom.xml",
                "pom.xml does not set project.build.sourceEncoding, so Maven reads the sources in each machine's own encoding and warns that the build is platform dependent. `fscale sync` adds UTF-8.".to_owned(),
                if with_encoding(&pom).is_some() {
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
        if without_comments(&pom).contains("<parent>") || has_encoding(&pom) {
            return Ok(result);
        }
        if let Some(updated) = with_encoding(&pom) {
            ctx.write("pom.xml", &updated)?;
            result.changed_files.push("pom.xml".to_owned());
        }
        Ok(result)
    }
}
