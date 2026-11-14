//! The local step cache (M18, ADR 0036).
//!
//! A verify step that **passed** is remembered on this machine, under
//! `.firescale/steps/`, keyed by everything its result can depend on:
//!
//! - the firescale version and the format of this cache;
//! - the step, the folder it runs in and its command;
//! - the bytes of the files it can read: every file of its project's
//!   folder, of the projects and shared folders it depends on in the
//!   project graph (ADR 0032), the files directly in each folder above it,
//!   and every file that belongs to no project. A file is hashed as it is
//!   in the working tree: tracked files with uncommitted changes, and
//!   untracked files that git does not ignore. Bytes, not text, so a CRLF
//!   checkout is a different key, never a wrong hit;
//! - the versions of the tools the command runs, and what is installed
//!   (the package manager's marker in `node_modules`, the packages of a
//!   project's virtual environment);
//! - the OS and the CPU architecture;
//! - the values of a fixed list of environment variables, hashed. Nothing
//!   else from the environment is read.
//!
//! Only a pass is stored. A step is never cached when it needs a service
//! that the repo does not start, when its project has a dynamic import, or
//! when one of its inputs could not be read.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::doctor::{Machine, ThisMachine};
use crate::git::run_git;
use crate::graph::{is_document_or_media, project_id, Graph, NodeType, Source};
use crate::services::services_in;
use crate::walk::{walk_entries, EntryKind};

/// Bumped when the entry format or the key recipe changes, so old entries
/// are never read.
const FORMAT: u32 = 1;

/// The folder of the step cache, relative to the repo root.
pub const STEP_CACHE_DIR: [&str; 2] = [".firescale", "steps"];

/// Entries older than this are removed (ADR 0036).
pub const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// At most this many entries are kept; the least recently used go first.
pub const MAX_ENTRIES: usize = 500;

/// The environment variables whose values are part of a key. Their values
/// are hashed and never written down. No other variable is read.
pub const ENV_ALLOWLIST: [&str; 23] = [
    "CI",
    "NODE_PATH",
    "PYTEST_ADDOPTS",
    "NODE_ENV",
    "NODE_OPTIONS",
    "TZ",
    "LANG",
    "LC_ALL",
    "PYTHONPATH",
    "PYTHONHASHSEED",
    "VIRTUAL_ENV",
    "PYENV_VERSION",
    "RUSTFLAGS",
    "RUSTUP_TOOLCHAIN",
    "CARGO_BUILD_TARGET",
    "GOFLAGS",
    "GOOS",
    "GOARCH",
    "CGO_ENABLED",
    "GOTOOLCHAIN",
    "JAVA_HOME",
    // The image of a GitHub-hosted runner (ADR 0041).
    "ImageOS",
    "ImageVersion",
];

/// One file of the working tree, as the key sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FileState {
    /// The blake3 hash of its bytes, and on Unix whether it is executable.
    Bytes(String),
    /// A symbolic link to a file: its target, and the hash of the bytes it
    /// points to.
    Link(String),
    /// Tracked but deleted.
    Absent,
    /// It exists and could not be read: a permission error, a path git
    /// gave that is not valid UTF-8, a link to a folder or out of reach.
    Unreadable(String),
    /// A git submodule or a nested repository: its files are not hashed.
    Opaque,
}

/// The size and modification time of a file, so a second capture in the
/// same run can reuse a hash. Never kept across runs.
type Stamp = (u64, Option<SystemTime>);

/// The files of a working tree and their contents.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// Repo-relative paths with forward slashes, sorted.
    files: BTreeMap<String, (FileState, Option<Stamp>)>,
    /// Set when the list of files could not be read: nothing is cached.
    problem: Option<String>,
    /// When it was taken. A hash is reused only for a file last changed
    /// well before this (git's "racily clean" rule).
    taken: Option<SystemTime>,
}

fn abs(root: &Path, rel: &str) -> PathBuf {
    if rel == "." {
        return root.to_path_buf();
    }
    rel.split('/')
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

fn stamp(meta: &fs::Metadata) -> Stamp {
    (meta.len(), meta.modified().ok())
}

#[cfg(unix)]
fn executable(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn executable(_: &fs::Metadata) -> bool {
    false
}

fn read_state(path: &Path) -> (FileState, Option<Stamp>) {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (FileState::Absent, None)
        }
        Err(error) => return (FileState::Unreadable(error.to_string()), None),
    };
    if meta.file_type().is_symlink() {
        // A link counts by its target and by what it points to.
        let Ok(target) = fs::read_link(path) else {
            return (
                FileState::Unreadable("a link that cannot be read".to_owned()),
                None,
            );
        };
        let target = target.to_string_lossy().replace('\\', "/");
        return match fs::metadata(path) {
            Ok(pointed) if pointed.is_dir() => (
                FileState::Unreadable(format!(
                    "a link to the folder {target}, whose files are not hashed"
                )),
                None,
            ),
            Ok(_) => match fs::read(path) {
                Ok(bytes) => (
                    FileState::Link(format!("{target}\0{}", blake3::hash(&bytes).to_hex())),
                    None,
                ),
                Err(error) => (FileState::Unreadable(error.to_string()), None),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                (FileState::Link(format!("{target}\0dangling")), None)
            }
            Err(error) => (FileState::Unreadable(error.to_string()), None),
        };
    }
    if meta.is_dir() {
        return (FileState::Opaque, None);
    }
    match fs::read(path) {
        Ok(bytes) => {
            let mode = if executable(&meta) { "x" } else { "-" };
            (
                FileState::Bytes(format!("{}{mode}", blake3::hash(&bytes).to_hex())),
                Some(stamp(&meta)),
            )
        }
        Err(error) => (FileState::Unreadable(error.to_string()), None),
    }
}

