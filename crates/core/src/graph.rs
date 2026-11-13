//! The project graph (ADR 0032): projects, the folders and files they share,
//! and the edges between them, each with the evidence that gave it.
//!
//! Edge sources, most trusted first:
//!
//! 1. Local paths in manifests: requirements files, `pyproject.toml` path
//!    sources, `package.json` `file:`, `link:` and `workspace:`, Cargo path
//!    dependencies, and dependencies on workspace members by name.
//! 2. A Dockerfile `COPY` of a folder outside the project, Compose
//!    `build.context` and `depends_on`, and shared inputs: a `.env.example`,
//!    Compose file or OpenAPI spec that several projects read.
//! 3. Python imports that resolve to another project or a shared folder, and
//!    JavaScript or TypeScript relative imports that leave the project's
//!    folder. Read with tree-sitter (ADR 0033), never with patterns.
//! 4. A dynamic Python import (`importlib.import_module`, `__import__`): the
//!    project counts as depending on every other Python project.
//!
//! A folder or file that projects use but that belongs to no project is a
//! **shared** node. A change there affects only the projects that use it.
//!
//! The graph is built on demand (`fscale graph`, `fscale facts`, `verify
//! --affected`), never by `detect_facts`, which rules call on every check.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use serde::Serialize;

use crate::affected::Changed;
use crate::facts::{Project, RepoFacts, DISCOVERY_SKIPPED_DIRS, LOCKFILE_NAMES};
use crate::imports::{js_dialect, js_imports, python_imports, JsImports, PythonImports};
use crate::references::{
    cargo_path_refs, compose_builds, dockerfile_copies, package_json_refs, pyproject_refs,
    requirements_refs,
};

/// Version of the `fscale graph --json` format.
pub const GRAPH_SCHEMA_VERSION: u32 = 1;

/// Source files larger than this are not read for imports: they are almost
/// always generated or bundled.
const MAX_SOURCE_BYTES: u64 = 1024 * 1024;

/// How deep the walk for source files goes. Deeper files are not read.
const MAX_SOURCE_DEPTH: usize = 32;

/// Where an edge came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    /// A dependency on another project of the repo by its name
    /// (`package.json`, `Cargo.toml`), as `dependsOn` in `fscale facts` lists it.
    Dependency,
    /// A local path in a requirements file: `-e ../x`, `../x`, `-r ../x.txt`.
    Requirements,
    /// A path source in `pyproject.toml`.
    Pyproject,
    /// `file:`, `link:`, `portal:` or `workspace:` in `package.json`.
    PackageJson,
    /// A `path` dependency in `Cargo.toml`.
    Cargo,
    /// A Dockerfile `COPY` or `ADD` of a folder outside the project.
    Dockerfile,
    /// A Compose `build.context` outside the project.
    ComposeBuild,
    /// A Compose `depends_on` between two services that are projects.
    ComposeDependsOn,
    /// A `.env.example`, Compose file or OpenAPI spec the project reads.
    SharedInput,
    /// A Python import.
    PythonImport,
    /// A JavaScript or TypeScript relative import.
    JsImport,
    /// A Python import of a module chosen at run time.
    DynamicImport,
}

impl Source {
    /// 1 is the most trusted: manifests. 4 is the widest guess.
    #[must_use]
    pub fn tier(self) -> u8 {
        match self {
            Self::Dependency
            | Self::Requirements
            | Self::Pyproject
            | Self::PackageJson
            | Self::Cargo => 1,
            Self::Dockerfile | Self::ComposeBuild | Self::ComposeDependsOn | Self::SharedInput => 2,
            Self::PythonImport | Self::JsImport => 3,
            Self::DynamicImport => 4,
        }
    }

    /// The source for people.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Dependency => "dependency by name",
            Self::Requirements => "requirements file",
            Self::Pyproject => "pyproject.toml",
            Self::PackageJson => "package.json",
            Self::Cargo => "Cargo.toml",
            Self::Dockerfile => "Dockerfile COPY",
            Self::ComposeBuild => "Compose build context",
            Self::ComposeDependsOn => "Compose depends_on",
            Self::SharedInput => "shared input",
            Self::PythonImport => "Python import",
            Self::JsImport => "JS/TS import",
            Self::DynamicImport => "dynamic import",
        }
    }
}

/// What a node is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NodeType {
    /// A project from `fscale facts`.
    Project,
    /// A folder or file that projects use and that belongs to no project.
    Shared,
}

/// A node of the graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Node {
    /// `<kind>:<path>` for a project (`python:backend/auth`), `shared:<path>`
    /// for a shared folder or file.
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: NodeType,
    /// Repo-relative with forward slashes; `.` is the repo root.
    pub path: String,
    /// For a project: `node`, `python`, `rust` or `go`. For a shared node:
    /// `folder` or `file`.
    pub kind: String,
    /// For a project: its name in `fscale facts`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// One piece of evidence for an edge.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct EdgeSource {
    pub source: Source,
    /// 1 (manifests) to 4 (dynamic imports); see [`Source::tier`].
    pub tier: u8,
    /// The repo-relative file the evidence is in.
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    /// What was found, for people: the line, or a sentence.
    pub detail: String,
}

/// `from` depends on `to`: a change in `to` affects `from`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    /// Most trusted first.
    pub sources: Vec<EdgeSource>,
}

/// The project graph.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Graph {
    /// Projects first, then shared nodes, each sorted by path.
    pub nodes: Vec<Node>,
    /// Sorted by `from`, then `to`.
    pub edges: Vec<Edge>,
    /// Things the graph cannot see, such as dynamic imports. Sorted.
    #[serde(skip)]
    pub warnings: Vec<String>,
}

impl Graph {
    #[must_use]
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|node| node.id == id)
    }

    /// How a node is named for people: its path, `(repo root)` for `.`, and
    /// the kind when two projects share the folder.
    #[must_use]
    pub fn display(&self, id: &str) -> String {
        let Some(node) = self.node(id) else {
            return id.to_owned();
        };
        let place = if node.path == "." {
            "(repo root)".to_owned()
        } else {
            node.path.clone()
        };
        let shared_folder = node.node_type == NodeType::Project
            && self
                .nodes
                .iter()
                .any(|other| other.id != node.id && other.path == node.path);
        if shared_folder || (node.path == "." && node.node_type == NodeType::Project) {
            format!("{place} [{}]", node.kind)
        } else {
            place
        }
    }

    /// The ids of the nodes `id` depends on.
    pub fn dependencies<'a>(&'a self, id: &'a str) -> impl Iterator<Item = &'a Edge> + 'a {
        self.edges.iter().filter(move |edge| edge.from == id)
    }
}

/// The `fscale graph --json` document. Described by
/// `packages/cli/schemas/graph-output.schema.json`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphReport<'a> {
    pub schema_version: u32,
    pub nodes: &'a [Node],
    pub edges: &'a [Edge],
    pub warnings: &'a [String],
}

// ------------------------------------------------------------------ paths

