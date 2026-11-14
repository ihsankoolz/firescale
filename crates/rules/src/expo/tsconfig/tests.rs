//! Tests for `expo/tsconfig`: passing, failing and fixed repos, and the fix
//! run twice.

use firescale_core::model::Fixable;
use firescale_core::rule::Options;

use crate::expo::fixtures::{app_without, with};
use crate::expo::tsconfig::TsConfig;

#[test]
fn a_missing_tsconfig_is_created_once_and_a_javascript_app_gets_none() {
    assert!(app_without(|_| false)
        .check(&TsConfig, &Options::new())
        .unwrap()
        .is_empty());
    let f = app_without(|p| p == "tsconfig.json");
    assert_eq!(
        f.check(&TsConfig, &Options::new()).unwrap()[0].fixable,
        Fixable::Deterministic
    );
    assert_eq!(
        f.fix(&TsConfig, &Options::new()).unwrap().changed_files,
        vec!["tsconfig.json"]
    );
    assert!(f
        .fix(&TsConfig, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
    let js = with(
        |p| p == "tsconfig.json",
        &[(
            "package.json",
            "{ \"dependencies\": { \"expo\": \"~58.0.25\" } }\n",
        )],
    );
    assert!(js.check(&TsConfig, &Options::new()).unwrap().is_empty());
}
