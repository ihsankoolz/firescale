//! Sends Ctrl+Break to `fscale` on Windows, the way a console does, for the
//! test that proves verify stops every command it started.
//!
//! `fscale-ctrl-break <ready-file> <output-file> <program> [arguments...]`
//! gives itself a console of its own, starts the program in a new process
//! group on that console with its output going to `<output-file>` (stdout)
//! and `<output-file>.err` (stderr), waits until `<ready-file>` exists,
//! sends Ctrl+Break to the program's group, waits for the program, and
//! exits with its exit code. Elsewhere it prints an error and exits with 2.

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    use std::fs::File;
    use std::os::windows::process::CommandExt;
    use std::path::Path;
    use std::process::{Command, ExitCode, Stdio};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::System::Console::{
        AllocConsole, FreeConsole, GenerateConsoleCtrlEvent, CTRL_BREAK_EVENT,
    };
    use windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let [ready, output, program, rest @ ..] = args.as_slice() else {
        eprintln!("usage: fscale-ctrl-break <ready-file> <output-file> <program> [arguments...]");
        return ExitCode::from(2);
    };
    let (Ok(stdout), Ok(stderr)) = (File::create(output), File::create(format!("{output}.err")))
    else {
        eprintln!("could not create the output files");
        return ExitCode::from(2);
    };
    // A console of our own, which the program shares: console control
    // events only reach processes on the sender's console.
    // SAFETY: plain Win32 calls with no pointers.
    unsafe {
        FreeConsole();
        if AllocConsole() == 0 {
            eprintln!(
                "could not allocate a console: {}",
                std::io::Error::last_os_error()
            );
            return ExitCode::from(2);
        }
    }
    let mut child = match Command::new(program)
        .args(rest)
        .creation_flags(CREATE_NEW_PROCESS_GROUP)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            eprintln!("could not start {program}: {error}");
            return ExitCode::from(2);
        }
    };
    let deadline = Instant::now() + Duration::from_secs(60);
    while !Path::new(ready).exists() {
        if Instant::now() > deadline || matches!(child.try_wait(), Ok(Some(_))) {
            eprintln!("{ready} did not appear");
            let _ = child.kill();
            return ExitCode::from(2);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // SAFETY: plain Win32 call; the group id is the program's process id.
    let sent = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, child.id()) };
    if sent == 0 {
        eprintln!(
            "could not send Ctrl+Break: {}",
            std::io::Error::last_os_error()
        );
        let _ = child.kill();
        return ExitCode::from(2);
    }
    match child.wait() {
        Ok(status) => ExitCode::from(u8::try_from(status.code().unwrap_or(2) & 0xff).unwrap_or(2)),
        Err(_) => ExitCode::from(2),
    }
}

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("fscale-ctrl-break only works on Windows");
    std::process::ExitCode::from(2)
}
