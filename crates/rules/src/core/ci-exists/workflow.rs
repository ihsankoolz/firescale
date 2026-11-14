//! The CI workflow that `core/ci-exists` writes (M22, ADR 0041), and how to
//! tell a workflow firescale wrote, untouched since, from one a person wrote.
//!
//! Two jobs:
//!
//! - `pull-request` runs `fscale verify --affected --pr-base`: the base is
//!   the one GitHub's event names (ADR 0045), not a ref in the clone. It can restore caches and never save them: its token
//!   has `cache-mode: read`, and it has no save step at all, only
//!   `actions/cache/restore` steps. The setup actions' own caches are off
//!   there, since they save in a post step.
//! - `default-branch` runs a full `fscale verify` on a push, and saves the
//!   shared step records and the package managers' download caches, from
//!   the default branch only.
//!
//! And a second file, `.github/workflows/tamper-guard.yml` ([`TAMPER_PATH`]),
//! started only by `pull_request_target`, so GitHub takes it from the
//! default branch's copy: a pull request can neither change nor remove it,
//! and no skipped job of the same name exists in another run (a skipped job
//! counts as a passed required check). It runs none of the pull request's
//! code: it checks out the base branch, fetches the pull request's commits
//! as git objects, and runs `fscale tamper --pr-base` from npm at the version
//! that wrote the file (ADR 0045, which changes ADR 0041's "never
//! `pull_request_target`" for this one workflow).
//! Both fetch the whole history, so the merge base is in the clone. A new
//! push to a pull request cancels its run in progress; a push to a branch
//! never does.
//!
//! Every action is pinned to a commit; the comment names its tag. The tags
//! and what each input does were read from GitHub's docs and the actions'
//! own `action.yml` on 2026-11-13 (ADR 0041).

use firescale_core::facts::RepoFacts;

/// Where the workflow is written.
pub(crate) const PATH: &str = ".github/workflows/ci.yml";

pub(crate) const CHECKOUT: &str =
    "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1";
pub(crate) const SETUP_NODE: &str =
    "actions/setup-node@949feb2413d6458794dcd2491c4babbbce0c15c1 # v7.1.0";
pub(crate) const SETUP_PYTHON: &str =
    "actions/setup-python@6d4fd22b20465ccc0f7354476588e8df7c9ca0e0 # v7.1.0";
pub(crate) const CACHE_RESTORE: &str =
    "actions/cache/restore@55cc8345863c7cc4c66a329aec7e433d2d1c52a9 # v6.1.0";
pub(crate) const CACHE_SAVE: &str =
    "actions/cache/save@55cc8345863c7cc4c66a329aec7e433d2d1c52a9 # v6.1.0";

/// The folder of the shared step records, in the workspace.
pub(crate) const SHARED: &str = ".firescale/shared";

/// The last line of a workflow firescale wrote: a digest of everything
/// above it. While it holds, nobody has edited the file.
const DIGEST_PREFIX: &str = "# firescale-workflow: ";

const INSTALL_PLACEHOLDER: &str = "# Add the step that installs your dependencies here.";

/// What the workflow sets up, read from the repo's facts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Setup {
    /// The root's Node package manager, when there is one.
    pub node: Option<Node>,
    /// Python projects installed with pip: (folder, install command).
    pub python: Vec<(String, String)>,
    /// Python projects firescale cannot install in CI (uv, poetry, pipenv).
    pub python_other: Vec<String>,
    /// A `.python-version` at the root, which setup-python reads.
    pub python_version_file: bool,
    /// The branch pushes run on: the default branch when it is known,
    /// else `main` and `master`.
    pub default_branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Node {
    pub manager: String,
    /// The install commands, or `None` when firescale does not know them.
    pub install: Option<Vec<&'static str>>,
}

