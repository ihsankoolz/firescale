//! Live progress lines for `fscale verify`, written to stderr (ADR 0024).
//!
//! One line when a step starts, one when it ends, and a heartbeat line while
//! a step runs for a long time. Each line is written whole, under a lock, so
//! lines from projects that run in parallel never mix. The summary on stdout
//! (and `--json`) is unchanged by progress.

use std::io::Write;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

/// How often a running step gets a heartbeat line, unless
/// `FSCALE_HEARTBEAT_MS` says otherwise.
pub const DEFAULT_HEARTBEAT: Duration = Duration::from_secs(60);

/// The environment variable that sets the heartbeat interval, in
/// milliseconds. Meant for tests; `0` turns heartbeats off.
pub const HEARTBEAT_ENV: &str = "FSCALE_HEARTBEAT_MS";

/// The heartbeat interval: [`DEFAULT_HEARTBEAT`], or `FSCALE_HEARTBEAT_MS`.
#[must_use]
pub fn heartbeat_from_env() -> Duration {
    std::env::var(HEARTBEAT_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map_or(DEFAULT_HEARTBEAT, Duration::from_millis)
}

/// When the heartbeat after the one due at `at` is due, for a step whose
/// heartbeat comes every `every`.
#[must_use]
pub fn next_heartbeat(at: Duration, every: Duration) -> Duration {
    at + every
}

/// Where progress lines go.
pub struct Progress {
    out: Mutex<Box<dyn Write + Send>>,
}

impl Progress {
    #[must_use]
    pub fn new(out: Box<dyn Write + Send>) -> Self {
        Self {
            out: Mutex::new(out),
        }
    }

    /// Progress on the standard error stream.
    #[must_use]
    pub fn stderr() -> Self {
        Self::new(Box::new(std::io::stderr()))
    }

    /// Writes one line, prefixed with `[verify] `.
    pub fn line(&self, text: &str) {
        let mut out = self.out.lock().unwrap_or_else(PoisonError::into_inner);
        // A closed stderr is not worth failing the run for.
        let _ = writeln!(out, "[verify] {text}");
        let _ = out.flush();
    }
}

/// A duration for people: `850ms`, `12.3s`, `4m 05s`, `1h 02m`.
#[must_use]
pub fn format_duration(duration: Duration) -> String {
    let millis = duration.as_millis();
    if millis < 1000 {
        return format!("{millis}ms");
    }
    let seconds = duration.as_secs();
    if seconds < 60 {
        return format!("{}.{}s", seconds, (millis % 1000) / 100);
    }
    if seconds < 3600 {
        return format!("{}m {:02}s", seconds / 60, seconds % 60);
    }
    format!("{}h {:02}m", seconds / 3600, (seconds % 3600) / 60)
}

/// A limit as people write it in the config: `90s`, `10m`, `1h`, `1h 30m`.
#[must_use]
pub fn format_limit(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let (hours, minutes, rest) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    let mut parts = Vec::new();
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes > 0 {
        parts.push(format!("{minutes}m"));
    }
    if rest > 0 || parts.is_empty() {
        parts.push(format!("{rest}s"));
    }
    parts.join(" ")
}

/// A writer tests can read back.
#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct Shared(pub(crate) std::sync::Arc<Mutex<Vec<u8>>>);

#[cfg(test)]
impl Shared {
    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap_or_else(PoisonError::into_inner)).into_owned()
    }
}

#[cfg(test)]
impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_well() {
        assert_eq!(format_duration(Duration::from_millis(850)), "850ms");
        assert_eq!(format_duration(Duration::from_millis(12_345)), "12.3s");
        assert_eq!(format_duration(Duration::from_secs(245)), "4m 05s");
        assert_eq!(format_duration(Duration::from_secs(13 * 60 + 2)), "13m 02s");
        assert_eq!(format_duration(Duration::from_secs(3720)), "1h 02m");
    }

    /// When the heartbeats of a step come, up to `until`, as the watch
    /// loop works them out: no clock, no waiting.
    fn heartbeats(every: Duration, until: Duration) -> Vec<u64> {
        let mut times = Vec::new();
        let mut at = every;
        while at <= until {
            times.push(at.as_secs());
            at = next_heartbeat(at, every);
        }
        times
    }

    #[test]
    fn a_step_gets_a_heartbeat_a_minute_and_after_ten_minutes_one_every_five() {
        let minutes: Vec<u64> = heartbeats(DEFAULT_HEARTBEAT, Duration::from_secs(40 * 60))
            .into_iter()
            .map(|seconds| seconds / 60)
            .collect();
        assert_eq!(
            minutes,
            [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 15, 20, 25, 30, 35, 40]
        );
        // The course project's slow step, 1h 40m: 28 lines, not 100.
        assert_eq!(
            heartbeats(DEFAULT_HEARTBEAT, Duration::from_secs(100 * 60)).len(),
            28
        );
        // An interval longer than five minutes is kept as it is.
        assert_eq!(
            heartbeats(Duration::from_secs(8 * 60), Duration::from_secs(30 * 60)),
            [8 * 60, 16 * 60, 24 * 60]
        );
        // The short interval tests set is kept for the first ten minutes.
        assert_eq!(
            heartbeats(Duration::from_millis(500), Duration::from_secs(2)),
            [0, 1, 1, 2]
        );
    }

    #[test]
    fn limits_read_like_the_config() {
        assert_eq!(format_limit(Duration::from_secs(600)), "10m");
        assert_eq!(format_limit(Duration::from_secs(90)), "1m 30s");
        assert_eq!(format_limit(Duration::from_secs(5400)), "1h 30m");
        assert_eq!(format_limit(Duration::from_secs(45)), "45s");
    }

    #[test]
    fn lines_from_many_threads_never_mix() {
        let shared = Shared::default();
        let progress = Progress::new(Box::new(shared.clone()));
        std::thread::scope(|scope| {
            for thread in 0..8 {
                let progress = &progress;
                scope.spawn(move || {
                    for line in 0..50 {
                        progress.line(&format!("thread {thread} line {line} {}", "x".repeat(200)));
                    }
                });
            }
        });
        let text = shared.text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 400);
        for line in lines {
            assert!(line.starts_with("[verify] thread "), "{line}");
            assert!(line.ends_with(&"x".repeat(200)), "{line}");
        }
    }
}
