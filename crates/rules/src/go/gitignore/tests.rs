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

// Found in review: each of these hid source, or reversed a person's choice.
#[test]
fn the_program_line_never_ignores_a_folder_or_a_generator() {
    let same_name = with(
        |p| p == ".gitignore",
        &[
            ("go.mod", "module github.com/acme/server\n\ngo 1.26.0\n"),
            ("server/server.go", "package server\n"),
        ],
    );
    same_name.fix(&GoGitignore, &Options::new()).unwrap();
    assert!(!same_name.read(".gitignore").unwrap().contains("/server"));
    let generator = with(
        |p| p == ".gitignore" || p.ends_with(".go"),
        &[
            ("go.mod", "module github.com/acme/widget\n\ngo 1.26.0\n"),
            ("widget.go", "package widget\n"),
            ("widget_test.go", "package widget\n"),
            (
                "gen.go",
                "//go:build ignore\n\npackage main\n\nfunc main() {}\n",
            ),
        ],
    );
    generator.fix(&GoGitignore, &Options::new()).unwrap();
    assert!(!generator.read(".gitignore").unwrap().contains("/widget"));
    // A folder-only line does not ignore the program file.
    let folder_only = with(
        |_| false,
        &[(
            ".gitignore",
            "app/\n*.exe\n*.test\n*.out\ngo.work\ngo.work.sum\n",
        )],
    );
    assert_eq!(
        folder_only
            .check(&GoGitignore, &Options::new())
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn a_negation_or_a_tracked_go_work_is_left_as_it_is() {
    let negated = with(
        |_| false,
        &[(".gitignore", "/app\n*.exe\n*.test\n*.out\n!go.work\n")],
    );
    negated.fix(&GoGitignore, &Options::new()).unwrap();
    let text = negated.read(".gitignore").unwrap();
    assert!(!text.contains("\ngo.work\n"), "{text}");
    assert!(text.contains("go.work.sum"));
    let tracked = with(
        |_| false,
        &[
            (".gitignore", "/app\n*.exe\n*.test\n*.out\n"),
            ("go.work", "go 1.26.0\n\nuse .\n"),
        ],
    );
    tracked.fix(&GoGitignore, &Options::new()).unwrap();
    let text = tracked.read(".gitignore").unwrap();
    assert!(!text.contains("\ngo.work\n"), "{text}");
    // One `# Go` header, also on a later run.
    let twice = with(|_| false, &[(".gitignore", "# Go\n/app\n")]);
    twice.fix(&GoGitignore, &Options::new()).unwrap();
    assert_eq!(twice.read(".gitignore").unwrap().matches("# Go").count(), 1);
}
