//! How long each verify step took the last time it ran (ADR 0057).
//!
//! Kept in `.firescale/step-times.json`, one record per step. A step is
//! known by its project, its name and its command, never by the content of
//! the files it reads: the time must be found again after the files change,
//! which is when the step runs again. (The step cache, ADR 0036, is keyed by
//! content and only keeps passes, so it cannot answer this.)
//!
//! Verify reads a record before it starts a step: to warn when the step is
//! slow, and to leave it out when `--budget` is smaller than its time. It
//! writes one when a step ends, and while a long step runs, so a run that is
//! stopped, timed out or killed still leaves how long the step ran at least.
//!
//! Nothing is read or written through a symbolic link, as for the step
//! cache (ADR 0056, bug 17).

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

use crate::progress::format_duration;
use crate::stepcache::{ignore_firescale_folder, now_secs};

/// The format of the file. A file with another format is not read, and is
/// replaced by the next write.
const FORMAT: u32 = 1;

/// The file's name in `.firescale`.
pub const STEP_TIMES_FILE: &str = "step-times.json";

/// A step that took longer than this the last time is announced before it
/// starts.
pub const SLOW_STEP: Duration = Duration::from_secs(5 * 60);

/// At most this many records are kept; the ones that ran longest ago go
/// first. A repo has a few steps per project, so this is only a bound.
const MAX_RECORDS: usize = 2000;

/// How a run of a step ended, for its record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// It ran to its end and passed: this is how long the step takes.
    Passed,
    /// It ran to its end and did not pass. A failure is often quicker than
    /// a whole run, so it never lowers the time that is kept.
    Failed,
    /// It was stopped before its end (verify was interrupted, a time limit,
    /// or it is still running): it takes at least this long.
    Unfinished,
}

/// What is known about a step's time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Known {
    pub duration: Duration,
    /// False when the run this comes from did not finish: the step takes at
    /// least `duration`.
    pub finished: bool,
}

impl Known {
    /// `it took 1h 40m when it last ran`, or `it ran at least 1h 33m when it
    /// last ran and did not finish`.
    #[must_use]
    pub fn describe(&self) -> String {
        let time = format_duration(self.duration);
        if self.finished {
            format!("it took {time} when it last ran")
        } else {
            format!("it ran at least {time} when it last ran and did not finish")
        }
    }

    /// The line verify prints before it starts a slow step, or `None` for a
    /// step that is not slow. A run that already has a `--budget` is not
    /// told to use one.
    #[must_use]
    pub fn warning(&self, budget_in_use: bool) -> Option<String> {
        (self.duration > SLOW_STEP).then(|| {
            let said = format!("slow step: {}", self.describe());
            if budget_in_use {
                said
            } else {
                format!("{said}. To leave out steps this slow, run fscale verify --budget <time>")
            }
        })
    }

    /// The time in milliseconds, for JSON.
    #[must_use]
    pub fn millis(&self) -> u64 {
        u64::try_from(self.duration.as_millis()).unwrap_or(u64::MAX)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct Record {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project: Option<String>,
    step: String,
    command: String,
    duration_ms: u64,
    finished: bool,
    /// When the run started that this comes from, in seconds since 1970.
    ran_at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct File {
    format: u32,
    steps: Vec<Record>,
}

/// The kept times of one repo.
pub struct StepTimes {
    root: PathBuf,
    /// One writer at a time in this process: projects run side by side.
    lock: Mutex<()>,
}

/// The file is there, but could not be read just now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unreadable {
    reason: String,
}

impl fmt::Display for Unreadable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "could not read .firescale/{STEP_TIMES_FILE} ({})",
            self.reason
        )
    }
}

impl std::error::Error for Unreadable {}

/// A read is tried this many times, this far apart, before the file counts
/// as unreadable.
const READ_TRIES: u32 = 5;
const READ_PAUSE: Duration = Duration::from_millis(20);

/// How often, and how far apart, the rename over the file is tried: for a
/// write made while a step runs (80 ms in all), and for the record of a
/// step that ended (about a second in all).
const RENAME_RUNNING: (u32, Duration) = (5, Duration::from_millis(20));
const RENAME_ENDED: (u32, Duration) = (21, Duration::from_millis(50));

