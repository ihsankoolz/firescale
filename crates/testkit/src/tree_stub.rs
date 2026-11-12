//! A process tree for tests of stopping verify's commands.
//!
//! `fscale-tree-stub <folder> <depth>` writes its process id to
//! `<folder>/<pid>.pid`, starts a copy of itself with `<depth> - 1` (when
//! the depth is above 0), prints a line, and then sleeps for five minutes.
//! The copy at depth 0 also writes `<folder>/ready` once it has started, so
//! a test knows that the whole tree is up. A test then stops verify and
//! checks that no process id in the folder is still running.

use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::time::Duration;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let (Some(folder), Some(depth)) = (args.first(), args.get(1)) else {
        eprintln!("usage: fscale-tree-stub <folder> <depth>");
        return ExitCode::from(2);
    };
    let Ok(depth) = depth.parse::<u32>() else {
        eprintln!("the depth must be a whole number");
        return ExitCode::from(2);
    };
    let folder = PathBuf::from(folder);
    if fs::create_dir_all(&folder).is_err() {
        return ExitCode::from(2);
    }
    let pid = std::process::id();
    if fs::write(folder.join(format!("{pid}.pid")), pid.to_string()).is_err() {
        return ExitCode::from(2);
    }
    let _child = if depth > 0 {
        let Ok(me) = env::current_exe() else {
            return ExitCode::from(2);
        };
        match Command::new(me)
            .arg(&folder)
            .arg((depth - 1).to_string())
            .spawn()
        {
            Ok(child) => Some(child),
            Err(_) => return ExitCode::from(2),
        }
    } else {
        // Written last, beside the process ids: the whole tree is up.
        let _ = fs::write(folder.join("ready"), "ready");
        None
    };
    println!("tree-stub level {depth} running as process {pid}");
    let _ = std::io::stdout().flush();
    std::thread::sleep(Duration::from_secs(300));
    ExitCode::SUCCESS
}