fn abs(root: &Path, rel: &str) -> PathBuf {
    if rel == "." {
        return root.to_path_buf();
    }
    rel.split('/')
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

fn is_dir(root: &Path, rel: &str) -> bool {
    abs(root, rel).is_dir()
}

fn is_file(root: &Path, rel: &str) -> bool {
    abs(root, rel).is_file()
}

fn exists(root: &Path, rel: &str) -> bool {
    abs(root, rel).exists()
}

fn join(dir: &str, name: &str) -> String {
    if dir == "." || dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

/// The folder of a repo-relative path; `.` at the top.
fn parent(path: &str) -> &str {
    path.rfind('/').map_or(".", |index| &path[..index])
}

fn parts(path: &str) -> Vec<&str> {
    if path == "." {
        Vec::new()
    } else {
        path.split('/').collect()
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

/// `target`, written relative to the folder `dir`, as a repo-relative path.
/// `None` for an absolute path, a URL, or one that leaves the repo.
fn resolve(dir: &str, target: &str) -> Option<String> {
    let target = target.trim();
    if target.is_empty()
        || target.starts_with(['/', '\\'])
        || target.as_bytes().get(1) == Some(&b':')
        || target.contains("://")
    {
        return None;
    }
    let mut out: Vec<&str> = parts(dir);
    for part in target.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            other => out.push(other),
        }
    }
    Some(if out.is_empty() {
        ".".to_owned()
    } else {
        out.join("/")
    })
}

/// The folder (or file) below the deepest common folder of `from` and
/// `target`, on `target`'s side: what `from` reaches when it uses `target`.
/// `backend/auth` using `backend/shared/db.py` reaches `backend/shared`.
fn reach(from: &str, target: &str) -> String {
    let a = parts(from);
    let b = parts(target);
    let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    if b.len() > common {
        b[..=common].join("/")
    } else {
        target.to_owned()
    }
}

/// The text of a file, without a byte-order mark. `None` when it cannot be read.
fn read(root: &Path, rel: &str) -> Option<String> {
    fs::read_to_string(abs(root, rel))
        .ok()
        .map(|text| text.trim_start_matches('\u{feff}').to_owned())
}

// ------------------------------------------------------------ the builder

#[derive(Debug, Clone)]
struct ProjectNode {
    id: String,
    root: String,
    kind: String,
    name: String,
}

/// The id of a project node: `<kind>:<folder>`.
pub fn project_id(kind: &str, root: &str) -> String {
    format!("{kind}:{root}")
}

fn shared_id(path: &str) -> String {
    format!("shared:{path}")
}

/// Who uses a source file's imports: a project, or a shared folder.
#[derive(Debug, Clone)]
struct Importer {
    id: String,
    /// The folder the importer's own code is in.
    root: String,
    is_root_project: bool,
}

struct Builder<'a> {
    root: &'a Path,
    projects: Vec<ProjectNode>,
    shared: BTreeMap<String, bool>,
    edges: BTreeMap<(String, String), BTreeSet<EdgeSource>>,
    warnings: BTreeSet<String>,
}

impl<'a> Builder<'a> {
    fn new(root: &'a Path, facts: &RepoFacts) -> Self {
        let mut projects: Vec<ProjectNode> = Vec::new();
        for project in &facts.projects {
            let id = project_id(&project.kind, &project.root);
            if projects.iter().any(|known| known.id == id) {
                continue;
            }
            projects.push(ProjectNode {
                id,
                root: project.root.clone(),
                kind: project.kind.clone(),
                name: project.name.clone(),
            });
        }
        projects.sort_by(|a, b| (&a.root, &a.kind).cmp(&(&b.root, &b.kind)));
        Self {
            root,
            projects,
            shared: BTreeMap::new(),
            edges: BTreeMap::new(),
            warnings: BTreeSet::new(),
        }
    }

    fn project(&self, id: &str) -> Option<&ProjectNode> {
        self.projects.iter().find(|project| project.id == id)
    }

    fn importer_of(&self, project: &ProjectNode) -> Importer {
        Importer {
            id: project.id.clone(),
            root: project.root.clone(),
            is_root_project: project.root == ".",
        }
    }

    /// The projects below the repo root with the deepest folder that holds
    /// `path`. Two when two kinds share that folder.
    fn owners(&self, path: &str) -> Vec<&ProjectNode> {
        let deepest = self
            .projects
            .iter()
            .filter(|project| project.root != "." && inside(path, &project.root))
            .map(|project| project.root.len())
            .max();
        match deepest {
            None => Vec::new(),
            Some(len) => self
                .projects
                .iter()
                .filter(|project| {
                    project.root != "." && project.root.len() == len && inside(path, &project.root)
                })
                .collect(),
        }
    }

    fn root_projects(&self) -> Vec<&ProjectNode> {
        self.projects
            .iter()
            .filter(|project| project.root == ".")
            .collect()
    }

    /// The nodes `importer` depends on when it uses `target` (a
    /// repo-relative path): nothing when the target is its own, the projects
    /// that own the target, or else a shared node.
    fn targets(&mut self, importer: &Importer, target: &str, source: Source) -> Vec<String> {
        let owners: Vec<String> = self
            .owners(target)
            .into_iter()
            .map(|project| project.id.clone())
            .filter(|id| *id != importer.id)
            .collect();
        if importer.is_root_project {
            // The repo root's project holds everything no other project
            // does, so only other projects are edges.
            return owners;
        }
        if inside(target, &importer.root) {
            return Vec::new();
        }
        if !owners.is_empty() {
            return owners;
        }
        if !self.owners(target).is_empty() {
            // Owned by the importer itself (another kind in the same folder).
            return Vec::new();
        }
        // A path a manifest, Dockerfile or Compose file names is the node
        // itself. An imported file stands for the package it is part of:
        // the folder below the deepest folder the two sides share.
        let is_import = matches!(source, Source::PythonImport | Source::JsImport);
        let path = if is_import && !is_dir(self.root, target) {
            self.narrow(reach(&importer.root, target), target)
        } else {
            target.to_owned()
        };
        let is_folder = is_dir(self.root, &path);
        self.shared.entry(path.clone()).or_insert(is_folder);
        vec![shared_id(&path)]
    }

    /// `path`, an ancestor of `target`, moved down toward `target` until it
    /// holds no project: `backend/shared`, not `backend`, for a frontend
    /// importing `../backend/shared/x.ts`, so the shared node does not
    /// swallow the files around the projects in `backend/`.
    fn narrow(&self, path: String, target: &str) -> String {
        let mut path = path;
        let wanted = parts(target);
        while path != target
            && self
                .projects
                .iter()
                .any(|project| project.root != "." && inside(&project.root, &path))
        {
            let depth = parts(&path).len();
            if depth >= wanted.len() {
                break;
            }
            path = wanted[..=depth].join("/");
        }
        path
    }

    fn add(
        &mut self,
        from: &str,
        to: &str,
        source: Source,
        file: &str,
        line: Option<usize>,
        detail: String,
    ) {
        if from == to {
            return;
        }
        self.edges
            .entry((from.to_owned(), to.to_owned()))
            .or_default()
            .insert(EdgeSource {
                source,
                tier: source.tier(),
                file: file.to_owned(),
                line,
                detail,
            });
    }

    fn link(
        &mut self,
        importer: &Importer,
        target: &str,
        source: Source,
        file: &str,
        line: Option<usize>,
        detail: &str,
    ) {
        for to in self.targets(importer, target, source) {
            self.add(&importer.id, &to, source, file, line, detail.to_owned());
        }
    }

    // ------------------------------------------------------------ tier 1

    fn workspace_names(&mut self, facts: &RepoFacts) {
        for project in &facts.projects {
            let manifest = match project.kind.as_str() {
                "node" => "package.json",
                "rust" => "Cargo.toml",
                _ => continue,
            };
            let from = project_id(&project.kind, &project.root);
            for name in &project.depends_on {
                let targets: Vec<String> = facts
                    .projects
                    .iter()
                    .filter(|other| other.kind == project.kind && other.name == *name)
                    .map(|other| project_id(&other.kind, &other.root))
                    .collect();
                for to in targets {
                    self.add(
                        &from,
                        &to,
                        Source::Dependency,
                        &join(&project.root, manifest),
                        None,
                        format!("depends on {name}, a project of this repo"),
                    );
                }
            }
        }
    }

    fn manifests(&mut self) {
        let projects = self.projects.clone();
        for project in &projects {
            let importer = self.importer_of(project);
            match project.kind.as_str() {
                "python" => {
                    for file in requirement_files(self.root, &project.root) {
                        let Some(text) = read(self.root, &file) else {
                            continue;
                        };
                        for found in requirements_refs(&text) {
                            if let Some(target) = resolve(parent(&file), &found.path) {
                                let detail = format!("{}: {}", found.what, found.text);
                                self.link(
                                    &importer,
                                    &target,
                                    Source::Requirements,
                                    &file,
                                    Some(found.line),
                                    &detail,
                                );
                            }
                        }
                    }
                    let file = join(&project.root, "pyproject.toml");
                    if let Some(text) = read(self.root, &file) {
                        for found in pyproject_refs(&text) {
                            if let Some(target) = resolve(&project.root, &found.path) {
                                let detail = format!("{}: {}", found.what, found.text);
                                self.link(
                                    &importer,
                                    &target,
                                    Source::Pyproject,
                                    &file,
                                    Some(found.line),
                                    &detail,
                                );
                            }
                        }
                    }
                }
                "node" => {
                    let file = join(&project.root, "package.json");
                    let Some(json) = read(self.root, &file)
                        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                    else {
                        continue;
                    };
                    for found in package_json_refs(&json) {
                        let detail = format!(
                            "{} \"{}\": \"{}\"",
                            found.field, found.dependency, found.spec
                        );
                        if let Some(path) = &found.path {
                            if let Some(target) = resolve(&project.root, path) {
                                self.link(
                                    &importer,
                                    &target,
                                    Source::PackageJson,
                                    &file,
                                    None,
                                    &detail,
                                );
                            }
                        } else if let Some(name) = &found.package {
                            let targets: Vec<String> = self
                                .projects
                                .iter()
                                .filter(|other| {
                                    other.kind == "node"
                                        && other.name == *name
                                        && other.id != project.id
                                })
                                .map(|other| other.id.clone())
                                .collect();
                            for to in targets {
                                self.add(
                                    &project.id,
                                    &to,
                                    Source::PackageJson,
                                    &file,
                                    None,
                                    detail.clone(),
                                );
                            }
                        }
                    }
                }
                "rust" => {
                    let file = join(&project.root, "Cargo.toml");
                    if let Some(text) = read(self.root, &file) {
                        for found in cargo_path_refs(&text) {
                            if let Some(target) = resolve(&project.root, &found.path) {
                                let detail = format!("{}: {}", found.what, found.text);
                                self.link(
                                    &importer,
                                    &target,
                                    Source::Cargo,
                                    &file,
                                    Some(found.line),
                                    &detail,
                                );
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // ------------------------------------------------------------ tier 2

    /// The project an image builds: the owner of its Dockerfile, else of its
    /// build context, else the repo root's project.
    fn image_project(&self, dockerfile: &str, context: &str) -> Option<Importer> {
        let owner = self
            .owners(dockerfile)
            .first()
            .copied()
            .or_else(|| self.owners(context).first().copied())
            .or_else(|| self.root_projects().first().copied())?;
        Some(self.importer_of(owner))
    }

    /// Compose files: build contexts, `depends_on`, and the file itself as an
    /// input of each project it builds. Returns the build context of each
    /// Dockerfile a Compose file names, with where it was said.
    fn compose(&mut self, files: &[String]) -> BTreeMap<String, Vec<(String, String)>> {
        let mut contexts: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
        for file in files {
            let Some(text) = read(self.root, file) else {
                continue;
            };
            let dir = parent(file).to_owned();
            let builds = compose_builds(&text);
            let mut images: BTreeMap<String, Importer> = BTreeMap::new();
            for build in &builds {
                let Some((context, line)) = &build.context else {
                    continue;
                };
                let Some(context_path) = resolve(&dir, context) else {
                    continue;
                };
                let dockerfile = build
                    .dockerfile
                    .as_deref()
                    .and_then(|name| resolve(&context_path, name))
                    .unwrap_or_else(|| join(&context_path, "Dockerfile"));
                contexts
                    .entry(dockerfile.clone())
                    .or_default()
                    .push((context_path.clone(), file.clone()));
                let Some(importer) = self.image_project(&dockerfile, &context_path) else {
                    continue;
                };
                // A context above the project only says what COPY may reach;
                // the Dockerfile's COPY lines decide.
                if !inside(&importer.root, &context_path) {
                    let detail = format!("service {}: build context {context}", build.service);
                    self.link(
                        &importer,
                        &context_path,
                        Source::ComposeBuild,
                        file,
                        Some(*line),
                        &detail,
                    );
                }
                let detail = format!("service {} is built from this project", build.service);
                self.link(
                    &importer,
                    file,
                    Source::SharedInput,
                    file,
                    Some(build.line),
                    &detail,
                );
                images.insert(build.service.clone(), importer);
            }
            for build in &builds {
                let Some(from) = images.get(&build.service).cloned() else {
                    continue;
                };
                for (service, line) in &build.depends_on {
                    let Some(to) = images.get(service) else {
                        continue;
                    };
                    let to = to.id.clone();
                    let detail = format!("service {} depends_on {service}", build.service);
                    self.add(
                        &from.id,
                        &to,
                        Source::ComposeDependsOn,
                        file,
                        Some(*line),
                        detail,
                    );
                }
            }
        }
        contexts
    }

    fn dockerfiles(
        &mut self,
        files: &[String],
        contexts: &BTreeMap<String, Vec<(String, String)>>,
    ) {
        let mut all: BTreeSet<String> = files.iter().cloned().collect();
        all.extend(
            contexts
                .keys()
                .filter(|file| is_file(self.root, file))
                .cloned(),
        );
        for file in all {
            let Some(text) = read(self.root, &file) else {
                continue;
            };
            let dir = parent(&file).to_owned();
            let known: Vec<(String, String)> = contexts.get(&file).cloned().unwrap_or_default();
            for copy in dockerfile_copies(&text) {
                for source in &copy.sources {
                    let Some(source) = literal_prefix(source) else {
                        continue;
                    };
                    let choices: Vec<(String, String)> = if known.is_empty() {
                        let context = infer_context(self.root, &dir, &source);
                        let said = if context == dir {
                            String::new()
                        } else {
                            format!(" (build context {context}, where {source} is)")
                        };
                        vec![(context, said)]
                    } else {
                        known
                            .iter()
                            .map(|(context, from)| {
                                (
                                    context.clone(),
                                    format!(" (build context {context}, from {from})"),
                                )
                            })
                            .collect()
                    };
                    for (context, said) in choices {
                        let Some(importer) = self.image_project(&file, &context) else {
                            continue;
                        };
                        let Some(target) = resolve(&context, &source) else {
                            continue;
                        };
                        let detail = format!("{}{said}", copy.text);
                        self.link(
                            &importer,
                            &target,
                            Source::Dockerfile,
                            &file,
                            Some(copy.line),
                            &detail,
                        );
                    }
                }
            }
        }
    }

    /// `.env.example` files and OpenAPI specs that no project owns are inputs
    /// of every project in their folder or below it.
    fn shared_inputs(&mut self, files: &[String]) {
        for file in files {
            if !self.owners(file).is_empty() {
                continue;
            }
            let dir = parent(file).to_owned();
            let what = if file_name(file) == ".env.example" {
                "the example env file of a folder above this project"
            } else {
                "an OpenAPI spec in a folder above this project"
            };
            let users: Vec<ProjectNode> = self
                .projects
                .iter()
                .filter(|project| project.root != "." && inside(&project.root, &dir))
                .cloned()
                .collect();
            for project in users {
                let importer = self.importer_of(&project);
                self.link(&importer, file, Source::SharedInput, file, None, what);
            }
        }
    }

    // ------------------------------------------------------------ tier 3

    /// The node whose code a source file is: the deepest project holding it
    /// when that project is of `kind`, else the deepest shared folder holding
    /// it, else the repo root's project of `kind`.
    fn importer_for(&self, file: &str, kind: &str, root_fallback: bool) -> Option<Importer> {
        let owners = self.owners(file);
        if !owners.is_empty() {
            // A Python script in a Node project's folder is still that
            // project's code.
            return owners
                .iter()
                .find(|project| project.kind == kind)
                .or_else(|| owners.first())
                .map(|project| self.importer_of(project));
        }
        let folder = self
            .shared
            .iter()
            .filter(|(path, is_folder)| **is_folder && inside(file, path))
            .map(|(path, _)| path)
            .max_by_key(|path| path.len());
        if let Some(path) = folder {
            return Some(Importer {
                id: shared_id(path),
                root: path.clone(),
                is_root_project: false,
            });
        }
        if self.shared.contains_key(file) {
            return Some(Importer {
                id: shared_id(file),
                root: file.to_owned(),
                is_root_project: false,
            });
        }
        if !root_fallback {
            return None;
        }
        let roots = self.root_projects();
        roots
            .iter()
            .find(|project| project.kind == kind)
            .or_else(|| roots.first())
            .map(|project| self.importer_of(project))
    }

    fn python_file(
        &mut self,
        file: &str,
        imports: &PythonImports,
        importer: &Importer,
        dynamic: &mut BTreeMap<String, Vec<(String, usize, String)>>,
    ) {
        let dir = parent(file).to_owned();
        for import in &imports.imports {
            for target in python_targets(
                self.root,
                &dir,
                importer,
                &import.module,
                &import.names,
                &self.projects,
            ) {
                let detail = import.text.clone();
                self.link(
                    importer,
                    &target,
                    Source::PythonImport,
                    file,
                    Some(import.line),
                    &detail,
                );
            }
        }
        for call in &imports.dynamic {
            dynamic.entry(importer.id.clone()).or_default().push((
                file.to_owned(),
                call.line,
                call.call.clone(),
            ));
        }
    }

    fn js_file(&mut self, file: &str, imports: &JsImports, importer: &Importer) {
        let dir = parent(file).to_owned();
        for import in &imports.imports {
            let specifier = import.specifier.as_str();
            if !(specifier == "."
                || specifier == ".."
                || specifier.starts_with("./")
                || specifier.starts_with("../"))
            {
                continue;
            }
            let Some(target) = resolve(&dir, specifier) else {
                continue;
            };
            let target = resolve_js(self.root, &target);
            self.link(
                importer,
                &target,
                Source::JsImport,
                file,
                Some(import.line),
                &import.text,
            );
        }
    }

    /// Reads the imports of every source file, in rounds: a file's imports
    /// can make a shared folder whose own files are read in the next round.
    fn imports(&mut self, sources: &[SourceFile]) {
        let parsed = parse_all(self.root, sources);
        let mut done = vec![false; parsed.len()];
        let mut dynamic: BTreeMap<String, Vec<(String, usize, String)>> = BTreeMap::new();
        // Files that only the repo root's project would hold wait until no
        // more shared folders appear: such a file may turn out to be part of
        // a shared folder another file imports, and then its imports are the
        // folder's. The last round lets the root's project take the rest.
        let mut root_fallback = false;
        loop {
            let shared_before = self.shared.len();
            for (index, (file, result)) in parsed.iter().enumerate() {
                if done[index] {
                    continue;
                }
                let kind = match result {
                    Parsed::Python(_) => "python",
                    Parsed::Js(_) => "node",
                };
                let Some(importer) = self.importer_for(file, kind, root_fallback) else {
                    continue;
                };
                done[index] = true;
                match result {
                    Parsed::Python(imports) => {
                        self.python_file(file, imports, &importer, &mut dynamic)
                    }
                    Parsed::Js(imports) => self.js_file(file, imports, &importer),
                }
            }
            if self.shared.len() == shared_before {
                if root_fallback {
                    break;
                }
                root_fallback = true;
            }
        }
        self.dynamic_imports(&dynamic, sources);
    }

    /// A dynamic import may load any Python code: the importer depends on
    /// every other Python project and every shared folder with Python in it.
    fn dynamic_imports(
        &mut self,
        dynamic: &BTreeMap<String, Vec<(String, usize, String)>>,
        sources: &[SourceFile],
    ) {
        let python_shared: Vec<String> = self
            .shared
            .iter()
            .filter(|(path, is_folder)| {
                (!**is_folder && path.ends_with(".py"))
                    || **is_folder
                        && sources
                            .iter()
                            .any(|source| source.python && inside(&source.path, path))
            })
            .map(|(path, _)| shared_id(path))
            .collect();
        for (importer, calls) in dynamic {
            let Some((file, line, call)) = calls.first() else {
                continue;
            };
            let place = self.project(importer).map_or_else(
                || importer.trim_start_matches("shared:").to_owned(),
                |project| {
                    if project.root == "." {
                        "the repo root".to_owned()
                    } else {
                        project.root.clone()
                    }
                },
            );
            let more = if calls.len() > 1 {
                format!(" ({} more in the same project)", calls.len() - 1)
            } else {
                String::new()
            };
            self.warnings.insert(format!(
                "{file}:{line}: {call} loads a module chosen at run time{more}, so firescale cannot tell what {place} imports. It counts as depending on every other Python project."
            ));
            let mut targets: Vec<String> = self
                .projects
                .iter()
                .filter(|project| project.kind == "python")
                .map(|project| project.id.clone())
                .collect();
            targets.extend(python_shared.iter().cloned());
            let detail = format!("{call} loads a module chosen at run time");
            for to in targets {
                self.add(
                    importer,
                    &to,
                    Source::DynamicImport,
                    file,
                    Some(*line),
                    detail.clone(),
                );
            }
        }
    }

    fn finish(self) -> Graph {
        let mut nodes: Vec<Node> = self
            .projects
            .iter()
            .map(|project| Node {
                id: project.id.clone(),
                node_type: NodeType::Project,
                path: project.root.clone(),
                kind: project.kind.clone(),
                name: Some(project.name.clone()),
            })
            .collect();
        nodes.extend(self.shared.iter().map(|(path, is_folder)| Node {
            id: shared_id(path),
            node_type: NodeType::Shared,
            path: path.clone(),
            kind: if *is_folder { "folder" } else { "file" }.to_owned(),
            name: None,
        }));
        let order = |id: &str| -> (NodeType, String, String) {
            nodes
                .iter()
                .find(|node| node.id == id)
                .map_or((NodeType::Shared, id.to_owned(), String::new()), |node| {
                    (node.node_type, node.path.clone(), node.kind.clone())
                })
        };
        let mut edges: Vec<Edge> = self
            .edges
            .into_iter()
            .map(|((from, to), sources)| {
                let mut sources: Vec<EdgeSource> = sources.into_iter().collect();
                sources.sort_by(|a, b| {
                    (a.tier, a.source, &a.file, a.line, &a.detail)
                        .cmp(&(b.tier, b.source, &b.file, b.line, &b.detail))
                });
                Edge { from, to, sources }
            })
            .collect();
        edges.sort_by_key(|edge| (order(&edge.from), order(&edge.to)));
        Graph {
            nodes,
            edges,
            warnings: self.warnings.into_iter().collect(),
        }
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The part of a COPY source before its first wildcard or variable, or
/// `None` when nothing literal is left to resolve.
fn literal_prefix(source: &str) -> Option<String> {
    let mut kept = Vec::new();
    for part in source.split(['/', '\\']) {
        if part.contains(['*', '?', '[', '$']) {
            break;
        }
        kept.push(part);
    }
    let joined = kept.join("/");
    if joined.is_empty() && !source.is_empty() && source.starts_with(['*', '?', '[', '$']) {
        // `*.py`: the context itself.
        return Some(".".to_owned());
    }
    (!joined.is_empty()).then_some(joined)
}

/// The build context of a Dockerfile no Compose file names: its own folder,
/// unless `source` is not there and is found in a folder above it.
fn infer_context(root: &Path, dir: &str, source: &str) -> String {
    let mut candidate = dir.to_owned();
    loop {
        if resolve(&candidate, source).is_some_and(|path| exists(root, &path)) {
            return candidate;
        }
        if candidate == "." {
            return dir.to_owned();
        }
        candidate = parent(&candidate).to_owned();
    }
}

/// A relative JS import's file: the path itself, with a known extension, or
/// a folder's index file. The path as resolved when none exists.
fn resolve_js(root: &Path, target: &str) -> String {
    if is_file(root, target) {
        return target.to_owned();
    }
    // `./x.js` in TypeScript names `./x.ts`.
    for (written, real) in [
        (".js", ".ts"),
        (".js", ".tsx"),
        (".mjs", ".mts"),
        (".cjs", ".cts"),
    ] {
        if let Some(stem) = target.strip_suffix(written) {
            let candidate = format!("{stem}{real}");
            if is_file(root, &candidate) {
                return candidate;
            }
        }
    }
    for extension in [
        ".ts", ".tsx", ".d.ts", ".js", ".jsx", ".mjs", ".cjs", ".json",
    ] {
        let candidate = format!("{target}{extension}");
        if is_file(root, &candidate) {
            return candidate;
        }
    }
    target.to_owned()
}

/// Requirements files in a Python project's folder, as `fscale facts` reads them.
fn requirement_files(root: &Path, dir: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (folder, any_txt) in [(dir.to_owned(), false), (join(dir, "requirements"), true)] {
        let Ok(entries) = fs::read_dir(abs(root, &folder)) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let lower = name.to_ascii_lowercase();
            if entry.path().is_file()
                && lower.ends_with(".txt")
                && (any_txt || lower.contains("requirements"))
            {
                found.push(join(&folder, &name));
            }
        }
    }
    found.sort();
    found
}

/// The top-level packages a Python project provides to others: folders with
/// an `__init__.py` in its folder or in its `src/`.
fn python_packages(root: &Path, project: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for folder in [project.to_owned(), join(project, "src")] {
        let Ok(entries) = fs::read_dir(abs(root, &folder)) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            if entry.path().join("__init__.py").is_file() {
                if let Ok(name) = entry.file_name().into_string() {
                    found.insert(name);
                }
            }
        }
    }
    found
}

/// True when `dir/name` is a module or package: `name.py`, `name.pyi` or a
/// folder `name`.
fn python_module_at(root: &Path, dir: &str, name: &str) -> Option<String> {
    let folder = join(dir, name);
    if is_dir(root, &folder) {
        return Some(folder);
    }
    for extension in [".py", ".pyi"] {
        let file = join(dir, &format!("{name}{extension}"));
        if is_file(root, &file) {
            return Some(file);
        }
    }
    None
}

/// The deepest existing path of `names` below `start`.
fn deepest(root: &Path, start: String, names: &[&str]) -> String {
    let mut path = start;
    for name in names {
        if !is_dir(root, &path) {
            break;
        }
        match python_module_at(root, &path, name) {
            Some(next) => path = next,
            None => break,
        }
    }
    path
}

/// Where a Python import leads, as repo-relative paths outside the importer's
/// own code. An import the importer resolves itself, or that resolves to
/// nothing in the repo (the standard library, an installed package), gives
/// nothing.
fn python_targets(
    root: &Path,
    dir: &str,
    importer: &Importer,
    module: &str,
    names: &[String],
    projects: &[ProjectNode],
) -> Vec<String> {
    let dots = module.chars().take_while(|c| *c == '.').count();
    let rest: Vec<&str> = module[dots..]
        .split('.')
        .filter(|part| !part.is_empty())
        .collect();
    if dots > 0 {
        // Relative: from the file's package, up one folder per extra dot.
        let mut base = dir.to_owned();
        for _ in 1..dots {
            if base == "." {
                return Vec::new();
            }
            base = parent(&base).to_owned();
        }
        if rest.is_empty() {
            return names
                .iter()
                .filter(|name| name.as_str() != "*")
                .map(|name| python_module_at(root, &base, name).unwrap_or_else(|| base.clone()))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
        }
        return vec![deepest(root, base, &rest)];
    }
    let Some(first) = rest.first() else {
        return Vec::new();
    };
    // The importer's own code: the file's folder and the folders above it up
    // to the importer's folder, and its `src/`.
    let mut own: Vec<String> = Vec::new();
    let mut folder = dir.to_owned();
    loop {
        own.push(folder.clone());
        if folder == importer.root || folder == "." || !inside(&folder, &importer.root) {
            break;
        }
        folder = parent(&folder).to_owned();
    }
    own.push(join(&importer.root, "src"));
    if let Some(start) = own
        .iter()
        .find_map(|folder| python_module_at(root, folder, first))
    {
        // Found from the importer's own folders: its own code, unless the
        // path leads into another project (the repo root's project importing
        // `services.auth.client`). The caller tells the two apart.
        return vec![deepest(root, start, &rest[1..])];
    }
    // A folder above the importer's own: `backend/` holding `shared/` for
    // services that run with `backend/` on the import path.
    let mut above = importer.root.clone();
    while above != "." {
        above = parent(&above).to_owned();
        if let Some(start) = python_module_at(root, &above, first) {
            let found = deepest(root, start, &rest[1..]);
            let is_python = !is_dir(root, &found) || folder_has_python(root, &found);
            if !is_python {
                continue;
            }
            if inside(&found, &importer.root) {
                // The importer's own package, reached from a folder above it.
                return Vec::new();
            }
            return vec![found];
        }
    }
    // Another Python project's top-level package.
    projects
        .iter()
        .filter(|project| project.kind == "python" && project.id != importer.id)
        .filter(|project| python_packages(root, &project.root).contains(*first))
        .map(|project| project.root.clone())
        .collect()
}

/// True when a folder has a `.py` file in it or in a subfolder, three levels
/// down: `shared/` holding only the package `shared/db/` is Python too.
fn folder_has_python(root: &Path, dir: &str) -> bool {
    fn search(path: &Path, depth: usize) -> bool {
        let Ok(entries) = fs::read_dir(path) else {
            return false;
        };
        entries.filter_map(Result::ok).any(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.ends_with(".py") {
                return true;
            }
            depth > 0
                && !name.starts_with('.')
                && entry.file_type().is_ok_and(|kind| kind.is_dir())
                && search(&entry.path(), depth - 1)
        })
    }
    search(&abs(root, dir), 3)
}

// ------------------------------------------------------------- the walk

#[derive(Debug, Clone)]
struct SourceFile {
    path: String,
    python: bool,
}

#[derive(Default)]
struct Files {
    sources: Vec<SourceFile>,
    dockerfiles: Vec<String>,
    compose: Vec<String>,
    inputs: Vec<String>,
}

fn is_dockerfile(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == "dockerfile"
        || lower == "containerfile"
        || lower.starts_with("dockerfile.")
        || lower.ends_with(".dockerfile")
}

/// Compose file names: `compose.yaml`, `docker-compose.yml`, their override
/// files, and variants such as `docker-compose.prod.yml`.
pub(crate) fn is_compose_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    (lower.starts_with("docker-compose")
        || lower == "compose.yaml"
        || lower == "compose.yml"
        || lower.starts_with("compose."))
        && (lower.ends_with(".yml") || lower.ends_with(".yaml"))
}

fn is_openapi_spec(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let stem = lower
        .strip_suffix(".yaml")
        .or_else(|| lower.strip_suffix(".yml"))
        .or_else(|| lower.strip_suffix(".json"));
    stem.is_some_and(|stem| {
        stem == "openapi"
            || stem == "swagger"
            || stem.ends_with(".openapi")
            || stem.starts_with("openapi.")
            || stem.starts_with("openapi-")
    })
}

/// Every file the graph reads, in one walk of the repo. Skips what
/// `.gitignore` ignores, folders that start with a dot, and the folders
/// `fscale facts` never searches.
fn walk(root: &Path) -> Files {
    let mut files = Files::default();
    let walk = WalkBuilder::new(root)
        .max_depth(Some(MAX_SOURCE_DEPTH))
        .hidden(false)
        .follow_links(false)
        .parents(false)
        .ignore(false)
        .git_global(false)
        .git_exclude(false)
        .git_ignore(true)
        .require_git(false)
        .sort_by_file_name(|a, b| a.cmp(b))
        .filter_entry(|entry| {
            let Some(name) = entry.file_name().to_str() else {
                return false;
            };
            if entry.depth() == 0 {
                return true;
            }
            let is_dir = entry.file_type().is_some_and(|kind| kind.is_dir());
            !(is_dir && (name.starts_with('.') || DISCOVERY_SKIPPED_DIRS.contains(&name)))
        })
        .build();
    for entry in walk.filter_map(Result::ok) {
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(root) else {
            continue;
        };
        let rel: Vec<String> = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect();
        let path = rel.join("/");
        let name = rel.last().cloned().unwrap_or_default();
        let lower = name.to_ascii_lowercase();
        if is_dockerfile(&name) {
            files.dockerfiles.push(path);
        } else if is_compose_file(&name) {
            files.compose.push(path);
        } else if name == ".env.example" || is_openapi_spec(&name) {
            files.inputs.push(path);
        } else if lower.ends_with(".py")
            || (js_dialect(&name).is_some() && !lower.ends_with(".min.js"))
        {
            let small = entry
                .metadata()
                .is_ok_and(|meta| meta.len() <= MAX_SOURCE_BYTES);
            if small {
                files.sources.push(SourceFile {
                    python: lower.ends_with(".py"),
                    path,
                });
            }
        }
    }
    files
}

enum Parsed {
    Python(PythonImports),
    Js(JsImports),
}

fn parse_one(root: &Path, source: &SourceFile) -> Option<Parsed> {
    let text = read(root, &source.path)?;
    if source.python {
        Some(Parsed::Python(python_imports(&text)))
    } else {
        let dialect = js_dialect(file_name(&source.path))?;
        Some(Parsed::Js(js_imports(&text, dialect)))
    }
}

/// Parses every source file, on as many threads as the machine has.
fn parse_all(root: &Path, sources: &[SourceFile]) -> Vec<(String, Parsed)> {
    let threads = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1)
        .clamp(1, 16);
    let chunk = sources.len().div_ceil(threads).max(1);
    let mut out: Vec<(String, Parsed)> = Vec::with_capacity(sources.len());
    std::thread::scope(|scope| {
        let handles: Vec<_> = sources
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    part.iter()
                        .filter_map(|source| {
                            parse_one(root, source).map(|parsed| (source.path.clone(), parsed))
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for handle in handles {
            if let Ok(part) = handle.join() {
                out.extend(part);
            }
        }
    });
    out
}

/// Builds the graph of the repo at `root` from its facts and its files.
#[must_use]
pub fn build_graph(root: &Path, facts: &RepoFacts) -> Graph {
    let files = walk(root);
    let mut builder = Builder::new(root, facts);
    builder.workspace_names(facts);
    builder.manifests();
    let contexts = builder.compose(&files.compose);
    builder.dockerfiles(&files.dockerfiles, &contexts);
    builder.shared_inputs(&files.inputs);
    builder.imports(&files.sources);
    builder.finish()
}

// ------------------------------------------------------------- affected

/// True for a file that cannot change what a build or a test does, so a
/// change to it affects no project when no project or shared node holds it
/// (ADR 0034): Markdown (`.md`, `.markdown`), text files (`.txt`, except
/// requirement and constraint lists and `CMakeLists.txt`), a licence
/// (`LICENSE`, `LICENCE`, with any extension) and anything under the repo
/// root's `docs/` folder.
#[must_use]
pub fn cannot_affect_builds(file: &str) -> bool {
    if file.starts_with("docs/") {
        return true;
    }
    let name = file_name(file).to_ascii_lowercase();
    if name.starts_with("license") || name.starts_with("licence") {
        return true;
    }
    if name.ends_with(".md") || name.ends_with(".markdown") {
        return true;
    }
    name.ends_with(".txt")
        && !name.contains("requirements")
        && !name.contains("constraints")
        && name != "cmakelists.txt"
}

/// Why a changed file affects every project, if it does: a root lockfile, a
/// CI workflow, a Compose file or the firescale config.
#[must_use]
pub fn widening_reason(file: &str) -> Option<String> {
    let name = file_name(file);
    let at_root = !file.contains('/');
    if at_root && LOCKFILE_NAMES.contains(&name) {
        return Some(format!("the root lockfile {file} changed"));
    }
    let ci = file.starts_with(".github/workflows/")
        || file.starts_with(".github/actions/")
        || file.starts_with(".circleci/")
        || (at_root
            && matches!(
                name,
                ".gitlab-ci.yml"
                    | "azure-pipelines.yml"
                    | "bitbucket-pipelines.yml"
                    | "Jenkinsfile"
            ));
    if ci {
        return Some(format!("the CI file {file} changed"));
    }
    if is_compose_file(name) {
        return Some(format!("the Compose file {file} changed"));
    }
    if at_root && name.starts_with("firescale.config.") {
        return Some(format!("the firescale config {file} changed"));
    }
    None
}

/// What a change affects.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Affected {
    /// Set when every project is affected, with the reason.
    pub everything: Option<String>,
    /// The affected nodes, projects and shared ones, each with why.
    pub nodes: BTreeMap<String, String>,
    /// How many files changed.
    pub changed_files: usize,
    /// Changed files that no project owns and that cannot affect a build or
    /// a test ([`cannot_affect_builds`]): they affect no project.
    pub inert_files: Vec<String>,
}

impl Affected {
    /// True when the project node `id` is affected.
    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        self.everything.is_some() || self.nodes.contains_key(id)
    }

    /// Why `id` is affected, for people.
    #[must_use]
    pub fn reason(&self, id: &str) -> Option<String> {
        match &self.everything {
            Some(reason) => Some(reason.clone()),
            None => self.nodes.get(id).cloned(),
        }
    }

    /// True when nothing at all is affected.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.everything.is_none() && self.nodes.is_empty()
    }
}

fn changed_reason(files: &[&String]) -> String {
    match files {
        [] => "changed".to_owned(),
        [one] => format!("changed: {one}"),
        [first, rest @ ..] => format!("changed: {first} and {} more", rest.len()),
    }
}

/// The nodes a set of changed files affects: the projects and shared nodes
/// that hold a changed file, and every node that depends on one of those,
/// directly or not. A changed file that no node holds, or one that
/// [`widening_reason`] names, affects everything.
#[must_use]
pub fn affected(graph: &Graph, changed: &Changed) -> Affected {
    let files = match changed {
        Changed::Everything => {
            return Affected {
                everything: Some("there is no commit to compare with".to_owned()),
                ..Affected::default()
            }
        }
        Changed::Shallow(note) => {
            return Affected {
                everything: Some(note.clone()),
                ..Affected::default()
            }
        }
        Changed::Files(files) => files,
    };
    let mut result = Affected {
        changed_files: files.len(),
        ..Affected::default()
    };
    let projects: Vec<&Node> = graph
        .nodes
        .iter()
        .filter(|node| node.node_type == NodeType::Project)
        .collect();
    let mut direct: BTreeMap<String, Vec<&String>> = BTreeMap::new();
    for file in files {
        if let Some(reason) = widening_reason(file) {
            result.everything = Some(reason);
            return result;
        }
        let deepest = projects
            .iter()
            .filter(|node| node.path != "." && inside(file, &node.path))
            .map(|node| node.path.len())
            .max();
        let mut holders: Vec<String> = match deepest {
            Some(len) => projects
                .iter()
                .filter(|node| {
                    node.path != "." && node.path.len() == len && inside(file, &node.path)
                })
                .map(|node| node.id.clone())
                .collect(),
            None => Vec::new(),
        };
        // A shared node that holds a project's folder (a Docker build
        // context of `.`, say) is "wide": it says that one project uses
        // everything there, not that nobody else does. Only a narrow shared
        // node tells who reads a file that no project owns.
        let mut narrow = false;
        for node in graph
            .nodes
            .iter()
            .filter(|node| node.node_type == NodeType::Shared && inside(file, &node.path))
        {
            narrow |= !projects
                .iter()
                .any(|project| project.path != "." && inside(&project.path, &node.path));
            holders.push(node.id.clone());
        }
        let roots: Vec<String> = projects
            .iter()
            .filter(|node| node.path == ".")
            .map(|node| node.id.clone())
            .collect();
        if deepest.is_none() && !narrow && cannot_affect_builds(file) {
            // README.md, LICENSE, docs/: nothing to run for it.
            result.inert_files.push(file.clone());
            continue;
        }
        if deepest.is_none() && !narrow {
            if roots.is_empty() {
                result.everything = Some(format!("{file} belongs to no project"));
                return result;
            }
            if !file.contains('/') {
                // Settings that projects below may read (ruff.toml,
                // tsconfig.base.json, .prettierrc) live at the repo root.
                result.everything = Some(format!(
                    "{file} is at the repo root, where settings every project may read are kept"
                ));
                return result;
            }
        }
        if deepest.is_none() {
            // The repo root's project holds every file no other project
            // does, shared ones included.
            holders.extend(roots);
        }
        for holder in holders {
            direct.entry(holder).or_default().push(file);
        }
    }
    let mut queue: VecDeque<String> = VecDeque::new();
    for (id, changed_here) in direct {
        result
            .nodes
            .insert(id.clone(), changed_reason(&changed_here));
        queue.push_back(id);
    }
    while let Some(id) = queue.pop_front() {
        for edge in graph.edges.iter().filter(|edge| edge.to == id) {
            if result.nodes.contains_key(&edge.from) {
                continue;
            }
            let how: Vec<&str> = edge
                .sources
                .iter()
                .map(|source| source.source.label())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            result.nodes.insert(
                edge.from.clone(),
                format!("depends on {} ({})", graph.display(&id), how.join(", ")),
            );
            queue.push_back(edge.from.clone());
        }
    }
    result
}

// ------------------------------------------------------------ rendering

/// The graph as `fscale graph --json` prints it.
#[must_use]
pub fn format_graph_json(graph: &Graph) -> String {
    let report = GraphReport {
        schema_version: GRAPH_SCHEMA_VERSION,
        nodes: &graph.nodes,
        edges: &graph.edges,
        warnings: &graph.warnings,
    };
    let mut text = serde_json::to_string_pretty(&report).unwrap_or_else(|_| "{}".to_owned());
    text.push('\n');
    text
}

fn source_line(source: &EdgeSource) -> String {
    let place = match source.line {
        Some(line) => format!("{}:{line}", source.file),
        None => source.file.clone(),
    };
    format!("{}: {place}: {}", source.source.label(), source.detail)
}

/// The edges, one per line, with their sources' labels or, with `explain`,
/// every piece of evidence.
#[must_use]
pub fn edge_lines(graph: &Graph, explain: bool, indent: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for edge in &graph.edges {
        let labels: Vec<&str> = edge
            .sources
            .iter()
            .map(|source| source.source.label())
            .fold(Vec::new(), |mut seen, label| {
                if !seen.contains(&label) {
                    seen.push(label);
                }
                seen
            });
        lines.push(format!(
            "{indent}{} -> {} ({})",
            graph.display(&edge.from),
            graph.display(&edge.to),
            labels.join(", ")
        ));
        if explain {
            for source in &edge.sources {
                lines.push(format!("{indent}    {}", source_line(source)));
            }
        }
    }
    lines
}

/// The graph for people: projects, shared nodes, edges and warnings.
#[must_use]
pub fn format_graph_human(graph: &Graph, explain: bool) -> String {
    let mut out = String::new();
    let projects: Vec<&Node> = graph
        .nodes
        .iter()
        .filter(|node| node.node_type == NodeType::Project)
        .collect();
    out.push_str(&format!("Projects ({}):\n", projects.len()));
    if projects.is_empty() {
        out.push_str("  none\n");
    }
    for node in projects {
        let name = node.name.as_deref().unwrap_or_default();
        let folder = file_name(&node.path);
        let named = if name.is_empty() || name == folder || name == "." {
            String::new()
        } else {
            format!(" {name}")
        };
        let place = if node.path == "." {
            "(repo root)"
        } else {
            node.path.as_str()
        };
        out.push_str(&format!("  {place} [{}]{named}\n", node.kind));
    }
    let shared: Vec<&Node> = graph
        .nodes
        .iter()
        .filter(|node| node.node_type == NodeType::Shared)
        .collect();
    if !shared.is_empty() {
        out.push_str(&format!(
            "Shared, used by projects but part of none ({}):\n",
            shared.len()
        ));
        for node in shared {
            out.push_str(&format!("  {} ({})\n", node.path, node.kind));
        }
    }
    out.push_str(&format!(
        "Edges ({}), \"a -> b\" means a change in b affects a:\n",
        graph.edges.len()
    ));
    if graph.edges.is_empty() {
        out.push_str("  none\n");
    }
    for line in edge_lines(graph, explain, "  ") {
        out.push_str(&line);
        out.push('\n');
    }
    if !explain && !graph.edges.is_empty() {
        out.push_str("Run fscale graph --explain to see where each edge comes from.\n");
    }
    if !graph.warnings.is_empty() {
        out.push_str("Warnings:\n");
        for warning in &graph.warnings {
            out.push_str(&format!("  {warning}\n"));
        }
    }
    out
}

/// The project of `facts` that a graph node stands for.
#[must_use]
pub fn project_of<'a>(facts: &'a RepoFacts, node: &Node) -> Option<&'a Project> {
    facts
        .projects
        .iter()
        .find(|project| project.root == node.path && project.kind == node.kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::detect_facts_with;
    use crate::testing::{course_project, Fixture, Track, COURSE_SERVICES, COURSE_SHARED_USERS};

    fn graph_of(fixture: &Fixture) -> Graph {
        let facts = detect_facts_with(fixture.root(), None).unwrap();
        build_graph(fixture.root(), &facts)
    }

    fn fixture(files: &[(&str, &str)]) -> Fixture {
        Fixture::new(files, Track::NoGit).unwrap()
    }

    /// The edge from the node at path `from` to the node at path `to`.
    fn edge<'a>(graph: &'a Graph, from: &str, to: &str) -> Option<&'a Edge> {
        let path = |id: &str| graph.node(id).map(|node| node.path.clone());
        graph.edges.iter().find(|edge| {
            path(&edge.from).as_deref() == Some(from) && path(&edge.to).as_deref() == Some(to)
        })
    }

    fn sources(edge: Option<&Edge>) -> Vec<Source> {
        edge.map(|edge| edge.sources.iter().map(|source| source.source).collect())
            .unwrap_or_default()
    }

    fn changed(files: &[&str]) -> Changed {
        Changed::Files(files.iter().map(|file| (*file).to_owned()).collect())
    }

    /// The paths of the affected projects.
    fn affected_projects(graph: &Graph, files: &[&str]) -> Vec<String> {
        let set = affected(graph, &changed(files));
        graph
            .nodes
            .iter()
            .filter(|node| node.node_type == NodeType::Project && set.contains(&node.id))
            .map(|node| node.path.clone())
            .collect()
    }

    fn services(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| format!("backend/{name}")).collect()
    }

    // ------------------------------------------------- the course project

    #[test]
    fn course_services_that_import_the_shared_folder_depend_on_it() {
        let fixture = course_project().unwrap();
        let graph = graph_of(&fixture);
        let shared = graph.node("shared:backend/shared").expect("a shared node");
        assert_eq!(shared.kind, "folder");
        for service in COURSE_SERVICES {
            let found = edge(&graph, &format!("backend/{service}"), "backend/shared");
            assert_eq!(
                found.is_some(),
                COURSE_SHARED_USERS.contains(&service),
                "{service}"
            );
            if let Some(found) = found {
                assert_eq!(sources(Some(found)), [Source::PythonImport]);
                assert_eq!(found.sources[0].file, format!("backend/{service}/main.py"));
                assert_eq!(found.sources[0].line, Some(1));
            }
        }
        assert!(graph.warnings.is_empty(), "{:?}", graph.warnings);
    }

    #[test]
    fn a_change_in_the_shared_folder_affects_every_service_that_imports_it() {
        let fixture = course_project().unwrap();
        let graph = graph_of(&fixture);
        assert_eq!(
            affected_projects(&graph, &["backend/shared/db.py"]),
            services(&COURSE_SHARED_USERS)
        );
    }

    #[test]
    fn a_change_in_one_service_affects_only_that_service() {
        let fixture = course_project().unwrap();
        let graph = graph_of(&fixture);
        for service in COURSE_SERVICES {
            let file = format!("backend/{service}/main.py");
            assert_eq!(affected_projects(&graph, &[&file]), services(&[service]));
        }
    }

    #[test]
    fn a_frontend_change_affects_only_the_frontend() {
        let fixture = course_project().unwrap();
        let graph = graph_of(&fixture);
        assert_eq!(
            affected_projects(&graph, &["frontend/src/main.ts"]),
            ["frontend"]
        );
    }

    #[test]
    fn a_change_to_the_root_compose_file_affects_everything() {
        let fixture = course_project().unwrap();
        let graph = graph_of(&fixture);
        let set = affected(&graph, &changed(&["docker-compose.yml"]));
        assert!(set
            .everything
            .as_deref()
            .unwrap()
            .contains("docker-compose.yml"));
        assert_eq!(affected_projects(&graph, &["docker-compose.yml"]).len(), 6);
    }

    #[test]
    fn widening_files_and_files_no_project_owns_affect_everything() {
        let fixture = course_project().unwrap();
        let graph = graph_of(&fixture);
        for file in [
            "package-lock.json",
            ".github/workflows/ci.yml",
            "firescale.config.json",
            "backend/docker-compose.override.yml",
            "deploy.sh",
            "backend/.coveragerc",
        ] {
            let set = affected(&graph, &changed(&[file]));
            assert!(set.everything.is_some(), "{file}");
        }
        // A lockfile inside a project is that project's.
        assert_eq!(
            affected_projects(&graph, &["frontend/package-lock.json"]),
            ["frontend"]
        );
    }

    #[test]
    fn a_root_readme_change_affects_no_project_but_an_unowned_root_script_affects_all() {
        let fixture = course_project().unwrap();
        let graph = graph_of(&fixture);
        for file in [
            "README.md",
            "CLAUDE.md",
            "LICENSE",
            "LICENSE-MIT.txt",
            "notes.txt",
            "docs/setup.png",
            "docs/api/guide.rst",
            ".github/pull_request_template.md",
            "backend/NOTES.markdown",
        ] {
            let set = affected(&graph, &changed(&[file]));
            assert!(set.is_empty(), "{file}: {set:?}");
            assert_eq!(set.inert_files, [file]);
        }
        for file in [
            "deploy.sh",
            "scripts/seed.py",
            "requirements.txt",
            "backend/constraints.txt",
            "CMakeLists.txt",
            "Makefile",
        ] {
            assert!(
                affected(&graph, &changed(&[file])).everything.is_some(),
                "{file}"
            );
        }
        // Together with a real change, the README adds nothing.
        assert_eq!(
            affected_projects(&graph, &["README.md", "frontend/src/main.ts"]),
            ["frontend"]
        );
        // A doc file a project owns is that project's.
        assert_eq!(
            affected_projects(&graph, &["frontend/README.md"]),
            ["frontend"]
        );
    }

    #[test]
    fn the_root_env_example_is_an_input_of_every_project_below_it() {
        let fixture = course_project().unwrap();
        let graph = graph_of(&fixture);
        for project in ["frontend", "backend/auth_service"] {
            assert_eq!(
                sources(edge(&graph, project, ".env.example")),
                [Source::SharedInput]
            );
        }
        assert_eq!(affected_projects(&graph, &[".env.example"]).len(), 6);
        assert!(affected(&graph, &changed(&[".env.example"]))
            .everything
            .is_none());
    }

    #[test]
    fn nothing_changed_affects_nothing_and_no_commit_affects_everything() {
        let fixture = course_project().unwrap();
        let graph = graph_of(&fixture);
        assert!(affected(&graph, &changed(&[])).is_empty());
        let set = affected(&graph, &Changed::Everything);
        assert!(set.everything.is_some());
    }

    // ------------------------------------------------ tier 1: manifests

    fn py_service(dir: &str, extra: &[(&str, &str)]) -> Vec<(String, String)> {
        let mut files = vec![
            (format!("{dir}/requirements.txt"), "fastapi\n".to_owned()),
            (format!("{dir}/main.py"), "app = None\n".to_owned()),
        ];
        files.extend(
            extra
                .iter()
                .map(|(file, text)| (format!("{dir}/{file}"), (*text).to_owned())),
        );
        files
    }

    fn owned(files: &[(String, String)]) -> Fixture {
        let borrowed: Vec<(&str, &str)> = files
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        fixture(&borrowed)
    }

    #[test]
    fn an_editable_install_of_a_sibling_in_requirements_is_an_edge() {
        let mut files = py_service(
            "api",
            &[(
                "requirements-dev.txt",
                "-e ../lib\n../tools[dev]\n-r ../common/requirements.txt\n",
            )],
        );
        files.extend(py_service("lib", &[]));
        files.extend(py_service("tools", &[]));
        files.push(("common/requirements.txt".to_owned(), "pytest\n".to_owned()));
        let fixture = owned(&files);
        let graph = graph_of(&fixture);
        let to_lib = edge(&graph, "api", "lib").expect("api -> lib");
        assert_eq!(to_lib.sources[0].source, Source::Requirements);
        assert_eq!(to_lib.sources[0].file, "api/requirements-dev.txt");
        assert_eq!(to_lib.sources[0].line, Some(1));
        assert_eq!(
            sources(edge(&graph, "api", "tools")),
            [Source::Requirements]
        );
        // A requirements file outside every project is a shared file.
        assert_eq!(
            sources(edge(&graph, "api", "common/requirements.txt")),
            [Source::Requirements]
        );
        assert_eq!(affected_projects(&graph, &["lib/main.py"]), ["api", "lib"]);
    }

    #[test]
    fn a_pyproject_path_source_is_an_edge() {
        let fixture = fixture(&[
            (
                "app/pyproject.toml",
                "[project]\nname = \"app\"\ndependencies = [\"core\"]\n\n[tool.uv.sources]\ncore = { path = \"../core\", editable = true }\n",
            ),
            ("app/app/__init__.py", ""),
            ("core/pyproject.toml", "[project]\nname = \"core\"\n"),
            ("core/core/__init__.py", ""),
        ]);
        let graph = graph_of(&fixture);
        assert_eq!(sources(edge(&graph, "app", "core")), [Source::Pyproject]);
    }

    #[test]
    fn package_json_file_link_and_workspace_specs_are_edges() {
        let fixture = fixture(&[
            (
                "web/package.json",
                r#"{"name":"web","dependencies":{"ui":"file:../ui","kit":"link:../kit"}}"#,
            ),
            ("ui/package.json", r#"{"name":"ui"}"#),
            ("kit/package.json", r#"{"name":"kit"}"#),
            (
                "package.json",
                r#"{"name":"root","workspaces":["packages/*"]}"#,
            ),
            (
                "packages/a/package.json",
                r#"{"name":"a","dependencies":{"b":"workspace:*"}}"#,
            ),
            ("packages/b/package.json", r#"{"name":"b"}"#),
        ]);
        let graph = graph_of(&fixture);
        assert_eq!(
            sources(edge(&graph, "packages/a", "packages/b")),
            [Source::Dependency, Source::PackageJson]
        );
        // Without a workspace file the three are discovered projects; the
        // names in `dependencies` match too, so both sources are listed.
        let fixture = self::fixture(&[
            (
                "web/package.json",
                r#"{"name":"web","dependencies":{"ui":"file:../ui","kit":"link:../kit"}}"#,
            ),
            ("ui/package.json", r#"{"name":"ui"}"#),
            ("kit/package.json", r#"{"name":"kit"}"#),
        ]);
        let graph = graph_of(&fixture);
        assert_eq!(
            sources(edge(&graph, "web", "ui")),
            [Source::Dependency, Source::PackageJson]
        );
        assert_eq!(
            sources(edge(&graph, "web", "kit")),
            [Source::Dependency, Source::PackageJson]
        );
    }

    #[test]
    fn a_cargo_path_dependency_is_an_edge() {
        let fixture = fixture(&[
            (
                "cli/Cargo.toml",
                "[package]\nname = \"cli\"\n\n[dependencies]\nengine = { path = \"../engine\" }\n",
            ),
            ("engine/Cargo.toml", "[package]\nname = \"engine\"\n"),
        ]);
        let graph = graph_of(&fixture);
        assert_eq!(
            sources(edge(&graph, "cli", "engine")),
            [Source::Dependency, Source::Cargo]
        );
    }

    // ------------------------------------------- tier 2: Docker, Compose

    #[test]
    fn a_dockerfile_copy_of_a_sibling_folder_is_an_edge_to_a_shared_folder() {
        let mut files = py_service(
            "services/api",
            &[(
                "Dockerfile",
                "FROM python:3.13\nCOPY common /app/common\nCOPY requirements.txt .\n",
            )],
        );
        files.push(("services/common/util.txt".to_owned(), "x\n".to_owned()));
        let fixture = owned(&files);
        let graph = graph_of(&fixture);
        let found = edge(&graph, "services/api", "services/common").expect("an edge");
        assert_eq!(found.sources[0].source, Source::Dockerfile);
        assert_eq!(found.sources[0].line, Some(2));
        assert!(
            found.sources[0].detail.contains("build context services"),
            "{}",
            found.sources[0].detail
        );
        assert_eq!(graph.node("shared:services/common").unwrap().kind, "folder");
        assert_eq!(
            affected_projects(&graph, &["services/common/util.txt"]),
            ["services/api"]
        );
    }

    #[test]
    fn compose_build_context_and_depends_on_are_edges() {
        let mut files = py_service("api", &[]);
        files.extend(py_service(
            "worker",
            &[("Dockerfile", "FROM python:3.13\nCOPY shared ./shared\n")],
        ));
        files.push(("shared/x.py".to_owned(), "X = 1\n".to_owned()));
        files.push((
            "libs/proto/a.proto".to_owned(),
            "syntax = \"proto3\";\n".to_owned(),
        ));
        files.push((
            "compose.yaml".to_owned(),
            "services:\n  api:\n    build:\n      context: ./libs/proto\n      dockerfile: ../../api/Dockerfile\n    depends_on:\n      - worker\n      - db\n  worker:\n    build:\n      context: .\n      dockerfile: worker/Dockerfile\n  db:\n    image: postgres:16\n".to_owned(),
        ));
        files.push((
            "api/Dockerfile".to_owned(),
            "FROM python:3.13\nCOPY . .\n".to_owned(),
        ));
        let fixture = owned(&files);
        let graph = graph_of(&fixture);
        assert_eq!(
            sources(edge(&graph, "api", "libs/proto")),
            [Source::Dockerfile, Source::ComposeBuild]
        );
        assert_eq!(
            sources(edge(&graph, "api", "worker")),
            [Source::ComposeDependsOn]
        );
        // The context is the repo root: COPY shared reaches the root's shared/.
        let to_shared = edge(&graph, "worker", "shared").expect("worker -> shared");
        assert_eq!(to_shared.sources[0].source, Source::Dockerfile);
        assert!(to_shared.sources[0].detail.contains("from compose.yaml"));
        // The Compose file is a shared input of each project it builds.
        assert_eq!(
            sources(edge(&graph, "worker", "compose.yaml")),
            [Source::SharedInput]
        );
    }

    #[test]
    fn an_openapi_spec_above_projects_is_their_input() {
        let mut files = py_service("backend/api", &[]);
        files.extend(py_service("backend/jobs", &[]));
        files.push((
            "backend/openapi.yaml".to_owned(),
            "openapi: 3.1.0\n".to_owned(),
        ));
        files.push((
            "web/package.json".to_owned(),
            r#"{"name":"web"}"#.to_owned(),
        ));
        let fixture = owned(&files);
        let graph = graph_of(&fixture);
        assert_eq!(
            sources(edge(&graph, "backend/api", "backend/openapi.yaml")),
            [Source::SharedInput]
        );
        assert_eq!(
            affected_projects(&graph, &["backend/openapi.yaml"]),
            ["backend/api", "backend/jobs"]
        );
    }

    // ---------------------------------------------- tier 3: imports

    #[test]
    fn python_imports_reach_sibling_packages_and_shared_folders_but_not_their_own_or_installed_ones(
    ) {
        let fixture = fixture(&[
            ("svc/requirements.txt", "fastapi\n"),
            (
                "svc/main.py",
                "import os\nimport fastapi\nfrom app import routes\nfrom shared.db import URL\nimport corelib.models\ntext = \"import other\"\n# import other\n",
            ),
            ("svc/app/__init__.py", ""),
            ("svc/app/routes.py", ""),
            ("shared/db.py", "URL = 1\n"),
            ("lib/pyproject.toml", "[project]\nname = \"corelib\"\n"),
            ("lib/corelib/__init__.py", ""),
            ("lib/corelib/models.py", ""),
            ("other/requirements.txt", "x\n"),
            ("other/main.py", ""),
        ]);
        let graph = graph_of(&fixture);
        // `shared/` is not on svc's own path; it is reached from the folder
        // above svc (the repo root).
        assert_eq!(
            sources(edge(&graph, "svc", "shared")),
            [Source::PythonImport]
        );
        assert_eq!(sources(edge(&graph, "svc", "lib")), [Source::PythonImport]);
        assert!(edge(&graph, "svc", "other").is_none());
        let targets: Vec<&str> = graph
            .edges
            .iter()
            .filter(|e| e.from == "python:svc")
            .map(|e| e.to.as_str())
            .collect();
        assert_eq!(targets, ["python:lib", "shared:shared"]);
    }

    #[test]
    fn a_relative_python_import_out_of_the_project_is_an_edge() {
        let fixture = fixture(&[
            ("pkg/svc/requirements.txt", "x\n"),
            ("pkg/svc/__init__.py", "from ..common import helpers\n"),
            ("pkg/common/helpers.py", ""),
        ]);
        let graph = graph_of(&fixture);
        assert_eq!(
            sources(edge(&graph, "pkg/svc", "pkg/common")),
            [Source::PythonImport]
        );
    }

    #[test]
    fn a_shared_folder_s_own_imports_are_followed() {
        let mut files = py_service("backend/api", &[("main.py", "from shared import db\n")]);
        files.push(("backend/shared/__init__.py".to_owned(), String::new()));
        files.push((
            "backend/shared/db.py".to_owned(),
            "from common import config\n".to_owned(),
        ));
        files.push(("backend/common/config.py".to_owned(), "X = 1\n".to_owned()));
        let fixture = owned(&files);
        let graph = graph_of(&fixture);
        assert!(edge(&graph, "backend/shared", "backend/common").is_some());
        assert_eq!(
            affected_projects(&graph, &["backend/common/config.py"]),
            ["backend/api"]
        );
    }

    #[test]
    fn js_relative_imports_that_leave_the_project_are_edges() {
        let fixture = fixture(&[
            ("web/package.json", r#"{"name":"web"}"#),
            (
                "web/src/a.ts",
                "import { b } from './b';\nimport x from '../../shared/x.js';\nimport React from 'react';\nconst s = \"import '../../other'\";\nexport * from '../../admin/src/lib';\n",
            ),
            ("web/src/b.ts", "export const b = 1;\n"),
            ("shared/x.ts", "export default 1;\n"),
            ("admin/package.json", r#"{"name":"admin"}"#),
            ("admin/src/lib.ts", "export {};\n"),
            ("other/x.ts", ""),
        ]);
        let graph = graph_of(&fixture);
        let to_shared = edge(&graph, "web", "shared").expect("web -> shared");
        assert_eq!(to_shared.sources[0].source, Source::JsImport);
        assert_eq!(to_shared.sources[0].line, Some(2));
        assert_eq!(sources(edge(&graph, "web", "admin")), [Source::JsImport]);
        assert!(graph.node("shared:other").is_none());
        assert_eq!(affected_projects(&graph, &["shared/x.ts"]), ["web"]);
    }

    #[test]
    fn a_dynamic_import_gives_a_warning_and_an_edge_to_every_other_python_project() {
        let mut files = py_service(
            "plugins",
            &[(
                "main.py",
                "import importlib\n\nmod = importlib.import_module(name)\n",
            )],
        );
        files.extend(py_service("a", &[]));
        files.extend(py_service("b", &[]));
        files.push((
            "web/package.json".to_owned(),
            r#"{"name":"web"}"#.to_owned(),
        ));
        let fixture = owned(&files);
        let graph = graph_of(&fixture);
        assert_eq!(graph.warnings.len(), 1, "{:?}", graph.warnings);
        assert!(graph.warnings[0].starts_with("plugins/main.py:3: importlib.import_module"));
        assert!(graph.warnings[0].contains("every other Python project"));
        for other in ["a", "b"] {
            assert_eq!(
                sources(edge(&graph, "plugins", other)),
                [Source::DynamicImport]
            );
        }
        assert!(edge(&graph, "plugins", "web").is_none());
        assert_eq!(affected_projects(&graph, &["b/main.py"]), ["b", "plugins"]);
    }

    #[test]
    fn a_root_file_that_a_project_copies_affects_that_project_and_the_root_project() {
        let mut files = py_service(
            "backend",
            &[("Dockerfile", "FROM python:3.13\nCOPY package.json /app/\n")],
        );
        files.push((
            "package.json".to_owned(),
            r#"{"name":"root","scripts":{"test":"node t.js"}}"#.to_owned(),
        ));
        files.push((
            "web/package.json".to_owned(),
            r#"{"name":"web"}"#.to_owned(),
        ));
        files.push((
            "compose.yaml".to_owned(),
            "services:\n  api:\n    build:\n      context: .\n      dockerfile: backend/Dockerfile\n".to_owned(),
        ));
        let fixture = owned(&files);
        let graph = graph_of(&fixture);
        assert_eq!(
            sources(edge(&graph, "backend", "package.json")),
            [Source::Dockerfile]
        );
        assert_eq!(
            affected_projects(&graph, &["package.json"]),
            [".", "backend"]
        );
        // Another file at the root may be a setting every project reads:
        // everything. One in a folder of the root's project is the root's.
        assert!(affected(&graph, &changed(&["Makefile"]))
            .everything
            .is_some());
        // A README cannot change a build or a test: nothing.
        assert!(affected(&graph, &changed(&["README.md"])).is_empty());
        assert_eq!(affected_projects(&graph, &["scripts/x.js"]), ["."]);
    }

    // ------------------------------- regressions from the M17 review

    #[test]
    fn the_root_project_s_imports_of_another_project_are_edges() {
        let fixture = fixture(&[
            ("pyproject.toml", "[project]\nname = \"app\"\n"),
            ("app/main.py", "from services.auth.authlib import client\n"),
            (
                "services/auth/pyproject.toml",
                "[project]\nname = \"auth\"\n",
            ),
            ("services/auth/authlib/__init__.py", ""),
            ("services/auth/authlib/client.py", ""),
        ]);
        let graph = graph_of(&fixture);
        assert_eq!(
            sources(edge(&graph, ".", "services/auth")),
            [Source::PythonImport]
        );
        assert_eq!(
            affected_projects(&graph, &["services/auth/authlib/client.py"]),
            [".", "services/auth"]
        );
    }

    #[test]
    fn a_shared_folder_keeps_its_own_imports_when_there_is_a_root_project() {
        for with_root in [false, true] {
            let mut files = vec![
                ("libs/common/db.py", "from billing import client\n"),
                ("services/auth/requirements.txt", "x\n"),
                ("services/auth/main.py", "import libs.common.db\n"),
                ("services/billing/requirements.txt", "x\n"),
                ("services/billing/main.py", ""),
                ("services/billing/billing/__init__.py", ""),
                ("services/billing/billing/client.py", ""),
            ];
            if with_root {
                files.push(("pyproject.toml", "[project]\nname = \"root\"\n"));
            }
            let fixture = fixture(&files);
            let graph = graph_of(&fixture);
            assert!(
                edge(&graph, "libs", "services/billing").is_some(),
                "root: {with_root}"
            );
            let hit = affected_projects(&graph, &["services/billing/billing/client.py"]);
            assert!(
                hit.contains(&"services/auth".to_owned()),
                "root: {with_root}: {hit:?}"
            );
        }
    }

    #[test]
    fn a_wide_shared_node_does_not_hide_files_no_project_owns() {
        let mut files = py_service(
            "backend/auth",
            &[("Dockerfile", "FROM python:3.13\nCOPY . .\n")],
        );
        files.extend(py_service("backend/ledger", &[]));
        files.push((
            "web/package.json".to_owned(),
            r#"{"name":"web"}"#.to_owned(),
        ));
        files.push(("ruff.toml".to_owned(), "line-length = 100\n".to_owned()));
        files.push(("backend/conftest.py".to_owned(), String::new()));
        files.push((
            "compose.yaml".to_owned(),
            "services:\n  auth:\n    build:\n      context: .\n      dockerfile: backend/auth/Dockerfile\n".to_owned(),
        ));
        let fixture = owned(&files);
        let graph = graph_of(&fixture);
        assert!(graph.node("shared:.").is_some());
        for file in ["ruff.toml", "backend/conftest.py"] {
            assert!(
                affected(&graph, &changed(&[file])).everything.is_some(),
                "{file}"
            );
        }
    }

    #[test]
    fn an_import_into_a_folder_of_projects_makes_a_narrow_shared_node() {
        let mut files = py_service("backend/auth", &[]);
        files.push((
            "backend/shared/x.ts".to_owned(),
            "export const x = 1;\n".to_owned(),
        ));
        files.push(("backend/notes.cfg".to_owned(), "x\n".to_owned()));
        files.push((
            "frontend/package.json".to_owned(),
            r#"{"name":"frontend"}"#.to_owned(),
        ));
        files.push((
            "frontend/src/a.ts".to_owned(),
            "import { x } from '../../backend/shared/x';\n".to_owned(),
        ));
        let fixture = owned(&files);
        let graph = graph_of(&fixture);
        assert!(edge(&graph, "frontend", "backend/shared").is_some());
        assert!(graph.node("shared:backend").is_none());
        // A file beside the projects in backend/ still belongs to nobody.
        assert!(affected(&graph, &changed(&["backend/notes.cfg"]))
            .everything
            .is_some());
    }

    #[test]
    fn a_shared_namespace_folder_with_only_subpackages_is_found() {
        let mut files = py_service("backend/auth", &[("main.py", "from shared import db\n")]);
        files.push((
            "backend/shared/db/__init__.py".to_owned(),
            "URL = 1\n".to_owned(),
        ));
        let fixture = owned(&files);
        let graph = graph_of(&fixture);
        assert!(edge(&graph, "backend/auth", "backend/shared").is_some());
        assert_eq!(
            affected_projects(&graph, &["backend/shared/db/__init__.py"]),
            ["backend/auth"]
        );
    }

    #[test]
    fn a_python_file_in_a_node_project_is_read_as_that_project_s_code() {
        let fixture = fixture(&[
            ("web/package.json", r#"{"name":"web"}"#),
            ("web/scripts/gen.py", "from tools.codegen import run\n"),
            ("tools/codegen.py", "def run(): pass\n"),
        ]);
        let graph = graph_of(&fixture);
        assert_eq!(
            sources(edge(&graph, "web", "tools")),
            [Source::PythonImport]
        );
    }

    #[test]
    fn a_dynamic_import_also_depends_on_shared_python_files() {
        let mut files = py_service(
            "backend/plugins",
            &[(
                "main.py",
                "import common\nimport importlib\nimportlib.import_module(n)\n",
            )],
        );
        files.extend(py_service("backend/api", &[("main.py", "import common\n")]));
        files.push(("backend/common.py".to_owned(), "X = 1\n".to_owned()));
        let fixture = owned(&files);
        let graph = graph_of(&fixture);
        assert_eq!(
            sources(edge(&graph, "backend/plugins", "backend/common.py")),
            [Source::PythonImport, Source::DynamicImport]
        );
    }

    // --------------------------------------------------------- output

    #[test]
    fn explain_names_the_source_of_each_edge() {
        let fixture = course_project().unwrap();
        let graph = graph_of(&fixture);
        let text = format_graph_human(&graph, true);
        assert!(
            text.contains("backend/auth_service -> backend/shared (Python import)\n"),
            "{text}"
        );
        assert!(
            text.contains(
                "    Python import: backend/auth_service/main.py:1: from shared.db import URL\n"
            ),
            "{text}"
        );
        let plain = format_graph_human(&graph, false);
        assert!(!plain.contains("main.py:1"));
        assert!(plain.contains("Run fscale graph --explain"));
    }

    #[test]
    fn paths_resolve_and_reach_the_right_folder() {
        assert_eq!(resolve("a/b", "../c").as_deref(), Some("a/c"));
        assert_eq!(resolve("a", "..\\c\\d").as_deref(), Some("c/d"));
        assert_eq!(resolve(".", "../x"), None);
        assert_eq!(resolve("a", "/etc"), None);
        assert_eq!(resolve("a", "C:\\x"), None);
        assert_eq!(
            reach("backend/auth", "backend/shared/db.py"),
            "backend/shared"
        );
        assert_eq!(reach("frontend", "shared/ui/x.ts"), "shared");
        assert_eq!(reach("backend/auth", "backend"), "backend");
        assert_eq!(reach("a", "config.ts"), "config.ts");
        assert!(inside("a/b", "a") && inside("a", "a") && !inside("ab", "a") && inside("x", "."));
    }
}
