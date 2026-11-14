//! Tests for the marker-file ecosystems (ADR 0051): each is found by its
//! files, named from its manifest, and given the commands its own tools
//! document; `fscale facts` reports them at the root and in subfolders.

use crate::facts::detect_facts_with;
use crate::testing::{Fixture, Track};

use super::{ecosystem, matching, wrapper, Folder, Missing, ECOSYSTEMS};

fn fixture(files: &[(&str, &str)]) -> Fixture {
    Fixture::new(files, Track::All).unwrap()
}

/// The one project of a fixture repo, as facts reports it.
fn only_project(files: &[(&str, &str)]) -> crate::facts::Project {
    let f = fixture(files);
    let facts = detect_facts_with(f.root(), None).unwrap();
    assert_eq!(facts.projects.len(), 1, "{:?}", facts.projects);
    facts.projects.into_iter().next().unwrap()
}

#[test]
fn every_ecosystem_has_a_kind_of_its_own_and_reasons_for_missing_steps() {
    let mut kinds: Vec<&str> = ECOSYSTEMS.iter().map(|eco| eco.kind).collect();
    kinds.sort_unstable();
    kinds.dedup();
    assert_eq!(kinds.len(), ECOSYSTEMS.len());
    for eco in &ECOSYSTEMS {
        for missing in [eco.typecheck, eco.lint, eco.test] {
            let text = match missing {
                Missing::NotNeeded(t) | Missing::Skipped(t) => t,
            };
            assert!(!text.is_empty(), "{}", eco.kind);
        }
        assert!(!eco.programs.is_empty(), "{}", eco.kind);
    }
}

#[test]
fn go_is_unchanged_by_the_move() {
    let project = only_project(&[("go.mod", "module example.com/acme/tool\n\ngo 1.22\n")]);
    assert_eq!(project.kind, "go");
    assert_eq!(project.name, "tool");
    assert_eq!(project.commands.test.as_deref(), Some("go test ./..."));
    assert_eq!(project.commands.lint.as_deref(), Some("go vet ./..."));
}

#[test]
fn maven_with_its_wrapper_a_linter_and_kotlin() {
    let project = only_project(&[
        (
            "pom.xml",
            "<project><parent><artifactId>spring-boot-starter-parent</artifactId></parent>\n<artifactId>orders</artifactId>\n<build><plugins><plugin><artifactId>spotless-maven-plugin</artifactId></plugin></plugins></build></project>\n",
        ),
        ("mvnw", "#!/bin/sh\n"),
        ("src/main/kotlin/App.kt", "fun main() {}\n"),
    ]);
    assert_eq!(project.kind, "maven");
    assert_eq!(project.name, "orders");
    assert_eq!(project.languages, ["java", "kotlin"]);
    let mvn = wrapper("mvnw");
    assert_eq!(project.commands.test, Some(format!("{mvn} -B test")));
    assert_eq!(
        project.commands.lint,
        Some(format!("{mvn} -B spotless:check"))
    );
    assert_eq!(project.commands.typecheck, None);
}

#[test]
fn an_android_gradle_build_lints_with_android_lint() {
    let project = only_project(&[
        (
            "settings.gradle.kts",
            "rootProject.name = \"Notes\"\ninclude(\":app\")\n",
        ),
        (
            "build.gradle.kts",
            "plugins {\n  id(\"com.android.application\") version \"8.7.0\" apply false\n}\n",
        ),
        (
            "app/build.gradle.kts",
            "plugins { id(\"com.android.application\"); kotlin(\"android\") }\n",
        ),
        ("gradlew", "#!/bin/sh\n"),
        ("gradlew.bat", "@echo off\n"),
    ]);
    assert_eq!(project.kind, "gradle");
    assert_eq!(project.name, "Notes");
    assert_eq!(project.languages, ["kotlin"]);
    assert!(project.tools.contains(&"android".to_owned()));
    let gradle = wrapper("gradlew");
    assert_eq!(project.commands.test, Some(format!("{gradle} test")));
    assert_eq!(project.commands.lint, Some(format!("{gradle} lint")));
}

#[test]
fn a_plain_gradle_java_build_without_a_linter() {
    let project = only_project(&[("build.gradle", "plugins { id 'java' }\n")]);
    assert_eq!(project.languages, ["java"]);
    assert_eq!(project.commands.test.as_deref(), Some("gradle test"));
    assert_eq!(project.commands.lint, None);
    assert!(matches!(
        ecosystem("gradle").unwrap().lint,
        Missing::Skipped(_)
    ));
}

