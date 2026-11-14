//! Tests for `maven/properties`.

use firescale_core::model::Fixable;
use firescale_core::rule::Options;

use crate::maven::fixtures::{project_without, with};
use crate::maven::properties::{
    has_encoding, has_java_version, with_encoding, Properties, ENCODING,
};

const BARE: &str = "<project>\n  <modelVersion>4.0.0</modelVersion>\n  <groupId>g</groupId>\n  <artifactId>a</artifactId>\n  <version>1</version>\n\n  <dependencies>\n  </dependencies>\n</project>\n";

#[test]
fn a_set_up_pom_passes() {
    assert!(project_without(|_| false)
        .check(&Properties, &Options::new())
        .unwrap()
        .is_empty());
}

#[test]
fn the_encoding_is_added_to_the_projects_properties_in_its_indentation_once() {
    let pom = "<project>\r\n    <modelVersion>4.0.0</modelVersion>\r\n    <properties>\r\n        <maven.compiler.release>17</maven.compiler.release>\r\n    </properties>\r\n</project>\r\n";
    let f = with(|_| false, &[("pom.xml", pom)]);
    let found = f.check(&Properties, &Options::new()).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].fixable, Fixable::Deterministic);
    f.fix(&Properties, &Options::new()).unwrap();
    assert_eq!(
        f.read("pom.xml").unwrap(),
        format!("<project>\r\n    <modelVersion>4.0.0</modelVersion>\r\n    <properties>\r\n        {ENCODING}\r\n        <maven.compiler.release>17</maven.compiler.release>\r\n    </properties>\r\n</project>\r\n")
    );
    assert!(f
        .fix(&Properties, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}

#[test]
fn a_pom_without_properties_gets_a_block_before_its_dependencies() {
    let updated = with_encoding(BARE).unwrap();
    assert!(
        updated.contains(&format!(
            "  <properties>\n    {ENCODING}\n  </properties>\n\n  <dependencies>"
        )),
        "{updated}"
    );
    assert!(has_encoding(&updated));
    // Before </project> when nothing else is there.
    let minimal = "<project>\n  <modelVersion>4.0.0</modelVersion>\n</project>\n";
    let updated = with_encoding(minimal).unwrap();
    assert!(
        updated.ends_with(&format!(
            "  <properties>\n    {ENCODING}\n  </properties>\n</project>\n"
        )),
        "{updated}"
    );
    // A self-closing <properties/> is for a person: a second block is invalid.
    assert_eq!(
        with_encoding("<project>\n  <properties/>\n</project>\n"),
        None
    );
}

#[test]
fn a_profiles_properties_are_not_the_projects() {
    let pom = "<project>\n  <modelVersion>4.0.0</modelVersion>\n  <profiles>\n    <profile>\n      <properties>\n        <x>1</x>\n      </properties>\n    </profile>\n  </profiles>\n</project>\n";
    let updated = with_encoding(pom).unwrap();
    assert!(
        updated.starts_with("<project>\n  <modelVersion>4.0.0</modelVersion>\n  <profiles>"),
        "{updated}"
    );
    assert!(
        updated.contains(&format!(
            "  <properties>\n    {ENCODING}\n  </properties>\n</project>"
        )),
        "{updated}"
    );
}

#[test]
fn the_java_version_is_for_a_person_and_a_parent_is_trusted() {
    assert!(!has_java_version(BARE));
    assert!(has_java_version(
        "<maven.compiler.source>17</maven.compiler.source>"
    ));
    assert!(!has_java_version(
        "<!-- <maven.compiler.release>17</maven.compiler.release> -->"
    ));
    let f = with(|_| false, &[("pom.xml", BARE)]);
    let found = f.check(&Properties, &Options::new()).unwrap();
    assert_eq!(
        found
            .iter()
            .filter(|d| d.fixable == Fixable::Manual)
            .count(),
        1
    );
    let child = "<project>\n  <parent>\n    <groupId>g</groupId>\n    <artifactId>p</artifactId>\n    <version>1</version>\n  </parent>\n  <artifactId>c</artifactId>\n</project>\n";
    let f = with(|_| false, &[("pom.xml", child)]);
    assert!(f.check(&Properties, &Options::new()).unwrap().is_empty());
    assert!(f
        .fix(&Properties, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}