impl Setup {
    pub(crate) fn from_facts(facts: &RepoFacts, has_python_version: bool) -> Self {
        let node = facts.package_manager.as_ref().map(|manager| Node {
            manager: manager.clone(),
            install: super::super::install::ci_install_commands(manager, &facts.lockfiles),
        });
        let mut python = Vec::new();
        let mut python_other = Vec::new();
        for project in facts.projects.iter().filter(|p| p.kind == "python") {
            let managed = project
                .interpreter
                .as_ref()
                .is_some_and(|i| matches!(i.source.as_str(), "uv" | "poetry" | "pipenv"));
            match project.commands.install.as_deref() {
                Some(install) if !managed => {
                    // On a fresh runner there is no .venv: pip installs into
                    // the python that setup-python put on the PATH.
                    if let Some((_, rest)) = install.split_once(" -m pip install ") {
                        python.push((
                            project.root.clone(),
                            format!("python -m pip install {rest}"),
                        ));
                    } else {
                        python_other.push(project.root.clone());
                    }
                }
                _ => python_other.push(project.root.clone()),
            }
        }
        python.sort();
        python.dedup();
        python_other.sort();
        python_other.dedup();
        Self {
            node,
            python,
            python_other,
            python_version_file: has_python_version,
            default_branch: None,
        }
    }

    /// Notes for the person who reads `init`'s output.
    pub(crate) fn notes(&self) -> Vec<String> {
        let mut notes = Vec::new();
        if self
            .node
            .as_ref()
            .is_some_and(|node| node.install.is_none())
        {
            notes.push(format!(
                "{PATH} has no install step: firescale does not know the install command for CI with your package manager. Add it where the comment says."
            ));
        }
        if !self.python_other.is_empty() {
            notes.push(format!(
                "{PATH} does not install the Python projects in {} (uv, poetry, pipenv or no requirements file): add their install steps.",
                self.python_other.join(", ")
            ));
        }
        notes
    }
}

/// A job's kind: what it may do with the caches.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Job {
    PullRequest,
    DefaultBranch,
}

/// The key prefix of the shared step records: OS, runner image and this
/// workflow file, so a record is never restored under another of them.
const SHARED_KEY: &str = "firescale-steps-${{ runner.os }}-${{ steps.image.outputs.id }}-${{ hashFiles('.github/workflows/ci.yml') }}-";

struct Lines(Vec<String>);

impl Lines {
    fn push(&mut self, indent: usize, text: &str) {
        if text.is_empty() {
            self.0.push(String::new());
        } else {
            self.0.push(format!("{}{text}", " ".repeat(indent)));
        }
    }
}

/// A download cache: its folder (from a step's output), key and the files
/// its key hashes.
struct DownloadCache {
    id: &'static str,
    find_dir: &'static str,
    key: &'static str,
    files: &'static str,
}

fn download_caches(setup: &Setup) -> Vec<DownloadCache> {
    let mut caches = Vec::new();
    if let Some(node) = &setup.node {
        if node.install.is_some() {
            match node.manager.as_str() {
                "npm" => caches.push(DownloadCache {
                    id: "npm",
                    find_dir: "npm config get cache",
                    key: "npm",
                    files:
                        "'**/package-lock.json', '**/npm-shrinkwrap.json', '!**/node_modules/**'",
                }),
                "pnpm" => caches.push(DownloadCache {
                    id: "pnpm",
                    find_dir: "pnpm store path --silent",
                    key: "pnpm",
                    files: "'**/pnpm-lock.yaml', '!**/node_modules/**'",
                }),
                _ => {}
            }
        }
    }
    if !setup.python.is_empty() {
        caches.push(DownloadCache {
            id: "pip",
            find_dir: "python -m pip cache dir",
            key: "pip",
            files: "'**/requirements*.txt', '**/pyproject.toml', '!**/node_modules/**', '!**/.venv/**'",
        });
    }
    caches
}

