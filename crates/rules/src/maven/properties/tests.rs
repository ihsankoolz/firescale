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
        "<project><properties><maven.compiler.source>17</maven.compiler.source></properties></project>"
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

// Found in review: each of these was written outside the project's
// <properties>, into a plugin's, or after </project>.
#[test]
fn the_encoding_goes_into_the_projects_properties_however_the_pom_is_laid_out() {
    let check = |pom: &str, inside: &str| {
        let updated = with_encoding(pom).unwrap_or_else(|| panic!("no fix for {pom}"));
        assert!(has_encoding(&updated), "{updated}");
        assert!(updated.contains(inside), "{updated}");
        assert_eq!(
            with_encoding(&updated).map(|again| has_encoding(&again)),
            Some(true)
        );
    };
    // <properties> with content on its line, and empty.
    check(
        "<project>\n  <modelVersion>4.0.0</modelVersion>\n  <properties><maven.compiler.release>21</maven.compiler.release></properties>\n</project>\n",
        &format!("<properties>\n    {ENCODING}<maven.compiler.release>"),
    );
    check(
        "<project>\n  <properties></properties>\n</project>\n",
        &format!("<properties>\n    {ENCODING}</properties>"),
    );
    // A POM on one line.
    check(
        "<project><modelVersion>4.0.0</modelVersion><artifactId>a</artifactId></project>\n",
        &format!("<properties>\n    {ENCODING}\n  </properties></project>"),
    );
    // A plugin's <dependencies> is not the anchor, and its <properties> are
    // not the project's.
    let plugin = "<project>\n  <modelVersion>4.0.0</modelVersion>\n  <build>\n    <plugins>\n      <plugin>\n        <artifactId>maven-surefire-plugin</artifactId>\n        <configuration>\n          <properties><x>1</x></properties>\n        </configuration>\n        <dependencies>\n          <dependency><artifactId>d</artifactId></dependency>\n        </dependencies>\n      </plugin>\n    </plugins>\n  </build>\n</project>\n";
    let updated = with_encoding(plugin).unwrap();
    assert!(
        updated.contains(&format!(
            "  <properties>\n    {ENCODING}\n  </properties>\n\n  <build>"
        )),
        "{updated}"
    );
    assert!(updated.contains("<properties><x>1</x></properties>"));
}

#[test]
fn the_files_own_step_and_tabs_are_kept() {
    let tabs = "<project>\n\t<modelVersion>4.0.0</modelVersion>\n\t<dependencies>\n\t</dependencies>\n</project>\n";
    let updated = with_encoding(tabs).unwrap();
    assert!(
        updated.contains(&format!(
            "\t<properties>\n\t\t{ENCODING}\n\t</properties>\n\n\t<dependencies>"
        )),
        "{updated}"
    );
    let four = "<project>\n    <modelVersion>4.0.0</modelVersion>\n</project>\n";
    let updated = with_encoding(four).unwrap();
    assert!(
        updated.contains(&format!(
            "    <properties>\n        {ENCODING}\n    </properties>\n</project>"
        )),
        "{updated}"
    );
}

#[test]
fn other_plugins_settings_and_profiles_do_not_count() {
    let pom = "<project><properties></properties><build><plugins>\
        <plugin><artifactId>maven-antrun-plugin</artifactId><configuration><target/></configuration></plugin>\
        <plugin><artifactId>maven-resources-plugin</artifactId><configuration><encoding>UTF-8</encoding></configuration></plugin>\
        </plugins></build><profiles><profile><properties><project.build.sourceEncoding>UTF-8</project.build.sourceEncoding></properties></profile></profiles></project>";
    assert!(!has_encoding(pom));
    assert!(!has_java_version(pom));
    let compiler = "<project><build><plugins><plugin><artifactId>maven-compiler-plugin</artifactId><configuration><release>17</release><encoding>UTF-8</encoding></configuration></plugin></plugins></build></project>";
    assert!(has_encoding(compiler));
    assert!(has_java_version(compiler));
}

#[test]
fn a_pom_it_cannot_read_is_never_edited_and_non_ascii_does_not_panic() {
    assert_eq!(with_encoding("<project><properties>"), None);
    let f = with(|_| false, &[("pom.xml", "<project><name>caf\u{e9}\u{a0}")]);
    assert!(f.check(&Properties, &Options::new()).unwrap().is_empty());
    assert!(f
        .fix(&Properties, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}
