//! Tests for `expo/app-config`: passing, failing and fixed repos, and the fix
//! run twice.

use firescale_core::model::Fixable;
use firescale_core::rule::Options;

use crate::expo::app_config::AppConfig;
use crate::expo::fixtures::{app_without, with};

#[test]
fn a_set_up_app_and_a_dynamic_config_pass() {
    assert!(app_without(|_| false)
        .check(&AppConfig, &Options::new())
        .unwrap()
        .is_empty());
    let dynamic = with(
        |p| p == "app.json",
        &[("app.config.ts", "export default {};\n")],
    );
    assert!(dynamic
        .check(&AppConfig, &Options::new())
        .unwrap()
        .is_empty());
    assert!(dynamic
        .fix(&AppConfig, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}

#[test]
fn a_missing_config_is_created_once() {
    let f = app_without(|p| p == "app.json");
    let found = f.check(&AppConfig, &Options::new()).unwrap();
    assert_eq!(found[0].fixable, Fixable::Deterministic);
    assert_eq!(
        f.fix(&AppConfig, &Options::new()).unwrap().changed_files,
        vec!["app.json"]
    );
    assert!(f
        .read("app.json")
        .unwrap()
        .contains("\"slug\": \"expo-app\""));
    assert!(f.check(&AppConfig, &Options::new()).unwrap().is_empty());
    assert!(f
        .fix(&AppConfig, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}

#[test]
fn missing_names_are_added_and_sdk_version_is_for_an_agent() {
    let f = with(
        |_| false,
        &[("app.json", "{\n  \"expo\": {\n    \"version\": \"1.0.0\",\n    \"sdkVersion\": \"50.0.0\"\n  }\n}\n")],
    );
    let found = f.check(&AppConfig, &Options::new()).unwrap();
    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!(
        f.fix(&AppConfig, &Options::new()).unwrap().changed_files,
        vec!["app.json"]
    );
    let text = f.read("app.json").unwrap();
    assert!(
        text.contains("\"name\": \"expo-app\"") && text.contains("\"slug\": \"expo-app\""),
        "{text}"
    );
    let left = f.check(&AppConfig, &Options::new()).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].fixable, Fixable::Agent);
    assert!(f
        .fix(&AppConfig, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}