fn job(lines: &mut Lines, setup: &Setup, kind: Job) {
    let (name, condition) = match kind {
        Job::PullRequest => ("pull-request", "github.event_name == 'pull_request'"),
        Job::DefaultBranch => ("default-branch", "github.event_name == 'push'"),
    };
    match kind {
        Job::PullRequest => {
            lines.push(
                2,
                "# Pull requests: only the projects the change reaches, judged against",
            );
            lines.push(
                2,
                "# the base branch. This job restores caches and can never save them.",
            );
        }
        Job::DefaultBranch => {
            lines.push(
                2,
                "# Pushes: everything. On the default branch it also saves the caches",
            );
            lines.push(2, "# that pull requests restore.");
        }
    }
    lines.push(2, &format!("{name}:"));
    lines.push(4, &format!("if: {condition}"));
    lines.push(4, "runs-on: ubuntu-latest");
    if kind == Job::PullRequest {
        lines.push(4, "cache-mode: read");
    }
    lines.push(4, "steps:");
    lines.push(6, &format!("- uses: {CHECKOUT}"));
    lines.push(8, "with:");
    lines.push(
        10,
        "# The whole history, so the merge base with the base branch is here.",
    );
    lines.push(10, "fetch-depth: 0");
    lines.push(6, "- id: image");
    lines.push(
        8,
        "run: echo \"id=${ImageOS:-unknown}-${ImageVersion:-unknown}\" >> \"$GITHUB_OUTPUT\"",
    );
    lines.push(6, &format!("- uses: {SETUP_NODE}"));
    lines.push(8, "with:");
    lines.push(10, "node-version: 24");
    lines.push(
        10,
        "# Its cache saves in a post step; the caches below are explicit.",
    );
    lines.push(10, "package-manager-cache: false");
    if !setup.python.is_empty() {
        lines.push(6, &format!("- uses: {SETUP_PYTHON}"));
        if !setup.python_version_file {
            lines.push(8, "with:");
            lines.push(10, "python-version: \"3.x\"");
        }
    }
    let caches = download_caches(setup);
    // pnpm itself first, so its store path can be asked.
    if let Some(node) = &setup.node {
        if let Some(install) = &node.install {
            for command in install
                .iter()
                .filter(|c| c.starts_with("npm install --global"))
            {
                lines.push(6, &format!("- run: {command}"));
            }
        }
    }
    for cache in &caches {
        lines.push(6, &format!("- id: {}-dir", cache.id));
        lines.push(
            8,
            &format!(
                "run: echo \"dir=$({})\" >> \"$GITHUB_OUTPUT\"",
                cache.find_dir
            ),
        );
        lines.push(6, &format!("- id: {}-cache", cache.id));
        lines.push(8, &format!("uses: {CACHE_RESTORE}"));
        lines.push(8, "with:");
        lines.push(
            10,
            &format!("path: ${{{{ steps.{}-dir.outputs.dir }}}}", cache.id),
        );
        lines.push(
            10,
            &format!(
                "key: {}-${{{{ runner.os }}}}-${{{{ hashFiles({}) }}}}",
                cache.key, cache.files
            ),
        );
        lines.push(
            10,
            &format!("restore-keys: {}-${{{{ runner.os }}}}-", cache.key),
        );
    }
    if let Some(node) = &setup.node {
        match &node.install {
            Some(install) => {
                for command in install
                    .iter()
                    .filter(|c| !c.starts_with("npm install --global"))
                {
                    lines.push(6, &format!("- run: {command}"));
                }
            }
            None => lines.push(6, INSTALL_PLACEHOLDER),
        }
    }
    for (folder, install) in &setup.python {
        lines.push(6, &format!("- run: {install}"));
        if folder != "." {
            lines.push(8, &format!("working-directory: {folder}"));
        }
    }
    lines.push(6, "- id: firescale-cache");
    lines.push(8, &format!("uses: {CACHE_RESTORE}"));
    lines.push(8, "with:");
    lines.push(10, &format!("path: {SHARED}"));
    lines.push(10, &format!("key: {SHARED_KEY}${{{{ github.sha }}}}"));
    lines.push(10, &format!("restore-keys: {SHARED_KEY}"));
    lines.push(6, "# Runs check, lint, typecheck and test.");
    match kind {
        Job::PullRequest => {
            lines.push(
                6,
                "# The base is the one GitHub's event names, not a ref in the clone.",
            );
            lines.push(
                6,
                &format!("- run: npx firescale verify --affected --pr-base --jobs 2 --shared-cache {SHARED}"),
            );
        }
        Job::DefaultBranch => {
            lines.push(
                6,
                &format!("- run: npx firescale verify --jobs 2 --shared-cache {SHARED}"),
            );
            lines.push(
                6,
                "# Saved from the default branch only: what pull requests may trust.",
            );
            lines.push(6, "# Not saved when it holds just what was restored.");
            lines.push(6, &format!("- uses: {CACHE_SAVE}"));
            lines.push(
                8,
                &format!("if: ${{{{ !cancelled() && github.ref_name == github.event.repository.default_branch && hashFiles('{SHARED}/*.json') != '' && !endsWith(steps.firescale-cache.outputs.cache-matched-key, hashFiles('{SHARED}/*.json')) }}}}"),
            );
            lines.push(8, "with:");
            lines.push(10, &format!("path: {SHARED}"));
            lines.push(
                10,
                &format!("key: {SHARED_KEY}${{{{ hashFiles('{SHARED}/*.json') }}}}"),
            );
            for cache in &caches {
                lines.push(6, &format!("- uses: {CACHE_SAVE}"));
                lines.push(
                    8,
                    &format!("if: ${{{{ !cancelled() && github.ref_name == github.event.repository.default_branch && steps.{}-cache.outputs.cache-hit != 'true' }}}}", cache.id),
                );
                lines.push(8, "with:");
                lines.push(
                    10,
                    &format!("path: ${{{{ steps.{}-dir.outputs.dir }}}}", cache.id),
                );
                lines.push(
                    10,
                    &format!(
                        "key: {}-${{{{ runner.os }}}}-${{{{ hashFiles({}) }}}}",
                        cache.key, cache.files
                    ),
                );
            }
        }
    }
}

