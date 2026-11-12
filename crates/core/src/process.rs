//! Running one verify command as a process tree that can be stopped as a
//! whole (ADR 0024).
//!
//! A step's command is started through the platform shell, and the shell
//! starts the real tools, which start their own children (test workers,
//! language servers, database fixtures). Stopping only the shell leaves the
//! rest running, so every command runs in a group that is stopped together:
//!
//! - On macOS and Linux the shell is the leader of a new process group. To
//!   stop it, the whole group gets `SIGTERM`, then `SIGKILL` after a short
//!   grace period.
//! - On Windows the shell is started suspended, put in a job object that
//!   kills every process in it when its last handle closes, and only then
//!   resumed, so nothing it starts can escape before it is in the job. To
//!   stop it, the job is terminated. If fscale itself dies, Windows closes
//!   the handle and the job's processes die with it.
//!
//! When the command ends by itself, whatever it left running in its group is
//! stopped too, and the wait for its output is bounded: a background process
//! that holds the output pipe open can no longer make verify wait forever.
//!
//! Ctrl+C and termination signals only set a flag ([`STOP`]); the loop that
//! watches the running command sees it within a few milliseconds and stops
//! the tree, so the summary of what finished can still be printed.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::verify::strip_ansi;

/// Set once Ctrl+C or a termination signal arrives, after
/// [`install_stop_handler`] ran.
pub static STOP: AtomicBool = AtomicBool::new(false);

/// How often the watch loop looks at the child, the stop flag and the clock.
const POLL: Duration = Duration::from_millis(20);

/// How long a stopped tree gets to exit after `SIGTERM` before `SIGKILL`.
#[cfg(unix)]
const GRACE: Duration = Duration::from_secs(2);

/// How long to wait for the output pipes to close once the tree is gone. A
/// process that left the group (a daemon) may hold them open for ever.
const DRAIN: Duration = Duration::from_secs(2);

/// At most this much output is kept per stream; older output is dropped.
const MAX_OUTPUT: usize = 16 * 1024 * 1024;

/// How a command ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// It exited by itself, with this exit code (`None`: killed by a signal).
    Exited(Option<i32>),
    /// It ran past its time limit and was stopped, with its whole tree.
    TimedOut,
    /// Verify was asked to stop (Ctrl+C or a termination signal), so the
    /// command was stopped, with its whole tree.
    Stopped,
}

/// A finished command: how it ended and what it printed.
#[derive(Debug, Clone)]
pub struct Finished {
    pub ending: Ending,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub duration: Duration,
}

/// What to watch for while a command runs.
pub struct Watch<'a> {
    /// Stop the tree once this much time has passed. `None`: no limit.
    pub timeout: Option<Duration>,
    /// Stop the tree once this is set.
    pub stop: &'a AtomicBool,
    /// How often to call `on_heartbeat` while the command runs.
    pub heartbeat: Duration,
    /// Called every `heartbeat` with the time so far and the last line of
    /// output, if any.
    pub on_heartbeat: &'a (dyn Fn(Duration, Option<&str>) + Sync),
    /// Called once the command started, with its process id (on macOS and
    /// Linux also the id of its process group).
    pub on_spawn: &'a (dyn Fn(u32) + Sync),
}

/// Output collected by a reader thread, shared with the watch loop.
#[derive(Default)]
struct Collected {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    last_line: Option<String>,
}

fn lock(collected: &Mutex<Collected>) -> std::sync::MutexGuard<'_, Collected> {
    collected.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The last non-empty line of a chunk of output, without colour codes.
fn last_line(chunk: &[u8]) -> Option<String> {
    let text = strip_ansi(&String::from_utf8_lossy(chunk));
    text.lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(|line| line.chars().take(160).collect())
}

/// Reads one stream to its end into `collected`, then reports on `done`.
fn spawn_reader(
    mut stream: impl Read + Send + 'static,
    collected: Arc<Mutex<Collected>>,
    is_stderr: bool,
    done: mpsc::Sender<()>,
) {
    std::thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    let chunk = &buffer[..read];
                    let mut collected = lock(&collected);
                    let target = if is_stderr {
                        &mut collected.stderr
                    } else {
                        &mut collected.stdout
                    };
                    target.extend_from_slice(chunk);
                    if target.len() > MAX_OUTPUT {
                        let excess = target.len() - MAX_OUTPUT / 2;
                        target.drain(..excess);
                    }
                    if let Some(line) = last_line(chunk) {
                        collected.last_line = Some(line);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        let _ = done.send(());
    });
}

