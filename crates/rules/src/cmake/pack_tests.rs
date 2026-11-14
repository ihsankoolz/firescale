//! Fixture tests for the `cmake` pack as a whole.

use firescale_core::model::{Diagnostic, Fixable};
use firescale_core::rule::{BuiltinRule, Options};
use firescale_core::testing::Fixture;

use super::fixtures::{files, fixture, project_without, set_up};
use super::pack;

fn rules() -> Vec<Box<dyn BuiltinRule>> {
    pack().rules
}

fn findings(f: &Fixture) -> Vec<Diagnostic> {
    rules()
        .iter()
        .flat_map(|rule| f.check(rule.as_ref(), &Options::new()).unwrap())
        .collect()
}

fn fix_all(f: &Fixture) -> Vec<String> {
    let mut changed = Vec::new();
    for rule in rules() {
        changed.extend(f.fix(rule.as_ref(), &Options::new()).unwrap().changed_files);
    }
    changed
}

#[test]
fn the_pack_has_five_rules_with_skills() {
    let rules = rules();
    assert_eq!(rules.len(), 5);
    for rule in &rules {
        let meta = rule.meta();
        let name = meta.id.trim_start_matches("cmake/");
        assert!(
            rule.skill().contains(&format!("name: {name}")),
            "{}",
            meta.id
        );
    }
}

#[test]
fn a_set_up_project_has_no_findings_and_nothing_to_fix() {
    let f = project_without(|_| false);
    assert_eq!(findings(&f), Vec::<Diagnostic>::new());
    assert_eq!(fix_all(&f), Vec::<String>::new());
}

#[test]
fn an_empty_folder_is_built_by_the_fixes_and_a_second_run_changes_nothing() {
    let f = fixture(&[]);
    let before = findings(&f);
    assert!(
        before.iter().all(|d| d.fixable == Fixable::Deterministic),
        "{before:?}"
    );
    fix_all(&f);
    fix_all(&f);
    assert_eq!(findings(&f), Vec::<Diagnostic>::new());
    assert_eq!(fix_all(&f), Vec::<String>::new());
    for (file, content) in set_up() {
        if file != ".gitignore" {
            assert_eq!(f.read(file).as_deref(), Some(content.as_str()), "{file}");
        }
    }
}

#[test]
fn another_stack_or_another_c_build_gets_nothing() {
    for pairs in [
        vec![("go.mod", "module x\n")],
        vec![("README.md", "x\n"), ("src/app.py", "print(1)\n")],
        vec![("Cargo.toml", "[package]\nname = \"x\"\n")],
        // C code built another way is never given a CMakeLists.txt.
        vec![
            ("Makefile", "all:\n\tcc main.c\n"),
            ("main.c", "int main(void){return 0;}\n"),
        ],
        vec![("configure.ac", "AC_INIT([x], [1])\n")],
        vec![("app.sln", "\n"), ("app/main.cpp", "int main(){}\n")],
        vec![("meson.build", "project('x', 'c')\n")],
        vec![("main.c", "int main(void){return 0;}\n")],
    ] {
        let f = files(&pairs);
        assert_eq!(findings(&f), Vec::<Diagnostic>::new(), "{pairs:?}");
        assert_eq!(fix_all(&f), Vec::<String>::new(), "{pairs:?}");
    }
}

#[test]
fn a_project_in_a_subfolder_is_named_once_and_nothing_is_written() {
    let f = files(&[
        ("README.md", "x\n"),
        (
            "engine/CMakeLists.txt",
            "cmake_minimum_required(VERSION 3.20)\nproject(e)\n",
        ),
        ("engine/src/a.cpp", "int a() { return 1; }\n"),
    ]);
    let found = findings(&f);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].rule_id, "cmake/lists");
    assert_eq!(found[0].file, "engine/CMakeLists.txt");
    assert_eq!(found[0].fixable, Fixable::Manual);
    assert!(fix_all(&f).is_empty());
    // Below another stack's root, it is that stack's business (an Android
    // app's native code, a Python extension).
    let f = files(&[
        (
            "build.gradle.kts",
            "plugins { id(\"com.android.application\") }\n",
        ),
        ("app/src/main/cpp/CMakeLists.txt", "project(n)\n"),
    ]);
    assert_eq!(findings(&f), Vec::<Diagnostic>::new());
}