#[test]
fn a_swift_package_with_swiftlint() {
    let project = only_project(&[
        (
            "Package.swift",
            "// swift-tools-version:5.10\nimport PackageDescription\nlet package = Package(\n    name: \"Parsing\",\n    targets: []\n)\n",
        ),
        (".swiftlint.yml", "disabled_rules: []\n"),
    ]);
    assert_eq!(project.kind, "swift");
    assert_eq!(project.name, "Parsing");
    assert_eq!(project.commands.test.as_deref(), Some("swift test"));
    assert_eq!(
        project.commands.lint.as_deref(),
        Some("swiftlint lint --strict")
    );
}

#[test]
fn an_ios_app_builds_for_a_generic_simulator_and_leaves_tests_to_a_person() {
    let project = only_project(&[
        (
            "Notes.xcodeproj/project.pbxproj",
            "buildSettings = { SDKROOT = iphoneos; };\n",
        ),
        (
            "Notes.xcodeproj/xcshareddata/xcschemes/Notes.xcscheme",
            "<Scheme/>\n",
        ),
        ("Notes/ContentView.swift", "import SwiftUI\n"),
    ]);
    assert_eq!(project.kind, "xcode");
    assert_eq!(project.name, "Notes");
    assert_eq!(
        project.commands.typecheck.as_deref(),
        Some("xcodebuild build -project \"Notes.xcodeproj\" -scheme \"Notes\" -destination \"generic/platform=iOS Simulator\" -quiet CODE_SIGNING_ALLOWED=NO")
    );
    assert_eq!(project.commands.test, None);
    assert!(
        matches!(ecosystem("xcode").unwrap().test, Missing::Skipped(t) if t.contains("simulator"))
    );
}

#[test]
fn a_macos_app_with_cocoapods_tests_on_this_mac() {
    let project = only_project(&[
        ("Desk.xcodeproj/project.pbxproj", "SDKROOT = macosx;\n"),
        (
            "Desk.xcworkspace/contents.xcworkspacedata",
            "<Workspace/>\n",
        ),
        (
            "Desk.xcodeproj/xcshareddata/xcschemes/Desk.xcscheme",
            "<Scheme/>\n",
        ),
        ("Podfile", "platform :osx, '14.0'\n"),
    ]);
    assert_eq!(project.package_manager.as_deref(), Some("cocoapods"));
    assert_eq!(project.commands.install.as_deref(), Some("pod install"));
    assert_eq!(
        project.commands.test.as_deref(),
        Some("xcodebuild test -workspace \"Desk.xcworkspace\" -scheme \"Desk\" -destination \"platform=macOS\" -quiet CODE_SIGNING_ALLOWED=NO")
    );
}

#[test]
fn a_swift_package_with_an_xcode_project_beside_it_is_one_package() {
    let f = fixture(&[
        ("Package.swift", "let package = Package(name: \"Kit\")\n"),
        ("Kit.xcodeproj/project.pbxproj", "\n"),
    ]);
    let kinds: Vec<&str> = matching(&Folder::read(f.root(), "."), &[])
        .iter()
        .map(|eco| eco.kind)
        .collect();
    assert_eq!(kinds, ["swift"]);
}

#[test]
fn cmake_configures_builds_and_runs_ctest() {
    let project = only_project(&[
        ("CMakeLists.txt", "cmake_minimum_required(VERSION 3.20)\nproject(Codec LANGUAGES CXX)\nenable_testing()\n"),
        ("src/codec.cpp", "int f() { return 0; }\n"),
        ("vcpkg.json", "{}\n"),
    ]);
    assert_eq!(project.kind, "cmake");
    assert_eq!(project.name, "Codec");
    assert_eq!(project.languages, ["cpp"]);
    assert_eq!(project.package_manager.as_deref(), Some("vcpkg"));
    assert_eq!(
        project.commands.test.as_deref(),
        Some("cmake -S . -B build && cmake --build build && ctest --test-dir build --output-on-failure")
    );
}

#[test]
fn meson_and_cmake_in_one_folder_build_with_cmake() {
    let f = fixture(&[
        ("CMakeLists.txt", "project(a)\n"),
        ("meson.build", "project('a', 'c')\n"),
    ]);
    let kinds: Vec<&str> = matching(&Folder::read(f.root(), "."), &[])
        .iter()
        .map(|eco| eco.kind)
        .collect();
    assert_eq!(kinds, ["cmake"]);
    let meson = only_project(&[
        ("meson.build", "project('zlib-ng', 'c')\n"),
        ("adler.c", "int a;\n"),
    ]);
    assert_eq!(meson.name, "zlib-ng");
    assert_eq!(meson.languages, ["c"]);
    assert_eq!(
        meson.commands.test.as_deref(),
        Some("meson setup build && meson test -C build")
    );
}

