//! Records of `fscale verify` runs in progress (ADR 0024).
//!
//! Verify takes no lock: a second run never waits for another one. Instead,
//! each run keeps a small record in the temp folder: its process id, when it
//! started, and the commands it is running with their process ids. A new
//! run reads the records of the same repo and says, on stderr:
//!
//! - when another verify is running there, which one and what it runs, since
//!   the two may compete for the same files, ports or test database;
//! - when an earlier verify ended without stopping its commands (it was
//!   killed outright) and they are still running, which ones and how to
//!   stop them.
//!
//! A record is removed when its run ends, and a stale record whose processes
//! are all gone is removed by the next run that finds it.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::process::{group_exists, process_exists, stop_command};
use crate::progress::format_duration;

/// A command a run has started and not yet seen end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunningStep {
    /// `test in backend/api`, or `lint` for a root step.
    pub place: String,
    pub command: String,
    /// The shell's process id, which on macOS and Linux is also the id of
    /// the command's process group.
    pub pid: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct State {
    pid: u32,
    /// Seconds since the Unix epoch.
    started: u64,
    root: String,
    running: Vec<RunningStep>,
}

/// This run's record. Removed when dropped.
pub struct RunRecord {
    path: PathBuf,
    state: Mutex<State>,
}

/// Records older than this are removed without a note.
const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// The folder that holds the records: `firescale-runs` in the temp folder.
#[must_use]
pub fn default_dir() -> PathBuf {
    std::env::temp_dir().join("firescale-runs")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// A short, stable name for a repo root, so records of different repos do
/// not mix.
fn repo_key(root: &Path) -> String {
    let root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let text = root.to_string_lossy().replace('\\', "/");
    let hash = blake3::hash(text.as_bytes()).to_hex();
    hash.as_str()[..16].to_owned()
}

/// What a new run should know about another run's record.
fn describe(other: &State, now: u64) -> Option<String> {
    let running: Vec<String> = other
        .running
        .iter()
        .map(|step| format!("{} ({})", step.place, step.command))
        .collect();
    if process_exists(other.pid) {
        let age = format_duration(Duration::from_secs(now.saturating_sub(other.started)));
        let doing = if running.is_empty() {
            String::new()
        } else {
            format!(", running {}", running.join(", "))
        };
        return Some(format!(
            "another fscale verify is running in this repo (process {}, started {age} ago{doing}). This run does not wait for it, but the two may compete for the same files, ports or test database.",
            other.pid
        ));
    }
    let left: Vec<&RunningStep> = other
        .running
        .iter()
        .filter(|step| group_exists(step.pid))
        .collect();
    if left.is_empty() {
        return None;
    }
    let names: Vec<String> = left
        .iter()
        .map(|step| format!("{} ({})", step.place, step.command))
        .collect();
    let stops: Vec<String> = left.iter().map(|step| stop_command(step.pid)).collect();
    Some(format!(
        "an earlier fscale verify (process {}) ended without stopping its commands, and they may still be running: {}. If they are, they can slow this run down or hold its files, ports or test database. Stop them with: {}",
        other.pid,
        names.join(", "),
        stops.join(" and ")
    ))
}

impl RunRecord {
    /// Starts this run's record in the default folder and returns what the
    /// run should say about other runs of the same repo.
    #[must_use]
    pub fn start(root: &Path) -> (Option<Self>, Vec<String>) {
        Self::start_in(&default_dir(), root, std::process::id())
    }

    /// [`RunRecord::start`] with the folder and this run's process id given.
    #[must_use]
    pub fn start_in(dir: &Path, root: &Path, pid: u32) -> (Option<Self>, Vec<String>) {
        let key = repo_key(root);
        let mut notes = Vec::new();
        let now = now();
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let Some(rest) = name
                    .strip_prefix(&format!("{key}-"))
                    .and_then(|rest| rest.strip_suffix(".json"))
                else {
                    continue;
                };
                if rest == pid.to_string() {
                    continue;
                }
                // A file that cannot be read now may be in the middle of a
                // write by its run: leave it. One that reads but does not
                // parse is broken: remove it.
                let Ok(text) = fs::read_to_string(entry.path()) else {
                    continue;
                };
                let Ok(other) = serde_json::from_str::<State>(&text) else {
                    let _ = fs::remove_file(entry.path());
                    continue;
                };
                // A record older than a day is from a run long gone, whose
                // process ids may since belong to other processes.
                if now.saturating_sub(other.started) > MAX_AGE.as_secs() {
                    let _ = fs::remove_file(entry.path());
                    continue;
                }
                match describe(&other, now) {
                    Some(note) => notes.push(note),
                    // Nothing of that run is left: forget it.
                    None => {
                        let _ = fs::remove_file(entry.path());
                    }
                }
            }
        }
        notes.sort();
        if fs::create_dir_all(dir).is_err() {
            return (None, notes);
        }
        let record = Self {
            path: dir.join(format!("{key}-{pid}.json")),
            state: Mutex::new(State {
                pid,
                started: now,
                root: root.to_string_lossy().replace('\\', "/"),
                running: Vec::new(),
            }),
        };
        record.save();
        (Some(record), notes)
    }

    fn save(&self) {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Ok(text) = serde_json::to_string(&*state) {
            // Written beside, then renamed, so a reader never sees half a file.
            let temporary = self.path.with_extension("json.tmp");
            if fs::write(&temporary, text).is_ok() && fs::rename(&temporary, &self.path).is_err() {
                let _ = fs::remove_file(&temporary);
            }
        }
    }

    /// Notes that a step's command started.
    pub fn started(&self, step: RunningStep) {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .running
            .push(step);
        self.save();
    }

    /// Notes that the command with this process id ended, with its tree.
    pub fn ended(&self, pid: u32) {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .running
            .retain(|step| step.pid != pid);
        self.save();
    }
}

