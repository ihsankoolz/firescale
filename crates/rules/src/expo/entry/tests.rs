//! Tests for `expo/entry`: passing, failing and fixed repos, and the fix run
//! twice.

use firescale_core::model::Fixable;
use firescale_core::rule::Options;

use crate::expo::entry::Entry;
use crate::expo::fixtures::{app_without, with};

#[test]
fn a_set_up_app_passes() {
    let f = app_without(|_| false);
    assert!(f.check(&Entry, &Options::new()).unwrap().is_empty());
    assert!(f
        .fix(&Entry, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}

#[test]
fn a_new_app_gets_app_and_index_once() {
    let f = app_without(|p| p == "App.tsx" || p == "index.ts");
    assert_eq!(
        f.check(&Entry, &Options::new()).unwrap()[0].fixable,
        Fixable::Deterministic
    );
    assert_eq!(
        f.fix(&Entry, &Options::new()).unwrap().changed_files,
        vec!["App.tsx", "index.ts"]
    );
    assert!(f.check(&Entry, &Options::new()).unwrap().is_empty());
    assert!(f
        .fix(&Entry, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}

#[test]
fn a_missing_index_is_created_and_other_entries_are_for_an_agent() {
    let f = app_without(|p| p == "index.ts");
    assert_eq!(
        f.fix(&Entry, &Options::new()).unwrap().changed_files,
        vec!["index.ts"]
    );

    let router = with(
        |p| p == "App.tsx" || p == "index.ts",
        &[(
            "package.json",
            "{ \"main\": \"expo-router/entry\", \"dependencies\": { \"expo\": \"~58.0.25\" } }\n",
        )],
    );
    assert_eq!(
        router.check(&Entry, &Options::new()).unwrap()[0].fixable,
        Fixable::Agent
    );
    let laid_out = with(
        |p| p == "App.tsx" || p == "index.ts",
        &[
            (
                "package.json",
                "{ \"main\": \"expo-router/entry\", \"dependencies\": { \"expo\": \"~58.0.25\" } }\n",
            ),
            ("src/app/_layout.tsx", "export default function L() { return null; }\n"),
        ],
    );
    assert!(laid_out.check(&Entry, &Options::new()).unwrap().is_empty());

    let gone = with(
        |_| false,
        &[(
            "package.json",
            "{ \"main\": \"./src/main\", \"dependencies\": { \"expo\": \"~58.0.25\" } }\n",
        )],
    );
    assert_eq!(
        gone.check(&Entry, &Options::new()).unwrap()[0].fixable,
        Fixable::Agent
    );
    assert!(gone
        .fix(&Entry, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
    // An index without extension resolves as Metro would.
    let plain = with(
        |_| false,
        &[(
            "package.json",
            "{ \"main\": \"index\", \"dependencies\": { \"expo\": \"~58.0.25\" } }\n",
        )],
    );
    assert!(plain.check(&Entry, &Options::new()).unwrap().is_empty());
}