#[test]
fn dotnet_needs_one_solution_or_project_to_name() {
    let one = only_project(&[
        ("Shop.sln", "\n"),
        ("Shop.Api/Shop.Api.csproj", "<Project/>\n"),
    ]);
    assert_eq!(one.kind, "dotnet");
    assert_eq!(one.name, "Shop");
    assert_eq!(
        one.commands.test.as_deref(),
        Some("dotnet test \"Shop.sln\"")
    );
    assert_eq!(
        one.commands.lint.as_deref(),
        Some("dotnet format \"Shop.sln\" --verify-no-changes")
    );
    let two = only_project(&[("A.sln", "\n"), ("B.sln", "\n")]);
    assert_eq!(two.commands.test, None);
}

#[test]
fn flutter_and_dart_packages() {
    let flutter = only_project(&[
        (
            "pubspec.yaml",
            "name: notes_app\ndependencies:\n  flutter:\n    sdk: flutter\n",
        ),
        ("test/widget_test.dart", "void main() {}\n"),
    ]);
    assert_eq!(flutter.kind, "dart");
    assert_eq!(flutter.name, "notes_app");
    assert_eq!(flutter.commands.test.as_deref(), Some("flutter test"));
    assert_eq!(flutter.commands.lint.as_deref(), Some("flutter analyze"));
    let dart = only_project(&[("pubspec.yaml", "name: args\n")]);
    assert_eq!(dart.commands.lint.as_deref(), Some("dart analyze"));
    assert_eq!(dart.commands.test, None);
}

#[test]
fn a_makefile_is_a_project_only_for_c_code_or_a_test_target_and_never_beside_another() {
    let c = only_project(&[
        (
            "Makefile",
            "CC := cc\nall: app\n\ntest: app\n\t./app --self-test\n",
        ),
        ("main.c", "int main(void) { return 0; }\n"),
    ]);
    assert_eq!(c.kind, "make");
    assert_eq!(c.languages, ["c"]);
    assert_eq!(c.commands.test.as_deref(), Some("make test"));
    // A Makefile of docs is not a project.
    let docs = fixture(&[("Makefile", "html:\n\tsphinx-build . _build\n")]);
    assert!(detect_facts_with(docs.root(), None)
        .unwrap()
        .projects
        .is_empty());
    // Beside a Go module, the Makefile is the module's.
    let go = fixture(&[
        ("go.mod", "module a\n"),
        ("Makefile", "test:\n\tgo test ./...\n"),
    ]);
    let facts = detect_facts_with(go.root(), None).unwrap();
    assert_eq!(
        facts
            .projects
            .iter()
            .map(|p| p.kind.as_str())
            .collect::<Vec<_>>(),
        ["go"]
    );
}

#[test]
fn projects_in_subfolders_are_found_beside_a_node_app() {
    // A React Native app with its native folders, and a C library.
    let f = fixture(&[
        (
            "package.json",
            "{ \"name\": \"app\", \"scripts\": { \"test\": \"jest\" } }\n",
        ),
        ("android/settings.gradle", "rootProject.name = 'app'\n"),
        (
            "android/build.gradle",
            "buildscript { dependencies { classpath('com.android.tools.build:gradle') } }\n",
        ),
        ("ios/App.xcodeproj/project.pbxproj", "SDKROOT = iphoneos;\n"),
        ("native/CMakeLists.txt", "project(core C)\n"),
    ]);
    let facts = detect_facts_with(f.root(), None).unwrap();
    let mut found: Vec<(String, String)> = facts
        .projects
        .iter()
        .map(|p| (p.root.clone(), p.kind.clone()))
        .collect();
    found.sort();
    assert_eq!(
        found,
        [
            (".".to_owned(), "node".to_owned()),
            ("android".to_owned(), "gradle".to_owned()),
            ("ios".to_owned(), "xcode".to_owned()),
            ("native".to_owned(), "cmake".to_owned()),
        ]
    );
    for language in ["c", "java", "javascript", "swift"] {
        assert!(
            facts.languages.iter().any(|l| l == language),
            "{language}: {:?}",
            facts.languages
        );
    }
}

#[test]
fn the_wrapper_form_follows_the_shell_verify_uses() {
    if cfg!(windows) {
        assert_eq!(wrapper("gradlew"), "gradlew");
    } else {
        assert_eq!(wrapper("gradlew"), "./gradlew");
    }
}

