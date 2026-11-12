//! Fixture tests for the `nextjs` pack: passing, failing and fixed repos,
//! and every fix run twice.

use firescale_core::model::{Diagnostic, Fixable};
use firescale_core::rule::{BuiltinRule, Options};
use firescale_core::testing::{Fixture, Track};

use super::{app_router, eslint, gitignore, pack, package_json};

fn rules() -> Vec<Box<dyn BuiltinRule>> {
    pack().rules
}

fn findings(f: &Fixture) -> Vec<Diagnostic> {
    rules()
        .iter()
        .flat_map(|rule| f.check(rule.as_ref(), &Options::new()).unwrap())
        .collect()
}

/// Runs every fix in the pack's order until nothing changes.
fn fix_all(f: &Fixture) -> Vec<String> {
    let mut changed = Vec::new();
    for rule in rules() {
        changed.extend(f.fix(rule.as_ref(), &Options::new()).unwrap().changed_files);
    }
    changed
}

use super::fixtures::set_up;

fn fixture(files: &[(&str, String)]) -> Fixture {
    let refs: Vec<(&str, &str)> = files.iter().map(|(p, c)| (*p, c.as_str())).collect();
    Fixture::new(&refs, Track::All).unwrap()
}

#[test]
fn the_pack_has_six_rules_with_fixes_and_skills() {
    let rules = rules();
    assert_eq!(rules.len(), 6);
    for rule in &rules {
        let meta = rule.meta();
        assert!(meta.id.starts_with("nextjs/"), "{}", meta.id);
        assert!(meta.has_fix, "{}", meta.id);
        assert!(rule
            .skill()
            .contains(&format!("fscale check --rules {}", meta.id)));
    }
}

mod passing_repo {
    use super::*;

    #[test]
    fn an_app_set_up_like_create_next_app_has_no_findings_and_nothing_to_fix() {
        let f = fixture(&set_up());
        assert_eq!(findings(&f), []);
        assert!(fix_all(&f).is_empty());
    }

    #[test]
    fn a_pages_router_app_and_other_config_names_pass() {
        let mut files = set_up();
        files.retain(|(path, _)| {
            !path.starts_with("app/") && *path != "next.config.ts" && *path != "eslint.config.mjs"
        });
        files.push((
            "pages/index.tsx",
            "export default function Home() { return null; }\n".to_owned(),
        ));
        files.push(("next.config.mjs", "export default {};\n".to_owned()));
        files.push((".eslintrc.json", "{}\n".to_owned()));
        let f = fixture(&files);
        assert_eq!(findings(&f), []);
    }
}

mod failing_repo {
    use super::*;

    #[test]
    fn an_empty_folder_misses_everything_and_each_finding_is_deterministic() {
        let f = Fixture::new(&[], Track::None).unwrap();
        let found = findings(&f);
        let ids: Vec<&str> = found.iter().map(|d| d.rule_id.as_str()).collect();
        // nextjs/eslint waits for package.json, which the first fix writes.
        assert_eq!(ids.len(), 5, "{ids:?}");
        assert!(!ids.contains(&eslint::ID));
        assert!(
            found.iter().all(|d| d.fixable == Fixable::Deterministic),
            "{found:?}"
        );
    }

    #[test]
    fn a_known_gap_is_exactly_one_finding() {
        let mut files = set_up();
        files.retain(|(path, _)| *path != "app/layout.tsx");
        let f = fixture(&files);
        let found = findings(&f);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].rule_id, app_router::ID);
        assert_eq!(found[0].file, "app/layout.tsx");
        assert_eq!(fix_all(&f), ["app/layout.tsx"]);
    }

    #[test]
    fn an_existing_app_without_eslint_deps_is_not_given_a_config_it_cannot_load() {
        let package = "{\n  \"name\": \"shop\",\n  \"scripts\": {\n    \"build\": \"next build\"\n  },\n  \"dependencies\": {\n    \"next\": \"15.0.0\",\n    \"react\": \"19.0.0\",\n    \"react-dom\": \"19.0.0\"\n  }\n}\n";
        let f = Fixture::new(
            &[
                ("package.json", package),
                ("app/layout.tsx", "x"),
                ("next.config.ts", "x"),
            ],
            Track::All,
        )
        .unwrap();
        let eslint: Vec<Diagnostic> = findings(&f)
            .into_iter()
            .filter(|d| d.rule_id == eslint::ID)
            .collect();
        assert_eq!(eslint[0].fixable, Fixable::Agent);
        let changed = fix_all(&f);
        assert!(!changed.contains(&"eslint.config.mjs".to_owned()));
        // Only the missing dev and start scripts were added, as new lines.
        let after = f.read("package.json").unwrap();
        let mut lines = after.lines();
        for line in package.lines() {
            assert!(lines.any(|candidate| candidate == line), "{line}: {after}");
        }
        assert!(after.contains("\"dev\": \"next dev\""));
        assert!(after.contains("\"start\": \"next start\""));
        assert!(
            !after.contains("\"lint\""),
            "no lint script without ESLint: {after}"
        );
    }

    #[test]
    fn missing_react_is_for_a_person_to_install() {
        let f = Fixture::new(
            &[(
                "package.json",
                "{ \"dependencies\": { \"next\": \"16.0.0\" } }\n",
            )],
            Track::None,
        )
        .unwrap();
        let found: Vec<Diagnostic> = findings(&f)
            .into_iter()
            .filter(|d| d.rule_id == package_json::ID)
            .collect();
        assert!(found
            .iter()
            .any(|d| d.fixable == Fixable::Agent && d.message.contains("react, react-dom")));
    }
}

