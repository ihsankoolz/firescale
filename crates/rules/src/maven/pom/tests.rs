//! Tests for `maven/pom`.

use firescale_core::model::Fixable;
use firescale_core::rule::Options;

use crate::maven::fixtures::{fixture, project_without, with};
use crate::maven::pom::{missing_coordinates, new_pom, Pom};

#[test]
fn a_missing_pom_is_created_once() {
    assert!(project_without(|_| false)
        .check(&Pom, &Options::new())
        .unwrap()
        .is_empty());
    let f = project_without(|p| p == "pom.xml");
    assert_eq!(
        f.check(&Pom, &Options::new()).unwrap()[0].fixable,
        Fixable::Deterministic
    );
    assert_eq!(
        f.fix(&Pom, &Options::new()).unwrap().changed_files,
        vec!["pom.xml"]
    );
    assert_eq!(f.read("pom.xml").as_deref(), Some(new_pom().as_str()));
    assert!(f
        .fix(&Pom, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}

#[test]
fn coordinates_are_the_projects_own_not_a_dependencys() {
    assert_eq!(missing_coordinates(&new_pom()), Some(vec![]));
    let child = "<project>\n  <modelVersion>4.0.0</modelVersion>\n  <parent>\n    <groupId>g</groupId>\n    <artifactId>p</artifactId>\n    <version>1</version>\n  </parent>\n  <artifactId>child</artifactId>\n</project>\n";
    assert_eq!(missing_coordinates(child), Some(vec![]));
    let only_a_dependency = "<project>\n  <modelVersion>4.0.0</modelVersion>\n  <dependencies>\n    <dependency>\n      <groupId>g</groupId>\n      <artifactId>a</artifactId>\n      <version>1</version>\n    </dependency>\n  </dependencies>\n</project>\n";
    assert_eq!(
        missing_coordinates(only_a_dependency),
        Some(vec!["groupId", "artifactId", "version"])
    );
    let commented = "<project><modelVersion>4.0.0</modelVersion><!-- <artifactId>x</artifactId> --><groupId>g</groupId><version>1</version></project>";
    assert_eq!(missing_coordinates(commented), Some(vec!["artifactId"]));
    // A flatten plugin's <parent> setting is not a parent (found in review).
    let flatten = "<project><modelVersion>4.0.0</modelVersion><artifactId>a</artifactId><build><plugins><plugin><configuration><pomElements><parent>expand</parent></pomElements></configuration></plugin></plugins></build></project>";
    assert_eq!(
        missing_coordinates(flatten),
        Some(vec!["groupId", "version"])
    );
    assert_eq!(missing_coordinates("<project><artifactId>"), None);
}

#[test]
fn a_pom_without_coordinates_is_for_a_person() {
    let f = with(
        |_| false,
        &[(
            "pom.xml",
            "<project>\n  <modelVersion>4.0.0</modelVersion>\n</project>\n",
        )],
    );
    let found = f.check(&Pom, &Options::new()).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].fixable, Fixable::Manual);
    assert!(f
        .fix(&Pom, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}

#[test]
fn a_project_in_a_subfolder_gets_one_finding_and_nothing_written() {
    let f = fixture(&[("service/pom.xml", "<project/>\n".to_owned())]);
    let found = f.check(&Pom, &Options::new()).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].file, "service/pom.xml");
    assert!(f
        .fix(&Pom, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}
