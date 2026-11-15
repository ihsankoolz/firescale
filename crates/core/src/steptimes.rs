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

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

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
    /// step that is not slow.
    #[must_use]
    pub fn warning(&self) -> Option<String> {
        (self.duration > SLOW_STEP).then(|| {
            format!(
                "slow step: {}. To leave out steps this slow, run fscale verify --budget <time>",
                self.describe()
            )
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

    fn read(&self) -> File {
        if self.linked() {
            return File::default();
        }
        fs::read_to_string(self.path())
            .ok()
            .and_then(|text| serde_json::from_str::<File>(&text).ok())
            .filter(|file| file.format == FORMAT)
            .unwrap_or_default()
    }

    /// What is known about the time of this step, if anything.
    #[must_use]
    pub fn last(&self, project: Option<&str>, step: &str, command: &str) -> Option<Known> {
        self.read()
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
            })
    }

    /// Keeps how long a run of this step took, or has taken so far. The
    /// file is read again first, so another verify's records are kept, and
    /// written beside and renamed, so a reader never sees half a file.
    /// A file that cannot be written is not worth failing the run for.
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
        let mut file = self.read();
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
        let folder = self.folder();
        if fs::create_dir_all(&folder).is_err() {
            return;
        }
        ignore_firescale_folder(&self.root);
        let temporary = folder.join(format!(".{STEP_TIMES_FILE}.{}.tmp", std::process::id()));
        if is_symlink(&temporary) {
            return;
        }
        if fs::write(&temporary, format!("{text}\n")).is_err()
            || fs::rename(&temporary, self.path()).is_err()
        {
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
            Some(Known {
                duration: HOUR,
                finished: true
            })
        );
        assert_eq!(times.last(Some("web"), "test", "pytest"), None);
        assert_eq!(times.last(None, "test", "pytest"), None);
        assert_eq!(times.last(Some("api"), "lint", "pytest"), None);
        assert_eq!(times.last(Some("api"), "test", "pytest -x"), None);
    }

    #[test]
    fn a_pass_sets_the_time_and_anything_else_only_raises_it() {
        let (_temp, times) = times();
        let last = |times: &StepTimes| times.last(None, "test", "t").unwrap();
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
        assert_eq!(known(300, true).warning(), None);
        assert_eq!(
            known(100 * 60, true).warning().as_deref(),
            Some("slow step: it took 1h 40m when it last ran. To leave out steps this slow, run fscale verify --budget <time>")
        );
        assert_eq!(
            known(93 * 60, false).warning().as_deref(),
            Some("slow step: it ran at least 1h 33m when it last ran and did not finish. To leave out steps this slow, run fscale verify --budget <time>")
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
            assert_eq!(times.last(None, "test", "t"), None);
            times.record(None, "test", "t", HOUR, Outcome::Passed);
            assert!(times.last(None, "test", "t").is_some());
        }
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
        assert_eq!(times.last(None, "test", "t"), None);

        // `.firescale` is a link to a folder outside the repo.
        let (temp, times) = self::times();
        std::os::unix::fs::symlink(outside.path(), temp.path().join(".firescale")).unwrap();
        fs::remove_file(&target).unwrap();
        times.record(None, "test", "t", HOUR, Outcome::Passed);
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
        assert_eq!(times.last(None, "test", "t"), None);
    }
}