/// Runs `once` until it succeeds, at most `tries` times, `pause` apart, and
/// returns the last error when none did.
fn retrying<T>(
    tries: u32,
    pause: Duration,
    mut once: impl FnMut() -> io::Result<T>,
) -> io::Result<T> {
    let mut attempt = 1;
    loop {
        match once() {
            Err(_) if attempt < tries => {
                attempt += 1;
                std::thread::sleep(pause);
            }
            result => return result,
        }
    }
}

/// The lock file's name in `.firescale`: whoever created it is the one
/// writer of the step times file, in any process.
const LOCK_FILE: &str = "step-times.json.lock";

/// How long a write waits for the lock: one made while a step runs gives up
/// soon (the next heartbeat writes again); one made when a step ended waits
/// until a lock left by a killed process has become stale.
const LOCK_WAIT_RUNNING: Duration = Duration::from_millis(100);
const LOCK_WAIT_ENDED: Duration = Duration::from_secs(7);
const LOCK_PAUSE: Duration = Duration::from_millis(2);

/// A lock this old was left by a process that was killed while it held it:
/// a writer holds it for a second at most. It is removed, so it cannot block
/// the file for good.
const LOCK_STALE: Duration = Duration::from_secs(5);

/// On Windows, creating the lock is refused ("Access is denied") for a
/// moment while the one before it is being deleted. Refused for this long,
/// the folder really cannot be written.
const LOCK_REFUSED: Duration = Duration::from_secs(1);

/// The lock file, held: created with `create_new`, which fails when the
/// name is taken, also by a symbolic link, so nothing is written through
/// one. Removed when dropped.
struct FileLock {
    path: PathBuf,
    /// What this holder wrote in the file, to know its own lock again.
    token: String,
}

