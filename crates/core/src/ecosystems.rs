//! Ecosystems found by a marker file (ADR 0051): Go, Maven, Gradle (Java,
//! Kotlin, Android), SwiftPM, Xcode, CMake, Meson, Make, .NET and
//! Dart/Flutter. Each is one entry of [`ECOSYSTEMS`]: how to recognise its
//! folder, what to call the project, the commands verify runs, why a step
//! with no command is not needed or skipped, the programs it needs, and the
//! build output git should ignore. `facts`, `verify`, `doctor`, `setup`,
//! the step cache and `core/gitignore-basics` read the table; adding an
//! ecosystem is adding an entry.
//!
//! Node, Python and Rust have workspaces, interpreters and lockfile rules of
//! their own and stay in `facts`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::facts::Commands;

/// The files and folders directly in a folder, sorted.
#[derive(Debug, Clone, Default)]
pub struct Folder {
    pub path: PathBuf,
    pub files: Vec<String>,
    pub dirs: Vec<String>,
}

impl Folder {
    /// Reads `dir` (repo-relative, `/`-separated, `.` for the root).
    #[must_use]
    pub fn read(root: &Path, dir: &str) -> Self {
        let path = if dir == "." {
            root.to_path_buf()
        } else {
            dir.split('/')
                .fold(root.to_path_buf(), |path, part| path.join(part))
        };
        let mut files = Vec::new();
        let mut dirs = Vec::new();
        if let Ok(entries) = fs::read_dir(&path) {
            for entry in entries.filter_map(std::result::Result::ok) {
                let Ok(name) = entry.file_name().into_string() else {
                    continue;
                };
                match entry.file_type() {
                    Ok(kind) if kind.is_dir() => dirs.push(name),
                    Ok(kind) if kind.is_file() => files.push(name),
                    _ => {}
                }
            }
        }
        files.sort();
        dirs.sort();
        Self { path, files, dirs }
    }

    fn has(&self, name: &str) -> bool {
        self.files.iter().any(|file| file == name)
    }

    fn has_dir(&self, name: &str) -> bool {
        self.dirs.iter().any(|dir| dir == name)
    }

    fn text(&self, name: &str) -> Option<String> {
        fs::read_to_string(self.path.join(name))
            .ok()
            .map(|text| text.trim_start_matches('\u{feff}').to_owned())
    }

    fn files_ending(&self, suffix: &str) -> Vec<&str> {
        self.files
            .iter()
            .filter(|f| f.ends_with(suffix))
            .map(String::as_str)
            .collect()
    }

    fn dirs_ending(&self, suffix: &str) -> Vec<&str> {
        self.dirs
            .iter()
            .filter(|d| d.ends_with(suffix))
            .map(String::as_str)
            .collect()
    }
}

/// What a project's folder says about it.
#[derive(Debug, Clone, Default)]
pub struct Found {
    /// Subfolders that belong to this project, which are not looked at
    /// for projects of their own (a Flutter app's platform runners).
    pub claims: Vec<String>,
    pub languages: Vec<String>,
    pub package_manager: Option<String>,
    pub lockfiles: Vec<String>,
    pub tools: Vec<String>,
    pub commands: Commands,
}

/// Why verify runs nothing for a step that has no command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Missing {
    /// Another step already covers it (a build that type-checks).
    NotNeeded(&'static str),
    /// Nothing was found to run: shown as skipped, with what was looked for.
    Skipped(&'static str),
}

/// A program a project needs, for `fscale doctor`.
#[derive(Debug, Clone, Copy)]
pub struct Tool {
    /// Any one of these will do (`cc`, `gcc`, `clang`, `cl`).
    pub programs: &'static [&'static str],
    pub version_args: &'static [&'static str],
    /// What to do when none is found.
    pub install: &'static str,
}

/// One ecosystem found by a marker file.
pub struct Ecosystem {
    /// The project kind, as `fscale facts` reports it.
    pub kind: &'static str,
    /// True when the folder holds a project of this kind.
    pub marks: fn(&Folder) -> bool,
    /// Kinds that take the folder when they are there too: a Makefile
    /// beside a `pom.xml` belongs to Maven.
    pub yields_to: &'static [&'static str],
    /// The one operating system its builds run on (`macos` for Xcode):
    /// elsewhere its steps are not needed, and doctor asks for nothing.
    pub only_on: Option<&'static str>,
    /// True when a project of this kind builds the same kind's projects in
    /// its subfolders as members (Gradle's `include`, Maven's `<modules>`,
    /// CMake's `add_subdirectory`, a .NET solution): they are not projects
    /// of their own. Go, Swift and Dart packages nest as separate ones.
    pub contains_nested: bool,
    /// The manifest files of a build that takes nested folders as members:
    /// a folder with one is a member when a folder above it has one too,
    /// whatever project that folder above is.
    pub manifests: &'static [&'static str],
    /// The project's name from its manifest, when it has one.
    pub name: fn(&Folder) -> Option<String>,
    pub found: fn(&Folder) -> Found,
    pub typecheck: Missing,
    pub lint: Missing,
    pub test: Missing,
    /// Programs the commands start, which a project's own install does not
    /// provide (verify's "setup needed").
    pub programs: &'static [&'static str],
    /// What `fscale doctor` checks for on this machine.
    pub doctor: fn(&Folder) -> Vec<Tool>,
    /// Build output and local state `core/gitignore-basics` asks git to ignore.
    pub ignored: &'static [&'static str],
}