impl Drop for RunRecord {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    /// The id of a process that has ended.
    fn ended_pid() -> u32 {
        let mut command = if cfg!(windows) {
            let mut command = Command::new("cmd");
            command.args(["/C", "exit 0"]);
            command
        } else {
            let mut command = Command::new("sh");
            command.args(["-c", "exit 0"]);
            command
        };
        let mut child = command.stdout(Stdio::null()).spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        pid
    }

    /// A process that runs for a minute, in its own group on Unix.
    fn long_running() -> std::process::Child {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            Command::new("sleep")
                .arg("60")
                .process_group(0)
                .spawn()
                .unwrap()
        }
        #[cfg(windows)]
        {
            Command::new("ping")
                .args(["-n", "60", "127.0.0.1"])
                .stdout(Stdio::null())
                .spawn()
                .unwrap()
        }
    }

    fn write_other(dir: &Path, root: &Path, state: &State) {
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join(format!("{}-{}.json", repo_key(root), state.pid)),
            serde_json::to_string(state).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn a_run_that_is_still_going_is_named_and_not_waited_for() {
        let temp = tempfile::tempdir().unwrap();
        let (dir, root) = (temp.path().join("runs"), temp.path().join("repo"));
        fs::create_dir_all(&root).unwrap();
        // This test process plays the other run: it is alive.
        write_other(
            &dir,
            &root,
            &State {
                pid: std::process::id(),
                started: now() - 125,
                root: String::new(),
                running: vec![RunningStep {
                    place: "test in backend/api".to_owned(),
                    command: "python -m pytest".to_owned(),
                    pid: 1,
                }],
            },
        );
        let started = std::time::Instant::now();
        let (record, notes) = RunRecord::start_in(&dir, &root, ended_pid());
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(record.is_some());
        assert_eq!(notes.len(), 1, "{notes:?}");
        let note = &notes[0];
        assert!(
            note.starts_with("another fscale verify is running in this repo"),
            "{note}"
        );
        assert!(
            note.contains(&format!("process {}", std::process::id())),
            "{note}"
        );
        assert!(note.contains("started 2m 05s ago"), "{note}");
        assert!(
            note.contains("test in backend/api (python -m pytest)"),
            "{note}"
        );
        assert!(note.contains("does not wait"), "{note}");
    }

    #[cfg(unix)]
    #[test]
    fn commands_left_by_a_killed_run_are_named_with_how_to_stop_them() {
        let temp = tempfile::tempdir().unwrap();
        let (dir, root) = (temp.path().join("runs"), temp.path().join("repo"));
        fs::create_dir_all(&root).unwrap();
        let mut leftover = long_running();
        let dead = ended_pid();
        write_other(
            &dir,
            &root,
            &State {
                pid: dead,
                started: now(),
                root: String::new(),
                running: vec![RunningStep {
                    place: "test in backend/api".to_owned(),
                    command: "python -m pytest".to_owned(),
                    pid: leftover.id(),
                }],
            },
        );
        let (_record, notes) = RunRecord::start_in(&dir, &root, std::process::id());
        let _ = leftover.kill();
        let _ = leftover.wait();
        assert_eq!(notes.len(), 1, "{notes:?}");
        let note = &notes[0];
        assert!(
            note.contains(&format!("process {dead}) ended without stopping")),
            "{note}"
        );
        assert!(
            note.contains("test in backend/api (python -m pytest)"),
            "{note}"
        );
        assert!(note.contains(&stop_command(leftover.id())), "{note}");
    }

    /// On Windows a killed run's job takes its commands with it, so a live
    /// process with a recorded id is another process: never named.
    #[cfg(windows)]
    #[test]
    fn on_windows_a_reused_process_id_is_never_named_for_stopping() {
        let temp = tempfile::tempdir().unwrap();
        let (dir, root) = (temp.path().join("runs"), temp.path().join("repo"));
        fs::create_dir_all(&root).unwrap();
        let mut other = long_running();
        write_other(
            &dir,
            &root,
            &State {
                pid: ended_pid(),
                started: now(),
                root: String::new(),
                running: vec![RunningStep {
                    place: "lint".to_owned(),
                    command: "eslint .".to_owned(),
                    pid: other.id(),
                }],
            },
        );
        let (_record, notes) = RunRecord::start_in(&dir, &root, std::process::id());
        let _ = other.kill();
        let _ = other.wait();
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn a_record_older_than_a_day_is_removed_quietly() {
        let temp = tempfile::tempdir().unwrap();
        let (dir, root) = (temp.path().join("runs"), temp.path().join("repo"));
        fs::create_dir_all(&root).unwrap();
        // Alive, but its record is two days old: the id was reused.
        write_other(
            &dir,
            &root,
            &State {
                pid: std::process::id(),
                started: now() - 2 * 24 * 60 * 60,
                root: String::new(),
                running: Vec::new(),
            },
        );
        let (_record, notes) = RunRecord::start_in(&dir, &root, ended_pid());
        assert!(notes.is_empty(), "{notes:?}");
        let old = dir.join(format!("{}-{}.json", repo_key(&root), std::process::id()));
        assert!(!old.exists());
    }

    #[test]
    fn a_stale_record_with_nothing_left_is_removed_quietly() {
        let temp = tempfile::tempdir().unwrap();
        let (dir, root) = (temp.path().join("runs"), temp.path().join("repo"));
        fs::create_dir_all(&root).unwrap();
        let dead = ended_pid();
        write_other(
            &dir,
            &root,
            &State {
                pid: dead,
                started: now(),
                root: String::new(),
                running: vec![RunningStep {
                    place: "lint".to_owned(),
                    command: "eslint .".to_owned(),
                    pid: ended_pid(),
                }],
            },
        );
        let (record, notes) = RunRecord::start_in(&dir, &root, std::process::id());
        assert!(notes.is_empty(), "{notes:?}");
        let stale = dir.join(format!("{}-{dead}.json", repo_key(&root)));
        assert!(!stale.exists());
        // Its own record is there while it runs, and gone when it ends.
        let own = dir.join(format!("{}-{}.json", repo_key(&root), std::process::id()));
        assert!(own.exists());
        drop(record);
        assert!(!own.exists());
    }

    #[test]
    fn records_of_another_repo_are_ignored() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("runs");
        let (here, there) = (temp.path().join("a"), temp.path().join("b"));
        fs::create_dir_all(&here).unwrap();
        fs::create_dir_all(&there).unwrap();
        write_other(
            &dir,
            &there,
            &State {
                pid: std::process::id(),
                started: now(),
                root: String::new(),
                running: Vec::new(),
            },
        );
        let (_record, notes) = RunRecord::start_in(&dir, &here, ended_pid());
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn the_record_lists_what_is_running() {
        let temp = tempfile::tempdir().unwrap();
        let (dir, root) = (temp.path().join("runs"), temp.path().join("repo"));
        fs::create_dir_all(&root).unwrap();
        let (record, _) = RunRecord::start_in(&dir, &root, 4242);
        let record = record.unwrap();
        record.started(RunningStep {
            place: "lint".to_owned(),
            command: "eslint .".to_owned(),
            pid: 77,
        });
        let path = dir.join(format!("{}-4242.json", repo_key(&root)));
        let state: State = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(state.running.len(), 1);
        assert_eq!(state.running[0].pid, 77);
        record.ended(77);
        let state: State = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(state.running.is_empty());
    }
}
