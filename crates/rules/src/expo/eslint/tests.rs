//! Tests for `expo/eslint`: passing, failing and fixed repos, and the fix run
//! twice.

use firescale_core::model::Fixable;
use firescale_core::rule::Options;

use crate::expo::eslint::EslintConfig;
use crate::expo::fixtures::{app_without, with};

#[test]
fn a_missing_config_is_created_once_when_eslint_is_installed() {
    assert!(app_without(|_| false)
        .check(&EslintConfig, &Options::new())
        .unwrap()
        .is_empty());
    let f = app_without(|p| p == "eslint.config.js");
    assert_eq!(
        f.check(&EslintConfig, &Options::new()).unwrap()[0].fixable,
        Fixable::Deterministic
    );
    assert_eq!(
        f.fix(&EslintConfig, &Options::new()).unwrap().changed_files,
        vec!["eslint.config.js"]
    );
    assert!(f
        .fix(&EslintConfig, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}

#[test]
fn without_eslint_installed_it_is_for_an_agent_and_nothing_is_written() {
    let f = with(
        |p| p == "eslint.config.js",
        &[(
            "package.json",
            "{ \"dependencies\": { \"expo\": \"~58.0.25\" } }\n",
        )],
    );
    let found = f.check(&EslintConfig, &Options::new()).unwrap();
    assert_eq!(found[0].fixable, Fixable::Agent);
    assert!(f
        .fix(&EslintConfig, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}
