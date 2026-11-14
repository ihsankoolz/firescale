//! Tests for `supabase/gitignore`: passing, failing and fixed repos, and the
//! fix run twice.

use firescale_core::rule::Options;

use crate::supabase::fixtures::project;
use crate::supabase::gitignore;

#[test]
fn missing_lines_are_appended_in_the_files_own_line_endings() {
    let f = project(|_| false, &[("supabase/.gitignore", "# mine\r\n.temp\r\n")]);
    let found = f
        .check(&gitignore::SupabaseGitignore, &Options::new())
        .unwrap();
    assert!(found[0].message.contains("supabase/.branches"));
    f.fix(&gitignore::SupabaseGitignore, &Options::new())
        .unwrap();
    assert_eq!(
        f.read("supabase/.gitignore").unwrap(),
        "# mine\r\n.temp\r\n# Supabase CLI local state\r\n.branches\r\n"
    );
    assert!(f
        .fix(&gitignore::SupabaseGitignore, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}

#[test]
fn the_root_gitignore_counts() {
    let f = project(
        |p| p == "supabase/.gitignore",
        &[(
            ".gitignore",
            "node_modules/\nsupabase/.branches\n**/.temp\n",
        )],
    );
    assert!(f
        .check(&gitignore::SupabaseGitignore, &Options::new())
        .unwrap()
        .is_empty());
}