/// Where the tamper guard's own workflow is written.
pub(crate) const TAMPER_PATH: &str = ".github/workflows/tamper-guard.yml";

/// The tamper guard's workflow (ADR 0045). LF line endings; the last line
/// is its digest, as for the CI workflow.
pub(crate) fn tamper_workflow() -> String {
    let mut lines = Lines(Vec::new());
    for line in [
        "# Created by firescale (core/ci-exists). `fscale sync` replaces this file",
        "# with a newer one only while it is exactly as firescale wrote it (the",
        "# digest on the last line says so). Once you edit it, it is yours.",
        "#",
        "# The tamper guard. On pull_request_target GitHub runs this workflow as",
        "# the default branch has it, so a pull request cannot edit or remove it.",
        "# It runs no code from the pull request: it checks out the base branch,",
        "# fetches the pull request's commits as git objects, and compares what",
        "# merging them would change with the base GitHub names. Keep it that way:",
        "# never check out the pull request's files here, install them or run them.",
        "# To make it block, require the tamper-guard check in branch protection",
        "# and set verify.tamper to \"enforced\" on the default branch.",
        "name: Tamper guard",
        "",
        "on:",
        "  pull_request_target:",
        "",
        "permissions:",
        "  contents: read",
        "",
        "concurrency:",
        "  group: tamper-guard-${{ github.event.pull_request.number }}",
        "  cancel-in-progress: true",
        "",
        "jobs:",
    ] {
        lines.push(0, line);
    }
    lines.push(2, "tamper-guard:");
    lines.push(4, "runs-on: ubuntu-latest");
    lines.push(4, "cache-mode: none");
    lines.push(4, "steps:");
    lines.push(6, &format!("- uses: {CHECKOUT}"));
    lines.push(8, "with:");
    lines.push(10, "fetch-depth: 0");
    lines.push(6, "- run: git fetch --no-tags origin \"$HEAD_SHA\"");
    lines.push(8, "env:");
    lines.push(10, "HEAD_SHA: ${{ github.event.pull_request.head.sha }}");
    lines.push(6, &format!("- uses: {SETUP_NODE}"));
    lines.push(8, "with:");
    lines.push(10, "node-version: 24");
    lines.push(10, "package-manager-cache: false");
    lines.push(
        6,
        &format!(
            "- run: npx --yes firescale@{} tamper --pr-base",
            firescale_core::version()
        ),
    );
    let mut text = lines.0.join("\n");
    text.push('\n');
    let digest = digest_of(&text);
    text.push_str(&format!("{DIGEST_PREFIX}{digest}\n"));
    text
}

