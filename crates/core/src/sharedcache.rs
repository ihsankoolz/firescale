//! The shared step cache of CI (M22, ADR 0041).
//!
//! In GitHub Actions, the step cache's passes are also kept as small
//! records in a folder that the workflow restores from, and on the default
//! branch saves to, the GitHub Actions cache. A record is a key, a result,
//! a time, a digest of the output and who wrote it (workflow, ref, commit);
//! never a build output.
//!
//! - Only a run on a push to the default branch writes records. The pull
//!   request job of the generated workflow cannot save the cache at all
//!   (a restore-only step and `cache-mode: read`); firescale also refuses to
//!   write anywhere else.
//! - A record is used only when its whole key matches the step's key, it
//!   says `passed`, it is younger than 30 days, and it was written from the
//!   default branch.
//! - Laptops never read the shared cache: outside GitHub Actions the folder
//!   is ignored, with a note.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// The format of a record. A record of another format is never used.
pub const RECORD_FORMAT: u32 = 1;

/// Records older than this are not used, and are removed.
pub const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);

/// At most this many records are kept; the oldest go first.
pub const MAX_RECORDS: usize = 2000;

/// Who wrote a record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Writer {
    /// `GITHUB_WORKFLOW_REF`: `owner/repo/.github/workflows/ci.yml@refs/heads/main`.
    pub workflow: String,
    /// `GITHUB_REF`: `refs/heads/main`.
    #[serde(rename = "ref")]
    pub git_ref: String,
    /// `GITHUB_SHA`.
    pub commit: String,
}

/// One step's result, as kept in the shared cache.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub format: u32,
    /// The step's whole cache key.
    pub key: String,
    /// Always `passed`: only passes are kept.
    pub result: String,
    /// When the step ran, in seconds since 1970 (UTC).
    pub time: u64,
    /// How long it took, for the summary's "saved" line.
    pub duration_ms: u64,
    /// A blake3 digest of its output. The output itself is never kept.
    pub output_digest: String,
    pub writer: Writer,
}

/// The GitHub Actions run verify is part of, read from its environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiRun {
    pub event: String,
    pub writer: Writer,
    /// The repository's default branch, from the event's payload.
    pub default_branch: Option<String>,
    /// A digest of the workflow file's bytes, `missing` when it cannot be
    /// read. Part of every step key in CI.
    pub workflow_digest: String,
}

impl CiRun {
    /// The run, when verify runs in GitHub Actions (`GITHUB_ACTIONS=true`).
    /// `read` reads one environment variable; `workspace` is where the
    /// workflow file's path is relative to (`GITHUB_WORKSPACE`, else the
    /// repo root).
    #[must_use]
    pub fn from_env(read: &dyn Fn(&str) -> Option<String>, root: &Path) -> Option<Self> {
        if read("GITHUB_ACTIONS").as_deref() != Some("true") {
            return None;
        }
        let get = |name: &str| read(name).unwrap_or_default();
        let workflow = get("GITHUB_WORKFLOW_REF");
        let default_branch = read("GITHUB_EVENT_PATH")
            .and_then(|path| fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|event| {
                event
                    .get("repository")?
                    .get("default_branch")?
                    .as_str()
                    .map(str::to_owned)
            })
            .filter(|branch| !branch.is_empty());
        let workspace = read("GITHUB_WORKSPACE").map_or_else(|| root.to_path_buf(), PathBuf::from);
        let workflow_digest = workflow_file(&workflow, &get("GITHUB_REPOSITORY"))
            .map(|relative| {
                relative
                    .split('/')
                    .fold(workspace.clone(), |path, part| path.join(part))
            })
            .and_then(|path| fs::read(path).ok())
            .map_or_else(
                || "missing".to_owned(),
                |bytes| blake3::hash(&bytes).to_hex().to_string(),
            );
        Some(Self {
            event: get("GITHUB_EVENT_NAME"),
            writer: Writer {
                workflow,
                git_ref: get("GITHUB_REF"),
                commit: get("GITHUB_SHA"),
            },
            default_branch,
            workflow_digest,
        })
    }

    /// `refs/heads/<default branch>`, when the default branch is known.
    #[must_use]
    pub fn default_ref(&self) -> Option<String> {
        self.default_branch
            .as_ref()
            .map(|branch| format!("refs/heads/{branch}"))
    }

    /// True for a push to the default branch: the only run that writes
    /// records. Every other event, pull requests above all, only reads.
    #[must_use]
    pub fn writes(&self) -> bool {
        self.event == "push" && self.default_ref().as_deref() == Some(self.writer.git_ref.as_str())
    }