/// Runs `command` with no input and its output captured, as a tree that is
/// stopped as a whole on a timeout or a stop request. Returns an error only
/// when the command could not be started.
pub fn run_tree(mut command: Command, watch: &Watch<'_>) -> std::io::Result<Finished> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let started = Instant::now();
    let mut tree = Tree::spawn(command)?;
    (watch.on_spawn)(tree.child.id());
    let collected = Arc::new(Mutex::new(Collected::default()));
    let (done, finished_readers) = mpsc::channel();
    let mut readers = 0;
    if let Some(stdout) = tree.child.stdout.take() {
        spawn_reader(stdout, Arc::clone(&collected), false, done.clone());
        readers += 1;
    }
    if let Some(stderr) = tree.child.stderr.take() {
        spawn_reader(stderr, Arc::clone(&collected), true, done.clone());
        readers += 1;
    }
    drop(done);

    let mut next_heartbeat = watch.heartbeat;
    // The time is taken when the ending is known, before any stopping.
    let (ending, duration) = loop {
        let elapsed = started.elapsed();
        match tree.child.try_wait() {
            Ok(Some(status)) => break (Ending::Exited(status.code()), elapsed),
            Ok(None) => {}
            // The child cannot be waited on: stop it, and call it stopped.
            Err(_) => {
                tree.stop();
                break (Ending::Stopped, elapsed);
            }
        }
        if watch.stop.load(Ordering::SeqCst) {
            tree.stop();
            break (Ending::Stopped, elapsed);
        }
        if watch.timeout.is_some_and(|limit| elapsed >= limit) {
            tree.stop();
            break (Ending::TimedOut, elapsed);
        }
        if !watch.heartbeat.is_zero() && elapsed >= next_heartbeat {
            let line = lock(&collected).last_line.clone();
            (watch.on_heartbeat)(elapsed, line.as_deref());
            while next_heartbeat <= elapsed {
                next_heartbeat += watch.heartbeat;
            }
        }
        std::thread::sleep(POLL);
    };
    // Whatever the command left running in its group goes too.
    tree.stop_rest();
    let deadline = Instant::now() + DRAIN;
    for _ in 0..readers {
        let left = deadline.saturating_duration_since(Instant::now());
        if finished_readers.recv_timeout(left).is_err() {
            break;
        }
    }
    let collected = lock(&collected);
    Ok(Finished {
        ending,
        stdout: collected.stdout.clone(),
        stderr: collected.stderr.clone(),
        duration,
    })
}

#[cfg(unix)]
mod platform {
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    use super::{GRACE, POLL, STOP};

    /// A command running as the leader of its own process group.
    pub(super) struct Tree {
        pub(super) child: Child,
        group: libc::pid_t,
        /// The leader was waited on, so its exit is known.
        reaped: bool,
    }

    impl Tree {
        pub(super) fn spawn(mut command: Command) -> std::io::Result<Self> {
            command.process_group(0);
            let child = command.spawn()?;
            let group = libc::pid_t::try_from(child.id())
                .map_err(|_| std::io::Error::other("the process id does not fit in pid_t"))?;
            Ok(Self {
                child,
                group,
                reaped: false,
            })
        }

        fn signal(&self, signal: libc::c_int) {
            // SAFETY: kill(2) with a negative pid signals a process group;
            // it has no memory-safety preconditions.
            unsafe {
                libc::kill(-self.group, signal);
            }
        }

        /// True while any process of the group exists (a zombie leader
        /// counts until it is waited on).
        fn group_alive(&self) -> bool {
            exists(-self.group)
        }

        fn reap(&mut self) {
            if !self.reaped {
                self.reaped = matches!(self.child.try_wait(), Ok(Some(_)));
            }
        }

