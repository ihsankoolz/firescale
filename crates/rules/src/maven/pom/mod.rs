//! `maven/pom`: the root has a `pom.xml` that names the project: its
//! `groupId`, `artifactId` and `version` (the first and last may come from a
//! `<parent>`), with `modelVersion` 4.0.0.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{
    any_exists, applies, create, finding, poms_below, without_comments, JAVA_RELEASE, VERSIONS,
};

#[cfg(test)]
mod tests;

pub const ID: &str = "maven/pom";

pub struct Pom;

/// The `pom.xml` of a new project, as run with Maven 3.9.11 and JDK 21.
#[must_use]
pub fn new_pom() -> String {
    let version = |name: &str| {
        VERSIONS
            .iter()
            .find(|(artifact, _)| *artifact == name)
            .map_or("", |(_, version)| *version)
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<project xmlns="http://maven.apache.org/POM/4.0.0"
         xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
         xsi:schemaLocation="http://maven.apache.org/POM/4.0.0 https://maven.apache.org/xsd/maven-4.0.0.xsd">
  <modelVersion>4.0.0</modelVersion>

  <groupId>com.example</groupId>
  <artifactId>app</artifactId>
  <version>0.1.0-SNAPSHOT</version>

  <properties>
    <maven.compiler.release>{JAVA_RELEASE}</maven.compiler.release>
    <project.build.sourceEncoding>UTF-8</project.build.sourceEncoding>
  </properties>

  <dependencyManagement>
    <dependencies>
      <dependency>
        <groupId>org.junit</groupId>
        <artifactId>junit-bom</artifactId>
        <version>{junit}</version>
        <type>pom</type>
        <scope>import</scope>
      </dependency>
    </dependencies>
  </dependencyManagement>

  <dependencies>
    <dependency>
      <groupId>org.junit.jupiter</groupId>
      <artifactId>junit-jupiter</artifactId>
      <scope>test</scope>
    </dependency>
  </dependencies>

  <build>
    <plugins>
      <plugin>
        <groupId>org.apache.maven.plugins</groupId>
        <artifactId>maven-compiler-plugin</artifactId>
        <version>{compiler}</version>
      </plugin>
      <plugin>
        <groupId>org.apache.maven.plugins</groupId>
        <artifactId>maven-surefire-plugin</artifactId>
        <version>{surefire}</version>
      </plugin>
      <plugin>
        <groupId>org.apache.maven.plugins</groupId>
        <artifactId>maven-checkstyle-plugin</artifactId>
        <version>{checkstyle_plugin}</version>
        <configuration>
          <configLocation>google_checks.xml</configLocation>
          <consoleOutput>true</consoleOutput>
          <violationSeverity>warning</violationSeverity>
          <includeTestSourceDirectory>true</includeTestSourceDirectory>
        </configuration>
        <dependencies>
          <dependency>
            <groupId>com.puppycrawl.tools</groupId>
            <artifactId>checkstyle</artifactId>
            <version>{checkstyle}</version>
          </dependency>
        </dependencies>
      </plugin>
    </plugins>
  </build>
</project>
"#,
        junit = version("junit-bom"),
        compiler = version("maven-compiler-plugin"),
        surefire = version("maven-surefire-plugin"),
        checkstyle_plugin = version("maven-checkstyle-plugin"),
        checkstyle = version("checkstyle"),
    )
}

/// Blocks whose elements are not the project's own: a dependency's
/// `artifactId` is not the project's.
const NESTED: [&str; 12] = [
    "parent",
    "dependencies",
    "dependencyManagement",
    "build",
    "profiles",
    "reporting",
    "distributionManagement",
    "repositories",
    "pluginRepositories",
    "modules",
    "properties",
    "developers",
];

/// The POM with comments and nested blocks taken out: what is left are the
/// project's own elements.
#[must_use]
pub fn own_elements(pom: &str) -> String {
    let mut text = without_comments(pom);
    for name in NESTED {
        let open = format!("<{name}>");
        let close = format!("</{name}>");
        while let Some(start) = text.find(&open) {
            let Some(end) = text[start..].find(&close) else {
                break;
            };
            text.replace_range(start..start + end + close.len(), "");
        }
    }
    text
}

/// The coordinates the POM lacks, by element name.
#[must_use]
pub fn missing_coordinates(pom: &str) -> Vec<&'static str> {
    let has_parent = without_comments(pom).contains("<parent>");
    let own = own_elements(pom);
    let mut missing = Vec::new();
    if !own.contains("<modelVersion>") {
        missing.push("modelVersion");
    }
    if !own.contains("<groupId>") && !has_parent {
        missing.push("groupId");
    }
    if !own.contains("<artifactId>") {
        missing.push("artifactId");
    }
    if !own.contains("<version>") && !has_parent {
        missing.push("version");
    }
    missing
}

impl BuiltinRule for Pom {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::High,
            category: Category::Consistency,
            description:
                "The root has a pom.xml with modelVersion, groupId, artifactId and version."
                    .to_owned(),
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
            // A Maven project in a subfolder: said once, nothing written.
            if !any_exists(ctx, &["pom.xml"]) {
                let below = poms_below(ctx)?;
                if !below.is_empty() {
                    return Ok(vec![finding(
                        &format!("{}/pom.xml", below[0]),
                        format!(
                            "The maven pack checks a Maven project at the repo root, and this repo's pom.xml files are in {}. Nothing is checked or written; move the project to the root, or turn the pack off.",
                            below.join(", ")
                        ),
                        Fixable::Manual,
                    )]);
                }
            }
            return Ok(Vec::new());
        }
        let Some(text) = ctx.read("pom.xml")? else {
            return Ok(vec![finding(
                "pom.xml",
                format!("There is no pom.xml, so Maven builds nothing. `fscale sync` creates one for com.example:app, built for Java {JAVA_RELEASE} with JUnit tests; change the groupId and artifactId to the project's."),
                Fixable::Deterministic,
            )]);
        };
        let missing = missing_coordinates(&text);
        if missing.is_empty() {
            return Ok(Vec::new());
        }
        Ok(vec![finding(
            "pom.xml",
            format!(
                "pom.xml has no {}: Maven stops before building (\"'{}' is missing\"). Add {} to the <project>.",
                missing.join(", "),
                missing[0],
                if missing.len() == 1 { "it" } else { "them" }
            ),
            Fixable::Manual,
        )])
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        if applies(ctx)? {
            create(ctx, "pom.xml", &new_pom(), &mut result)?;
        }
        Ok(result)
    }
}