    /// What is added to every step key in CI: the workflow file's bytes.
    /// The runner image reaches the key through `ImageOS` and
    /// `ImageVersion`, which are on the environment allowlist.
    #[must_use]
    pub fn key_salt(&self) -> String {
        format!("ci-workflow\0{}", self.workflow_digest)
    }
}

/// The workflow file's path in the repository, from `GITHUB_WORKFLOW_REF`
/// (`owner/repo/.github/workflows/ci.yml@refs/heads/main`).
fn workflow_file(workflow_ref: &str, repository: &str) -> Option<String> {
    let path = workflow_ref.split('@').next()?;
    let path = if repository.is_empty() {
        path.splitn(3, '/').nth(2)?
    } else {
        path.strip_prefix(repository)?.strip_prefix('/')?
    };
    let safe = !path.is_empty()
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..");
    safe.then(|| path.to_owned())
}

/// A short blake3 digest of `text` (32 hex digits), for files firescale
/// writes and must recognise later, such as its CI workflow.
#[must_use]
pub fn short_digest(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex()[..32].to_owned()
}

/// The shared cache of one verify run.
#[derive(Debug, Clone)]
pub struct SharedCache {
    dir: PathBuf,
    run: CiRun,
}

/// Why a record was not used, for `FSCALE_LOG=cache`.
pub type Refusal = String;

fn is_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 128 && key.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

impl SharedCache {
    /// The shared cache in `dir` for a CI run.
    #[must_use]
    pub fn new(dir: PathBuf, run: CiRun) -> Self {
        Self { dir, run }
    }

    #[must_use]
    pub fn run(&self) -> &CiRun {
        &self.run
    }

    /// True when this run writes records (a push to the default branch).
    #[must_use]
    pub fn writes(&self) -> bool {
        self.run.writes()
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.json"))
    }

    /// Why `record` may not be used for `key`, or `None` when it may.
    fn refusal(&self, record: &Record, key: &str, now: u64) -> Option<Refusal> {
        if record.format != RECORD_FORMAT {
            return Some(format!(
                "its format is {}, not {RECORD_FORMAT}",
                record.format
            ));
        }
        if record.key != key {
            return Some("its key is not the step's key".to_owned());
        }
        if record.result != "passed" {
            return Some(format!("its result is {}", record.result));
        }
        if now.saturating_sub(record.time) > MAX_AGE.as_secs() || record.time > now + 300 {
            return Some("it is older than 30 days, or dated in the future".to_owned());
        }
        match self.run.default_ref() {
            None => Some(
                "the default branch is not known (no repository.default_branch in the event)"
                    .to_owned(),
            ),
            Some(default) if record.writer.git_ref != default => Some(format!(
                "it was written from {}, not from the default branch {default}",
                if record.writer.git_ref.is_empty() {
                    "an unknown ref"
                } else {
                    record.writer.git_ref.as_str()
                }
            )),
            Some(_) => None,
        }
    }

    /// The record for `key`, when one may be used.
    ///
    /// # Errors
    ///
    /// Why there is none, or why it is not used.
    pub fn lookup(&self, key: &str) -> Result<Record, Refusal> {
        if !is_key(key) {
            return Err("the key is not a digest".to_owned());
        }
        let text = fs::read_to_string(self.path(key)).map_err(|_| "no record".to_owned())?;
        let record: Record =
            serde_json::from_str(&text).map_err(|_| "the record cannot be read".to_owned())?;
        match self.refusal(&record, key, now_secs()) {
            Some(refusal) => Err(refusal),
            None => Ok(record),
        }
    }

    /// Writes a pass, on a push to the default branch only.
    ///
    /// # Errors
    ///
    /// When this run may not write, or the file cannot be written.
    pub fn write(
        &self,
        key: &str,
        time: u64,
        duration_ms: u64,
        output_digest: &str,
    ) -> Result<(), String> {
        if !self.writes() {
            return Err("only a push to the default branch writes the shared cache".to_owned());
        }
        if !is_key(key) {
            return Err("the key is not a digest".to_owned());
        }
        let record = Record {
            format: RECORD_FORMAT,
            key: key.to_owned(),
            result: "passed".to_owned(),
            time,
            duration_ms,
            output_digest: output_digest.to_owned(),
            writer: self.run.writer.clone(),
        };
        fs::create_dir_all(&self.dir).map_err(|error| error.to_string())?;
        let text = serde_json::to_string(&record).map_err(|error| error.to_string())?;
        let temp = self.dir.join(format!(".{key}.{}.tmp", std::process::id()));
        fs::write(&temp, text).map_err(|error| error.to_string())?;
        fs::rename(&temp, self.path(key)).map_err(|error| {
            let _ = fs::remove_file(&temp);
            error.to_string()
        })
    }

