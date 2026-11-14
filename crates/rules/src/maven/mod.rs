//! The `maven` stack pack (ADR 0054): rules whose fixes create and repair
//! what a Java project built with Maven needs, so `fscale init --packs
//! core,maven` builds a working starter that `mvn -B test` passes. Opt-in:
//! add "maven" to `packs`.
//!
//! What the pack assumes was read on 2026-11-14 from Maven Central's
//! metadata, Maven's and JUnit's documentation, and Maven 3.9.11 with JDK 21
//! running the starter; the versions are in [`VERSIONS`].
//!
//! A fix only creates a file that is missing, or adds missing lines. It
//! never replaces a file a person wrote. The Maven wrapper is not written:
//! its script is Maven's own code, and the skill names the command that adds
//! it.

use firescale_core::rule::{BuiltinPack, RuleContext};
use firescale_core::Result;

pub(crate) use crate::stackkit::{
    any_exists, create, finding, other_stack, other_stack_inputs, set_aside,
};

pub mod entry;
pub mod pom;
pub mod properties;
#[path = "tests/mod.rs"]
pub mod test_files;
pub mod xml;

#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod pack_tests;

/// The versions a new project gets, newest stable on Maven Central on
/// 2026-11-14: JUnit's bill of materials (JUnit 6, which needs Java 17),
/// the compiler and Surefire plugins (Maven 3.9 binds older ones by
/// default), and Checkstyle with its plugin, for `fscale verify`'s lint
/// step (Checkstyle 14 runs on Java 21).
pub const VERSIONS: [(&str, &str); 5] = [
    ("junit-bom", "6.1.3"),
    ("maven-compiler-plugin", "3.16.0"),
    ("maven-surefire-plugin", "3.6.1"),
    ("maven-checkstyle-plugin", "3.6.0"),
    ("checkstyle", "14.4.0"),
];

/// The Java version a new project is built for: 21, a long-term-support
/// release that JUnit 6 supports and every newer JDK builds for.
pub const JAVA_RELEASE: &str = "21";

/// The package of a new project's code, from its `groupId` and `artifactId`.
pub const PACKAGE: &str = "com.example.app";

/// True when the pack applies to the repo root: it has a `pom.xml`, or it is
/// new to Java (no other stack's manifest or code, and no `pom.xml` in a
/// subfolder). A repo whose Maven project is in a subfolder gets nothing
/// written: `maven/pom` names it.
pub(crate) fn applies(ctx: &RuleContext) -> Result<bool> {
    if any_exists(ctx, &["pom.xml"]) {
        return Ok(true);
    }
    Ok(!other_stack(ctx, &["pom.xml"], &JVM_CODE)? && poms_below(ctx)?.is_empty())
}

/// The folders below the root that hold a `pom.xml`, outside folders no
/// build looks in.
pub(crate) fn poms_below(ctx: &RuleContext) -> Result<Vec<String>> {
    Ok(firescale_core::walk::walk_files(ctx.root())?
        .into_iter()
        .filter(|path| !set_aside(path) && !path.split('/').any(|part| part == "target"))
        .filter_map(|path| path.strip_suffix("/pom.xml").map(str::to_owned))
        .collect())
}

/// True when the root POM only aggregates or parents other projects
/// (`<packaging>pom</packaging>`, or `<modules>`): it has no code of its
/// own, so no starter code is written beside it.
#[must_use]
pub fn aggregates(pom: &str) -> bool {
    xml::elements(pom).is_some_and(|elements| {
        xml::find(&elements, "project/modules").is_some()
            || xml::find(&elements, "project/packaging").is_some_and(|p| p.text(pom) == "pom")
    })
}

/// Source extensions Maven builds for the JVM: Java, and Kotlin, Scala or
/// Groovy through their plugins. A project in any of them has code (found
/// in review: a Kotlin project was given a Java starter).
const JVM_CODE: [&str; 5] = ["java", "kt", "kts", "scala", "groovy"];

/// The project's JVM source files, by where Maven reads them: under
/// `src/main` and under `src/test`, not `target`.
pub(crate) fn java_files(ctx: &RuleContext) -> Result<(Vec<String>, Vec<String>)> {
    let mut main = Vec::new();
    let mut test = Vec::new();
    for path in firescale_core::walk::walk_files(ctx.root())? {
        let jvm = std::path::Path::new(&path)
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| JVM_CODE.contains(&extension));
        if !jvm {
            continue;
        }
        if path.starts_with("src/test/") {
            test.push(path);
        } else if path.starts_with("src/main/") {
            main.push(path);
        }
    }
    Ok((main, test))
}

/// What every rule of the pack reads besides its own files, for the cache.
pub(crate) fn inputs(own: &[&str]) -> Vec<String> {
    let mut all: Vec<String> = own.iter().map(|s| (*s).to_owned()).collect();
    for shared in ["pom.xml".to_owned(), "**/pom.xml".to_owned()]
        .into_iter()
        .chain(other_stack_inputs())
    {
        if !all.contains(&shared) {
            all.push(shared);
        }
    }
    all
}

#[must_use]
pub fn pack() -> BuiltinPack {
    BuiltinPack {
        name: "maven",
        rules: vec![
            Box::new(pom::Pom),
            Box::new(properties::Properties),
            Box::new(entry::Entry),
            Box::new(test_files::Tests),
        ],
    }
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn aggregators_and_comments() {
        assert!(aggregates("<project><packaging>pom</packaging></project>"));
        assert!(aggregates(
            "<project>\n  <packaging>\n    pom\n  </packaging>\n</project>"
        ));
        assert!(aggregates(
            "<project><modules><module>a</module></modules></project>"
        ));
        assert!(!aggregates("<project><packaging>jar</packaging></project>"));
        assert!(!aggregates("<project><!-- <modules> --></project>"));
        assert!(!aggregates("<project><build><plugins><plugin><configuration><packaging>pom</packaging></configuration></plugin></plugins></build></project>"));
    }
}