        /// Stops every process of the group: `SIGTERM`, then `SIGKILL` for
        /// whatever is left after the grace period.
        pub(super) fn stop(&mut self) {
            self.reap();
            if !self.group_alive() {
                return;
            }
            self.signal(libc::SIGTERM);
            let deadline = Instant::now() + GRACE;
            while Instant::now() < deadline {
                self.reap();
                if !self.group_alive() {
                    return;
                }
                std::thread::sleep(POLL);
            }
            self.signal(libc::SIGKILL);
            let _ = self.child.wait();
            self.reaped = true;
        }

        /// After the leader ended: stops what it left in its group.
        pub(super) fn stop_rest(&mut self) {
            self.stop();
        }
    }

    /// True when the process (`target` > 0) or process group (`target` < 0)
    /// exists.
    fn exists(target: libc::pid_t) -> bool {
        // SAFETY: signal 0 only checks that the target exists.
        let result = unsafe { libc::kill(target, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    /// True when a process group with this id exists.
    pub(crate) fn group_exists(group: u32) -> bool {
        libc::pid_t::try_from(group).is_ok_and(|group| group > 0 && exists(-group))
    }

    /// True when a process with this id exists.
    pub(crate) fn process_exists(pid: u32) -> bool {
        libc::pid_t::try_from(pid).is_ok_and(|pid| pid > 0 && exists(pid))
    }

    static INSTALLED: AtomicBool = AtomicBool::new(false);

    extern "C" fn on_signal(_signal: libc::c_int) {
        // Only an atomic store: safe inside a signal handler.
        STOP.store(true, Ordering::SeqCst);
    }

    /// Makes Ctrl+C (`SIGINT`), `SIGTERM` and `SIGHUP` set [`STOP`] instead
    /// of ending fscale at once.
    pub(super) fn install_stop_handler() -> bool {
        if INSTALLED.swap(true, Ordering::SeqCst) {
            return true;
        }
        let handler: extern "C" fn(libc::c_int) = on_signal;
        let mut ok = true;
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            // SAFETY: a zeroed sigaction is a valid starting value; the
            // handler only stores to an atomic, which is async-signal-safe.
            unsafe {
                let mut action: libc::sigaction = std::mem::zeroed();
                action.sa_sigaction = handler as libc::sighandler_t;
                action.sa_flags = libc::SA_RESTART;
                libc::sigemptyset(&raw mut action.sa_mask);
                ok &= libc::sigaction(signal, &raw const action, std::ptr::null_mut()) == 0;
            }
        }
        ok
    }

    /// How a person stops what an earlier run left in this group.
    pub(crate) fn stop_command(group: u32) -> String {
        format!("kill -TERM -{group}")
    }
}

#[cfg(windows)]
mod platform {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, STILL_ACTIVE};
    use windows_sys::Win32::System::Console::{
        SetConsoleCtrlHandler, CTRL_BREAK_EVENT, CTRL_CLOSE_EVENT, CTRL_C_EVENT, CTRL_LOGOFF_EVENT,
        CTRL_SHUTDOWN_EVENT,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, OpenThread, ResumeThread, CREATE_NEW_PROCESS_GROUP,
        CREATE_SUSPENDED, PROCESS_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME,
    };

    use super::STOP;

    /// A job object that kills its processes when its handle closes.
    struct Job(HANDLE);

    // SAFETY: a job handle may be used and closed from any thread.
    unsafe impl Send for Job {}

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle came from CreateJobObjectW and is closed once.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    impl Job {
        fn new() -> Option<Self> {
            // SAFETY: plain Win32 calls; the handle is owned by `Job`.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return None;
                }
                let job = Self(handle);
                let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let size = u32::try_from(std::mem::size_of_val(&limits)).ok()?;
                let set = SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&raw const limits).cast(),
                    size,
                );
                (set != 0).then_some(job)
            }
        }

        fn assign(&self, child: &Child) -> bool {
            // SAFETY: both handles are valid for the duration of the call.
            unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as HANDLE) != 0 }
        }

        fn terminate(&self) {
            // SAFETY: the handle is a valid job handle.
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    /// Resumes every thread of a process that was started suspended.
    fn resume(pid: u32) -> bool {
        // SAFETY: plain Win32 calls; every handle opened here is closed here.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return false;
            }
            let mut entry = THREADENTRY32 {
                dwSize: u32::try_from(std::mem::size_of::<THREADENTRY32>()).unwrap_or(28),
                ..THREADENTRY32::default()
            };
            let mut resumed = false;
            let mut more = Thread32First(snapshot, &raw mut entry) != 0;
            while more {
                if entry.th32OwnerProcessID == pid {
                    let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                    if !thread.is_null() {
                        resumed |= ResumeThread(thread) != u32::MAX;
                        CloseHandle(thread);
                    }
                }
                more = Thread32Next(snapshot, &raw mut entry) != 0;
            }
            CloseHandle(snapshot);
            resumed
        }
    }

    /// A command running in its own job object.
    pub(super) struct Tree {
        pub(super) child: Child,
        /// `None` when no job could be made (for example inside a job that
        /// forbids nesting); then `taskkill /T` stops the tree instead.
        job: Option<Job>,
    }

    impl Tree {
        pub(super) fn spawn(mut command: Command) -> std::io::Result<Self> {
            // Suspended, so nothing it starts runs before it is in the job.
            // Its own process group, so Ctrl+C reaches fscale only, which
            // then stops the whole job.
            command.creation_flags(CREATE_SUSPENDED | CREATE_NEW_PROCESS_GROUP);
            let job = Job::new();
            let mut child = command.spawn()?;
            let job = job.filter(|job| job.assign(&child));
            if !resume(child.id()) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(std::io::Error::other(
                    "the command was started but could not be resumed",
                ));
            }
            Ok(Self { child, job })
        }

        /// Stops every process of the tree at once.
        pub(super) fn stop(&mut self) {
            match &self.job {
                Some(job) => job.terminate(),
                None => {
                    let _ = Command::new("taskkill")
                        .args(["/T", "/F", "/PID", &self.child.id().to_string()])
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                }
            }
            let _ = self.child.wait();
        }

        /// After the shell ended: stops what it left in the job.
        pub(super) fn stop_rest(&mut self) {
            if let Some(job) = &self.job {
                job.terminate();
            }
        }
    }

    /// True when a process with this id is running.
    pub(crate) fn process_exists(pid: u32) -> bool {
        // SAFETY: plain Win32 calls; the handle is closed here.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                // Access denied still means the process exists.
                return std::io::Error::last_os_error().raw_os_error() == Some(5);
            }
            let mut code = 0u32;
            let ok = GetExitCodeProcess(handle, &raw mut code) != 0;
            CloseHandle(handle);
            ok && code == STILL_ACTIVE as u32
        }
    }

    /// Windows has no process groups that outlive their leader here: the
    /// job dies with fscale. The id is the shell's process id.
    pub(crate) fn group_exists(pid: u32) -> bool {
        process_exists(pid)
    }

    static INSTALLED: AtomicBool = AtomicBool::new(false);

    unsafe extern "system" fn on_control(event: u32) -> windows_sys::core::BOOL {
        match event {
            CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT
            | CTRL_SHUTDOWN_EVENT => {
                STOP.store(true, Ordering::SeqCst);
                1
            }
            _ => 0,
        }
    }

    /// Makes Ctrl+C, Ctrl+Break and closing the console set [`STOP`]
    /// instead of ending fscale at once.
    pub(super) fn install_stop_handler() -> bool {
        if INSTALLED.swap(true, Ordering::SeqCst) {
            return true;
        }
        // SAFETY: registers a handler that only stores to an atomic.
        unsafe { SetConsoleCtrlHandler(Some(on_control), 1) != 0 }
    }

    /// How a person stops what an earlier run left running.
    pub(crate) fn stop_command(pid: u32) -> String {
        format!("taskkill /T /F /PID {pid}")
    }
}

use platform::Tree;
pub(crate) use platform::{group_exists, process_exists, stop_command};

/// Makes Ctrl+C and termination signals set [`STOP`] instead of ending
/// fscale at once, so verify can stop its commands and print what finished.
/// Returns false when the handler could not be installed.
pub fn install_stop_handler() -> bool {
    platform::install_stop_handler()
}

/// The exit code of a run that was stopped by Ctrl+C or a signal.
pub const EXIT_STOPPED: u8 = 130;