/// The workflow for `setup`. LF line endings; the last line is its digest.
pub(crate) fn workflow(setup: &Setup) -> String {
    let mut lines = Lines(Vec::new());
    let branches = setup
        .default_branch
        .as_deref()
        .map_or_else(|| "main, master".to_owned(), str::to_owned);
    for line in [
        "# Created by firescale (core/ci-exists). `fscale sync` replaces this file",
        "# with a newer one only while it is exactly as firescale wrote it (the",
        "# digest on the last line says so). Once you edit it, it is yours, and",
        "# firescale never rewrites it.",
        "name: CI",
        "",
        "on:",
        "  pull_request:",
        "  push:",
        &format!("    branches: [{branches}]"),
        "",
        "permissions:",
        "  contents: read",
        "",
        "# A new push to a pull request cancels its run in progress. Each push",
        "# to a branch has a group of its own, so it is never cancelled: it",
        "# keeps the shared cache up to date.",
        "concurrency:",
        "  group: ci-${{ github.event.pull_request.number || github.run_id }}",
        "  cancel-in-progress: ${{ github.event_name == 'pull_request' }}",
        "",
        "jobs:",
    ] {
        lines.push(0, line);
    }
    job(&mut lines, setup, Job::PullRequest);
    lines.push(0, "");
    job(&mut lines, setup, Job::DefaultBranch);
    let mut text = lines.0.join("\n");
    text.push('\n');
    let digest = digest_of(&text);
    text.push_str(&format!("{DIGEST_PREFIX}{digest}\n"));
    text
}

fn digest_of(text: &str) -> String {
    firescale_core::sharedcache::short_digest(text)
}

/// The starter of firescale 0.1.0 to 0.3.0, before M22, for `install`.
fn legacy(install: Option<&[&str]>, has_package_manager: bool) -> String {
    let mut lines: Vec<String> = [
        "# Created by firescale (core/ci-exists). This file belongs to your team:",
        "# edit it freely. firescale will not rewrite it.",
        "name: CI",
        "",
        "on:",
        "  pull_request:",
        "  push:",
        "    branches: [main, master]",
        "",
        "permissions:",
        "  contents: read",
        "",
        "jobs:",
        "  verify:",
        "    runs-on: ubuntu-latest",
        "    steps:",
        "      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1",
        "      - uses: actions/setup-node@949feb2413d6458794dcd2491c4babbbce0c15c1 # v7.1.0",
        "        with:",
        "          node-version: 24",
    ]
    .iter()
    .map(|line| (*line).to_owned())
    .collect();
    match install {
        Some(commands) => lines.extend(commands.iter().map(|c| format!("      - run: {c}"))),
        None if has_package_manager => lines.push(format!("      {INSTALL_PLACEHOLDER}")),
        None => {}
    }
    lines.push("      # Runs check, lint, typecheck and test.".to_owned());
    lines.push("      - run: npx firescale verify".to_owned());
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// True when `text` is a workflow firescale wrote and nobody has edited
/// since: this version's (its digest line holds), or an earlier starter,
/// byte for byte (line endings aside, since git may check out CRLF).
pub(crate) fn firescale_wrote(text: &str) -> bool {
    let text = text.replace("\r\n", "\n");
    let trimmed = text.strip_suffix('\n').unwrap_or(&text);
    if let Some(at) = trimmed.rfind('\n') {
        if let Some(digest) = trimmed[at + 1..].strip_prefix(DIGEST_PREFIX) {
            return text.ends_with('\n') && digest_of(&text[..=at]) == digest;
        }
    }
    let installs: [Option<&[&str]>; 4] = [
        None,
        Some(&["npm ci"]),
        Some(&["npm install"]),
        Some(&[
            "npm install --global pnpm",
            "pnpm install --frozen-lockfile",
        ]),
    ];
    installs.iter().any(|install| {
        [false, true]
            .iter()
            .any(|has| text == legacy(*install, *has))
    })
}