impl FileLock {
    /// Takes the lock, waiting up to `patience`. `None` when another writer
    /// kept it for that long, or when the lock file cannot be created.
    fn acquire(path: &Path, patience: Duration) -> Option<Self> {
        use std::io::Write;
        let started = Instant::now();
        let token = format!(
            "{} {}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |since| since.as_nanos())
        );
        let mut refused_since: Option<Instant> = None;
        loop {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
            {
                Ok(mut file) => {
                    if file.write_all(token.as_bytes()).is_err() {
                        drop(file);
                        let _ = fs::remove_file(path);
                        return None;
                    }
                    return Some(Self {
                        path: path.to_path_buf(),
                        token,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    refused_since = None;
                    if lock_is_stale(path) {
                        let _ = fs::remove_file(path);
                    }
                }
                Err(error) if cfg!(windows) && error.kind() == io::ErrorKind::PermissionDenied => {
                    let since = *refused_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= LOCK_REFUSED {
                        return None;
                    }
                }
                Err(_) => return None,
            }
            if started.elapsed() >= patience {
                return None;
            }
            std::thread::sleep(LOCK_PAUSE);
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        // Only its own: after a very long pause another writer may have
        // taken this lock for a stale one.
        // Both are tried a few times: on Windows a file that another
        // program looks at for a moment cannot be opened or deleted.
        let own = retrying(READ_TRIES, LOCK_PAUSE, || {
            match fs::read_to_string(&self.path) {
                Ok(text) => Ok(text == self.token),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
                Err(error) => Err(error),
            }
        });
        if own.unwrap_or(false) {
            let _ = retrying(READ_TRIES, LOCK_PAUSE, || fs::remove_file(&self.path));
        }
    }
}

/// True when the lock file was last written longer ago than a writer ever
/// holds it (or its time is that far in the future: the clock was changed).
fn lock_is_stale(path: &Path) -> bool {
    let Ok(modified) = fs::symlink_metadata(path).and_then(|meta| meta.modified()) else {
        return false;
    };
    let now = SystemTime::now();
    now.duration_since(modified)
        .or_else(|_| modified.duration_since(now))
        .is_ok_and(|apart| apart > LOCK_STALE)
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

/// What to keep when `new` is reported for a step that has `old`.
fn merge(old: Option<&Record>, new: Record, outcome: Outcome) -> Option<Record> {
    match (old, outcome) {
        // A pass is the truth about the step, shorter or longer.
        (_, Outcome::Passed) | (None, _) => Some(new),
        // Anything else only ever raises what is known.
        (Some(old), _) if new.duration_ms > old.duration_ms => Some(new),
        (Some(_), _) => None,
    }
}

impl StepTimes {
    /// The times kept for the repo at `root`.
    #[must_use]
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            lock: Mutex::new(()),
        }
    }

    fn folder(&self) -> PathBuf {
        self.root.join(".firescale")
    }

    fn path(&self) -> PathBuf {
        self.folder().join(STEP_TIMES_FILE)
    }

    /// True when `.firescale` or the file is a symbolic link: then nothing
    /// is read and nothing is written.
    fn linked(&self) -> bool {
        is_symlink(&self.folder()) || is_symlink(&self.path())
    }

    /// The file as it is now. Three cases are told apart (ADR 0057):
    ///
    /// - there is no file: nothing is kept yet;
    /// - the file is there but is not a step times file of this format
    ///   (broken, or written by another version): it holds nothing that can
    ///   be used, and the next write replaces it, since nothing else would
    ///   repair it;
    /// - the file is there and cannot be read right now: an error. On
    ///   Windows a reader gets "Access is denied" for a moment while another
    ///   verify renames a new file over this one, so the read is tried a few
    ///   times first. An empty file must never stand for this case: a write
    ///   that started from it would drop every other step.
    fn read(&self) -> Result<File, Unreadable> {
        if self.linked() {
            return Ok(File::default());
        }
        let path = self.path();
        retrying(READ_TRIES, READ_PAUSE, || match fs::read(&path) {
            Ok(bytes) => Ok(serde_json::from_slice::<File>(&bytes)
                .ok()
                .filter(|file| file.format == FORMAT)
                .unwrap_or_default()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(File::default()),
            Err(error) => Err(error),
        })
        .map_err(|error| Unreadable {
            reason: error.to_string(),
        })
    }

    /// What is known about the time of this step, if anything.
    ///
    /// # Errors
    ///
    /// When the file is there but could not be read, after a few tries: the
    /// step may have a kept time that this run cannot see.
    pub fn last(
        &self,
        project: Option<&str>,
        step: &str,
        command: &str,
    ) -> Result<Option<Known>, Unreadable> {
        Ok(self
            .read()?
            .steps
            .iter()
            .find(|record| {
                record.project.as_deref() == project
                    && record.step == step
                    && record.command == command
            })
            .map(|record| Known {
                duration: Duration::from_millis(record.duration_ms),
                finished: record.finished,
            }))
    }

    /// Keeps how long a run of this step took, or has taken so far.
    ///
    /// The file is read again first, so another verify's records are kept,
    /// and written beside and renamed, so a reader never sees half a file.
    /// A lock file is held from the read to the rename, so two verify runs
    /// of one repo do not undo each other's writes.
    ///
    /// A file that cannot be written is not worth failing the run for, and
    /// one that cannot be read right now is left as it is: this record is
    /// dropped, not the other steps'. The record of a step that ended
    /// waits longer for the lock than one written while the step runs: the
    /// next heartbeat writes that one again.
    pub fn record(
        &self,
        project: Option<&str>,
        step: &str,
        command: &str,
        duration: Duration,
        outcome: Outcome,
    ) {
        let _guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        if self.linked() {
            return;
        }
        let duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
        let new = Record {
            project: project.map(str::to_owned),
            step: step.to_owned(),
            command: command.to_owned(),
            duration_ms,
            finished: outcome != Outcome::Unfinished,
            ran_at: now_secs().saturating_sub(duration.as_secs()),
        };
        let folder = self.folder();
        if fs::create_dir_all(&folder).is_err() {
            return;
        }
        ignore_firescale_folder(&self.root);
        let patience = if outcome == Outcome::Unfinished {
            LOCK_WAIT_RUNNING
        } else {
            LOCK_WAIT_ENDED
        };
        let Some(_lock) = FileLock::acquire(&folder.join(LOCK_FILE), patience) else {
            return;
        };
        let Ok(mut file) = self.read() else {
            return;
        };
        file.format = FORMAT;
        let at = file.steps.iter().position(|record| {
            record.project.as_deref() == project && record.step == step && record.command == command
        });
        let Some(kept) = merge(at.map(|index| &file.steps[index]), new, outcome) else {
            return;
        };
        match at {
            Some(index) => file.steps[index] = kept,
            None => file.steps.push(kept),
        }
        if file.steps.len() > MAX_RECORDS {
            file.steps.sort_by_key(|record| record.ran_at);
            let extra = file.steps.len() - MAX_RECORDS;
            file.steps.drain(..extra);
        }
        file.steps.sort_by(|a, b| {
            (&a.project, &a.step, &a.command).cmp(&(&b.project, &b.step, &b.command))
        });
        let Ok(text) = serde_json::to_string_pretty(&file) else {
            return;
        };
        let temporary = folder.join(format!(".{STEP_TIMES_FILE}.{}.tmp", std::process::id()));
        if is_symlink(&temporary) {
            return;
        }
        if fs::write(&temporary, format!("{text}\n")).is_err() {
            let _ = fs::remove_file(&temporary);
            return;
        }
        // On Windows a rename fails while another program (a virus scanner,
        // an indexer, an editor) holds the file open: try again for a
        // while. A step that ended is worth about a second: a pass that is
        // dropped leaves the old time in place. A write made while the step
        // runs is not: the next heartbeat makes it again.
        let (tries, pause) = if outcome == Outcome::Unfinished {
            RENAME_RUNNING
        } else {
            RENAME_ENDED
        };
        let path = self.path();
        if retrying(tries, pause, || fs::rename(&temporary, &path)).is_err() {
            let _ = fs::remove_file(&temporary);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: Duration = Duration::from_secs(3600);

    fn times() -> (tempfile::TempDir, StepTimes) {
        let temp = tempfile::tempdir().unwrap();
        let times = StepTimes::new(temp.path());
        (temp, times)
    }

    #[test]
    fn a_step_is_known_by_project_name_and_command() {
        let (_temp, times) = times();
        times.record(Some("api"), "test", "pytest", HOUR, Outcome::Passed);
        assert_eq!(
            times.last(Some("api"), "test", "pytest"),
            Ok(Some(Known {
                duration: HOUR,
                finished: true
            }))
        );
        assert_eq!(times.last(Some("web"), "test", "pytest"), Ok(None));
        assert_eq!(times.last(None, "test", "pytest"), Ok(None));
        assert_eq!(times.last(Some("api"), "lint", "pytest"), Ok(None));
        assert_eq!(times.last(Some("api"), "test", "pytest -x"), Ok(None));
    }

    #[test]
    fn a_pass_sets_the_time_and_anything_else_only_raises_it() {
        let (_temp, times) = times();
        let last = |times: &StepTimes| times.last(None, "test", "t").unwrap().unwrap();
        times.record(None, "test", "t", HOUR, Outcome::Passed);
        // A quick failure, or a run stopped early, says nothing new.
        times.record(None, "test", "t", Duration::from_secs(3), Outcome::Failed);
        times.record(
            None,
            "test",
            "t",
            Duration::from_secs(90),
            Outcome::Unfinished,
        );
        assert_eq!(
            last(&times),
            Known {
                duration: HOUR,
                finished: true
            }
        );
        // A run stopped after longer than that: at least this long.
        times.record(None, "test", "t", HOUR * 2, Outcome::Unfinished);
        assert_eq!(
            last(&times),
            Known {
                duration: HOUR * 2,
                finished: false
            }
        );
        // A failure that ran longer still is a finished time.
        times.record(None, "test", "t", HOUR * 3, Outcome::Failed);
        assert_eq!(
            last(&times),
            Known {
                duration: HOUR * 3,
                finished: true
            }
        );
        // A pass is the truth, also when it is quicker.
        times.record(None, "test", "t", Duration::from_secs(40), Outcome::Passed);
        assert_eq!(
            last(&times),
            Known {
                duration: Duration::from_secs(40),
                finished: true
            }
        );
    }

    #[test]
    fn the_warning_is_for_steps_over_five_minutes_and_says_what_is_known() {
        let known = |seconds: u64, finished: bool| Known {
            duration: Duration::from_secs(seconds),
            finished,
        };
        assert_eq!(known(300, true).warning(false), None);
        assert_eq!(known(300, true).warning(true), None);
        assert_eq!(
            known(100 * 60, true).warning(false).as_deref(),
            Some("slow step: it took 1h 40m when it last ran. To leave out steps this slow, run fscale verify --budget <time>")
        );
        assert_eq!(
            known(93 * 60, false).warning(false).as_deref(),
            Some("slow step: it ran at least 1h 33m when it last ran and did not finish. To leave out steps this slow, run fscale verify --budget <time>")
        );
        // A run that has a budget is not told to use one.
        assert_eq!(
            known(100 * 60, true).warning(true).as_deref(),
            Some("slow step: it took 1h 40m when it last ran")
        );
    }

    #[test]
    fn the_file_is_plain_json_with_forward_slashes_and_survives_another_writer() {
        let (temp, times) = times();
        times.record(
            Some("backend/api"),
            "test",
            "pytest",
            Duration::from_millis(1500),
            Outcome::Passed,
        );
        // Another verify of the same repo writes its own record.
        StepTimes::new(temp.path()).record(None, "lint", "eslint .", HOUR, Outcome::Unfinished);
        times.record(
            None,
            "typecheck",
            "tsc",
            Duration::from_secs(2),
            Outcome::Passed,
        );
        let text =
            fs::read_to_string(temp.path().join(".firescale").join(STEP_TIMES_FILE)).unwrap();
        let file: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(file["format"], 1);
        let steps = file["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[2]["project"], "backend/api");
        assert_eq!(steps[2]["durationMs"], 1500);
        assert_eq!(steps[2]["finished"], true);
        assert_eq!(steps[0]["step"], "lint");
        assert_eq!(steps[0]["finished"], false);
        assert!(steps[0].get("project").is_none());
        // firescale's own files never show up as changes in git.
        assert!(temp.path().join(".firescale").join(".gitignore").exists());
        assert!(fs::read_dir(temp.path().join(".firescale"))
            .unwrap()
            .flatten()
            .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp")));
    }

    #[test]
    fn a_file_that_is_broken_or_of_another_format_reads_as_nothing_and_is_replaced() {
        let (temp, times) = times();
        let folder = temp.path().join(".firescale");
        fs::create_dir_all(&folder).unwrap();
        for text in ["not json", r#"{"format": 99, "steps": []}"#] {
            fs::write(folder.join(STEP_TIMES_FILE), text).unwrap();
            assert_eq!(times.last(None, "test", "t"), Ok(None));
            times.record(None, "test", "t", HOUR, Outcome::Passed);
            assert!(times.last(None, "test", "t").unwrap().is_some());
        }
    }

    /// Another program has the file open and does not share reading, as a
    /// reader on Windows also sees for a moment ("Access is denied") while
    /// another verify renames a new file over it.
    #[cfg(windows)]
    fn held_without_sharing_read(path: &Path) -> fs::File {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_DELETE only: nobody else can read it, but it can be
        // replaced.
        fs::OpenOptions::new()
            .read(true)
            .share_mode(4)
            .open(path)
            .unwrap()
    }

    #[cfg(windows)]
    #[test]
    fn a_file_that_cannot_be_read_right_now_does_not_lose_the_other_steps_times() {
        let (temp, times) = times();
        times.record(
            Some("backend/slow"),
            "test",
            "pytest",
            HOUR,
            Outcome::Passed,
        );
        let held = held_without_sharing_read(&temp.path().join(".firescale").join(STEP_TIMES_FILE));
        // A reader is told so; it does not take the step for one with no
        // kept time.
        let unreadable = times
            .last(Some("backend/slow"), "test", "pytest")
            .unwrap_err();
        assert!(
            unreadable
                .to_string()
                .starts_with("could not read .firescale/step-times.json ("),
            "{unreadable}"
        );
        times.record(
            None,
            "lint",
            "eslint .",
            Duration::from_secs(2),
            Outcome::Passed,
        );
        drop(held);
        assert_eq!(
            times.last(Some("backend/slow"), "test", "pytest"),
            Ok(Some(Known {
                duration: HOUR,
                finished: true
            })),
            "the kept time of backend/slow is gone"
        );
        // The record that could not be merged was dropped, not written alone.
        assert_eq!(times.last(None, "lint", "eslint ."), Ok(None));
    }

    /// The same rule on every platform: a folder where the file should be
    /// cannot be read as a file, and is not "no file".
    #[test]
    fn something_unreadable_in_the_files_place_is_an_error_and_is_left_alone() {
        let (temp, times) = times();
        let path = temp.path().join(".firescale").join(STEP_TIMES_FILE);
        fs::create_dir_all(&path).unwrap();
        assert!(times.last(None, "test", "t").is_err());
        times.record(None, "test", "t", HOUR, Outcome::Passed);
        assert!(path.is_dir());
        // No half-written file and no lock are left beside it.
        assert!(
            fs::read_dir(temp.path().join(".firescale"))
                .unwrap()
                .flatten()
                .all(|entry| entry.file_name() == STEP_TIMES_FILE
                    || entry.file_name() == ".gitignore")
        );
    }

    #[test]
    fn a_read_is_tried_again_and_gives_up_with_the_last_error() {
        let denied = || io::Error::new(io::ErrorKind::PermissionDenied, "Access is denied");
        // It fails twice, as while another verify renames the file.
        let mut calls = 0;
        let read = retrying(5, Duration::ZERO, || {
            calls += 1;
            if calls < 3 {
                Err(denied())
            } else {
                Ok("read")
            }
        });
        assert_eq!((read.ok(), calls), (Some("read"), 3));
        // It never works: five tries, then the error.
        let mut calls = 0;
        let read: io::Result<()> = retrying(5, Duration::ZERO, || {
            calls += 1;
            Err(denied())
        });
        assert_eq!(
            (read.map_err(|error| error.kind()), calls),
            (Err(io::ErrorKind::PermissionDenied), 5)
        );
    }

    /// The repo and the step of a writer process of the test below. Set
    /// only for the processes that test starts.
    const WRITER_ROOT: &str = "FSCALE_TEST_STEP_TIMES_ROOT";
    const WRITER_STEP: &str = "FSCALE_TEST_STEP_TIMES_STEP";
    const WRITERS: usize = 4;
    const WRITES: u64 = 100;

    /// One verify among several: it keeps the time of its own step, over
    /// and over. Does nothing in a normal test run.
    #[test]
    fn writer_process() {
        let (Ok(root), Ok(step)) = (std::env::var(WRITER_ROOT), std::env::var(WRITER_STEP)) else {
            return;
        };
        let times = StepTimes::new(Path::new(&root));
        for millis in 1..=WRITES {
            times.record(
                None,
                &step,
                "cmd",
                Duration::from_millis(millis),
                Outcome::Passed,
            );
        }
    }

    #[test]
    fn verify_runs_at_the_same_time_keep_each_others_records() {
        if std::env::var(WRITER_ROOT).is_ok() {
            return;
        }
        let (temp, times) = times();
        // A step none of them runs: its time must still be there at the end.
        times.record(
            Some("backend/slow"),
            "test",
            "pytest",
            HOUR,
            Outcome::Passed,
        );
        let writers: Vec<std::process::Child> = (0..WRITERS)
            .map(|n| {
                std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "steptimes::tests::writer_process"])
                    .env(WRITER_ROOT, temp.path())
                    .env(WRITER_STEP, format!("step{n}"))
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .unwrap()
            })
            .collect();
        for mut writer in writers {
            assert!(writer.wait().unwrap().success());
        }
        // Every process's last write is in the file.
        let kept: Vec<Option<u128>> = (0..WRITERS)
            .map(|n| {
                times
                    .last(None, &format!("step{n}"), "cmd")
                    .unwrap()
                    .map(|known| known.duration.as_millis())
            })
            .collect();
        assert_eq!(kept, vec![Some(u128::from(WRITES)); WRITERS]);
        assert_eq!(
            times.last(Some("backend/slow"), "test", "pytest"),
            Ok(Some(Known {
                duration: HOUR,
                finished: true
            }))
        );
        // Nothing is left beside the file: no lock, no half-written file.
        let mut left: Vec<String> = fs::read_dir(temp.path().join(".firescale"))
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, [".gitignore", STEP_TIMES_FILE]);
    }

    #[test]
    fn a_held_lock_drops_a_write_made_while_a_step_runs_and_a_stale_one_is_taken() {
        let (temp, times) = times();
        let folder = temp.path().join(".firescale");
        fs::create_dir_all(&folder).unwrap();
        let lock = folder.join(LOCK_FILE);
        // Another verify is writing right now.
        fs::write(&lock, "another verify").unwrap();
        let started = Instant::now();
        times.record(None, "test", "t", HOUR, Outcome::Unfinished);
        assert!(started.elapsed() < LOCK_STALE);
        assert_eq!(times.last(None, "test", "t"), Ok(None));
        assert_eq!(fs::read_to_string(&lock).unwrap(), "another verify");
        // The lock of a verify that was killed while it wrote: old, so it
        // is removed, and the write is made.
        let long_ago = SystemTime::now() - LOCK_STALE - Duration::from_secs(1);
        fs::File::options()
            .write(true)
            .open(&lock)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
        times.record(None, "test", "t", HOUR, Outcome::Unfinished);
        assert_eq!(
            times.last(None, "test", "t"),
            Ok(Some(Known {
                duration: HOUR,
                finished: false
            }))
        );
        assert!(!lock.exists());
    }

    /// Another program (a virus scanner, an editor, Python's `open()`) has
    /// the file open without sharing delete for 300 ms, so nothing can be
    /// renamed over it for that long.
    #[cfg(windows)]
    #[test]
    fn a_pass_is_kept_when_another_program_holds_the_file_for_a_moment() {
        use std::os::windows::fs::OpenOptionsExt;
        let (temp, times) = times();
        times.record(None, "test", "t", HOUR, Outcome::Passed);
        // FILE_SHARE_READ | FILE_SHARE_WRITE, but not FILE_SHARE_DELETE.
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(1 | 2)
            .open(temp.path().join(".firescale").join(STEP_TIMES_FILE))
            .unwrap();
        let holder = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(held);
        });
        // The step now passes in 40 s: the truth about it.
        times.record(None, "test", "t", Duration::from_secs(40), Outcome::Passed);
        holder.join().unwrap();
        assert_eq!(
            times.last(None, "test", "t"),
            Ok(Some(Known {
                duration: Duration::from_secs(40),
                finished: true
            }))
        );
    }

    /// On Windows a folder link that needs no admin rights is a junction.
    #[cfg(windows)]
    #[test]
    fn nothing_is_read_or_written_through_a_junction() {
        let (temp, times) = times();
        let outside = tempfile::tempdir().unwrap();
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(temp.path().join(".firescale"))
            .arg(outside.path())
            .output()
            .unwrap();
        assert!(
            made.status.success(),
            "{}",
            String::from_utf8_lossy(&made.stderr)
        );
        times.record(None, "test", "t", HOUR, Outcome::Passed);
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
        fs::write(
            outside.path().join(STEP_TIMES_FILE),
            r#"{"format":1,"steps":[{"step":"test","command":"t","durationMs":3600000,"finished":true,"ranAt":1}]}"#,
        )
        .unwrap();
        // The link works (the file is there through it), and is not followed.
        assert!(temp
            .path()
            .join(".firescale")
            .join(STEP_TIMES_FILE)
            .exists());
        assert_eq!(times.last(None, "test", "t"), Ok(None));
    }

    #[cfg(unix)]
    #[test]
    fn the_lock_is_not_written_through_a_symbolic_link() {
        let (temp, times) = times();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("lock-target");
        let folder = temp.path().join(".firescale");
        fs::create_dir_all(&folder).unwrap();
        std::os::unix::fs::symlink(&target, folder.join(LOCK_FILE)).unwrap();
        times.record(None, "test", "t", HOUR, Outcome::Unfinished);
        assert!(!target.exists());
        assert_eq!(times.last(None, "test", "t"), Ok(None));
    }

    #[cfg(unix)]
    #[test]
    fn nothing_is_read_or_written_through_a_symbolic_link() {
        // The file itself is a link to a file outside the repo.
        let (temp, times) = times();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("target.json");
        fs::create_dir_all(temp.path().join(".firescale")).unwrap();
        std::os::unix::fs::symlink(
            &target,
            temp.path().join(".firescale").join(STEP_TIMES_FILE),
        )
        .unwrap();
        times.record(None, "test", "t", HOUR, Outcome::Passed);
        assert!(!target.exists());
        fs::write(
            &target,
            r#"{"format":1,"steps":[{"step":"test","command":"t","durationMs":3600000,"finished":true,"ranAt":1}]}"#,
        )
        .unwrap();
        assert_eq!(times.last(None, "test", "t"), Ok(None));

        // `.firescale` is a link to a folder outside the repo.
        let (temp, times) = self::times();
        std::os::unix::fs::symlink(outside.path(), temp.path().join(".firescale")).unwrap();
        fs::remove_file(&target).unwrap();
        times.record(None, "test", "t", HOUR, Outcome::Passed);
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
        assert_eq!(times.last(None, "test", "t"), Ok(None));
    }
}
