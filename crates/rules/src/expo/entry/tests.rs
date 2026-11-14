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

#[test]
fn no_main_with_a_root_index_passes_like_expo_resolves_it() {
    // bluesky-social/social-app's layout (review of M30).
    let f = with(
        |p| p == "App.tsx" || p == "index.ts",
        &[
            (
                "package.json",
                "{ \"dependencies\": { \"expo\": \"~58.0.25\" } }\n",
            ),
            ("index.ts", "import './src/App';\n"),
        ],
    );
    assert!(f.check(&Entry, &Options::new()).unwrap().is_empty());
    assert!(f
        .fix(&Entry, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}

#[test]
fn expo_router_needs_a_route_not_a_layout_and_reads_its_root_from_app_json() {
    let main =
        "{ \"main\": \"expo-router/entry\", \"dependencies\": { \"expo\": \"~58.0.25\" } }\n";
    let index_only = with(
        |p| p == "App.tsx" || p == "index.ts",
        &[
            ("package.json", main),
            (
                "app/index.tsx",
                "export default function I() { return null; }\n",
            ),
        ],
    );
    assert!(index_only
        .check(&Entry, &Options::new())
        .unwrap()
        .is_empty());
    let custom = with(
        |p| p == "App.tsx" || p == "index.ts",
        &[
            ("package.json", main),
            (
                "app.json",
                "{ \"expo\": { \"name\": \"a\", \"slug\": \"a\", \"plugins\": [[\"expo-router\", { \"root\": \"./screens\" }]] } }\n",
            ),
            ("screens/home.tsx", "export default function H() { return null; }\n"),
        ],
    );
    assert!(custom.check(&Entry, &Options::new()).unwrap().is_empty());
}

#[test]
fn platform_files_and_package_mains_resolve() {
    let platform = with(
        |p| p == "App.tsx" || p == "index.ts",
        &[
            (
                "package.json",
                "{ \"main\": \"index\", \"dependencies\": { \"expo\": \"~58.0.25\" } }\n",
            ),
            ("index.ios.js", "x\n"),
            ("index.android.js", "x\n"),
        ],
    );
    assert!(platform.check(&Entry, &Options::new()).unwrap().is_empty());
    let app_entry = with(
        |p| p == "index.ts",
        &[(
            "package.json",
            "{ \"main\": \"node_modules/expo/AppEntry.js\", \"dependencies\": { \"expo\": \"~58.0.25\" } }\n",
        )],
    );
    assert!(app_entry.check(&Entry, &Options::new()).unwrap().is_empty());
    // A main in any folder of the app is checked.
    let gone = with(
        |_| false,
        &[(
            "package.json",
            "{ \"main\": \"app/index.js\", \"dependencies\": { \"expo\": \"~58.0.25\" } }\n",
        )],
    );
    assert_eq!(
        gone.check(&Entry, &Options::new()).unwrap()[0].fixable,
        Fixable::Agent
    );
}