#[test]
fn members_of_a_root_build_are_not_projects_but_nested_go_modules_are() {
    let maven = fixture(&[
        ("pom.xml", "<project><artifactId>parent</artifactId><modules><module>api</module></modules></project>\n"),
        ("api/pom.xml", "<project><artifactId>api</artifactId></project>\n"),
    ]);
    let facts = detect_facts_with(maven.root(), None).unwrap();
    assert_eq!(facts.projects.len(), 1, "{:?}", facts.projects);
    let go = fixture(&[
        ("go.mod", "module a\n"),
        ("tools/go.mod", "module a/tools\n"),
    ]);
    let facts = detect_facts_with(go.root(), None).unwrap();
    assert_eq!(facts.projects.len(), 2, "{:?}", facts.projects);
}

#[test]
fn android_through_a_version_catalog_alias_is_android() {
    // android/nowinandroid's layout.
    let project = only_project(&[
        (
            "settings.gradle.kts",
            "rootProject.name = \"nowinandroid\"\n",
        ),
        (
            "build.gradle.kts",
            "plugins {\n    alias(libs.plugins.android.application) apply false\n}\n",
        ),
    ]);
    assert!(project.tools.contains(&"android".to_owned()));
    assert!(project.commands.lint.unwrap().ends_with(" lint"));
}

#[test]
fn a_swift_package_that_also_builds_with_cmake_is_one_swift_project() {
    // apple/swift-argument-parser's layout: CMake files in its subfolders
    // are part of the root's CMake build, not projects of their own.
    let f = fixture(&[
        (
            "Package.swift",
            "let package = Package(name: \"swift-argument-parser\")\n",
        ),
        (
            "CMakeLists.txt",
            "project(swift-argument-parser LANGUAGES Swift)\n",
        ),
        (
            "Sources/CMakeLists.txt",
            "add_subdirectory(ArgumentParser)\n",
        ),
        ("Sources/ArgumentParser/Parser.swift", "struct P {}\n"),
        ("Tests/CMakeLists.txt", "add_subdirectory(Unit)\n"),
    ]);
    let facts = detect_facts_with(f.root(), None).unwrap();
    let kinds: Vec<(&str, &str)> = facts
        .projects
        .iter()
        .map(|p| (p.root.as_str(), p.kind.as_str()))
        .collect();
    assert_eq!(kinds, [(".", "swift")]);
}

#[test]
fn a_cpp_library_that_also_ships_a_swift_package_is_one_cmake_project() {
    // nlohmann/json's layout: headers three levels down, a Package.swift
    // and a meson.build for other build systems.
    let f = fixture(&[
        (
            "CMakeLists.txt",
            "project(nlohmann_json VERSION 3.12.0 LANGUAGES CXX)\n",
        ),
        (
            "Package.swift",
            "let package = Package(name: \"nlohmann-json\")\n",
        ),
        ("meson.build", "project('nlohmann_json', 'cpp')\n"),
        ("include/nlohmann/detail/value_t.hpp", "#pragma once\n"),
        ("tests/CMakeLists.txt", "add_executable(t t.cpp)\n"),
    ]);
    let facts = detect_facts_with(f.root(), None).unwrap();
    let kinds: Vec<(&str, &str)> = facts
        .projects
        .iter()
        .map(|p| (p.root.as_str(), p.kind.as_str()))
        .collect();
    assert_eq!(kinds, [(".", "cmake")]);
    assert_eq!(facts.projects[0].languages, ["cpp"]);
}

#[test]
fn a_flutter_apps_platform_folders_are_its_own() {
    // flutter/gallery's layout: the runners are not projects.
    let f = fixture(&[
        (
            "pubspec.yaml",
            "name: gallery\ndependencies:\n  flutter:\n    sdk: flutter\n",
        ),
        ("android/build.gradle", "buildscript {}\n"),
        (
            "ios/Runner.xcodeproj/project.pbxproj",
            "SDKROOT = iphoneos;\n",
        ),
        ("linux/CMakeLists.txt", "project(runner CXX)\n"),
        ("windows/CMakeLists.txt", "project(gallery CXX)\n"),
        ("tool/CMakeLists.txt", "project(codegen C)\n"),
    ]);
    let facts = detect_facts_with(f.root(), None).unwrap();
    let kinds: Vec<(&str, &str)> = facts
        .projects
        .iter()
        .map(|p| (p.root.as_str(), p.kind.as_str()))
        .collect();
    // A folder the app does not own is still searched.
    let mut kinds = kinds;
    kinds.sort_unstable();
    assert_eq!(kinds, [(".", "dart"), ("tool", "cmake")]);
}