#[test]
fn every_rule_reads_what_decides_where_the_pack_applies() {
    for rule in rules() {
        let inputs = rule.meta().inputs;
        for shared in [
            "CMakeLists.txt",
            "**/CMakeLists.txt",
            "**/*.cmake",
            "Makefile",
            "*.sln",
            "**/*.cpp",
            "**/*.h",
            "go.mod",
            "**/*.py",
        ] {
            assert!(
                inputs.iter().any(|i| i == shared),
                "{} lacks {shared}",
                rule.meta().id
            );
        }
    }
}

#[test]
fn build_output_is_not_the_projects() {
    // A fetched GoogleTest under build/_deps registers tests and sets a
    // standard; the project's own files do neither.
    let f = files(&[
        ("CMakeLists.txt", "cmake_minimum_required(VERSION 3.20)\nproject(x LANGUAGES CXX)\nadd_executable(x main.cpp)\n"),
        ("main.cpp", "int main() {}\n"),
        ("build/_deps/googletest-src/CMakeLists.txt", "cmake_minimum_required(VERSION 2.8)\nset(CMAKE_CXX_STANDARD 14)\nenable_testing()\nadd_test(NAME t COMMAND t)\n"),
    ]);
    let mut ids: Vec<String> = findings(&f).into_iter().map(|d| d.rule_id).collect();
    ids.sort();
    assert_eq!(ids, ["cmake/standard", "cmake/tests"]);
}

// Found in review: native extensions of other stacks, CUDA and Arduino
// code, and a Visual Studio project one folder down.
#[test]
fn native_code_of_other_stacks_or_builds_is_left_alone() {
    for pairs in [
        vec![
            ("pyproject.toml", "[build-system]\nrequires = [\"scikit-build-core\", \"pybind11\"]\nbuild-backend = \"scikit_build_core.build\"\n"),
            ("CMakeLists.txt", "cmake_minimum_required(VERSION 3.15)\nproject(ext LANGUAGES CXX)\npybind11_add_module(ext src/main.cpp)\n"),
            ("src/main.cpp", "int x;\n"),
        ],
        vec![("kern.cu", "__global__ void k() {}\n"), ("build.sh", "nvcc kern.cu\n")],
        vec![("Blink/Blink.ino", "void setup() {}\n")],
        vec![("win/x.vcxproj", "<Project/>\n")],
    ] {
        let f = files(&pairs);
        assert_eq!(findings(&f), Vec::<Diagnostic>::new(), "{pairs:?}");
        assert_eq!(fix_all(&f), Vec::<String>::new(), "{pairs:?}");
    }
}

// Found in review: a repo with its own .clang-format got the starter's
// code in another style, which its lint target then failed.
#[test]
fn a_repos_own_clang_format_is_kept_and_named() {
    let f = files(&[(".clang-format", "BasedOnStyle: Mozilla\nIndentWidth: 4\n")]);
    fix_all(&f);
    assert_eq!(
        f.read(".clang-format").as_deref(),
        Some("BasedOnStyle: Mozilla\nIndentWidth: 4\n")
    );
    let f = files(&[
        (".clang-format", "BasedOnStyle: Mozilla\n"),
        ("CMakeLists.txt", &super::lists::new_lists()),
    ]);
    let entry = findings(&f)
        .into_iter()
        .find(|d| d.rule_id == "cmake/entry")
        .unwrap();
    assert!(
        entry.message.contains("its own .clang-format"),
        "{}",
        entry.message
    );
}

#[test]
fn a_missing_lint_script_is_written_back() {
    let f = project_without(|p| p == super::lists::LINT_PATH);
    let found = findings(&f);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].file, super::lists::LINT_PATH);
    assert_eq!(fix_all(&f), vec![super::lists::LINT_PATH.to_owned()]);
    assert_eq!(findings(&f), Vec::<Diagnostic>::new());
}
