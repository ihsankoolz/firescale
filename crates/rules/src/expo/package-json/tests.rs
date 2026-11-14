//! Tests for `expo/package-json`: passing, failing and fixed repos, and the
//! fix run twice.

use firescale_core::model::Fixable;
use firescale_core::rule::Options;

use crate::expo::fixtures::{app_without, fixture};
use crate::expo::package_json::PackageJson;

#[test]
fn a_set_up_app_passes_and_nothing_changes() {
    let f = app_without(|_| false);
    assert!(f.check(&PackageJson, &Options::new()).unwrap().is_empty());
    assert!(f
        .fix(&PackageJson, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}

#[test]
fn missing_scripts_are_added_once() {
    let f = fixture(&[(
        "package.json",
        "{\n  \"name\": \"app\",\n  \"dependencies\": {\n    \"expo\": \"~58.0.25\",\n    \"react\": \"19.3.0\",\n    \"react-native\": \"0.88.1\"\n  }\n}\n".to_owned(),
    )]);
    let found = f.check(&PackageJson, &Options::new()).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].fixable, Fixable::Deterministic);
    assert_eq!(
        f.fix(&PackageJson, &Options::new()).unwrap().changed_files,
        vec!["package.json"]
    );
    assert!(f
        .read("package.json")
        .unwrap()
        .contains("\"ios\": \"expo start --ios\""));
    // web only with react-native-web.
    assert!(!f.read("package.json").unwrap().contains("--web"));
    // lint only with eslint-config-expo.
    assert!(!f.read("package.json").unwrap().contains("expo lint"));
    assert!(f.check(&PackageJson, &Options::new()).unwrap().is_empty());
    assert!(f
        .fix(&PackageJson, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}

#[test]
fn missing_react_native_is_for_expo_install_and_a_workspace_root_is_left_alone() {
    let f = fixture(&[(
        "package.json",
        "{ \"dependencies\": { \"expo\": \"~58.0.25\", \"react\": \"19.3.0\" }, \"scripts\": { \"start\": \"expo start\", \"android\": \"a\", \"ios\": \"i\" } }\n".to_owned(),
    )]);
    let found = f.check(&PackageJson, &Options::new()).unwrap();
    assert_eq!(found[0].fixable, Fixable::Agent);
    assert!(found[0].message.contains("npx expo install react-native"));

    let root = fixture(&[(
        "package.json",
        "{ \"workspaces\": [\"apps/*\"] }\n".to_owned(),
    )]);
    let found = root.check(&PackageJson, &Options::new()).unwrap();
    assert_eq!(found[0].fixable, Fixable::Manual);
    assert!(root
        .fix(&PackageJson, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}