mod fixed_repo {
    use super::*;

    #[test]
    fn fixing_an_empty_folder_leaves_no_findings_and_a_second_run_changes_nothing() {
        let f = Fixture::new(&[], Track::None).unwrap();
        let first = fix_all(&f);
        assert!(first.contains(&"package.json".to_owned()));
        assert!(first.contains(&"app/page.tsx".to_owned()));
        assert_eq!(findings(&f), []);
        assert!(fix_all(&f).is_empty(), "every fix is idempotent");
    }

    #[test]
    fn the_gitignore_lines_are_appended_in_its_own_line_endings() {
        let f = Fixture::new(
            &[(".gitignore", "node_modules/\r\n# ours, no newline")],
            Track::None,
        )
        .unwrap();
        f.fix(&gitignore::NextGitignore, &Options::new()).unwrap();
        assert_eq!(
            f.read(".gitignore").unwrap(),
            "node_modules/\r\n# ours, no newline\r\n# Next.js build output\r\n.next/\r\nnext-env.d.ts\r\n"
        );
        assert!(f
            .fix(&gitignore::NextGitignore, &Options::new())
            .unwrap()
            .changed_files
            .is_empty());
    }
}

/// Regressions from the review of M15.
mod review {
    use super::*;

    #[test]
    fn a_workspace_root_with_the_app_in_a_subfolder_gets_nothing_written() {
        let root = "{\n  \"name\": \"mono\",\n  \"workspaces\": [\"apps/*\"],\n  \"scripts\": {\n    \"build\": \"turbo build\"\n  },\n  \"devDependencies\": {\n    \"turbo\": \"2.0.0\",\n    \"typescript\": \"5.0.0\"\n  }\n}\n";
        let f = Fixture::new(
            &[
                ("package.json", root),
                (
                    "apps/web/package.json",
                    "{\"name\":\"web\",\"dependencies\":{\"next\":\"16.4.6\"}}",
                ),
                ("apps/web/app/layout.tsx", "x"),
            ],
            Track::All,
        )
        .unwrap();
        let found = findings(&f);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].rule_id, package_json::ID);
        assert_eq!(found[0].fixable, Fixable::Manual);
        assert!(fix_all(&f).is_empty());
        assert_eq!(f.read("package.json").unwrap(), root);
    }

    #[test]
    fn route_groups_with_their_own_root_layouts_pass() {
        let mut files = set_up();
        files.retain(|(path, _)| !path.starts_with("app/"));
        files.push(("app/(shop)/layout.tsx", "x".to_owned()));
        files.push(("app/(marketing)/layout.tsx", "x".to_owned()));
        let f = fixture(&files);
        assert_eq!(findings(&f), []);
        assert!(fix_all(&f).is_empty());
    }

    #[test]
    fn eslint_alone_converges_without_a_package_json() {
        let f = Fixture::new(&[], Track::None).unwrap();
        assert!(f
            .check(&eslint::EslintConfig, &Options::new())
            .unwrap()
            .is_empty());
        assert!(f
            .fix(&eslint::EslintConfig, &Options::new())
            .unwrap()
            .changed_files
            .is_empty());
    }

    #[test]
    fn a_javascript_app_gets_jsx_files_and_no_typescript_rules() {
        let package = "{\n  \"name\": \"js\",\n  \"scripts\": {\n    \"dev\": \"next dev\",\n    \"build\": \"next build\",\n    \"start\": \"next start\",\n    \"lint\": \"eslint\"\n  },\n  \"dependencies\": {\n    \"next\": \"16.4.6\",\n    \"react\": \"19.3.1\",\n    \"react-dom\": \"19.3.1\"\n  },\n  \"devDependencies\": {\n    \"eslint\": \"^9\",\n    \"eslint-config-next\": \"16.4.6\"\n  }\n}\n";
        let f = Fixture::new(&[("package.json", package)], Track::None).unwrap();
        let changed = fix_all(&f);
        assert!(
            changed.contains(&"app/layout.jsx".to_owned()),
            "{changed:?}"
        );
        assert!(!changed.contains(&"tsconfig.json".to_owned()));
        assert!(!f.read("eslint.config.mjs").unwrap().contains("typescript"));
        assert_eq!(findings(&f), []);
    }

    #[test]
    fn an_older_eslint_setup_is_not_given_a_flat_config_it_cannot_load() {
        let package = "{ \"dependencies\": { \"next\": \"15.0.0\", \"react\": \"19.0.0\", \"react-dom\": \"19.0.0\" }, \"devDependencies\": { \"eslint\": \"^8\", \"eslint-config-next\": \"15.0.0\" } }\n";
        let f = Fixture::new(
            &[
                ("package.json", package),
                ("app/layout.tsx", "x"),
                ("next.config.js", "x"),
            ],
            Track::None,
        )
        .unwrap();
        let found: Vec<Diagnostic> = f.check(&eslint::EslintConfig, &Options::new()).unwrap();
        assert_eq!(found[0].fixable, Fixable::Agent);
        assert!(f
            .fix(&eslint::EslintConfig, &Options::new())
            .unwrap()
            .changed_files
            .is_empty());
    }
}