/// The files whose presence decides which ecosystems a folder holds, as
/// globs: the inputs of every rule that reads facts, so adding one
/// invalidates a cached result.
pub const MARKER_GLOBS: [&str; 22] = [
    "**/go.mod",
    "**/pom.xml",
    "**/build.gradle",
    "**/build.gradle.kts",
    "**/settings.gradle",
    "**/settings.gradle.kts",
    "**/Package.swift",
    "**/*.xcodeproj/project.pbxproj",
    "**/*.xcworkspace/contents.xcworkspacedata",
    "**/CMakeLists.txt",
    "**/meson.build",
    "**/*.sln",
    "**/*.slnx",
    "**/*.csproj",
    "**/*.fsproj",
    "**/*.vbproj",
    "**/pubspec.yaml",
    "**/Makefile",
    "**/makefile",
    "**/GNUmakefile",
    "**/binding.gyp",
    "**/Podfile",
];

/// The ecosystems, in the order a folder is matched.
pub const ECOSYSTEMS: [Ecosystem; 10] = [
    GO, MAVEN, GRADLE, SWIFT, XCODE, CMAKE, MESON, DOTNET, DART, MAKE,
];

/// The entry for a project kind.
#[must_use]
pub fn ecosystem(kind: &str) -> Option<&'static Ecosystem> {
    ECOSYSTEMS.iter().find(|eco| eco.kind == kind)
}

/// Every ecosystem a folder holds a project of. `present` names kinds
/// already found there by other means (node, rust, python).
#[must_use]
pub fn matching(folder: &Folder, present: &[&str]) -> Vec<&'static Ecosystem> {
    let mut out: Vec<&'static Ecosystem> = Vec::new();
    for eco in &ECOSYSTEMS {
        if !(eco.marks)(folder) {
            continue;
        }
        let taken = eco.yields_to.iter().any(|other| {
            present.contains(other)
                || out.iter().any(|found| found.kind == *other)
                || (*other == "*" && (!present.is_empty() || !out.is_empty()))
        });
        if !taken {
            out.push(eco);
        }
    }
    out
}

/// A wrapper script in the project's folder: `./gradlew`. The same text on
/// every machine, so files that quote it (AGENTS.md, agent permissions)
/// do not change with the OS; verify runs it as `.\gradlew` with `cmd` on
/// Windows, which finds `gradlew.bat`.
fn wrapper(name: &str) -> String {
    format!("./{name}")
}