    /// On a run that writes: removes every file that is not a record this
    /// run may use (another format, another writer, too old, unreadable,
    /// or not a record at all), and the oldest records beyond
    /// [`MAX_RECORDS`]. So the folder the workflow saves holds nothing but
    /// usable records written from the default branch. Returns how many
    /// files were removed.
    pub fn prune(&self) -> usize {
        if !self.writes() {
            return 0;
        }
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return 0;
        };
        let now = now_secs();
        let mut removed = 0;
        let mut kept: Vec<(u64, PathBuf)> = Vec::new();
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let key = name.strip_suffix(".json").unwrap_or_default();
            let usable = entry
                .file_type()
                .is_ok_and(|kind| kind.is_file())
                .then(|| fs::read_to_string(&path).ok())
                .flatten()
                .and_then(|text| serde_json::from_str::<Record>(&text).ok())
                .filter(|record| is_key(key) && self.refusal(record, key, now).is_none());
            match usable {
                Some(record) => kept.push((record.time, path)),
                None => {
                    let gone = if path.is_dir() {
                        fs::remove_dir_all(&path)
                    } else {
                        fs::remove_file(&path)
                    };
                    if gone.is_ok() {
                        removed += 1;
                    }
                }
            }
        }
        if kept.len() > MAX_RECORDS {
            kept.sort();
            let extra = kept.len() - MAX_RECORDS;
            for (_, path) in kept.into_iter().take(extra) {
                if fs::remove_file(path).is_ok() {
                    removed += 1;
                }
            }
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn run_with(vars: &[(&str, &str)], root: &Path) -> Option<CiRun> {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        CiRun::from_env(&|name| vars.get(name).cloned(), root)
    }

    fn event(dir: &Path, default_branch: &str) -> String {
        let path = dir.join("event.json");
        fs::write(
            &path,
            format!(r#"{{"repository":{{"default_branch":"{default_branch}"}}}}"#),
        )
        .unwrap();
        path.to_string_lossy().into_owned()
    }

    fn ci(dir: &Path, event_name: &str, git_ref: &str) -> CiRun {
        let event = event(dir, "main");
        run_with(
            &[
                ("GITHUB_ACTIONS", "true"),
                ("GITHUB_EVENT_NAME", event_name),
                ("GITHUB_REF", git_ref),
                ("GITHUB_SHA", "0123456789abcdef"),
                ("GITHUB_REPOSITORY", "owner/repo"),
                (
                    "GITHUB_WORKFLOW_REF",
                    "owner/repo/.github/workflows/ci.yml@refs/heads/main",
                ),
                ("GITHUB_EVENT_PATH", &event),
                ("GITHUB_WORKSPACE", &dir.to_string_lossy()),
            ],
            dir,
        )
        .unwrap()
    }

    const KEY: &str = "ab12";

    #[test]
    fn outside_github_actions_there_is_no_ci_run() {
        let temp = tempfile::tempdir().unwrap();
        assert!(run_with(&[], temp.path()).is_none());
        assert!(run_with(&[("GITHUB_ACTIONS", "false")], temp.path()).is_none());
    }

    #[test]
    fn only_a_push_to_the_default_branch_writes() {
        let temp = tempfile::tempdir().unwrap();
        assert!(ci(temp.path(), "push", "refs/heads/main").writes());
        assert!(!ci(temp.path(), "push", "refs/heads/feature").writes());
        assert!(!ci(temp.path(), "pull_request", "refs/pull/7/merge").writes());
        assert!(!ci(temp.path(), "pull_request_target", "refs/heads/main").writes());
        let pull = SharedCache::new(
            temp.path().join("shared"),
            ci(temp.path(), "pull_request", "refs/pull/7/merge"),
        );
        assert!(pull.write(KEY, now_secs(), 10, "d").is_err());
        assert!(!temp.path().join("shared").exists());
    }

    #[test]
    fn a_record_is_used_only_on_an_exact_key_from_the_default_branch() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("shared");
        let main = SharedCache::new(dir.clone(), ci(temp.path(), "push", "refs/heads/main"));
        main.write(KEY, now_secs(), 1200, "digest").unwrap();
        let pull = SharedCache::new(
            dir.clone(),
            ci(temp.path(), "pull_request", "refs/pull/7/merge"),
        );
        let record = pull.lookup(KEY).unwrap();
        assert_eq!(record.writer.git_ref, "refs/heads/main");
        assert_eq!(record.writer.commit, "0123456789abcdef");
        assert_eq!(record.duration_ms, 1200);
        // Another key, even one that starts the same, is a miss.
        assert!(pull.lookup("ab1").is_err());
        assert!(pull.lookup("ab123").is_err());
        // A record copied under another key's name is not used.
        fs::copy(dir.join("ab12.json"), dir.join("cd34.json")).unwrap();
        assert_eq!(
            pull.lookup("cd34").unwrap_err(),
            "its key is not the step's key"
        );
        // A key that is not a digest never reaches the file system.
        assert!(pull.lookup("../x").is_err());
    }

    #[test]
    fn a_record_written_from_another_ref_or_too_old_is_not_used() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("shared");
        fs::create_dir_all(&dir).unwrap();
        let mut record = Record {
            format: RECORD_FORMAT,
            key: KEY.to_owned(),
            result: "passed".to_owned(),
            time: now_secs(),
            duration_ms: 1,
            output_digest: "d".to_owned(),
            writer: Writer {
                workflow: "owner/repo/.github/workflows/ci.yml@refs/pull/7/merge".to_owned(),
                git_ref: "refs/pull/7/merge".to_owned(),
                commit: "f".to_owned(),
            },
        };
        let put = |record: &Record| {
            fs::write(
                dir.join(format!("{KEY}.json")),
                serde_json::to_string(record).unwrap(),
            )
            .unwrap();
        };
        put(&record);
        let pull = SharedCache::new(
            dir.clone(),
            ci(temp.path(), "pull_request", "refs/pull/8/merge"),
        );
        assert!(pull
            .lookup(KEY)
            .unwrap_err()
            .contains("not from the default branch"));
        record.writer.git_ref = "refs/heads/main".to_owned();
        record.time = now_secs() - MAX_AGE.as_secs() - 10;
        put(&record);
        assert!(pull.lookup(KEY).unwrap_err().contains("older than 30 days"));
        record.time = now_secs();
        record.result = "failed".to_owned();
        put(&record);
        assert!(pull.lookup(KEY).is_err());
        record.result = "passed".to_owned();
        put(&record);
        assert!(pull.lookup(KEY).is_ok());
    }

    #[test]
    fn pruning_keeps_only_usable_records_and_only_on_the_default_branch() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("shared");
        let main = SharedCache::new(dir.clone(), ci(temp.path(), "push", "refs/heads/main"));
        main.write(KEY, now_secs(), 1, "d").unwrap();
        fs::write(dir.join("cd34.json"), "not json").unwrap();
        fs::write(dir.join("notes.txt"), "x").unwrap();
        let mut foreign: Record =
            serde_json::from_str(&fs::read_to_string(dir.join("ab12.json")).unwrap()).unwrap();
        foreign.key = "ef56".to_owned();
        foreign.writer.git_ref = "refs/heads/feature".to_owned();
        fs::write(
            dir.join("ef56.json"),
            serde_json::to_string(&foreign).unwrap(),
        )
        .unwrap();
        let pull = SharedCache::new(
            dir.clone(),
            ci(temp.path(), "pull_request", "refs/pull/7/merge"),
        );
        assert_eq!(pull.prune(), 0);
        assert_eq!(main.prune(), 3);
        let left: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, vec!["ab12.json".to_owned()]);
    }

    #[test]
    fn the_workflow_file_is_found_from_its_ref_and_hashed() {
        let temp = tempfile::tempdir().unwrap();
        let workflows = temp.path().join(".github").join("workflows");
        fs::create_dir_all(&workflows).unwrap();
        fs::write(workflows.join("ci.yml"), "name: CI\n").unwrap();
        let first = ci(temp.path(), "push", "refs/heads/main");
        assert_ne!(first.workflow_digest, "missing");
        fs::write(workflows.join("ci.yml"), "name: CI 2\n").unwrap();
        let second = ci(temp.path(), "push", "refs/heads/main");
        assert_ne!(first.key_salt(), second.key_salt());
        assert_eq!(
            workflow_file(
                "owner/repo/.github/workflows/ci.yml@refs/heads/main",
                "owner/repo"
            )
            .as_deref(),
            Some(".github/workflows/ci.yml")
        );
        assert_eq!(
            workflow_file("owner/repo/../../etc/passwd@x", "owner/repo"),
            None
        );
    }
}
