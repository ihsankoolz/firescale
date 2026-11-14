//! Tests for `go/gitignore`.

use firescale_core::rule::Options;

use crate::go::fixtures::{module_without, with};
use crate::go::gitignore::GoGitignore;

#[test]
fn missing_lines_are_appended_once_in_the_files_line_endings() {
    assert!(module_without(|_| false)
        .check(&GoGitignore, &Options::new())
        .unwrap()
        .is_empty());
    let f = with(|_| false, &[(".gitignore", ".env\r\n*.out\r\n")]);
    assert_eq!(f.check(&GoGitignore, &Options::new()).unwrap().len(), 1);
    assert_eq!(
        f.fix(&GoGitignore, &Options::new()).unwrap().changed_files,
        vec![".gitignore"]
    );
    assert_eq!(
        f.read(".gitignore").as_deref(),
        Some(".env\r\n*.out\r\n# Go\r\n/app\r\n*.exe\r\n*.test\r\ngo.work\r\ngo.work.sum\r\n")
    );
    assert!(f
        .fix(&GoGitignore, &Options::new())
        .unwrap()
        .changed_files
        .is_empty());
}

#[test]
fn the_root_program_is_named_after_the_module_and_only_for_a_main_package() {
    let tool = with(
        |p| p == ".gitignore",
        &[("go.mod", "module github.com/acme/tool/v2\n\ngo 1.26.0\n")],
    );
    tool.fix(&GoGitignore, &Options::new()).unwrap();
    assert!(tool.read(".gitignore").unwrap().contains("/tool\n"));
    let library = with(
        |p| p == ".gitignore" || p.ends_with(".go"),
        &[
            ("calc.go", "package calc\n"),
            ("calc_test.go", "package calc\n"),
        ],
    );
    library.fix(&GoGitignore, &Options::new()).unwrap();
    let text = library.read(".gitignore").unwrap();
    assert!(!text.contains("/app"), "{text}");
    assert!(text.contains("*.test"));
}