/// Source languages of C-family files directly in the folder or in `src/`,
/// `lib/`, `include/`: `c`, `cpp`, `objective-c`.
fn c_family(folder: &Folder) -> Vec<String> {
    // The folder's own files, and source folders three levels down
    // (`include/nlohmann/detail/*.hpp`), at most 2,000 names.
    let mut names: Vec<String> = folder.files.clone();
    let mut queue: Vec<(PathBuf, usize)> = ["src", "lib", "include", "source"]
        .iter()
        .filter(|sub| folder.has_dir(sub))
        .map(|sub| (folder.path.join(sub), 1))
        .collect();
    while let Some((dir, depth)) = queue.pop() {
        if names.len() > 2_000 {
            break;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(std::result::Result::ok) {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            match entry.file_type() {
                Ok(kind) if kind.is_dir() && depth < 3 => queue.push((entry.path(), depth + 1)),
                Ok(kind) if kind.is_file() => names.push(name),
                _ => {}
            }
        }
    }
    let ext = |name: &String| {
        name.rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default()
    };
    let mut languages = Vec::new();
    if names.iter().any(|n| ext(n) == "c") {
        languages.push("c".to_owned());
    }
    if names.iter().any(|n| {
        matches!(
            ext(n).as_str(),
            "cpp" | "cc" | "cxx" | "c++" | "hpp" | "hh" | "hxx"
        )
    }) {
        languages.push("cpp".to_owned());
    }
    if names.iter().any(|n| matches!(ext(n).as_str(), "m" | "mm")) {
        languages.push("objective-c".to_owned());
    }
    languages
}

/// The first `"…"` or `'…'` string after `key` in `text`.
fn quoted_after(text: &str, key: &str) -> Option<String> {
    let at = text.find(key)? + key.len();
    let rest = &text[at..];
    let start = rest.find(['"', '\''])?;
    let quote = rest[start..].chars().next()?;
    let inner = &rest[start + 1..];
    let end = inner.find(quote)?;
    let value = inner[..end].trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// Lines of a file without `//` and `#` comments, for keyword searches.
fn code(text: &str) -> String {
    text.lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") || trimmed.starts_with('#') {
                ""
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ------------------------------------------------------------------ Go

const GO: Ecosystem = Ecosystem {
    kind: "go",
    marks: |f| f.has("go.mod"),
    yields_to: &[],
    only_on: None,
    contains_nested: false,
    manifests: &[],
    name: |f| {
        f.text("go.mod").and_then(|text| {
            text.lines()
                .find_map(|line| line.trim().strip_prefix("module "))
                .map(|module| {
                    let module = module.trim().trim_matches('"');
                    module.rsplit('/').next().unwrap_or(module).to_owned()
                })
                .filter(|name| !name.is_empty())
        })
    },
    found: |f| Found {
        claims: Vec::new(),
        languages: vec!["go".to_owned()],
        package_manager: Some("go".to_owned()),
        lockfiles: f.files.iter().filter(|n| *n == "go.sum").cloned().collect(),
        tools: Vec::new(),
        commands: Commands {
            build: Some("go build ./...".to_owned()),
            test: Some("go test ./...".to_owned()),
            lint: Some("go vet ./...".to_owned()),
            ..Commands::default()
        },
    },
    typecheck: Missing::NotNeeded("go vet already type-checks"),
    lint: Missing::Skipped("no linter was found"),
    test: Missing::Skipped("no test command was found"),
    programs: &["go"],
    // The go.mod version is checked by doctor itself.
    doctor: |_| Vec::new(),
    ignored: &[],
};

// --------------------------------------------------------------- Maven

const JDK: Tool = Tool {
    programs: &["java"],
    version_args: &["-version"],
    install:
        "install a JDK (for example Temurin from https://adoptium.net) and put java on the PATH",
};

/// The project's own `<artifactId>`, not its parent's.
fn maven_name(f: &Folder) -> Option<String> {
    let text = f.text("pom.xml")?;
    let without_parent = match (text.find("<parent>"), text.find("</parent>")) {
        (Some(a), Some(b)) if a < b => format!("{}{}", &text[..a], &text[b..]),
        _ => text,
    };
    let start = without_parent.find("<artifactId>")? + "<artifactId>".len();
    let end = without_parent[start..].find("</artifactId>")? + start;
    let name = without_parent[start..end].trim();
    (!name.is_empty() && !name.contains("${")).then(|| name.to_owned())
}

const MAVEN: Ecosystem = Ecosystem {
    kind: "maven",
    marks: |f| f.has("pom.xml"),
    yields_to: &[],
    only_on: None,
    contains_nested: true,
    manifests: &["pom.xml"],
    name: maven_name,
    found: |f| {
        let pom = code(&f.text("pom.xml").unwrap_or_default());
        let mvn = if f.has("mvnw") || f.has("mvnw.cmd") {
            wrapper("mvnw")
        } else {
            "mvn".to_owned()
        };
        let lint = if pom.contains("spotless-maven-plugin") {
            Some(format!("{mvn} -B spotless:check"))
        } else if pom.contains("maven-checkstyle-plugin") {
            Some(format!("{mvn} -B checkstyle:check"))
        } else if pom.contains("maven-pmd-plugin") {
            Some(format!("{mvn} -B pmd:check"))
        } else {
            None
        };
        let src = Folder::read(&f.path, "src/main");
        let mut languages = vec!["java".to_owned()];
        if src.has_dir("kotlin") || pom.contains("kotlin-maven-plugin") {
            languages.push("kotlin".to_owned());
        }
        Found {
            claims: Vec::new(),
            languages,
            package_manager: Some("maven".to_owned()),
            lockfiles: Vec::new(),
            tools: Vec::new(),
            commands: Commands {
                build: Some(format!("{mvn} -B package -DskipTests")),
                test: Some(format!("{mvn} -B test")),
                lint,
                ..Commands::default()
            },
        }
    },
    typecheck: Missing::NotNeeded("mvn test compiles the code first"),
    lint: Missing::Skipped(
        "no linter was found (looked for Spotless, Checkstyle and PMD in pom.xml)",
    ),
    test: Missing::Skipped("no test command was found"),
    programs: &["mvn", "java"],
    doctor: |f| {
        if f.has("mvnw") || f.has("mvnw.cmd") {
            vec![JDK]
        } else {
            vec![
                Tool {
                    programs: &["mvn"],
                    version_args: &["--version"],
                    install: "install Maven from https://maven.apache.org, or add the Maven wrapper (mvnw) to the project",
                },
                JDK,
            ]
        }
    },
    ignored: &["target"],
};

// -------------------------------------------------------------- Gradle

const GRADLE_FILES: [&str; 4] = [
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
];

/// The build scripts of the folder and of its direct subfolders (an
/// Android app's plugins are in `app/build.gradle.kts`).
fn gradle_scripts(f: &Folder) -> String {
    let mut text = String::new();
    for name in &GRADLE_FILES {
        if let Some(t) = f.text(name) {
            text.push_str(&code(&t));
            text.push('\n');
        }
    }
    for dir in &f.dirs {
        if dir.starts_with('.') || dir == "build" || dir == "gradle" {
            continue;
        }
        let sub = Folder::read(&f.path, dir);
        for name in ["build.gradle", "build.gradle.kts"] {
            if let Some(t) = sub.text(name) {
                text.push_str(&code(&t));
                text.push('\n');
            }
        }
    }
    text
}

const GRADLE: Ecosystem = Ecosystem {
    kind: "gradle",
    marks: |f| GRADLE_FILES.iter().any(|name| f.has(name)),
    yields_to: &[],
    only_on: None,
    contains_nested: true,
    manifests: &GRADLE_FILES,
    name: |f| {
        ["settings.gradle.kts", "settings.gradle"]
            .iter()
            .find_map(|name| quoted_after(&f.text(name)?, "rootProject.name"))
    },
    found: |f| {
        let scripts = gradle_scripts(f);
        let gradle = if f.has("gradlew") || f.has("gradlew.bat") {
            wrapper("gradlew")
        } else {
            "gradle".to_owned()
        };
        // Applied by id, through a version catalog alias
        // (`alias(libs.plugins.android.application)`), or by the
        // buildscript classpath.
        let android = scripts.contains("com.android.application")
            || scripts.contains("com.android.library")
            || scripts.contains("com.android.tools.build")
            || scripts.contains("plugins.android.application")
            || scripts.contains("plugins.android.library");
        let lint = if android {
            Some(format!("{gradle} lint"))
        } else if scripts.contains("com.diffplug.spotless") || scripts.contains("spotless {") {
            Some(format!("{gradle} spotlessCheck"))
        } else if scripts.contains("org.jlleitschuh.gradle.ktlint") {
            Some(format!("{gradle} ktlintCheck"))
        } else if scripts.contains("io.gitlab.arturbosch.detekt") {
            Some(format!("{gradle} detekt"))
        } else if scripts.contains("checkstyle") {
            Some(format!("{gradle} checkstyleMain"))
        } else {
            None
        };
        let kotlin = f.files.iter().any(|n| n.ends_with(".gradle.kts"))
            || scripts.contains("org.jetbrains.kotlin")
            || scripts.contains("kotlin(\"");
        let mut languages = Vec::new();
        if !kotlin || Folder::read(&f.path, "src/main").has_dir("java") {
            languages.push("java".to_owned());
        }
        if kotlin {
            languages.push("kotlin".to_owned());
        }
        Found {
            claims: Vec::new(),
            languages,
            package_manager: Some("gradle".to_owned()),
            lockfiles: f
                .files
                .iter()
                .filter(|n| *n == "gradle.lockfile")
                .cloned()
                .collect(),
            tools: if android {
                vec!["android".to_owned()]
            } else {
                Vec::new()
            },
            commands: Commands {
                build: Some(format!("{gradle} assemble")),
                test: Some(format!("{gradle} test")),
                lint,
                ..Commands::default()
            },
        }
    },
    typecheck: Missing::NotNeeded("gradle test compiles the code first"),
    lint: Missing::Skipped(
        "no linter was found (looked for Android lint, Spotless, ktlint, detekt and Checkstyle in the Gradle build)",
    ),
    test: Missing::Skipped("no test command was found"),
    programs: &["gradle", "java"],
    doctor: |f| {
        if f.has("gradlew") || f.has("gradlew.bat") {
            vec![JDK]
        } else {
            vec![
                Tool {
                    programs: &["gradle"],
                    version_args: &["--version"],
                    install: "install Gradle from https://gradle.org/install, or add the Gradle wrapper (gradlew) to the project",
                },
                JDK,
            ]
        }
    },
    ignored: &[".gradle", "build"],
};

// ------------------------------------------------------------ SwiftPM

/// A SwiftLint or swift-format config, with the command that runs it.
fn swift_lint(f: &Folder) -> Option<String> {
    if f.has(".swiftlint.yml") || f.has(".swiftlint.yaml") {
        Some("swiftlint lint --strict".to_owned())
    } else if f.has(".swift-format") {
        Some("swift format lint --recursive --strict .".to_owned())
    } else {
        None
    }
}

const SWIFT_TOOL: Tool = Tool {
    programs: &["swift"],
    version_args: &["--version"],
    install: "install Swift from https://www.swift.org/install (on macOS, with Xcode)",
};

/// True when a CMake or Meson build in the folder builds a native extension
/// of another ecosystem's package, and that package's own tool drives it:
/// scikit-build-core, meson-python, py-build-cmake or a `setup.py` using
/// CMake (Python); cmake-js or node-gyp (Node); a `Cargo.toml` beside it
/// (Rust). Then it is not a project of its own.
fn native_extension(f: &Folder) -> bool {
    const BACKENDS: [&str; 7] = [
        "scikit_build_core",
        "scikit-build",
        "mesonpy",
        "meson-python",
        "py_build_cmake",
        "CMakeExtension",
        "cmake_build_extension",
    ];
    let python = ["pyproject.toml", "setup.py"].iter().any(|name| {
        f.text(name)
            .is_some_and(|text| BACKENDS.iter().any(|backend| text.contains(backend)))
    });
    let node = f.has("binding.gyp")
        || f.text("package.json")
            .is_some_and(|text| text.contains("cmake-js") || text.contains("node-gyp"));
    python || node || f.has("Cargo.toml")
}

/// With both a `Package.swift` and a `CMakeLists.txt`, the sources decide:
/// C or C++ code makes it a CMake project that also ships a Swift package
/// (nlohmann/json); Swift code makes it a Swift package that also builds
/// with CMake (swift-argument-parser). A SwiftPM `Sources/` folder, or a
/// CMake build that names Swift among its languages, makes it Swift even
/// with C modules inside (swift-system, swift-crypto).
fn c_first(f: &Folder) -> bool {
    if f.has_dir("Sources") || cmake_languages(f).iter().any(|l| l == "swift") {
        return false;
    }
    !c_family(f).is_empty()
}

const SWIFT: Ecosystem = Ecosystem {
    kind: "swift",
    marks: |f| f.has("Package.swift") && !(f.has("CMakeLists.txt") && c_first(f)),
    yields_to: &[],
    only_on: None,
    contains_nested: false,
    manifests: &[],
    name: |f| {
        let text = f.text("Package.swift")?;
        let at = text.find("Package(")?;
        quoted_after(&text[at..], "name:")
    },
    found: |f| Found {
        claims: Vec::new(),
        languages: vec!["swift".to_owned()],
        package_manager: Some("swiftpm".to_owned()),
        lockfiles: f
            .files
            .iter()
            .filter(|n| *n == "Package.resolved")
            .cloned()
            .collect(),
        tools: Vec::new(),
        commands: Commands {
            build: Some("swift build".to_owned()),
            test: Some("swift test".to_owned()),
            lint: swift_lint(f),
            ..Commands::default()
        },
    },
    typecheck: Missing::NotNeeded("swift test builds, and so type-checks, the code first"),
    lint: Missing::Skipped("no linter was found (looked for .swiftlint.yml and .swift-format)"),
    test: Missing::Skipped("no test command was found"),
    programs: &["swift", "swiftlint"],
    doctor: |_| vec![SWIFT_TOOL],
    ignored: &[".build"],
};

// --------------------------------------------------------------- Xcode

/// The workspace (preferred: CocoaPods and multi-project apps use one) or
/// project of the folder, as `-workspace X` or `-project X`.
fn xcode_container(f: &Folder) -> Option<(String, String)> {
    let workspaces = f.dirs_ending(".xcworkspace");
    if let [only] = workspaces.as_slice() {
        return Some(("-workspace".to_owned(), (*only).to_owned()));
    }
    let projects = f.dirs_ending(".xcodeproj");
    if let [only] = projects.as_slice() {
        return Some(("-project".to_owned(), (*only).to_owned()));
    }
    None
}

/// The one shared scheme of the folder's Xcode projects
/// (`*/xcshareddata/xcschemes/*.xcscheme`), or the one named like the
/// container when there are several.
fn xcode_scheme(f: &Folder, container: &str) -> Option<String> {
    let mut schemes: Vec<String> = Vec::new();
    for dir in f
        .dirs_ending(".xcodeproj")
        .into_iter()
        .chain(f.dirs_ending(".xcworkspace"))
    {
        let shared = Folder::read(&f.path, &format!("{dir}/xcshareddata/xcschemes"));
        schemes.extend(
            shared
                .files_ending(".xcscheme")
                .into_iter()
                .map(|n| n.trim_end_matches(".xcscheme").to_owned()),
        );
    }
    schemes.sort();
    schemes.dedup();
    let stem = container
        .trim_end_matches(".xcworkspace")
        .trim_end_matches(".xcodeproj");
    match schemes.as_slice() {
        [only] => Some(only.clone()),
        many => many.iter().find(|s| *s == stem).cloned(),
    }
}

/// The platform the project builds for, from `SDKROOT` in its
/// `project.pbxproj`, as a generic build destination.
fn xcode_destination(f: &Folder) -> &'static str {
    let pbx = f
        .dirs_ending(".xcodeproj")
        .first()
        .and_then(|dir| fs::read_to_string(f.path.join(dir).join("project.pbxproj")).ok())
        .unwrap_or_default();
    let roots: Vec<&str> = pbx
        .lines()
        .filter_map(|line| line.trim().strip_prefix("SDKROOT = "))
        .map(|value| value.trim_end_matches(';').trim())
        .collect();
    if !roots.is_empty() && roots.iter().all(|root| *root == "macosx") {
        "generic/platform=macOS"
    } else if pbx.contains("SDKROOT = watchos") {
        "generic/platform=watchOS Simulator"
    } else if pbx.contains("SDKROOT = appletvos") {
        "generic/platform=tvOS Simulator"
    } else if pbx.contains("SDKROOT = xros") {
        "generic/platform=visionOS Simulator"
    } else {
        "generic/platform=iOS Simulator"
    }
}

const XCODE: Ecosystem = Ecosystem {
    kind: "xcode",
    marks: |f| {
        !f.dirs_ending(".xcodeproj").is_empty() || !f.dirs_ending(".xcworkspace").is_empty()
    },
    // A Swift package with an Xcode project beside it builds as a package.
    yields_to: &["swift"],
    only_on: Some("macos"),
    contains_nested: false,
    manifests: &[],
    name: |f| {
        xcode_container(f).map(|(_, name)| {
            name.trim_end_matches(".xcworkspace")
                .trim_end_matches(".xcodeproj")
                .to_owned()
        })
    },
    found: |f| {
        // Build (and so type-check) for a generic simulator, with no code
        // signing: it needs no device, simulator or certificate. Tests need
        // a concrete simulator, so a person names one (verify.test); a
        // macOS app tests on this Mac.
        let build = xcode_container(f).and_then(|(flag, container)| {
            let scheme = xcode_scheme(f, &container)?;
            Some((flag, container, scheme))
        });
        let destination = xcode_destination(f);
        let typecheck = build.as_ref().map(|(flag, container, scheme)| {
            format!("xcodebuild build {flag} \"{container}\" -scheme \"{scheme}\" -destination \"{destination}\" -quiet CODE_SIGNING_ALLOWED=NO")
        });
        let test = build
            .as_ref()
            .filter(|_| destination == "generic/platform=macOS")
            .map(|(flag, container, scheme)| {
                format!("xcodebuild test {flag} \"{container}\" -scheme \"{scheme}\" -destination \"platform=macOS\" -quiet CODE_SIGNING_ALLOWED=NO")
            });
        let pods = f.has("Podfile");
        let mut languages = vec!["swift".to_owned()];
        languages.extend(c_family(f).into_iter().filter(|l| l == "objective-c"));
        Found {
            claims: Vec::new(),
            languages,
            package_manager: pods.then(|| "cocoapods".to_owned()),
            lockfiles: f
                .files
                .iter()
                .filter(|n| *n == "Podfile.lock")
                .cloned()
                .collect(),
            tools: Vec::new(),
            commands: Commands {
                install: pods.then(|| "pod install".to_owned()),
                build: typecheck.clone(),
                test,
                lint: swift_lint(f),
                typecheck,
            },
        }
    },
    typecheck: Missing::Skipped(
        "no shared Xcode scheme was found (Xcode: Product > Scheme > Manage Schemes > Shared); set verify.typecheck to an xcodebuild build command",
    ),
    lint: Missing::Skipped("no linter was found (looked for .swiftlint.yml and .swift-format)"),
    test: Missing::Skipped(
        "xcodebuild test needs a scheme and a simulator or device; set verify.test, for example xcodebuild test -scheme <Scheme> -destination 'platform=iOS Simulator,name=iPhone 16'",
    ),
    programs: &["xcodebuild", "pod", "swiftlint"],
    doctor: |f| {
        let mut tools = vec![Tool {
            programs: &["xcodebuild"],
            version_args: &["-version"],
            install: "Xcode projects build only on macOS: install Xcode from the App Store, then run `xcode-select --install`",
        }];
        if f.has("Podfile") {
            tools.push(Tool {
                programs: &["pod"],
                version_args: &["--version"],
                install: "install CocoaPods (https://cocoapods.org), then run `pod install`",
            });
        }
        tools
    },
    ignored: &["xcuserdata"],
};

// --------------------------------------------------------------- CMake

const C_COMPILER: Tool = Tool {
    programs: &["cc", "gcc", "clang", "cl"],
    version_args: &["--version"],
    install: "install a C and C++ compiler (gcc or clang; on Windows, Visual Studio's Build Tools)",
};

/// `project(Name …)` of CMake or `project('name', …)` of Meson.
fn project_call(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let at = lower.find("project(")? + "project(".len();
    let rest = text[at..].trim_start();
    let name: String = rest
        .trim_start_matches(['"', '\''])
        .chars()
        .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .collect();
    (!name.is_empty() && !name.starts_with('$')).then_some(name)
}

/// The `LANGUAGES` of CMake's `project()` call, when it names them.
fn cmake_languages(f: &Folder) -> Vec<String> {
    let Some(text) = f.text("CMakeLists.txt") else {
        return Vec::new();
    };
    let lower = text.to_ascii_lowercase();
    let Some(at) = lower.find("project(") else {
        return Vec::new();
    };
    let call = &text[at..text[at..].find(')').map_or(text.len(), |end| at + end)];
    let Some(langs) = call.split("LANGUAGES").nth(1) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for word in langs.split_whitespace() {
        let language = match word {
            "C" => "c",
            "CXX" => "cpp",
            "Swift" => "swift",
            "OBJC" | "OBJCXX" => "objective-c",
            _ => continue,
        };
        if !out.iter().any(|l| l == language) {
            out.push(language.to_owned());
        }
    }
    out
}

/// The languages of a C or C++ build: from its source files, else both.
fn c_languages(f: &Folder) -> Vec<String> {
    let found = c_family(f);
    if found.is_empty() {
        vec!["c".to_owned(), "cpp".to_owned()]
    } else {
        found
    }
}

/// Conan or vcpkg, when the project uses one.
fn c_package_manager(f: &Folder) -> Option<String> {
    if f.has("conanfile.txt") || f.has("conanfile.py") {
        Some("conan".to_owned())
    } else if f.has("vcpkg.json") {
        Some("vcpkg".to_owned())
    } else {
        None
    }
}

/// True when a `CMakeLists.txt` adds a custom target named `lint` (the
/// cmake pack's starter checks formatting with it, ADR 0055): `fscale
/// verify` builds it as the lint step.
fn cmake_lint_target(text: &str) -> bool {
    text.lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .any(|line| {
            let lower = line.to_ascii_lowercase();
            lower.match_indices("add_custom_target").any(|(at, word)| {
                lower[at + word.len()..]
                    .trim_start()
                    .strip_prefix('(')
                    .and_then(|rest| rest.trim_start().strip_prefix("lint"))
                    .is_some_and(|after| {
                        after.is_empty()
                            || after.starts_with(|c: char| c.is_whitespace() || c == ')')
                    })
            })
        })
}

const CLANG_FORMAT: Tool = Tool {
    programs: &["clang-format"],
    version_args: &["--version"],
    install: "install clang-format (LLVM's code formatter: `apt install clang-format`, `brew install clang-format`, or LLVM's Windows installer)",
};

const CMAKE: Ecosystem = Ecosystem {
    kind: "cmake",
    marks: |f| {
        f.has("CMakeLists.txt") && !(f.has("Package.swift") && !c_first(f)) && !native_extension(f)
    },
    yields_to: &[],
    only_on: None,
    contains_nested: true,
    // A CMake build below a Gradle build is the NDK code an Android app
    // builds through `externalNativeBuild`: a member too.
    manifests: &[
        "CMakeLists.txt",
        "build.gradle",
        "build.gradle.kts",
        "settings.gradle",
        "settings.gradle.kts",
    ],
    name: |f| project_call(&f.text("CMakeLists.txt")?),
    found: |f| {
        // Configure into `build/`, build, then run CTest there. Configuring
        // an existing build folder again is a no-op that keeps its cache.
        let configure = "cmake -S . -B build";
        let declared = cmake_languages(f);
        Found {
            claims: Vec::new(),
            languages: if declared.is_empty() {
                c_languages(f)
            } else {
                declared
            },
            package_manager: c_package_manager(f),
            lockfiles: Vec::new(),
            tools: Vec::new(),
            commands: Commands {
                // `--config` and `-C` choose Debug for a multi-config
                // generator (Visual Studio, the default on Windows); a
                // single-config one ignores them.
                build: Some(format!("{configure} && cmake --build build --config Debug")),
                test: Some(format!(
                    "{configure} && cmake --build build --config Debug && ctest --test-dir build -C Debug --output-on-failure"
                )),
                lint: f
                    .text("CMakeLists.txt")
                    .is_some_and(|text| cmake_lint_target(&text))
                    .then(|| format!("{configure} && cmake --build build --config Debug --target lint")),
                ..Commands::default()
            },
        }
    },
    typecheck: Missing::NotNeeded("the test step builds the code, which type-checks it"),
    lint: Missing::Skipped(
        "no lint command was found (looked for a `lint` target in CMakeLists.txt; clang-tidy and clang-format need a file list); set verify.lint",
    ),
    test: Missing::Skipped("no test command was found"),
    programs: &[
        "cmake", "ctest", "ninja", "cc", "gcc", "clang", "c++", "g++", "clang++", "cl",
        "clang-format",
    ],
    doctor: |f| {
        let mut tools = vec![
            Tool {
                programs: &["cmake"],
                version_args: &["--version"],
                install: "install CMake 3.20 or later from https://cmake.org/download",
            },
            C_COMPILER,
        ];
        if f
            .text("CMakeLists.txt")
            .is_some_and(|text| cmake_lint_target(&text) && text.contains("clang-format"))
        {
            tools.push(CLANG_FORMAT);
        }
        tools
    },
    ignored: &["build"],
};

// --------------------------------------------------------------- Meson

const MESON: Ecosystem = Ecosystem {
    kind: "meson",
    marks: |f| f.has("meson.build") && !native_extension(f),
    yields_to: &["cmake"],
    only_on: None,
    contains_nested: true,
    manifests: &["meson.build"],
    name: |f| project_call(&f.text("meson.build")?),
    found: |f| Found {
        claims: Vec::new(),
        languages: c_languages(f),
        package_manager: c_package_manager(f),
        lockfiles: Vec::new(),
        tools: Vec::new(),
        // `meson setup` on a folder that is set up already succeeds and
        // keeps it (checked with Meson 1.12).
        commands: Commands {
            build: Some("meson setup build && meson compile -C build".to_owned()),
            test: Some("meson setup build && meson test -C build".to_owned()),
            ..Commands::default()
        },
    },
    typecheck: Missing::NotNeeded("the test step builds the code, which type-checks it"),
    lint: Missing::Skipped("no lint command was found; set verify.lint"),
    test: Missing::Skipped("no test command was found"),
    programs: &[
        "meson", "ninja", "cc", "gcc", "clang", "c++", "g++", "clang++", "cl",
    ],
    doctor: |_| {
        vec![
            Tool {
                programs: &["meson"],
                version_args: &["--version"],
                install: "install Meson (https://mesonbuild.com/Getting-meson.html), for example `pip install meson`",
            },
            Tool {
                programs: &["ninja"],
                version_args: &["--version"],
                install: "install Ninja (https://ninja-build.org), for example `pip install ninja`",
            },
            C_COMPILER,
        ]
    },
    ignored: &["build"],
};

// ---------------------------------------------------------------- .NET

const DOTNET_PROJECTS: [&str; 3] = [".csproj", ".fsproj", ".vbproj"];

/// The one solution, else the one project, that `dotnet` should be given.
fn dotnet_target(f: &Folder) -> Option<String> {
    let solutions: Vec<&str> = f
        .files_ending(".sln")
        .into_iter()
        .chain(f.files_ending(".slnx"))
        .collect();
    if let [only] = solutions.as_slice() {
        return Some((*only).to_owned());
    }
    if !solutions.is_empty() {
        return None;
    }
    let projects: Vec<&str> = DOTNET_PROJECTS
        .iter()
        .flat_map(|ext| f.files_ending(ext))
        .collect();
    match projects.as_slice() {
        [only] => Some((*only).to_owned()),
        _ => None,
    }
}

const DOTNET: Ecosystem = Ecosystem {
    kind: "dotnet",
    marks: |f| {
        [".sln", ".slnx"]
            .iter()
            .chain(DOTNET_PROJECTS.iter())
            .any(|ext| !f.files_ending(ext).is_empty())
    },
    yields_to: &[],
    only_on: None,
    contains_nested: true,
    manifests: &[],
    name: |f| {
        dotnet_target(f).map(|file| {
            file.rsplit_once('.')
                .map_or(file.clone(), |(stem, _)| stem.to_owned())
        })
    },
    found: |f| {
        let target = dotnet_target(f);
        let quoted = target.as_ref().map(|t| format!(" \"{t}\""));
        let mut languages = Vec::new();
        if !f.files_ending(".fsproj").is_empty() {
            languages.push("fsharp".to_owned());
        }
        if !f.files_ending(".vbproj").is_empty() {
            languages.push("visual-basic".to_owned());
        }
        if languages.is_empty() || !f.files_ending(".csproj").is_empty() {
            languages.insert(0, "csharp".to_owned());
        }
        Found {
            claims: Vec::new(),
            languages,
            package_manager: Some("nuget".to_owned()),
            lockfiles: f
                .files
                .iter()
                .filter(|n| *n == "packages.lock.json")
                .cloned()
                .collect(),
            tools: Vec::new(),
            commands: Commands {
                install: quoted.as_ref().map(|t| format!("dotnet restore{t}")),
                build: quoted.as_ref().map(|t| format!("dotnet build{t}")),
                test: quoted.as_ref().map(|t| format!("dotnet test{t}")),
                lint: quoted
                    .as_ref()
                    .map(|t| format!("dotnet format{t} --verify-no-changes")),
                typecheck: None,
            },
        }
    },
    typecheck: Missing::NotNeeded("dotnet test builds, and so type-checks, the code first"),
    lint: Missing::Skipped(
        "the folder has several solutions or projects, so dotnet cannot pick one; set verify.lint",
    ),
    test: Missing::Skipped(
        "the folder has several solutions or projects, so dotnet cannot pick one; set verify.test",
    ),
    programs: &["dotnet"],
    doctor: |_| {
        vec![Tool {
            programs: &["dotnet"],
            version_args: &["--version"],
            install: "install the .NET SDK from https://dotnet.microsoft.com/download",
        }]
    },
    ignored: &["bin", "obj"],
};

// -------------------------------------------------------- Dart, Flutter

/// True when pubspec.yaml depends on the Flutter SDK.
fn is_flutter(f: &Folder) -> bool {
    f.text("pubspec.yaml")
        .is_some_and(|text| text.lines().any(|line| line.trim() == "sdk: flutter"))
}

const DART: Ecosystem = Ecosystem {
    kind: "dart",
    marks: |f| f.has("pubspec.yaml"),
    yields_to: &[],
    only_on: None,
    contains_nested: false,
    manifests: &[],
    name: |f| {
        f.text("pubspec.yaml")?.lines().find_map(|line| {
            line.strip_prefix("name:")
                .map(|name| name.trim().trim_matches(['"', '\'']).to_owned())
                .filter(|name| !name.is_empty())
        })
    },
    found: |f| {
        let flutter = is_flutter(f);
        let tool = if flutter { "flutter" } else { "dart" };
        // A Flutter app's platform folders are runners `flutter` generates
        // and builds itself, not projects of their own.
        let claims = if flutter {
            ["android", "ios", "macos", "linux", "windows", "web"]
                .into_iter()
                .filter(|dir| f.has_dir(dir))
                .map(str::to_owned)
                .collect()
        } else {
            Vec::new()
        };
        Found {
            claims,
            languages: vec!["dart".to_owned()],
            package_manager: Some(tool.to_owned()),
            lockfiles: f
                .files
                .iter()
                .filter(|n| *n == "pubspec.lock")
                .cloned()
                .collect(),
            tools: if flutter {
                vec!["flutter".to_owned()]
            } else {
                Vec::new()
            },
            commands: Commands {
                install: Some(format!("{tool} pub get")),
                build: None,
                test: f.has_dir("test").then(|| format!("{tool} test")),
                lint: Some(format!("{tool} analyze")),
                typecheck: None,
            },
        }
    },
    typecheck: Missing::NotNeeded("dart analyze already type-checks"),
    lint: Missing::Skipped("no lint command was found"),
    test: Missing::Skipped("the project has no test folder"),
    programs: &["flutter", "dart"],
    doctor: |f| {
        vec![if is_flutter(f) {
            Tool {
                programs: &["flutter"],
                version_args: &["--version"],
                install: "install Flutter from https://docs.flutter.dev/get-started/install",
            }
        } else {
            Tool {
                programs: &["dart"],
                version_args: &["--version"],
                install: "install the Dart SDK from https://dart.dev/get-dart",
            }
        }]
    },
    ignored: &[".dart_tool"],
};

// ---------------------------------------------------------------- Make

const MAKEFILES: [&str; 3] = ["GNUmakefile", "makefile", "Makefile"];

/// The targets a Makefile defines (`name:` at the start of a line).
fn make_targets(f: &Folder) -> Vec<String> {
    let Some(text) = MAKEFILES.iter().find_map(|name| f.text(name)) else {
        return Vec::new();
    };
    text.lines()
        .filter(|line| !line.starts_with(['\t', ' ', '#', '.']))
        .filter_map(|line| {
            let (head, rest) = line.split_once(':')?;
            // `a := b` and `a ::= b` are variables, not targets.
            if rest.starts_with('=') || rest.starts_with(":=") || head.contains(['=', '%', '$']) {
                return None;
            }
            Some(
                head.split_whitespace()
                    .map(str::to_owned)
                    .collect::<Vec<_>>(),
            )
        })
        .flatten()
        .collect()
}

const MAKE: Ecosystem = Ecosystem {
    kind: "make",
    // A Makefile is a project of its own only for C-family code or when it
    // says how to test: a Makefile that wraps another tool, or builds docs,
    // is not.
    marks: |f| {
        MAKEFILES.iter().any(|name| f.has(name))
            && (!c_family(f).is_empty() || {
                let targets = make_targets(f);
                targets.iter().any(|t| t == "test" || t == "check")
            })
    },
    yields_to: &["*"],
    only_on: None,
    contains_nested: true,
    // A Makefile below another is a recursive make's member.
    manifests: &MAKEFILES,
    name: |_| None,
    found: |f| {
        let targets = make_targets(f);
        let has = |t: &str| targets.iter().any(|x| x == t);
        let test = if has("test") {
            Some("make test".to_owned())
        } else if has("check") {
            Some("make check".to_owned())
        } else {
            None
        };
        Found {
            claims: Vec::new(),
            languages: c_family(f),
            package_manager: None,
            lockfiles: Vec::new(),
            tools: Vec::new(),
            commands: Commands {
                build: Some("make".to_owned()),
                test,
                lint: has("lint").then(|| "make lint".to_owned()),
                typecheck: ["typecheck", "type-check"]
                    .iter()
                    .find(|t| has(t))
                    .map(|t| format!("make {t}")),
                install: None,
            },
        }
    },
    typecheck: Missing::Skipped("the Makefile has no typecheck target"),
    lint: Missing::Skipped("the Makefile has no lint target"),
    test: Missing::Skipped("the Makefile has no test or check target"),
    programs: &["make", "cc", "gcc", "clang", "c++", "g++", "clang++"],
    doctor: |f| {
        let mut tools = vec![Tool {
            programs: &["make", "gmake"],
            version_args: &["--version"],
            install: "install make (on Windows, for example with MSYS2 or Chocolatey)",
        }];
        if !c_family(f).is_empty() {
            tools.push(C_COMPILER);
        }
        tools
    },
    ignored: &[],
};

#[cfg(test)]
mod tests;
