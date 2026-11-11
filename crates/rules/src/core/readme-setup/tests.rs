//! Tests for `core/readme-setup`.

use firescale_core::model::{Category, Fixable, Severity};
use firescale_core::rule::{BuiltinRule, Options};
use firescale_core::testing::{Fixture, Track};

use super::{ReadmeSetup, ID};

const GOOD: &str =
    "# demo\n\n## Install\n\n```sh\npnpm install\n```\n\n## Usage\n\n```sh\npnpm dev\n```\n";

fn check(files: &[(&str, &str)]) -> Vec<firescale_core::model::Diagnostic> {
    let f = Fixture::new(files, Track::All).unwrap();
    f.check(&ReadmeSetup, &Options::new()).unwrap()
}

mod passing_repo {
    use super::*;

    #[test]
    fn readme_with_install_and_usage_sections_passes() {
        assert!(check(&[("README.md", GOOD)]).is_empty());
    }

    #[test]
    fn commands_alone_satisfy_both_parts() {
        let text = "# demo\n\nRun `npm ci` first, then:\n\n    npm start\n";
        assert!(check(&[("README.md", text)]).is_empty());
    }

    #[test]
    fn getting_started_heading_satisfies_both_parts() {
        assert!(check(&[("readme.md", "# demo\n\n## Getting started\n\nHello.\n")]).is_empty());
    }

    #[test]
    fn rst_underlined_headings_count() {
        let text = "demo\n====\n\nInstallation\n------------\n\nx\n\nUsage\n-----\n\nx\n";
        assert!(check(&[("README.rst", text)]).is_empty());
    }

    #[test]
    fn works_outside_a_git_repo() {
        let f = Fixture::new(&[("README.md", GOOD)], Track::NoGit).unwrap();
        assert!(f.check(&ReadmeSetup, &Options::new()).unwrap().is_empty());
    }
}

mod failing_repo {
    use super::*;

    #[test]
    fn missing_readme_is_reported_on_readme_md() {
        let d = check(&[("package.json", "{}\n")]);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].rule_id, ID);
        assert_eq!(d[0].file, "README.md");
        assert_eq!(d[0].fixable, Fixable::Agent);
        assert!(d[0].message.contains("Add a README.md"));
    }

    #[test]
    fn readme_without_either_part_gets_two_findings() {
        let d = check(&[(
            "README.md",
            "# demo\n\nA small tool. Make sure you like it.\n",
        )]);
        assert_eq!(d.len(), 2);
        assert!(d.iter().all(|x| x.file == "README.md"));
        assert!(d[0].message.contains("install") || d[1].message.contains("install"));
        assert!(d.iter().any(|x| x.message.contains("run the project")));
    }

    #[test]
    fn install_only_readme_is_missing_run() {
        let d = check(&[("README.md", "# demo\n\n## Install\n\nnpm install\n")]);
        assert_eq!(d.len(), 1);
        assert!(d[0].message.contains("run the project"));
    }

    #[test]
    fn finding_uses_the_actual_readme_name() {
        let d = check(&[("Readme.txt", "hello\n")]);
        assert_eq!(d.len(), 2);
        assert!(d.iter().all(|x| x.file == "Readme.txt"));
    }

    #[test]
    fn a_readme_in_a_subfolder_does_not_count() {
        let d = check(&[("docs/README.md", GOOD)]);
        assert_eq!(d[0].file, "README.md");
    }
}

mod metadata {
    use super::*;

    #[test]
    fn meta_matches_the_spec() {
        let meta = ReadmeSetup.meta();
        assert_eq!(meta.id, ID);
        assert_eq!(meta.severity, Severity::Low);
        assert_eq!(meta.category, Category::Maintainability);
        assert!(!meta.has_fix);
        assert_eq!(meta.inputs, vec!["README*", "readme*"]);
    }
}
