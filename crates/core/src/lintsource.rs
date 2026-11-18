//! Whether a lint command fscale chose is one the repo asks for itself
//! (ADR 0070).
//!
//! Most lint commands are found through something the repo wrote: a package
//! script, a build target, a linter's config file, a plugin in the build
//! file. Five are not: fscale runs them for every project of their kind.
//! Such a command is a guess until the repo shows that it holds itself to
//! it, and a guessed command only reports: it runs, it is shown, and it
//! never fails the run.

use std::fs;
use std::path::{Path, PathBuf};

/// One linter whose command fscale runs without being asked.
struct Linter {
    /// True for this linter's command, as `fscale facts` writes it.
    is: fn(&str) -> bool,
    /// Its config files: one in the project's folder, or in a folder above
    /// it up to the repo root, declares it.
    configs: &'static [&'static str],
    /// A build file (in the project's folder or above it) and a text: a
    /// line of the file that starts with the text declares it.
    manifest_lines: &'static [(&'static str, &'static str)],
    /// True when the folders directly below the project's folder count
    /// too: a Gradle build keeps a module's settings in the module's folder.
    modules: bool,
    /// True for a line of a CI file that runs it.
    ci: fn(&str) -> bool,
    /// What was looked for, for the summary.
    looked_for: &'static str,
}

fn first_word(command: &str) -> &str {
    command.split_whitespace().next().unwrap_or_default()
}

/// True when `line` holds `words` next to each other, as whole words.
fn has_words(line: &str, words: &[&str]) -> bool {
    let parts: Vec<&str> = line
        .split(|c: char| c.is_whitespace() || "\"'`;&|()".contains(c))
        .filter(|part| !part.is_empty())
        .collect();
    parts.windows(words.len()).any(|window| window == words)
}

fn gradle_program(word: &str) -> bool {
    let name = word.rsplit(['/', '\\']).next().unwrap_or(word);
    matches!(name, "gradle" | "gradlew" | "gradlew.bat")
}

const LINTERS: [Linter; 5] = [
    // Rust: clippy with warnings as errors.
    Linter {
        is: |command| command.starts_with("cargo clippy"),
        configs: &["clippy.toml", ".clippy.toml"],
        manifest_lines: &[("Cargo.toml", "[lints"), ("Cargo.toml", "[workspace.lints")],
        modules: false,
        ci: |line| has_words(line, &["cargo", "clippy"]),
        looked_for:
            "clippy.toml, a [lints] table in Cargo.toml, and `cargo clippy` in a CI file",
    },
    // Go: vet.
    Linter {
        is: |command| command.starts_with("go vet"),
        configs: &[],
        manifest_lines: &[],
        modules: false,
        ci: |line| has_words(line, &["go", "vet"]),
        looked_for: "`go vet` in a CI file",
    },
    // .NET: the formatter in its checking mode. An `.editorconfig` is not
    // a sign: editors read it too, and both .NET repos of the real-repos
    // review have one and run no formatter.
    Linter {
        is: |command| command.starts_with("dotnet format"),
        configs: &[],
        manifest_lines: &[],
        modules: false,
        ci: |line| has_words(line, &["dotnet", "format"]) || has_words(line, &["dotnet-format"]),
        looked_for: "`dotnet format` in a CI file",
    },
    // Dart and Flutter: the analyzer.
    Linter {
        is: |command| matches!(command, "dart analyze" | "flutter analyze"),
        configs: &["analysis_options.yaml"],
        manifest_lines: &[],
        modules: false,
        ci: |line| {
            has_words(line, &["dart", "analyze"]) || has_words(line, &["flutter", "analyze"])
        },
        looked_for:
            "analysis_options.yaml, and `flutter analyze` or `dart analyze` in a CI file",
    },
    // Android: the Gradle plugin's `lint` task.
    Linter {
        is: |command| gradle_program(first_word(command)) && command.ends_with(" lint"),
        configs: &["lint.xml"],
        manifest_lines: &[
            ("build.gradle", "lint {"),
            ("build.gradle", "lintOptions"),
            ("build.gradle.kts", "lint {"),
            ("build.gradle.kts", "lintOptions"),
        ],
        modules: true,
        ci: |line| {
            let mut words = line
                .split(|c: char| c.is_whitespace() || "\"'`;&|()".contains(c))
                .filter(|part| !part.is_empty());
            words.any(gradle_program)
                && words.any(|word| {
                    // `lint`, `lintDebug`, `:app:lint`, `:app:lintRelease`.
                    word.rsplit(':').next().is_some_and(|task| task.starts_with("lint"))
                })
        },
        looked_for:
            "lint.xml and a `lint {` block in a Gradle build file (in the project, a module of it or a folder above), and a Gradle `lint` task in a CI file",
    },
];

