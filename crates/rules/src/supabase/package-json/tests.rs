//! Tests for `supabase/package-json`: passing, failing and fixed repos, and the
//! fix run twice.

use firescale_core::model::Fixable;
use firescale_core::rule::Options;

use crate::supabase::fixtures::project;
use crate::supabase::package_json;

const WITHOUT_CLI: &str =
    "{\n  \"name\": \"app\",\n  \"dependencies\": {\n    \"@supabase/supabase-js\": \"2.119.0\"\n  }\n}\n";

#[test]
fn the_cli_is_added_when_there_is_no_lockfile() {
    let f = project(|_| false, &[("package.json", WITHOUT_CLI)]);
    let found = f
        .check(&package_json::PackageJson, &Options::new())
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].fixable, Fixable::Deterministic);
    assert_eq!(
        f.fix(&package_json::PackageJson, &Options::new())
            .unwrap()
            .changed_files,
        vec!["package.json"]
    );
    let text = f.read("package.json").unwrap();
    assert!(text.contains("\"supabase\": \"3.1.0\""), "{text}");
    // Every line that was there is still there.
    for line in WITHOUT_CLI
        .lines()
        .filter(|l| !l.trim().is_empty() && *l != "{" && *l != "}")
    {
        assert!(
            text.contains(line.trim_end_matches(',')),
            "{line} lost: {text}"
        );
    }
    assert!(f
        .check(&package_json::PackageJson, &Options::new())
        .unwrap()
        .is_empty());
    assert!(f
        .fix(&package_json::PackageJson, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}

#[test]
fn with_a_lockfile_installing_the_cli_is_for_the_package_manager() {
    let f = project(
        |_| false,
        &[("package.json", WITHOUT_CLI), ("package-lock.json", "{}\n")],
    );
    let found = f
        .check(&package_json::PackageJson, &Options::new())
        .unwrap();
    assert_eq!(found[0].fixable, Fixable::Agent);
    assert!(found[0].message.contains("npm install --save-dev supabase"));
    assert!(f
        .fix(&package_json::PackageJson, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}
