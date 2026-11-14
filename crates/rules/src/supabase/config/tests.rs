//! Tests for `supabase/config`: passing, failing and fixed repos, and the
//! fix run twice.

use firescale_core::model::Fixable;
use firescale_core::rule::Options;

use super::*;
use crate::supabase::config;
use crate::supabase::fixtures::{fixture, project};

#[test]
fn a_missing_project_id_is_added_before_the_first_table() {
    let f = project(
        |_| false,
        &[("supabase/config.toml", "[api]\nenabled = true\n")],
    );
    let found = f.check(&config::Config, &Options::new()).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].fixable, Fixable::Deterministic);
    f.fix(&config::Config, &Options::new()).unwrap();
    assert_eq!(
        f.read("supabase/config.toml").unwrap(),
        "project_id = \"supabase-app\"\n\n[api]\nenabled = true\n"
    );
    assert!(f
        .check(&config::Config, &Options::new())
        .unwrap()
        .is_empty());
    assert!(f
        .fix(&config::Config, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}

#[test]
fn the_project_id_comes_from_package_json_cleaned_as_the_cli_does() {
    let f = fixture(&[(
        "package.json",
        "{ \"name\": \"@acme/My App!\", \"devDependencies\": { \"supabase\": \"3.1.0\" } }\n"
            .to_owned(),
    )]);
    f.fix(&config::Config, &Options::new()).unwrap();
    assert!(f
        .read("supabase/config.toml")
        .unwrap()
        .contains("project_id = \"My_App_\""));
}

#[test]
fn project_id_goes_before_the_first_table_in_the_files_line_endings() {
    assert_eq!(
        with_project_id("# c\r\n[api]\r\nport = 1\r\n", "x"),
        "# c\r\nproject_id = \"x\"\r\n\r\n[api]\r\nport = 1\r\n"
    );
    assert_eq!(
        with_project_id("# only", "x"),
        "# only\nproject_id = \"x\"\n"
    );
    // A byte order mark before the first table (review of M29): the key
    // goes after it, before the table, and is read back.
    let out = with_project_id("\u{feff}[api]\n[db]\n", "x");
    assert_eq!(out, "\u{feff}project_id = \"x\"\n\n[api]\n[db]\n");
    assert!(crate::supabase::config_value(&out, None, "project_id").is_some());
}