/// The CI files of other systems than GitHub Actions, from the repo root.
const CI_FILES: [&str; 9] = [
    ".gitlab-ci.yml",
    ".circleci/config.yml",
    "azure-pipelines.yml",
    "bitbucket-pipelines.yml",
    ".travis.yml",
    "Jenkinsfile",
    ".buildkite/pipeline.yml",
    "appveyor.yml",
    ".appveyor.yml",
];

/// The repo's CI files: every `.yml` and `.yaml` file directly in
/// `.github/workflows`, and [`CI_FILES`].
fn ci_files(root: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = fs::read_dir(root.join(".github").join("workflows")) {
        for entry in entries.filter_map(std::result::Result::ok) {
            let path = entry.path();
            let workflow = path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext == "yml" || ext == "yaml");
            if workflow && path.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();
    files.extend(
        CI_FILES
            .iter()
            .map(|file| {
                file.split('/')
                    .fold(root.to_path_buf(), |path, part| path.join(part))
            })
            .filter(|path| path.is_file()),
    );
    files
}

/// True when a line of a CI file runs the linter. A line that is only a
/// comment does not count.
fn ci_runs(root: &Path, runs: fn(&str) -> bool) -> bool {
    ci_files(root).iter().any(|file| {
        fs::read_to_string(file).is_ok_and(|text| {
            text.lines()
                .map(str::trim)
                .filter(|line| !line.starts_with('#') && !line.starts_with("//"))
                .any(runs)
        })
    })
}

/// The project's folder and every folder above it, up to the repo root.
fn folders_up(root: &Path, folder: &str) -> Vec<PathBuf> {
    let mut dirs = vec![root.to_path_buf()];
    if folder != "." {
        let mut dir = root.to_path_buf();
        for part in folder.split('/').filter(|part| !part.is_empty()) {
            dir = dir.join(part);
            dirs.push(dir.clone());
        }
    }
    dirs
}

/// Why `command`, the lint command chosen for the project in `folder`
/// (`.` for the repo root), is a guess: `None` when the repo declares it,
/// or when it is not one of the commands fscale runs unasked. The caller
/// asks only for a command that facts detected, never for one from the
/// config.
#[must_use]
pub fn guessed(root: &Path, folder: &str, command: &str) -> Option<String> {
    let linter = LINTERS.iter().find(|linter| (linter.is)(command))?;
    let mut dirs = folders_up(root, folder);
    if linter.modules {
        let own = dirs.last().cloned().unwrap_or_else(|| root.to_path_buf());
        if let Ok(entries) = fs::read_dir(&own) {
            let mut below: Vec<PathBuf> = entries
                .filter_map(std::result::Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .collect();
            below.sort();
            dirs.extend(below);
        }
    }
    let has_config = dirs
        .iter()
        .any(|dir| linter.configs.iter().any(|file| dir.join(file).is_file()));
    let in_manifest = || {
        dirs.iter().any(|dir| {
            linter.manifest_lines.iter().any(|(file, start)| {
                fs::read_to_string(dir.join(file))
                    .is_ok_and(|text| text.lines().any(|line| line.trim().starts_with(start)))
            })
        })
    };
    if has_config || in_manifest() || ci_runs(root, linter.ci) {
        return None;
    }
    Some(format!(
        "fscale guessed this command, and nothing in this repo asks for it (looked for {})",
        linter.looked_for
    ))
}

/// The config that makes a guessed lint command block, as the text to add
/// to `firescale.config.json`: `"verify": { "lint": "go vet ./..." }`, or
/// the same under `verify.projects` for a project below the root.
#[must_use]
pub fn blocking_config(project: Option<&str>, command: &str) -> String {
    let command = serde_json::to_string(command).unwrap_or_else(|_| format!("\"{command}\""));
    match project.filter(|folder| *folder != ".") {
        Some(folder) => {
            let folder = serde_json::to_string(folder).unwrap_or_else(|_| format!("\"{folder}\""));
            format!("\"verify\": {{ \"projects\": {{ {folder}: {{ \"lint\": {command} }} }} }}")
        }
        None => format!("\"verify\": {{ \"lint\": {command} }}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (file, text) in files {
            let path = file
                .split('/')
                .fold(dir.path().to_path_buf(), |path, part| path.join(part));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        dir
    }

    const WORKFLOW: &str = ".github/workflows/ci.yml";

    fn workflow(run: &str) -> String {
        format!("on: [push]\njobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      - run: {run}\n")
    }

    #[test]
    fn a_command_a_script_a_target_or_a_config_file_gave_is_never_a_guess() {
        let dir = repo(&[("go.mod", "module m\n")]);
        for command in [
            "pnpm run lint",
            "npx --no eslint .",
            "python -m ruff check .",
            "make lint",
            "swiftlint lint --strict",
            "swift format lint --recursive --strict .",
            "mvn -B checkstyle:check",
            "./gradlew spotlessCheck",
            "./gradlew ktlintCheck",
            "cmake -S . -B build && cmake --build build --config Debug --target lint",
        ] {
            assert_eq!(guessed(dir.path(), ".", command), None, "{command}");
        }
    }

    #[test]
    fn each_command_fscale_runs_unasked_is_a_guess_in_a_bare_repo() {
        let dir = repo(&[("README.md", "")]);
        for (command, looked_for) in [
            (
                "cargo clippy --all-targets -- -D warnings",
                "clippy.toml, a [lints] table in Cargo.toml, and `cargo clippy` in a CI file",
            ),
            (
                "cargo clippy --workspace --all-targets -- -D warnings",
                "clippy.toml, a [lints] table in Cargo.toml, and `cargo clippy` in a CI file",
            ),
            ("go vet ./...", "`go vet` in a CI file"),
            (
                "dotnet format \"App.sln\" --verify-no-changes",
                "`dotnet format` in a CI file",
            ),
            (
                "dart analyze",
                "analysis_options.yaml, and `flutter analyze` or `dart analyze` in a CI file",
            ),
            (
                "flutter analyze",
                "analysis_options.yaml, and `flutter analyze` or `dart analyze` in a CI file",
            ),
            (
                "./gradlew lint",
                "lint.xml and a `lint {` block in a Gradle build file (in the project, a module of it or a folder above), and a Gradle `lint` task in a CI file",
            ),
            (
                "gradle lint",
                "lint.xml and a `lint {` block in a Gradle build file (in the project, a module of it or a folder above), and a Gradle `lint` task in a CI file",
            ),
        ] {
            assert_eq!(
                guessed(dir.path(), "app", command),
                Some(format!(
                    "fscale guessed this command, and nothing in this repo asks for it (looked for {looked_for})"
                )),
                "{command}"
            );
        }
    }

    #[test]
    fn a_config_file_of_the_linter_in_the_folder_or_above_it_declares_it() {
        for (file, command) in [
            ("clippy.toml", "cargo clippy --all-targets -- -D warnings"),
            (".clippy.toml", "cargo clippy --all-targets -- -D warnings"),
            ("analysis_options.yaml", "dart analyze"),
            ("analysis_options.yaml", "flutter analyze"),
            ("lint.xml", "./gradlew lint"),
        ] {
            let above = repo(&[(file, "")]);
            assert_eq!(guessed(above.path(), "a/b", command), None, "{file} above");
            let beside = repo(&[(&format!("a/b/{file}"), "")]);
            assert_eq!(
                guessed(beside.path(), "a/b", command),
                None,
                "{file} beside"
            );
            // One in a sibling folder, or two folders below the project,
            // says nothing.
            let elsewhere = repo(&[
                (&format!("a/c/{file}"), ""),
                (&format!("a/b/d/e/{file}"), ""),
            ]);
            assert!(
                guessed(elsewhere.path(), "a/b", command).is_some(),
                "{file} elsewhere"
            );
        }
    }

    #[test]
    fn a_table_or_a_block_in_the_build_file_declares_it() {
        let clippy = "cargo clippy --all-targets -- -D warnings";
        for manifest in [
            "[package]\nname = \"a\"\n\n[lints.clippy]\npedantic = \"warn\"\n",
            "[package]\nname = \"a\"\n\n[lints]\nworkspace = true\n",
            "[workspace]\nmembers = [\"a\"]\n\n[workspace.lints.clippy]\nall = \"deny\"\n",
        ] {
            let dir = repo(&[("Cargo.toml", manifest)]);
            assert_eq!(guessed(dir.path(), "a", clippy), None, "{manifest}");
        }
        let plain = repo(&[("Cargo.toml", "[package]\nname = \"a\"\n# [lints] one day\n")]);
        assert!(guessed(plain.path(), ".", clippy).is_some());

        for (file, text) in [
            (
                "build.gradle",
                "android {\n    lint {\n        abortOnError true\n    }\n}\n",
            ),
            (
                "build.gradle.kts",
                "android {\n    lintOptions {\n        isAbortOnError = true\n    }\n}\n",
            ),
        ] {
            let dir = repo(&[(&format!("app/{file}"), text)]);
            assert_eq!(guessed(dir.path(), "app", "./gradlew lint"), None, "{file}");
        }
    }

    #[test]
    fn a_gradle_modules_folder_counts_and_other_linters_look_no_lower() {
        let module = repo(&[(
            "app/build.gradle.kts",
            "android {\n    lint {\n        warningsAsErrors = true\n    }\n}\n",
        )]);
        assert_eq!(guessed(module.path(), ".", "./gradlew lint"), None);
        let below = repo(&[("pkg/example/analysis_options.yaml", "")]);
        assert!(guessed(below.path(), "pkg", "dart analyze").is_some());
    }

    /// The commands are matched by their text, so this ties them to what
    /// `fscale facts` writes: a new form of one of them fails here.
    #[test]
    fn the_commands_facts_detects_are_the_ones_read_here() {
        let dir = repo(&[
            (
                "svc/Cargo.toml",
                "[package]\nname = \"svc\"\nversion = \"0.1.0\"\n",
            ),
            ("svc/src/lib.rs", ""),
            ("tool/go.mod", "module example.com/tool\n"),
            ("net/App.sln", ""),
            ("pkg/pubspec.yaml", "name: pkg\n"),
            (
                "ui/pubspec.yaml",
                "name: ui\ndependencies:\n  flutter:\n    sdk: flutter\n",
            ),
            (
                "droid/settings.gradle.kts",
                "rootProject.name = \"droid\"\n",
            ),
            (
                "droid/build.gradle.kts",
                "plugins { id(\"com.android.application\") }\n",
            ),
        ]);
        let facts = crate::facts::detect_facts_with(dir.path(), None).unwrap();
        let mut seen: Vec<(String, String)> = Vec::new();
        for project in &facts.projects {
            let lint = project
                .commands
                .lint
                .clone()
                .unwrap_or_else(|| panic!("{} has no lint command", project.root));
            assert!(
                guessed(dir.path(), &project.root, &lint).is_some(),
                "{}: {lint}",
                project.root
            );
            seen.push((project.root.clone(), lint));
        }
        seen.sort();
        assert_eq!(
            seen,
            [
                ("droid".to_owned(), "gradle lint".to_owned()),
                (
                    "net".to_owned(),
                    "dotnet format \"App.sln\" --verify-no-changes".to_owned()
                ),
                ("pkg".to_owned(), "dart analyze".to_owned()),
                (
                    "svc".to_owned(),
                    "cargo clippy --all-targets -- -D warnings".to_owned()
                ),
                ("tool".to_owned(), "go vet ./...".to_owned()),
                ("ui".to_owned(), "flutter analyze".to_owned()),
            ]
        );
    }

    #[test]
    fn the_command_in_a_ci_file_declares_it() {
        for (run, command) in [
            (
                "cargo clippy --workspace -- -D warnings",
                "cargo clippy --all-targets -- -D warnings",
            ),
            ("go vet ./...", "go vet ./..."),
            ("cd tools && go vet ./... && go test ./...", "go vet ./..."),
            (
                "dotnet format --verify-no-changes",
                "dotnet format \"App.sln\" --verify-no-changes",
            ),
            (
                "dotnet-format --check",
                "dotnet format \"App.sln\" --verify-no-changes",
            ),
            ("flutter analyze --no-pub", "flutter analyze"),
            ("dart analyze --fatal-infos", "flutter analyze"),
            ("dart analyze", "dart analyze"),
            ("./gradlew lintDebug", "./gradlew lint"),
            ("./gradlew :app:lint test", "./gradlew lint"),
            ("gradle lint", "gradle lint"),
        ] {
            let dir = repo(&[(WORKFLOW, &workflow(run))]);
            assert_eq!(guessed(dir.path(), "app", command), None, "{run}");
        }
        // Other CI systems, read from the repo root.
        for file in CI_FILES {
            let dir = repo(&[(file, "script:\n  - go vet ./...\n")]);
            assert_eq!(guessed(dir.path(), ".", "go vet ./..."), None, "{file}");
        }
    }

    #[test]
    fn a_ci_file_that_runs_something_else_declares_nothing() {
        for (run, command) in [
            (
                "dotnet test",
                "dotnet format \"App.sln\" --verify-no-changes",
            ),
            (
                "dotnet build --no-format",
                "dotnet format \"App.sln\" --verify-no-changes",
            ),
            ("go test ./...", "go vet ./..."),
            ("go veto", "go vet ./..."),
            ("cargo test", "cargo clippy --all-targets -- -D warnings"),
            ("flutter test", "flutter analyze"),
            ("./gradlew test", "./gradlew lint"),
            ("npm run lint", "./gradlew lint"),
            ("./gradlew ktlintCheck", "./gradlew lint"),
        ] {
            let dir = repo(&[(WORKFLOW, &workflow(run))]);
            assert!(guessed(dir.path(), ".", command).is_some(), "{run}");
        }
        // A comment is not a step, and a file elsewhere is not a CI file.
        let dir = repo(&[
            (WORKFLOW, "on: [push]\n# - run: go vet ./...\n"),
            (".github/ci-notes.yml", "- run: go vet ./...\n"),
            ("docs/.gitlab-ci.yml", "script:\n  - go vet ./...\n"),
        ]);
        assert!(guessed(dir.path(), ".", "go vet ./...").is_some());
    }

    #[test]
    fn the_config_line_names_the_project_and_quotes_the_command() {
        assert_eq!(
            blocking_config(None, "go vet ./..."),
            "\"verify\": { \"lint\": \"go vet ./...\" }"
        );
        assert_eq!(
            blocking_config(Some("."), "dotnet format \"App.sln\" --verify-no-changes"),
            "\"verify\": { \"lint\": \"dotnet format \\\"App.sln\\\" --verify-no-changes\" }"
        );
        assert_eq!(
            blocking_config(Some("packages/app"), "flutter analyze"),
            "\"verify\": { \"projects\": { \"packages/app\": { \"lint\": \"flutter analyze\" } } }"
        );
    }
}
