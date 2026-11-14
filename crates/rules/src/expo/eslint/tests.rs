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

#[test]
fn an_app_that_lints_with_something_else_passes() {
    let biome = with(|p| p == "eslint.config.js", &[("biome.json", "{}\n")]);
    assert!(biome
        .check(&EslintConfig, &Options::new())
        .unwrap()
        .is_empty());
    let script = with(
        |p| p == "eslint.config.js",
        &[(
            "package.json",
            "{ \"dependencies\": { \"expo\": \"~58.0.25\" }, \"scripts\": { \"lint\": \"oxlint .\" } }\n",
        )],
    );
    assert!(script
        .check(&EslintConfig, &Options::new())
        .unwrap()
        .is_empty());
}

#[test]
fn an_older_eslint_setup_is_not_given_a_flat_config() {
    let old = with(
        |p| p == "eslint.config.js",
        &[(
            "package.json",
            "{ \"dependencies\": { \"expo\": \"~51.0.0\" }, \"devDependencies\": { \"eslint\": \"^8.57.0\", \"eslint-config-expo\": \"~7.1.2\" } }\n",
        )],
    );
    assert_eq!(
        old.check(&EslintConfig, &Options::new()).unwrap()[0].fixable,
        Fixable::Agent
    );
    assert!(old
        .fix(&EslintConfig, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
    let eslintrc = with(|p| p == "eslint.config.js", &[(".eslintrc", "{}\n")]);
    assert!(eslintrc
        .check(&EslintConfig, &Options::new())
        .unwrap()
        .is_empty());
}