/// The files git lists for the working tree: tracked files and untracked
/// files it does not ignore. Outside git, every file and link the walk finds.
fn list_files(root: &Path) -> Result<Vec<String>, String> {
    let top = run_git(root, &["rev-parse", "--show-toplevel"])
        .ok()
        .filter(|out| out.code == 0)
        .map(|out| PathBuf::from(out.stdout.trim()));
    let Some(top) = top else {
        return walk_entries(root)
            .map(|entries| {
                entries
                    .into_iter()
                    .filter(|(_, kind)| *kind != EntryKind::Folder)
                    .map(|(path, _)| path)
                    .collect()
            })
            .map_err(|error| error.message().to_owned());
    };
    let same = |a: &Path, b: &Path| match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    if !same(&top, root) {
        return Err(
            "the repo root is a folder inside a larger git repository, whose files above it are not hashed"
                .to_owned(),
        );
    }
    let out = run_git(
        root,
        &[
            "-c",
            "core.quotepath=off",
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    )
    .map_err(|error| error.message().to_owned())?;
    if out.code != 0 {
        return Err(format!(
            "git ls-files failed: {}",
            out.stderr.lines().next().unwrap_or("").trim()
        ));
    }
    let mut files: BTreeSet<String> = out
        .stdout
        .split('\0')
        .filter(|path| !path.is_empty())
        // A nested repository is listed as `folder/`.
        .map(|path| path.replace('\\', "/").trim_end_matches('/').to_owned())
        .collect();
    files.retain(|path| !path.starts_with(".firescale/") && !path.starts_with(".git/"));
    Ok(files.into_iter().collect())
}

/// Files changed within this long before a snapshot are hashed again, as
/// file systems with coarse times can hide a change.
const RACY: Duration = Duration::from_secs(2);

impl Snapshot {
    /// Lists and hashes the working tree under `root`.
    #[must_use]
    pub fn capture(root: &Path) -> Self {
        Self::capture_reusing(root, None)
    }

    /// Like [`Snapshot::capture`], reusing the hash of a file whose size
    /// and modification time are the same as in `earlier`, and were older
    /// than `earlier` by a margin.
    #[must_use]
    pub fn capture_reusing(root: &Path, earlier: Option<&Self>) -> Self {
        let taken = SystemTime::now();
        let paths = match list_files(root) {
            Ok(paths) => paths,
            Err(problem) => {
                return Self {
                    files: BTreeMap::new(),
                    problem: Some(problem),
                    taken: Some(taken),
                }
            }
        };
        let threads = std::thread::available_parallelism()
            .map_or(4, std::num::NonZero::get)
            .clamp(1, 16);
        let chunk = paths.len().div_ceil(threads).max(1);
        let hashed: Vec<(String, (FileState, Option<Stamp>))> = std::thread::scope(|scope| {
            let handles: Vec<_> = paths
                .chunks(chunk)
                .map(|part| {
                    scope.spawn(move || {
                        part.iter()
                            .map(|rel| {
                                let path = abs(root, rel);
                                if rel.contains('\u{fffd}') {
                                    return (
                                        rel.clone(),
                                        (
                                            FileState::Unreadable(
                                                "its name is not valid UTF-8".to_owned(),
                                            ),
                                            None,
                                        ),
                                    );
                                }
                                let reused = earlier.and_then(|snapshot| {
                                    let (state, old) = snapshot.files.get(rel)?;
                                    let (_, Some(modified)) = (*old)? else {
                                        return None;
                                    };
                                    let settled = snapshot
                                        .taken
                                        .and_then(|taken| taken.checked_sub(RACY))
                                        .is_some_and(|line| modified < line);
                                    let now = fs::symlink_metadata(&path).ok()?;
                                    (settled && *old == Some(stamp(&now)))
                                        .then(|| (state.clone(), *old))
                                });
                                (rel.clone(), reused.unwrap_or_else(|| read_state(&path)))
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|handle| handle.join().unwrap_or_default())
                .collect()
        });
        if hashed.len() != paths.len() {
            return Self {
                files: BTreeMap::new(),
                problem: Some("the files could not all be hashed".to_owned()),
                taken: Some(taken),
            };
        }
        Self {
            files: hashed.into_iter().collect(),
            problem: None,
            taken: Some(taken),
        }
    }

    /// How many files were captured.
    #[must_use]
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// True when no file was captured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// True when `path` is `dir` or inside it. Everything is inside `.`.
fn inside(path: &str, dir: &str) -> bool {
    dir == "."
        || path == dir
        || path
            .strip_prefix(dir)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn parent(path: &str) -> &str {
    path.rfind('/').map_or(".", |index| &path[..index])
}

/// What files a unit's key covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    /// Folders (or single files) whose files are all covered. `.` is the
    /// whole repo.
    pub folders: BTreeSet<String>,
    /// Folders whose own files (not their subfolders) are covered: the
    /// folders above the unit, where tools find their settings.
    pub above: BTreeSet<String>,
    /// Also every file that belongs to no project and no narrow shared node.
    pub unowned: bool,
    /// Projects (not shared nodes) whose folders are covered.
    pub projects: Vec<String>,
}

impl Scope {
    fn whole_repo() -> Self {
        Self {
            folders: BTreeSet::from([".".to_owned()]),
            above: BTreeSet::new(),
            unowned: true,
            projects: Vec::new(),
        }
    }
}

/// Why a unit's steps are not cached, or what they cover.
pub type UnitScope = Result<Scope, String>;

/// The scope of the unit whose steps run in `folder` for a project of
/// `kind` (`None` for the repo root's own steps, or a folder only named
/// in the config). The repo root and anything the graph cannot place
/// cover the whole repo.
pub fn unit_scope(graph: Option<&Graph>, folder: Option<&str>, kind: Option<&str>) -> UnitScope {
    let (Some(folder), Some(kind), Some(graph)) = (folder, kind, graph) else {
        return Ok(Scope::whole_repo());
    };
    if folder == "." {
        return Ok(Scope::whole_repo());
    }
    let start = project_id(kind, folder);
    if graph.node(&start).is_none() {
        return Ok(Scope::whole_repo());
    }
    let mut seen: BTreeSet<String> = BTreeSet::from([start.clone()]);
    let mut queue = vec![start];
    while let Some(id) = queue.pop() {
        for edge in graph.dependencies(&id) {
            if edge
                .sources
                .iter()
                .any(|source| source.source == Source::DynamicImport)
            {
                return Err(format!(
                    "{} imports modules chosen at run time, so its inputs are not known",
                    graph.display(&id)
                ));
            }
            if seen.insert(edge.to.clone()) {
                queue.push(edge.to.clone());
            }
        }
    }
    let mut scope = Scope {
        folders: BTreeSet::new(),
        above: BTreeSet::new(),
        unowned: true,
        projects: Vec::new(),
    };
    for id in &seen {
        let Some(node) = graph.node(id) else {
            continue;
        };
        if node.path == "." {
            return Ok(Scope::whole_repo());
        }
        scope.folders.insert(node.path.clone());
        if node.node_type == NodeType::Project {
            scope.projects.push(node.path.clone());
        }
        let mut up = parent(&node.path);
        loop {
            scope.above.insert(up.to_owned());
            if up == "." {
                break;
            }
            up = parent(up);
        }
    }
    Ok(scope)
}

/// The project folders and narrow shared nodes of the graph: a file inside
/// one of them belongs to someone. Everything else is unowned and is in
/// every key.
fn owners(graph: Option<&Graph>) -> Vec<String> {
    let Some(graph) = graph else {
        return Vec::new();
    };
    let projects: Vec<&str> = graph
        .nodes
        .iter()
        .filter(|node| node.node_type == NodeType::Project && node.path != ".")
        .map(|node| node.path.as_str())
        .collect();
    let mut owned: Vec<String> = projects.iter().map(|path| (*path).to_owned()).collect();
    for node in &graph.nodes {
        if node.node_type == NodeType::Shared
            && node.path != "."
            && !projects.iter().any(|project| inside(project, &node.path))
        {
            owned.push(node.path.clone());
        }
    }
    owned
}

/// True when the scope's key holds `path`.
fn covers(scope: &Scope, owned: &[String], path: &str) -> bool {
    if scope.folders.iter().any(|folder| inside(path, folder)) {
        return true;
    }
    let unowned = !owned.iter().any(|folder| inside(path, folder));
    // A file directly in the repo root that no project owns and that is a
    // picture, a document, an archive, media, Markdown or a licence (a
    // screenshot, the README) is in no project's key (ADR 0043). Deeper
    // ones stay in: a test may read `fixtures/words.txt` or a sample image
    // by its path, which the graph does not see.
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    let readme_like = name.ends_with(".md")
        || name.ends_with(".markdown")
        || name.starts_with("license")
        || name.starts_with("licence");
    if unowned && !path.contains('/') && (is_document_or_media(path) || readme_like) {
        return false;
    }
    scope.above.contains(parent(path)) || (scope.unowned && unowned)
}

/// The digest of the files a scope covers, or why it cannot be made.
fn files_digest(snapshot: &Snapshot, scope: &Scope, owned: &[String]) -> Result<String, String> {
    if let Some(problem) = &snapshot.problem {
        return Err(format!("the files could not be listed ({problem})"));
    }
    let mut hasher = blake3::Hasher::new();
    for (path, (state, _)) in &snapshot.files {
        if !covers(scope, owned, path) {
            continue;
        }
        let shown = match state {
            FileState::Bytes(hash) => hash.as_str(),
            FileState::Link(target) => target.as_str(),
            FileState::Absent => "-",
            FileState::Unreadable(why) => return Err(format!("{path} could not be read ({why})")),
            FileState::Opaque => {
                return Err(format!(
                    "{path} is a git submodule or a nested repository, whose files are not hashed"
                ))
            }
        };
        let marker = match state {
            FileState::Link(_) => "link",
            _ => "file",
        };
        hasher.update(format!("{marker}\0{path}\0{shown}\n").as_bytes());
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// The first word of a command, without quotes.
fn program_of(command: &str) -> Option<String> {
    let command = command.trim_start();
    let word = if let Some(rest) = command.strip_prefix('"') {
        rest.split('"').next()?.to_owned()
    } else {
        command.split_whitespace().next()?.to_owned()
    };
    (!word.is_empty()).then_some(word)
}

fn file_name(program: &str) -> &str {
    program.rsplit(['/', '\\']).next().unwrap_or(program)
}

fn file_stem(program: &str) -> String {
    let name = file_name(program).to_ascii_lowercase();
    for suffix in [".exe", ".cmd", ".bat"] {
        if let Some(stem) = name.strip_suffix(suffix) {
            return stem.to_owned();
        }
    }
    name
}

/// `python`, `python3` or `python3.12`.
fn is_python_name(stem: &str) -> bool {
    stem == "py"
        || stem.strip_prefix("python").is_some_and(|rest| {
            rest.is_empty() || rest.chars().all(|c| c.is_ascii_digit() || c == '.')
        })
}

/// True for the interpreter of a virtual environment: `.venv/bin/python`,
/// `venv\Scripts\python.exe`.
fn is_venv_interpreter(program: &str) -> bool {
    let parts: Vec<&str> = program.split(['/', '\\']).collect();
    parts.len() >= 3
        && matches!(parts[parts.len() - 2], "bin" | "Scripts")
        && is_python_name(&file_stem(program))
}

/// The version probes for a command: the program itself when it is a known
/// tool by its exact name, and the runtime it starts. Anything else (a
/// script of the repo, a path, `make`, Yarn, which runs the release the
/// repo pins) is never run: it is keyed by where it is and its size and
/// time (see [`program_identity`]).
fn probes(command: &str) -> Vec<(String, Vec<&'static str>)> {
    let Some(program) = program_of(command) else {
        return Vec::new();
    };
    let stem = file_stem(&program);
    let path = program.contains(['/', '\\']);
    // The Gradle and Maven wrappers are the repo's own scripts (keyed by
    // where they are), but the JDK they start is the machine's.
    if matches!(stem.as_str(), "gradlew" | "mvnw") {
        return vec![("java".to_owned(), vec!["--version"])];
    }
    if path && !is_venv_interpreter(&program) {
        return Vec::new();
    }
    let mut out: Vec<(String, Vec<&'static str>)> = Vec::new();
    let node_tools = ["npm", "npx", "pnpm", "bun", "deno", "node"];
    let python_tools = ["uv", "poetry", "pipenv", "pdm", "hatch", "tox", "nox"];
    if node_tools.contains(&stem.as_str()) {
        out.push((program.clone(), vec!["--version"]));
        if stem != "node" && stem != "bun" && stem != "deno" {
            out.push(("node".to_owned(), vec!["--version"]));
        }
    } else if is_python_name(&stem) || python_tools.contains(&stem.as_str()) {
        out.push((program.clone(), vec!["--version"]));
    } else if stem == "cargo" {
        out.push((program.clone(), vec!["--version"]));
        out.push(("rustc".to_owned(), vec!["--version"]));
    } else if stem == "go" {
        out.push((program.clone(), vec!["version"]));
    } else if matches!(stem.as_str(), "mvn" | "gradle") {
        out.push((program.clone(), vec!["--version"]));
        out.push(("java".to_owned(), vec!["--version"]));
    } else if matches!(
        stem.as_str(),
        "swift" | "cmake" | "meson" | "dotnet" | "dart" | "flutter"
    ) {
        out.push((program.clone(), vec!["--version"]));
    } else if stem == "xcodebuild" {
        out.push((program.clone(), vec!["-version"]));
    }
    out
}

/// Finds `program` on the PATH (never in the current folder), or, for a
/// path, relative to `dir`.
fn locate(dir: &Path, program: &str) -> Option<PathBuf> {
    if program.contains(['/', '\\']) {
        let path = if Path::new(program).is_absolute() {
            PathBuf::from(program)
        } else {
            dir.join(program)
        };
        return candidates(&path).into_iter().find(|path| path.is_file());
    }
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .filter(|folder| folder.is_absolute())
        .flat_map(|folder| candidates(&folder.join(program)))
        .find(|path| path.is_file())
}

/// The files `path` can name as a command on Windows, in the order `cmd`
/// tries them, which is how a step itself is started: a name without one of
/// the `PATHEXT` extensions is tried with each of them (`npm` is `npm.cmd`),
/// and never as it is. Node's installer puts a shell script named `npm`
/// beside `npm.cmd`; Windows cannot run it, so its version could never be
/// read and no Node step was ever cached (ADR 0043).
#[cfg(windows)]
fn candidates(path: &Path) -> Vec<PathBuf> {
    let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned());
    windows_candidates(path, &exts)
}

#[cfg(any(windows, test))]
fn windows_candidates(path: &Path, pathext: &str) -> Vec<PathBuf> {
    let exts: Vec<&str> = pathext.split(';').filter(|ext| !ext.is_empty()).collect();
    let has_ext = path
        .extension()
        .map(|ext| format!(".{}", ext.to_string_lossy()))
        .is_some_and(|ext| exts.iter().any(|known| known.eq_ignore_ascii_case(&ext)));
    if has_ext {
        return vec![path.to_path_buf()];
    }
    exts.iter()
        .map(|ext| {
            let mut name = path.as_os_str().to_owned();
            name.push(ext);
            PathBuf::from(name)
        })
        .collect()
}

#[cfg(not(windows))]
fn candidates(path: &Path) -> Vec<PathBuf> {
    vec![path.to_path_buf()]
}

/// Where the command's program is, with its size and modification time,
/// read without running it.
///
/// # Errors
///
/// When the program cannot be found.
fn program_identity(dir: &Path, command: &str) -> Result<String, String> {
    let Some(program) = program_of(command) else {
        return Err("the command is empty".to_owned());
    };
    let Some(path) = locate(dir, &program) else {
        return Err(format!("{program} was not found on the PATH"));
    };
    let meta = fs::metadata(&path).map_err(|error| format!("{program}: {error}"))?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_nanos());
    Ok(format!(
        "{}\0{}\0{modified}",
        path.to_string_lossy(),
        meta.len()
    ))
}

/// Asks tools for their versions, once per program and folder in a run.
pub struct ToolVersions<'a> {
    machine: &'a (dyn Machine + Sync),
    seen: std::sync::Mutex<HashMap<(PathBuf, String), Option<String>>>,
}

impl<'a> ToolVersions<'a> {
    #[must_use]
    pub fn new(machine: &'a (dyn Machine + Sync)) -> Self {
        Self {
            machine,
            seen: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// The version `program` gives in `dir`. Version managers (asdf, mise,
    /// pyenv, volta, rustup, corepack) answer per folder, so the folder is
    /// part of the memo. The program is found on the PATH first and run by
    /// its full path, with no shell, so nothing in the repo's folder runs.
    fn version(&self, dir: &Path, program: &str, args: &[&str]) -> Option<String> {
        let slot = (dir.to_path_buf(), format!("{program} {}", args.join(" ")));
        if let Some(found) = self
            .seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&slot)
        {
            return found.clone();
        }
        let found = locate(dir, program).and_then(|path| {
            self.machine
                .version(dir, &path.to_string_lossy(), args)
                .map(|version| format!("{}\0{version}", path.to_string_lossy()))
        });
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(slot, found.clone());
        found
    }
}

/// What is installed for a step in `dir`: the package manager's marker
/// files in `node_modules`, and the packages and `pyvenv.cfg` of the
/// virtual environments, from `dir` up to the root.
fn installed(root: &Path, dir: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    let markers = [
        ".package-lock.json",
        ".modules.yaml",
        ".yarn-integrity",
        ".yarn-state.yml",
        ".bun-tag",
    ];
    let hash_file = |path: &Path, lines: &mut Vec<String>| {
        if let Ok(bytes) = fs::read(path) {
            lines.push(format!(
                "installed\0{}\0{}",
                relative(root, path),
                blake3::hash(&bytes).to_hex()
            ));
        }
    };
    let mut at = Some(dir);
    while let Some(folder) = at {
        for marker in markers {
            hash_file(&folder.join("node_modules").join(marker), &mut lines);
        }
        hash_file(&folder.join(".yarn").join("install-state.gz"), &mut lines);
        for venv in [".venv", "venv"] {
            let path = folder.join(venv);
            if path.is_dir() {
                hash_file(&path.join("pyvenv.cfg"), &mut lines);
                for (package, version) in crate::doctor::installed_python_packages(&path) {
                    lines.push(format!(
                        "venv\0{}\0{package}\0{version}",
                        relative(root, &path)
                    ));
                }
            }
        }
        if folder == root {
            break;
        }
        at = folder.parent().filter(|up| up.starts_with(root));
    }
    lines
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|rel| {
            rel.components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        })
        .unwrap_or_default()
}

/// Everything a step's key is made of, apart from the files.
pub struct StepInputs<'a> {
    pub version: &'a str,
    /// `lint`, `typecheck` or `test`.
    pub step: &'a str,
    /// The project's folder, or `None` for the repo root's own steps.
    pub project: Option<&'a str>,
    pub kind: Option<&'a str>,
    pub command: &'a str,
    /// The folder the command runs in.
    pub dir: &'a Path,
    /// True when verify sets `CI=1` for the command.
    pub sets_ci: bool,
}

/// The values of the allowlisted variables, as one hash. `None` is unset.
fn environment_digest(read: &dyn Fn(&str) -> Option<String>) -> String {
    let mut hasher = blake3::Hasher::new();
    for name in ENV_ALLOWLIST {
        match read(name) {
            Some(value) => hasher.update(format!("{name}=\0{value}\n").as_bytes()),
            None => hasher.update(format!("{name}\0unset\n").as_bytes()),
        };
    }
    hasher.finalize().to_hex().to_string()
}

/// True for a command whose result depends on git history, not only on the
/// files: `nx affected`, `turbo run --affected`.
fn uses_history(command: &str) -> bool {
    command
        .split_whitespace()
        .any(|word| word == "affected" || word.starts_with("--affected"))
}

/// The key of one step, or why it cannot be cached.
///
/// # Errors
///
/// The reason, for `FSCALE_LOG=cache`, when the step is not cached.
pub fn step_key(
    root: &Path,
    inputs: &StepInputs<'_>,
    files: &str,
    tools: &ToolVersions<'_>,
    read_env: &dyn Fn(&str) -> Option<String>,
) -> Result<String, String> {
    if uses_history(inputs.command) {
        return Err(
            "its command picks what to run from git history, which the key does not hold"
                .to_owned(),
        );
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(format!("firescale-step-cache\0{FORMAT}\0{}\n", inputs.version).as_bytes());
    hasher.update(
        format!(
            "step\0{}\0{}\0{}\0{}\n",
            inputs.step,
            inputs.project.unwrap_or("(root)"),
            inputs.kind.unwrap_or("-"),
            inputs.command
        )
        .as_bytes(),
    );
    hasher.update(
        format!(
            "machine\0{}\0{}\0ci={}\n",
            std::env::consts::OS,
            std::env::consts::ARCH,
            inputs.sets_ci
        )
        .as_bytes(),
    );
    hasher.update(format!("env\0{}\n", environment_digest(read_env)).as_bytes());
    hasher.update(
        format!(
            "program\0{}\n",
            program_identity(inputs.dir, inputs.command)?
        )
        .as_bytes(),
    );
    for (program, args) in probes(inputs.command) {
        let Some(version) = tools.version(inputs.dir, &program, &args) else {
            return Err(format!("the version of {program} could not be read"));
        };
        hasher.update(format!("tool\0{program}\0{version}\n").as_bytes());
    }
    for line in installed(root, inputs.dir) {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    hasher.update(format!("files\0{files}\n").as_bytes());
    Ok(hasher.finalize().to_hex().to_string())
}

const COMPOSE_NAMES: [&str; 4] = [
    "compose.yaml",
    "compose.yml",
    "docker-compose.yml",
    "docker-compose.yaml",
];

/// Why a unit may need a service that the repo does not start, if it may.
/// Every `.env.example` the unit's files cover is read, with the folders
/// above the step's folder and every Compose file of the repo.
fn outside_service(snapshot: &Snapshot, scope: &Scope, root: &Path, dir: &Path) -> Option<String> {
    let mut folders: BTreeSet<PathBuf> = BTreeSet::new();
    let mut at = Some(dir);
    while let Some(folder) = at {
        folders.insert(folder.to_path_buf());
        if folder == root {
            break;
        }
        at = folder.parent().filter(|up| up.starts_with(root));
    }
    for path in snapshot.files.keys() {
        let name = path.rsplit('/').next().unwrap_or(path);
        let covered = scope.folders.iter().any(|folder| inside(path, folder))
            || scope.above.contains(parent(path));
        if (name == ".env.example" && covered) || COMPOSE_NAMES.contains(&name) {
            folders.insert(abs(root, parent(path)));
        }
    }
    let folders: Vec<PathBuf> = folders.into_iter().collect();
    services_in(root, &folders)
        .into_iter()
        .find(|service| service.start.is_none())
        .map(|service| {
            format!(
                "it may need {} (from {}), a service the repo does not start",
                service.describe(),
                service.source
            )
        })
}

/// Why a Node unit's imports may reach files the graph does not see:
/// a tsconfig with `paths` in its folders or above.
fn path_aliases(snapshot: &Snapshot, scope: &Scope, root: &Path) -> Option<String> {
    snapshot.files.keys().find_map(|path| {
        let name = path.rsplit('/').next().unwrap_or(path);
        let tsconfig = name.starts_with("tsconfig") || name == "jsconfig.json";
        if !tsconfig || !name.ends_with(".json") {
            return None;
        }
        let covered = scope.folders.iter().any(|folder| inside(path, folder))
            || scope.above.contains(parent(path));
        if !covered {
            return None;
        }
        let text = fs::read_to_string(abs(root, path)).ok()?;
        text.contains("\"paths\"").then(|| {
            format!("{path} maps import paths (\"paths\"), which the project graph does not follow")
        })
    })
}

/// The file digest of each unit, computed from one snapshot.
pub struct UnitDigests {
    owned: Vec<String>,
}

impl UnitDigests {
    #[must_use]
    pub fn new(graph: Option<&Graph>) -> Self {
        Self {
            owned: owners(graph),
        }
    }

    /// The digest of the files `scope` covers in `snapshot`.
    ///
    /// # Errors
    ///
    /// Why the files cannot be hashed: a file could not be read, or the
    /// list of files could not be made.
    pub fn digest(&self, snapshot: &Snapshot, scope: &Scope) -> Result<String, String> {
        files_digest(snapshot, scope, &self.owned)
    }
}

/// A stored pass.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub format: u32,
    pub key: String,
    pub step: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub command: String,
    /// How long the step took when it ran.
    pub duration_ms: u64,
    /// When it ran, in seconds since 1970 (UTC).
    pub ran_at: u64,
    /// The line about the outcome when it ran.
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests: Option<crate::verify::TestCounts>,
    /// A blake3 digest of the step's output, for the shared cache's record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_digest: Option<String>,
    /// For a hit from the shared cache of CI: the commit of the default
    /// branch that wrote it. Never stored.
    #[serde(skip)]
    pub shared_from: Option<String>,
}

/// The cache folder of a repo.
#[must_use]
pub fn cache_dir(root: &Path) -> PathBuf {
    STEP_CACHE_DIR
        .iter()
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

fn entry_path(root: &Path, key: &str) -> PathBuf {
    cache_dir(root).join(format!("{key}.json"))
}

/// The stored pass for `key`, if there is one. A hit is touched, so the
/// least recently used entries are the ones removed.
#[must_use]
pub fn lookup(root: &Path, key: &str) -> Option<Entry> {
    let path = entry_path(root, key);
    let text = fs::read_to_string(&path).ok()?;
    let entry: Entry = serde_json::from_str(&text).ok()?;
    if entry.format != FORMAT || entry.key != key {
        return None;
    }
    // A pass older than the age limit is not served, even before the
    // folder is cleaned.
    if now_secs().saturating_sub(entry.ran_at) > MAX_AGE.as_secs() {
        return None;
    }
    if let Ok(file) = fs::OpenOptions::new().append(true).open(&path) {
        let _ = file.set_modified(SystemTime::now());
    }
    Some(entry)
}

/// Seconds since 1970, now.
#[must_use]
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Stores a pass. Written to a temporary file first and renamed, so a
/// reader never sees half an entry.
///
/// # Errors
///
/// When the folder or the file cannot be written.
pub fn store(root: &Path, entry: &Entry) -> std::io::Result<()> {
    let dir = cache_dir(root);
    fs::create_dir_all(&dir)?;
    ignore_firescale_folder(root);
    let text = serde_json::to_string_pretty(entry).map_err(std::io::Error::other)?;
    let temp = dir.join(format!(".{}.{}.tmp", entry.key, std::process::id()));
    fs::write(&temp, text)?;
    fs::rename(&temp, entry_path(root, &entry.key)).inspect_err(|_| {
        let _ = fs::remove_file(&temp);
    })
}

/// Writes `.firescale/.gitignore` (`*`) when it is missing, so firescale's
/// own files never show up as changes in git, also when the repo's own
/// `.gitignore` does not list `.firescale/`.
pub fn ignore_firescale_folder(root: &Path) {
    let ignore = root.join(".firescale").join(".gitignore");
    if !ignore.exists() {
        let _ = fs::write(
            ignore,
            "# Created by firescale. Nothing in this folder should be committed.\n*\n",
        );
    }
}

/// Removes entries older than [`MAX_AGE`], and the least recently used
/// beyond [`MAX_ENTRIES`]. Runs at the end of a verify run that stored
/// something, so the folder never needs a command of its own.
pub fn clean(root: &Path) {
    clean_with(root, MAX_AGE, MAX_ENTRIES, SystemTime::now());
}

/// [`clean`] with its limits given, for tests.
pub fn clean_with(root: &Path, max_age: Duration, max_entries: usize, now: SystemTime) {
    let Ok(entries) = fs::read_dir(cache_dir(root)) else {
        return;
    };
    let mut kept: Vec<(SystemTime, PathBuf)> = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let modified = meta.modified().unwrap_or(UNIX_EPOCH);
        let old = now.duration_since(modified).unwrap_or_default() > max_age;
        // A temporary file left by a run that was killed.
        let stale_temp = name.ends_with(".tmp")
            && now.duration_since(modified).unwrap_or_default() > Duration::from_secs(3600);
        if old || stale_temp {
            let _ = fs::remove_file(&path);
        } else if name.ends_with(".json") {
            kept.push((modified, path));
        }
    }
    if kept.len() > max_entries {
        kept.sort();
        let extra = kept.len() - max_entries;
        for (_, path) in kept.into_iter().take(extra) {
            let _ = fs::remove_file(path);
        }
    }
}

/// Reads an allowlisted variable from this process's environment.
#[must_use]
pub fn process_env(name: &str) -> Option<String> {
    debug_assert!(ENV_ALLOWLIST.contains(&name));
    std::env::var(name).ok()
}

/// The real machine, for tool versions.
#[must_use]
pub fn this_machine() -> &'static (dyn Machine + Sync) {
    &ThisMachine
}

/// A UTC time as `2026-11-13T07:06:00Z`.
#[must_use]
pub fn format_iso(secs: u64) -> String {
    let shown = format_time(secs);
    let (date, rest) = shown.split_once(' ').unwrap_or((&shown, "00:00 UTC"));
    let clock = rest.trim_end_matches(" UTC");
    format!("{date}T{clock}:{:02}Z", secs % 60)
}

type UnitSlot = (Option<String>, Option<String>);

/// The files of a base commit with their blob ids.
type BaseTree = Vec<(String, String)>;

/// The step cache of one verify run: the snapshot of the working tree it
/// started from, and the passes it will store at the end.
pub struct StepCache<'a> {
    root: PathBuf,
    version: String,
    read: bool,
    sets_ci: bool,
    graph: Option<Graph>,
    /// The latest snapshot of the working tree. Each step takes a new one
    /// before it is keyed (reusing the hashes of files whose size and time
    /// did not change), so a step that an earlier step's writes reach is
    /// never served from the cache under the old content.
    snapshot: std::sync::Mutex<Snapshot>,
    /// The working tree as it was when verify started, kept to tell whether
    /// a step's files changed during the run (ADR 0039).
    first: Snapshot,
    digests: UnitDigests,
    tools: ToolVersions<'a>,
    read_env: &'a (dyn Fn(&str) -> Option<String> + Sync),
    scopes: std::sync::Mutex<HashMap<UnitSlot, UnitScope>>,
    /// The shared cache of CI, read on a miss and written on the default
    /// branch (ADR 0041).
    shared: Option<crate::sharedcache::SharedCache>,
    /// Added to every key in CI: the workflow file's digest.
    salt: Option<String>,
    base_tree: std::sync::Mutex<Option<(String, BaseTree)>>,
    log: std::sync::Mutex<Vec<String>>,
}

/// A step's key, and the digest of its files when it was made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheKey {
    pub key: String,
    files: String,
}

/// How a step's run uses the cache.
pub struct CacheSettings<'a> {
    pub version: &'a str,
    /// False with `--no-cache`: nothing is read, passes are still stored.
    pub read: bool,
    /// True when verify sets `CI=1` for the commands.
    pub sets_ci: bool,
    pub machine: &'a (dyn Machine + Sync),
    pub read_env: &'a (dyn Fn(&str) -> Option<String> + Sync),
}

impl<'a> StepCache<'a> {
    /// Captures the working tree under `root`. `graph` is the project
    /// graph; without one, every step's key covers the whole repo.
    #[must_use]
    pub fn new(root: &Path, graph: Option<Graph>, settings: &CacheSettings<'a>) -> Self {
        let mut cache = Self::empty(root, graph, settings);
        cache.first = cache
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        cache
    }

    fn empty(root: &Path, graph: Option<Graph>, settings: &CacheSettings<'a>) -> Self {
        Self {
            root: root.to_path_buf(),
            version: settings.version.to_owned(),
            read: settings.read,
            sets_ci: settings.sets_ci,
            digests: UnitDigests::new(graph.as_ref()),
            graph,
            snapshot: std::sync::Mutex::new(Snapshot::capture(root)),
            first: Snapshot::default(),
            tools: ToolVersions::new(settings.machine),
            read_env: settings.read_env,
            scopes: std::sync::Mutex::new(HashMap::new()),
            shared: None,
            salt: None,
            base_tree: std::sync::Mutex::new(None),
            log: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Adds what CI adds to every key (the workflow file's digest), and the
    /// shared cache to read on a miss and, on the default branch, to write
    /// (ADR 0041).
    #[must_use]
    pub fn with_ci(
        mut self,
        salt: Option<String>,
        shared: Option<crate::sharedcache::SharedCache>,
    ) -> Self {
        self.salt = salt;
        self.shared = shared;
        self
    }

    /// The key with what CI adds to it.
    fn salted(&self, key: String) -> String {
        match &self.salt {
            None => key,
            Some(salt) => blake3::hash(format!("{key}\0{salt}").as_bytes())
                .to_hex()
                .to_string(),
        }
    }

    /// False with `--no-cache`.
    #[must_use]
    pub fn reads(&self) -> bool {
        self.read
    }

    /// A new snapshot of the working tree, reusing what did not change.
    fn fresh(&self) -> Snapshot {
        let earlier = self
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let now = Snapshot::capture_reusing(&self.root, Some(&earlier));
        *self
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = now.clone();
        now
    }

    /// What the unit's steps read, or why they are not cached.
    fn scope(
        &self,
        folder: Option<&str>,
        kind: Option<&str>,
        dir: &Path,
        snapshot: &Snapshot,
    ) -> UnitScope {
        let slot = (folder.map(str::to_owned), kind.map(str::to_owned));
        let mut scopes = self
            .scopes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        scopes
            .entry(slot)
            .or_insert_with(|| {
                let scope = unit_scope(self.graph.as_ref(), folder, kind)?;
                if let Some(reason) = outside_service(snapshot, &scope, &self.root, dir) {
                    return Err(reason);
                }
                if let Some(reason) = path_aliases(snapshot, &scope, &self.root) {
                    return Err(reason);
                }
                Ok(scope)
            })
            .clone()
    }

    /// The key of a step, made from the working tree as it is now, or why
    /// it is not cached.
    ///
    /// # Errors
    ///
    /// The reason the step is not cached.
    pub fn key(
        &self,
        folder: Option<&str>,
        kind: Option<&str>,
        step: &str,
        command: &str,
        dir: &Path,
    ) -> Result<CacheKey, String> {
        let snapshot = self.fresh();
        let scope = self.scope(folder, kind, dir, &snapshot)?;
        let files = self.digests.digest(&snapshot, &scope)?;
        let key = step_key(
            &self.root,
            &StepInputs {
                version: &self.version,
                step,
                project: folder,
                kind,
                command,
                dir,
                sets_ci: self.sets_ci,
            },
            &files,
            &self.tools,
            self.read_env,
        )?;
        let key = self.salted(key);
        Ok(CacheKey { key, files })
    }

    /// The stored pass for `key`, unless reading is off: this machine's,
    /// else a record of the shared cache of CI (ADR 0041).
    #[must_use]
    pub fn hit(&self, key: &CacheKey) -> Option<Entry> {
        if !self.read {
            return None;
        }
        // A run that writes the shared cache never takes a pass from this
        // machine's own cache: on a runner that is kept between jobs it may
        // come from a pull request's run, and the record would vouch for it.
        let writes = self.shared.as_ref().is_some_and(|shared| shared.writes());
        if !writes {
            if let Some(entry) = lookup(&self.root, &key.key) {
                return Some(entry);
            }
        }
        let shared = self.shared.as_ref()?;
        match shared.lookup(&key.key) {
            Ok(record) => Some(Entry {
                format: FORMAT,
                key: record.key,
                step: String::new(),
                project: None,
                command: String::new(),
                duration_ms: record.duration_ms,
                ran_at: record.time,
                detail: String::new(),
                tests: None,
                output_digest: Some(record.output_digest),
                shared_from: Some(record.writer.commit),
            }),
            Err(refusal) => {
                if refusal != "no record" {
                    self.note(format!("cache: shared record not used: {refusal}"));
                }
                None
            }
        }
    }

    /// Stores a pass as soon as its step finishes, so a run that is stopped
    /// later keeps it (ADR 0043), if the files it depends on are the same
    /// now as when it started: a step that wrote a file it depends on, or
    /// one whose files another step or a person changed while it ran, is
    /// not stored. Returns why it was not stored, if it was not.
    pub fn remember(
        &self,
        folder: Option<&str>,
        kind: Option<&str>,
        key: &CacheKey,
        mut entry: Entry,
    ) -> Option<String> {
        entry.format = FORMAT;
        entry.key.clone_from(&key.key);
        let slot = (folder.map(str::to_owned), kind.map(str::to_owned));
        let scope = self
            .scopes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&slot)
            .cloned();
        let Some(Ok(scope)) = scope else {
            return None;
        };
        let place = entry.project.as_deref().map_or_else(
            || entry.step.clone(),
            |project| format!("{} in {project}", entry.step),
        );
        let now = self.fresh();
        match self.digests.digest(&now, &scope) {
            Ok(after) if after == key.files => {}
            _ => {
                let why = "files it depends on changed while verify ran".to_owned();
                self.note(format!("cache: not stored {place}: {why}"));
                return Some(why);
            }
        }
        let mut not_stored = None;
        match store(&self.root, &entry) {
            Ok(()) => self.note(format!("cache: stored {place}")),
            Err(error) => {
                let why = format!("it could not be written to .firescale/steps ({error})");
                self.note(format!("cache: not stored {place}: {why}"));
                not_stored = Some(why);
            }
        }
        if let Some(shared) = self.shared.as_ref().filter(|shared| shared.writes()) {
            let digest = entry.output_digest.as_deref().unwrap_or("");
            match shared.write(&entry.key, entry.ran_at, entry.duration_ms, digest) {
                Ok(()) => self.note(format!("cache: shared record written for {place}")),
                Err(error) => self.note(format!(
                    "cache: shared record not written for {place}: {error}"
                )),
            }
        }
        not_stored
    }

    /// True when the step's key is the same on the base commit and the
    /// working tree: the step is cached (every refusal of [`Self::key`]
    /// holds here too) and no file its key holds differs between them
    /// (`changed` lists the paths that do). Then its head result is its
    /// base result (ADR 0037).
    #[must_use]
    pub fn same_on_base(
        &self,
        folder: Option<&str>,
        kind: Option<&str>,
        step: &str,
        command: &str,
        dir: &Path,
        changed: &BTreeSet<String>,
    ) -> bool {
        if self.key(folder, kind, step, command, dir).is_err() {
            return false;
        }
        let snapshot = self
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Ok(scope) = self.scope(folder, kind, dir, &snapshot) else {
            return false;
        };
        let owned = &self.digests.owned;
        !changed.iter().any(|path| covers(&scope, owned, path))
    }

    /// The key of a step's result on the base commit: the step's own key
    /// with the base commit's files (by git's blob ids) in place of the
    /// working tree's, the head's lockfiles, since the base run uses the
    /// head's installed dependencies, and `environment`: what the base
    /// worktree gets (the linked folders, the install command) (ADR 0037).
    ///
    /// # Errors
    ///
    /// Why the base result cannot be kept.
    #[allow(clippy::too_many_arguments)]
    pub fn base_key(
        &self,
        folder: Option<&str>,
        kind: Option<&str>,
        step: &str,
        command: &str,
        dir: &Path,
        commit: &str,
        environment: &str,
    ) -> Result<String, String> {
        let snapshot = self
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let scope = self
            .scope(folder, kind, dir, &snapshot)
            .unwrap_or_else(|_| Scope::whole_repo());
        let tree = self.base_tree(commit)?;
        let owned = &self.digests.owned;
        let mut files = blake3::Hasher::new();
        files.update(format!("base\0{commit}\0{environment}\n").as_bytes());
        for (path, id) in &tree {
            if covers(&scope, owned, path) {
                files.update(format!("{path}\0{id}\n").as_bytes());
            }
        }
        for (path, (state, _)) in &snapshot.files {
            let name = path.rsplit('/').next().unwrap_or(path);
            if crate::facts::LOCKFILE_NAMES.contains(&name)
                || crate::facts::is_dependency_list(path)
            {
                files.update(format!("head-lock\0{path}\0{state:?}\n").as_bytes());
            }
        }
        let files = files.finalize().to_hex().to_string();
        step_key(
            &self.root,
            &StepInputs {
                version: &self.version,
                step,
                project: folder,
                kind,
                command,
                dir,
                sets_ci: self.sets_ci,
            },
            &files,
            &self.tools,
            self.read_env,
        )
        .map(|key| format!("base-{}", self.salted(key)))
    }

    /// The lockfiles and requirements lists among `changed` (the paths that
    /// differ between the base commit and the head) that the step's key
    /// covers: the whole repo for a step the cache does not key. When one
    /// is there, the head's installed dependencies do not stand for the
    /// base's for this step (ADR 0040).
    #[must_use]
    pub fn changed_dependencies(
        &self,
        folder: Option<&str>,
        kind: Option<&str>,
        dir: &Path,
        changed: &BTreeSet<String>,
    ) -> Vec<String> {
        let snapshot = self
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let scope = self
            .scope(folder, kind, dir, &snapshot)
            .unwrap_or_else(|_| Scope::whole_repo());
        let owned = &self.digests.owned;
        changed
            .iter()
            .filter(|path| crate::facts::is_dependency_list(path))
            .filter(|path| covers(&scope, owned, path))
            .cloned()
            .collect()
    }

    /// The files of the base commit under the root, with their blob ids.
    fn base_tree(&self, commit: &str) -> Result<Vec<(String, String)>, String> {
        let mut cached = self
            .base_tree
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((at, tree)) = cached.as_ref() {
            if at == commit {
                return Ok(tree.clone());
            }
        }
        let out = run_git(
            &self.root,
            &[
                "-c",
                "core.quotepath=off",
                "ls-tree",
                "-r",
                "-z",
                commit,
                "--",
                ".",
            ],
        )
        .map_err(|error| error.message().to_owned())?;
        if out.code != 0 {
            return Err(format!("git ls-tree {commit} failed"));
        }
        let tree: Vec<(String, String)> = out
            .stdout
            .split('\0')
            .filter_map(|line| {
                let (meta, path) = line.split_once('\t')?;
                let id = meta.split_whitespace().nth(2)?;
                let mode = meta.split_whitespace().next()?;
                Some((path.replace('\\', "/"), format!("{mode}:{id}")))
            })
            .collect();
        *cached = Some((commit.to_owned(), tree.clone()));
        Ok(tree)
    }

    /// True when a file the step's key covers (the whole repo when the step
    /// is not cached) differs now from when verify started: a step that
    /// wrote its own inputs, or another step that did. Then a pass on a
    /// second run says nothing about flakiness (ADR 0039).
    #[must_use]
    pub fn changed_during_run(&self, folder: Option<&str>, kind: Option<&str>, dir: &Path) -> bool {
        let now = self.fresh();
        if now.problem.is_some() || self.first.problem.is_some() {
            return true;
        }
        let scope = self
            .scope(folder, kind, dir, &now)
            .unwrap_or_else(|_| Scope::whole_repo());
        let owned = &self.digests.owned;
        match (
            files_digest(&self.first, &scope, owned),
            files_digest(&now, &scope, owned),
        ) {
            (Ok(before), Ok(after)) => before != after,
            _ => true,
        }
    }

    /// Adds a line for `FSCALE_LOG=cache`.
    pub fn note(&self, line: String) {
        self.log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(line);
    }

    /// One line when the firescale config changed since the last run that
    /// used this cache: the config is in every step's key (a file at the
    /// repo root), so no kept pass applies any more (ADR 0043). Said once,
    /// on the first run after the change; the config's digest is kept in
    /// `.firescale/step-cache-config.digest`, beside the cache's folder.
    #[must_use]
    pub fn config_note(&self) -> Option<String> {
        let mut hasher = blake3::Hasher::new();
        let mut names = Vec::new();
        for name in [crate::config::JSON_CONFIG, crate::config::TS_CONFIG] {
            if let Ok(bytes) = fs::read(self.root.join(name)) {
                hasher.update(name.as_bytes());
                hasher.update(b"\0");
                hasher.update(&bytes);
                names.push(name);
            }
        }
        let digest = hasher.finalize().to_hex().to_string();
        let dir = cache_dir(&self.root);
        let file = self
            .root
            .join(".firescale")
            .join("step-cache-config.digest");
        let before = fs::read_to_string(&file).ok();
        if before.as_deref().map(str::trim) == Some(digest.as_str()) {
            return None;
        }
        let had_passes = fs::read_dir(&dir).is_ok_and(|entries| {
            entries
                .filter_map(Result::ok)
                .any(|entry| entry.file_name().to_string_lossy().ends_with(".json"))
        });
        if file
            .parent()
            .is_some_and(|parent| fs::create_dir_all(parent).is_ok())
        {
            ignore_firescale_folder(&self.root);
            let _ = fs::write(&file, &digest);
        }
        let name = names.first().copied().unwrap_or(crate::config::JSON_CONFIG);
        (before.is_some() && had_passes).then(|| {
            format!(
                "{name} changed since the last run, and it is part of every step's cache key, so no pass kept before applies: every step runs again this time."
            )
        })
    }

    /// Ends the run: cleans the cache folder (passes were stored as their
    /// steps finished) and prunes the shared cache.
    pub fn finish(&self) {
        clean(&self.root);
        if let Some(shared) = &self.shared {
            let removed = shared.prune();
            if removed > 0 {
                self.note(format!(
                    "cache: removed {removed} shared files that were not usable records"
                ));
            }
        }
    }

    /// The lines for `FSCALE_LOG=cache`, in the order they were noted.
    #[must_use]
    pub fn log(&self) -> Vec<String> {
        self.log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// A UTC time for people: `2026-11-13 07:06 UTC`.
#[must_use]
pub fn format_time(secs: u64) -> String {
    let days = secs / 86_400;
    let rest = secs % 86_400;
    let (hour, minute) = (rest / 3600, (rest % 3600) / 60);
    // Civil date from days since 1970 (Howard Hinnant's algorithm).
    let z = i64::try_from(days).unwrap_or(0) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02} UTC")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::detect_facts_with;
    use crate::git::run_git;
    use crate::graph::build_graph;
    use crate::testing::COURSE_SHARED_USERS;
    use crate::testing::{course_project, Fixture, Track};

    struct NoTools;
    impl Machine for NoTools {
        fn run(&self, _: &Path, program: &str, _: &[&str]) -> Option<String> {
            Some(format!("{program} 1.0.0"))
        }
        fn env_set(&self, _: &str) -> bool {
            false
        }
        fn listening(&self, _: &str, _: u16) -> bool {
            false
        }
    }

    fn key_of(
        root: &Path,
        graph: Option<&Graph>,
        folder: Option<&str>,
        kind: Option<&str>,
    ) -> Result<String, String> {
        let snapshot = Snapshot::capture(root);
        let scope = unit_scope(graph, folder, kind)?;
        let dir = folder.map_or_else(|| root.to_path_buf(), |f| abs(root, f));
        if let Some(reason) = outside_service(&snapshot, &scope, root, &dir) {
            return Err(reason);
        }
        if let Some(reason) = path_aliases(&snapshot, &scope, root) {
            return Err(reason);
        }
        let files = UnitDigests::new(graph).digest(&snapshot, &scope)?;
        let machine = NoTools;
        let tools = ToolVersions::new(&machine);
        step_key(
            root,
            &StepInputs {
                version: "0.0.0",
                step: "test",
                project: folder,
                kind,
                command: "node -e 0",
                dir: &dir,
                sets_ci: true,
            },
            &files,
            &tools,
            &|_| None,
        )
    }

    #[test]
    fn times_are_shown_in_utc() {
        assert_eq!(format_time(0), "1970-01-01 00:00 UTC");
        assert_eq!(format_time(1_794_553_560), "2026-11-13 07:06 UTC");
    }

    #[test]
    fn a_crlf_file_is_a_different_key() {
        let fixture = Fixture::new(&[("a.py", "x = 1\n")], Track::NoGit).unwrap();
        let before = key_of(fixture.root(), None, None, None).unwrap();
        fs::write(fixture.root().join("a.py"), "x = 1\r\n").unwrap();
        let after = key_of(fixture.root(), None, None, None).unwrap();
        assert_ne!(before, after);
    }

    #[test]
    fn untracked_files_count_and_ignored_files_do_not() {
        let fixture = Fixture::new(
            &[(".gitignore", "build/\n"), ("a.py", "x = 1\n")],
            Track::All,
        )
        .unwrap();
        let root = fixture.root();
        let first = key_of(root, None, None, None).unwrap();
        fs::create_dir_all(root.join("build")).unwrap();
        fs::write(root.join("build/out.txt"), "ignored").unwrap();
        assert_eq!(first, key_of(root, None, None, None).unwrap());
        fs::write(root.join("new.py"), "y = 2\n").unwrap();
        let second = key_of(root, None, None, None).unwrap();
        assert_ne!(first, second);
        // A tracked file with an uncommitted change.
        fs::write(root.join("a.py"), "x = 2\n").unwrap();
        assert_ne!(second, key_of(root, None, None, None).unwrap());
    }

    #[test]
    fn the_cache_folder_is_never_part_of_a_key() {
        let fixture = Fixture::new(&[("a.py", "x = 1\n")], Track::All).unwrap();
        let root = fixture.root();
        let first = key_of(root, None, None, None).unwrap();
        fs::create_dir_all(cache_dir(root)).unwrap();
        fs::write(cache_dir(root).join("x.json"), "{}").unwrap();
        assert_eq!(first, key_of(root, None, None, None).unwrap());
    }

    #[test]
    fn a_service_the_repo_does_not_start_is_never_cached() {
        let fixture = Fixture::new(
            &[
                ("api/main.py", "print(1)\n"),
                (
                    "api/.env.example",
                    "DATABASE_URL=postgresql://u:p@localhost:5432/db\n",
                ),
            ],
            Track::NoGit,
        )
        .unwrap();
        let reason = key_of(fixture.root(), None, Some("api"), Some("python")).unwrap_err();
        assert!(reason.contains("PostgreSQL on localhost:5432"), "{reason}");
        assert!(reason.contains("does not start"), "{reason}");
    }

    #[test]
    fn a_service_from_compose_can_be_cached() {
        let fixture = Fixture::new(
            &[
                ("api/main.py", "print(1)\n"),
                (
                    "api/.env.example",
                    "DATABASE_URL=postgresql://u:p@localhost:5432/db\n",
                ),
                (
                    "compose.yaml",
                    "services:\n  db:\n    image: postgres:16\n    ports:\n      - \"5432:5432\"\n",
                ),
            ],
            Track::NoGit,
        )
        .unwrap();
        key_of(fixture.root(), None, Some("api"), Some("python")).unwrap();
    }

    #[test]
    fn a_dynamic_import_is_never_cached() {
        let fixture = Fixture::new(
            &[
                ("a/pyproject.toml", "[project]\nname = \"a\"\n"),
                (
                    "a/a/__init__.py",
                    "import importlib\nm = importlib.import_module(name)\n",
                ),
                ("b/pyproject.toml", "[project]\nname = \"b\"\n"),
                ("b/b/__init__.py", "x = 1\n"),
            ],
            Track::NoGit,
        )
        .unwrap();
        let facts = detect_facts_with(fixture.root(), None).unwrap();
        let graph = build_graph(fixture.root(), &facts);
        let reason = unit_scope(Some(&graph), Some("a"), Some("python")).unwrap_err();
        assert!(reason.contains("run time"), "{reason}");
        assert!(unit_scope(Some(&graph), Some("b"), Some("python")).is_ok());
    }

    #[test]
    fn what_ci_adds_changes_every_key() {
        let fixture = Fixture::new(&[("a.js", "1\n")], Track::NoGit).unwrap();
        let settings = |read_env: &'static (dyn Fn(&str) -> Option<String> + Sync)| CacheSettings {
            version: "1.0.0",
            read: true,
            sets_ci: true,
            machine: this_machine(),
            read_env,
        };
        let none: &'static (dyn Fn(&str) -> Option<String> + Sync) = &|_| None;
        let key = |salt: Option<&str>| {
            StepCache::new(fixture.root(), None, &settings(none))
                .with_ci(salt.map(str::to_owned), None)
                .key(None, None, "test", "node -e 0", fixture.root())
                .unwrap()
                .key
        };
        assert_eq!(key(None), key(None));
        assert_ne!(key(None), key(Some("ci-workflow\0a")));
        assert_ne!(key(Some("ci-workflow\0a")), key(Some("ci-workflow\0b")));
    }

    #[test]
    fn an_unreadable_input_is_never_cached() {
        let snapshot = Snapshot {
            files: BTreeMap::from([(
                "a.py".to_owned(),
                (FileState::Unreadable("permission denied".to_owned()), None),
            )]),
            problem: None,
            taken: None,
        };
        let reason = UnitDigests::new(None)
            .digest(&snapshot, &Scope::whole_repo())
            .unwrap_err();
        assert!(reason.contains("a.py could not be read"), "{reason}");
        let unlisted = Snapshot {
            files: BTreeMap::new(),
            problem: Some("git ls-files failed".to_owned()),
            taken: None,
        };
        assert!(UnitDigests::new(None)
            .digest(&unlisted, &Scope::whole_repo())
            .is_err());
    }

    #[test]
    fn a_service_key_reads_only_allowlisted_variables() {
        let fixture = Fixture::new(&[("a.py", "x = 1\n")], Track::NoGit).unwrap();
        let root = fixture.root();
        let snapshot = Snapshot::capture(root);
        let files = UnitDigests::new(None)
            .digest(&snapshot, &Scope::whole_repo())
            .unwrap();
        let asked = std::sync::Mutex::new(Vec::new());
        let machine = NoTools;
        let tools = ToolVersions::new(&machine);
        let key = |value: &'static str| {
            step_key(
                root,
                &StepInputs {
                    version: "0.0.0",
                    step: "test",
                    project: None,
                    kind: None,
                    command: "node -e 0",
                    dir: root,
                    sets_ci: true,
                },
                &files,
                &tools,
                &|name| {
                    asked.lock().unwrap().push(name.to_owned());
                    (name == "NODE_ENV").then(|| value.to_owned())
                },
            )
            .unwrap()
        };
        let a = key("test");
        let b = key("production");
        assert_ne!(a, b);
        assert!(asked
            .lock()
            .unwrap()
            .iter()
            .all(|name| ENV_ALLOWLIST.contains(&name.as_str())));
    }

    #[test]
    fn the_course_project_scopes_follow_the_graph() {
        let fixture = course_project().unwrap();
        let user = COURSE_SHARED_USERS[0];
        let folder = format!("backend/{user}");
        let facts = detect_facts_with(fixture.root(), None).unwrap();
        let graph = build_graph(fixture.root(), &facts);
        let scope = unit_scope(Some(&graph), Some(&folder), Some("python")).unwrap();
        assert!(scope.folders.contains(&folder));
        assert!(scope.folders.contains("backend/shared"));
        assert!(!scope.folders.contains("frontend"));
        assert!(scope.above.contains("backend") && scope.above.contains("."));
    }

    #[test]
    fn cleaning_removes_old_and_least_recently_used_entries() {
        let fixture = Fixture::new(&[("a.py", "x = 1\n")], Track::NoGit).unwrap();
        let root = fixture.root();
        let entry = |key: &str| Entry {
            format: FORMAT,
            key: key.to_owned(),
            step: "test".to_owned(),
            project: None,
            command: "true".to_owned(),
            duration_ms: 1,
            ran_at: now_secs(),
            detail: String::new(),
            tests: None,
            output_digest: None,
            shared_from: None,
        };
        for key in ["a", "b", "c", "d"] {
            store(root, &entry(key)).unwrap();
        }
        let now = SystemTime::now();
        let set = |key: &str, age: u64| {
            let file = fs::OpenOptions::new()
                .append(true)
                .open(entry_path(root, key))
                .unwrap();
            file.set_modified(now - Duration::from_secs(age)).unwrap();
        };
        set("a", 100 * 24 * 3600);
        set("b", 30);
        set("c", 20);
        set("d", 10);
        clean_with(root, MAX_AGE, 2, now);
        assert!(lookup(root, "a").is_none(), "older than the age limit");
        assert!(lookup(root, "b").is_none(), "least recently used");
        assert!(lookup(root, "c").is_some());
        assert!(lookup(root, "d").is_some());
    }

    #[test]
    fn a_root_step_sees_the_services_of_the_projects_below() {
        let fixture = Fixture::new(
            &[
                ("api/main.py", "print(1)\n"),
                (
                    "api/.env.example",
                    "DATABASE_URL=postgresql://u:p@localhost:5432/db\n",
                ),
            ],
            Track::NoGit,
        )
        .unwrap();
        let reason = key_of(fixture.root(), None, None, None).unwrap_err();
        assert!(reason.contains("PostgreSQL on localhost:5432"), "{reason}");
    }

    #[test]
    fn a_submodule_or_nested_repository_is_never_cached() {
        let fixture =
            Fixture::new(&[("a.js", "1\n"), ("nested/n.txt", "x\n")], Track::None).unwrap();
        let nested = fixture.root().join("nested");
        let init = run_git(&nested, &["init", "--quiet"]).unwrap();
        assert_eq!(init.code, 0);
        let reason = key_of(fixture.root(), None, None, None).unwrap_err();
        assert!(reason.contains("nested repository"), "{reason}");
    }

    #[cfg(unix)]
    #[test]
    fn a_link_counts_by_what_it_points_to() {
        let fixture = Fixture::new(&[("a.js", "1\n")], Track::NoGit).unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("data.txt");
        fs::write(&target, "one").unwrap();
        std::os::unix::fs::symlink(&target, fixture.root().join("data.txt")).unwrap();
        let first = key_of(fixture.root(), None, None, None).unwrap();
        fs::write(&target, "two").unwrap();
        assert_ne!(first, key_of(fixture.root(), None, None, None).unwrap());
        std::os::unix::fs::symlink(outside.path(), fixture.root().join("folder")).unwrap();
        let reason = key_of(fixture.root(), None, None, None).unwrap_err();
        assert!(reason.contains("a link to the folder"), "{reason}");
    }

    #[test]
    fn a_root_inside_a_larger_repository_is_never_cached() {
        let fixture =
            Fixture::new(&[("sub/a.js", "1\n"), ("shared.txt", "x\n")], Track::None).unwrap();
        let reason = key_of(&fixture.root().join("sub"), None, None, None).unwrap_err();
        assert!(
            reason.contains("inside a larger git repository"),
            "{reason}"
        );
    }

    #[test]
    fn a_repo_script_is_never_run_for_its_version() {
        for command in [
            "scripts/python-tests.sh",
            "./node test.js",
            "./python3 -m pytest",
            "python-tests.sh",
            "yarn test",
            "make test",
            "bin/pytest",
        ] {
            assert!(probes(command).is_empty(), "{command} would be run");
        }
        assert_eq!(probes(".venv/bin/python -m pytest").len(), 1);
        assert_eq!(probes("venv\\Scripts\\python.exe -m pytest").len(), 1);
        assert_eq!(probes("python3.12 -m pytest").len(), 1);
        assert_eq!(probes("pnpm test").len(), 2);
        // A build wrapper is not run, but the JDK it starts is asked.
        assert_eq!(
            probes("./gradlew test"),
            vec![("java".to_owned(), vec!["--version"])]
        );
        assert_eq!(probes("mvn -B test").len(), 2);
        assert_eq!(probes("swift test").len(), 1);
    }

    #[test]
    fn a_program_that_is_not_found_is_never_cached() {
        let fixture = Fixture::new(&[("a.js", "1\n")], Track::NoGit).unwrap();
        let reason = program_identity(fixture.root(), "no-such-program-firescale --x").unwrap_err();
        assert!(reason.contains("was not found"), "{reason}");
    }

    #[test]
    fn import_path_aliases_and_history_turn_the_cache_off() {
        let fixture = Fixture::new(
            &[
                ("a.js", "1\n"),
                (
                    "tsconfig.json",
                    "{ \"compilerOptions\": { \"paths\": { \"@x/*\": [\"libs/x/*\"] } } }\n",
                ),
            ],
            Track::NoGit,
        )
        .unwrap();
        let reason = key_of(fixture.root(), None, None, None).unwrap_err();
        assert!(reason.contains("\"paths\""), "{reason}");
        assert!(uses_history("pnpm exec nx affected -t test --base=main"));
        assert!(uses_history("pnpm exec turbo run test --affected"));
        assert!(!uses_history("pnpm test"));
    }

    #[test]
    fn a_stale_entry_is_not_served() {
        let fixture = Fixture::new(&[("a.py", "x = 1\n")], Track::NoGit).unwrap();
        let entry = Entry {
            format: FORMAT,
            key: "k".to_owned(),
            step: "test".to_owned(),
            project: None,
            command: "true".to_owned(),
            duration_ms: 1,
            ran_at: now_secs() - MAX_AGE.as_secs() - 10,
            detail: String::new(),
            tests: None,
            output_digest: None,
            shared_from: None,
        };
        store(fixture.root(), &entry).unwrap();
        assert!(lookup(fixture.root(), "k").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_launcher_with_no_shebang_line_gives_its_version() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("tool");
        // pnpm's own launcher under `pnpm exec` is such a script.
        fs::write(&tool, "echo \"tool 1.2.3 $1\"\n").unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
        let version = ThisMachine.version(dir.path(), &tool.to_string_lossy(), &["--version"]);
        assert_eq!(version.as_deref(), Some("tool 1.2.3 --version"));
    }

    #[test]
    fn a_name_without_an_extension_is_tried_as_cmd_tries_it_on_windows() {
        let exts = ".COM;.EXE;.BAT;.CMD";
        let npm = Path::new("C:/node").join("npm");
        let names: Vec<String> = windows_candidates(&npm, exts)
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        // Never the bare `npm`: Node's installer puts a shell script there.
        assert_eq!(names, ["npm.COM", "npm.EXE", "npm.BAT", "npm.CMD"]);
        let given = Path::new("C:/node").join("npm.cmd");
        assert_eq!(windows_candidates(&given, exts), vec![given.clone()]);
        // A dot in the name that is no extension of PATHEXT is not one.
        let dotted = Path::new("C:/py").join("python3.12");
        assert_eq!(windows_candidates(&dotted, exts).len(), 4);
    }

    #[cfg(windows)]
    #[test]
    fn the_version_of_npm_is_read_from_npm_cmd_beside_a_shell_script() {
        let dir = tempfile::tempdir().unwrap();
        // As Node's installer lays them out: a shell script and a batch file.
        fs::write(dir.path().join("npm"), "#!/bin/sh\necho shell\n").unwrap();
        fs::write(dir.path().join("npm.cmd"), "@echo off\r\necho 10.9.2\r\n").unwrap();
        let program = dir.path().join("npm");
        let found = locate(dir.path(), &program.to_string_lossy()).unwrap();
        // PATHEXT names it `.CMD`; Windows file names ignore case.
        assert!(
            found
                .to_string_lossy()
                .to_ascii_lowercase()
                .ends_with(".cmd"),
            "{found:?}"
        );
        let tools = ToolVersions::new(&ThisMachine);
        let version = tools.version(dir.path(), &program.to_string_lossy(), &["--version"]);
        assert!(version.is_some_and(|line| line.ends_with("10.9.2")));
    }
}
