//! Tests for `supabase/local-smtp`: passing, failing and fixed repos, and the
//! fix run twice.

use firescale_core::model::Fixable;
use firescale_core::rule::Options;

use super::*;
use crate::supabase::fixtures::project;
use crate::supabase::local_smtp;

const OLD: &str = "project_id = \"x\"\n\n[inbucket]\nenabled = true\nport = 54324\n";

#[test]
fn inbucket_is_renamed_to_local_smtp_once() {
    let f = project(|_| false, &[("supabase/config.toml", OLD)]);
    let found = f.check(&local_smtp::LocalSmtp, &Options::new()).unwrap();
    assert_eq!(found.len(), 1);
    assert!(found[0].message.contains("line 3"));
    assert_eq!(found[0].fixable, Fixable::Deterministic);
    f.fix(&local_smtp::LocalSmtp, &Options::new()).unwrap();
    assert_eq!(
        f.read("supabase/config.toml").unwrap(),
        "project_id = \"x\"\n\n[local_smtp]\nenabled = true\nport = 54324\n"
    );
    assert!(f
        .check(&local_smtp::LocalSmtp, &Options::new())
        .unwrap()
        .is_empty());
    assert!(f
        .fix(&local_smtp::LocalSmtp, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}

#[test]
fn an_older_pinned_cli_is_for_a_person() {
    let f = project(
        |_| false,
        &[
            ("supabase/config.toml", OLD),
            (
                "package.json",
                "{ \"devDependencies\": { \"supabase\": \"^1.200.3\" } }\n",
            ),
        ],
    );
    let found = f.check(&local_smtp::LocalSmtp, &Options::new()).unwrap();
    assert_eq!(found[0].fixable, Fixable::Manual);
    assert!(f
        .fix(&local_smtp::LocalSmtp, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}

#[test]
fn renames_only_the_headers() {
    let text = "project_id = \"x\"\r\n[inbucket]\r\nenabled = true # inbucket\r\n[ inbucket.extra ]\r\n[api]\r\n";
    assert_eq!(inbucket_lines(text), vec![2, 4]);
    let out = renamed(text);
    assert_eq!(
        out,
        "project_id = \"x\"\r\n[local_smtp]\r\nenabled = true # inbucket\r\n[ local_smtp.extra ]\r\n[api]\r\n"
    );
    assert!(inbucket_lines(&out).is_empty());
}

#[test]
fn a_config_with_both_tables_or_inbucket_in_a_string_is_not_renamed() {
    let both = "project_id = \"x\"\n[inbucket]\nport = 1\n[local_smtp]\nport = 2\n";
    let f = project(|_| false, &[("supabase/config.toml", both)]);
    let found = f.check(&local_smtp::LocalSmtp, &Options::new()).unwrap();
    assert_eq!(found[0].fixable, Fixable::Manual);
    assert!(f
        .fix(&local_smtp::LocalSmtp, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());

    let quoted = "project_id = \"x\"\n[auth]\nnote = \"\"\"\n[inbucket]\n\"\"\"\n";
    let g = project(|_| false, &[("supabase/config.toml", quoted)]);
    assert!(g
        .check(&local_smtp::LocalSmtp, &Options::new())
        .unwrap()
        .is_empty());
}
