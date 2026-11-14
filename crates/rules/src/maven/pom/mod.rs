//! `maven/pom`: the root has a `pom.xml` that names the project: its
//! `groupId`, `artifactId` and `version` (the first and last may come from a
//! `<parent>`), with `modelVersion` 4.0.0.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{
    any_exists, applies, create, finding, other_stack, poms_below, xml, JAVA_RELEASE, VERSIONS,
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

/// The coordinates the POM lacks, by element name: the project's own
/// children, read by their paths (a dependency's or a plugin's do not
/// count, nor a flatten plugin's `<parent>` setting; found in review).
/// `None` when the POM cannot be read.
#[must_use]
pub fn missing_coordinates(pom: &str) -> Option<Vec<&'static str>> {
    let elements = xml::elements(pom)?;
    let has = |name: &str| xml::find(&elements, &format!("project/{name}")).is_some();
    let has_parent = has("parent");
    let mut missing = Vec::new();
    if !has("modelVersion") {
        missing.push("modelVersion");
    }
    if !has("groupId") && !has_parent {
        missing.push("groupId");
    }
    if !has("artifactId") {
        missing.push("artifactId");
    }
    if !has("version") && !has_parent {
        missing.push("version");
    }
    Some(missing)
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
            // A Maven project in a subfolder of a repo that is no other
            // stack's: said once, nothing written. In a Gradle or Node repo
            // a pom.xml below is that repo's business (found on okhttp).
            if !any_exists(ctx, &["pom.xml"]) && !other_stack(ctx, &["pom.xml"], &["java"])? {
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
        let Some(missing) = missing_coordinates(&text) else {
            return Ok(vec![finding(
                "pom.xml",
                "pom.xml is not XML the pack can read (an unclosed element, comment or tag), so Maven will not read it either, and nothing in it is checked or changed. Run `mvn -B validate` to see where.".to_owned(),
                Fixable::Manual,
            )]);
        };
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
