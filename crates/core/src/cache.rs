//! The rule-result cache (SPEC §9).
//!
//! A rule's result is reused when nothing it depends on has changed. The key
//! is a hash of: the firescale version, the rule id, the rule's options, and
//! a fingerprint of the rule's input files. The fingerprint covers every file
//! that matches the rule's `inputs` globs: its path, its content hash, and
//! whether git tracks it. Entries live in `.firescale/cache/`.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::git::Git;
use crate::model::{Diagnostic, RuleMeta};
use crate::rule::Options;
use crate::walk::walk_files;

/// Bumped when the entry format or the key recipe changes, so old entries are ignored.
const CACHE_FORMAT: u32 = 1;

/// The cache folder, relative to the repo root.
pub const CACHE_DIR: [&str; 2] = [".firescale", "cache"];

#[derive(Serialize, Deserialize)]
struct Entry {
    key: String,
    diagnostics: Vec<Diagnostic>,
}

/// Builds a matcher for a rule's `inputs`. `*` stays inside one path segment;
/// `**` crosses segments.
pub fn input_matcher(rule_id: &str, inputs: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for input in inputs {
        let glob = GlobBuilder::new(input).literal_separator(true).build().map_err(|error| {
            Error::internal(format!(
                "Rule \"{rule_id}\" has an invalid inputs glob \"{input}\" ({error}). Fix the rule's inputs."
            ))
        })?;
        builder.add(glob);
    }
    builder.build().map_err(|error| {
        Error::internal(format!(
            "Rule \"{rule_id}\" has invalid inputs ({error}). Fix the rule's inputs."
        ))
    })
}

/// The state of the repo that cache keys are computed from. Built once per run.
pub struct RepoSnapshot {
    root: PathBuf,
    /// Every file git would not ignore, plus every tracked file. Sorted.
    files: Vec<String>,
    tracked: BTreeSet<String>,
    hashes: HashMap<String, String>,
}

impl RepoSnapshot {
    /// Walks the repo and reads the git index.
    pub fn capture(root: &Path) -> Result<Self> {
        // The walk and the git call are independent, so they run side by side.
        let (walked, tracked) = std::thread::scope(|scope| {
            let walk = scope.spawn(|| walk_files(root));
            let tracked = Git::new(root).tracked_files();
            let walked = walk.join().unwrap_or_else(|_| {
                Err(Error::internal(
                    "The file walk panicked. Please report this.",
                ))
            });
            (walked, tracked)
        });
        let tracked: BTreeSet<String> = tracked?.into_iter().collect();
        let mut files: BTreeSet<String> = walked?.into_iter().collect();
        // A tracked file can be ignored (added with --force), so the walk alone misses it.
        files.extend(tracked.iter().cloned());
        Ok(Self {
            root: root.to_path_buf(),
            files: files.into_iter().collect(),
            tracked,
            hashes: HashMap::new(),
        })
    }

    fn content_hash(&mut self, file: &str) -> String {
        if let Some(hash) = self.hashes.get(file) {
            return hash.clone();
        }
        let path = file
            .split('/')
            .fold(self.root.clone(), |path, part| path.join(part));
        // A file that cannot be read (deleted but still tracked, or a folder
        // entry) still contributes a stable marker to the fingerprint.
        let hash = fs::read(path).map_or_else(
            |_| "missing".to_owned(),
            |bytes| blake3::hash(&bytes).to_hex().to_string(),
        );
        self.hashes.insert(file.to_owned(), hash.clone());
        hash
    }

    /// Hashes everything a rule's result depends on.
    pub fn rule_key(
        &mut self,
        version: &str,
        meta: &RuleMeta,
        options: &Options,
    ) -> Result<String> {
        let matcher = input_matcher(&meta.id, &meta.inputs)?;
        let matched: Vec<String> = self
            .files
            .iter()
            .filter(|file| matcher.is_match(file.as_str()))
            .cloned()
            .collect();

        let mut hasher = blake3::Hasher::new();
        let mut field = |text: &str| {
            hasher.update(&(text.len() as u64).to_le_bytes());
            hasher.update(text.as_bytes());
        };
        field(&CACHE_FORMAT.to_string());
        field(version);
        field(&meta.id);
        field(&serde_json::to_string(options).unwrap_or_default());
        for input in &meta.inputs {
            field(input);
        }
        for file in &matched {
            let hash = self.content_hash(file);
            field(file);
            field(&hash);
            field(if self.tracked.contains(file) {
                "tracked"
            } else {
                "untracked"
            });
        }
        Ok(hasher.finalize().to_hex().to_string())
    }
}

/// Reads and writes cache entries for one repo.
pub struct Cache {
    dir: PathBuf,
}

impl Cache {
    #[must_use]
    pub fn new(root: &Path) -> Self {
        Self {
            dir: CACHE_DIR
                .iter()
                .fold(root.to_path_buf(), |path, part| path.join(part)),
        }
    }

    fn entry_path(&self, rule_id: &str) -> PathBuf {
        self.dir
            .join(format!("{}.json", rule_id.replace('/', "__")))
    }

    /// Returns the stored diagnostics when the entry's key matches. A missing,
    /// unreadable or outdated entry is a miss, never an error.
    #[must_use]
    pub fn get(&self, rule_id: &str, key: &str) -> Option<Vec<Diagnostic>> {
        let text = fs::read_to_string(self.entry_path(rule_id)).ok()?;
        let entry: Entry = serde_json::from_str(&text).ok()?;
        (entry.key == key).then_some(entry.diagnostics)
    }

    /// Stores a rule's result. The cache folder ignores itself, so it is never committed.
    pub fn put(&self, rule_id: &str, key: &str, diagnostics: &[Diagnostic]) -> Result<()> {
        let failed = |error: std::io::Error| {
            Error::user(format!(
                "Could not write the cache in {} ({error}). Check the folder's permissions, or run with --no-cache.",
                self.dir.display()
            ))
        };
        fs::create_dir_all(&self.dir).map_err(failed)?;
        if let Some(parent) = self.dir.parent() {
            let ignore = parent.join(".gitignore");
            if !ignore.exists() {
                fs::write(
                    ignore,
                    "# Created by firescale. Nothing in this folder should be committed.\n*\n",
                )
                .map_err(failed)?;
            }
        }
        let entry = Entry {
            key: key.to_owned(),
            diagnostics: diagnostics.to_vec(),
        };
        let json = serde_json::to_string(&entry).unwrap_or_default();
        // Write then rename, so a concurrent run never reads half an entry.
        let target = self.entry_path(rule_id);
        let temporary = target.with_extension(format!("{}.tmp", std::process::id()));
        fs::write(&temporary, json).map_err(failed)?;
        fs::rename(&temporary, &target).map_err(failed)
    }
}
