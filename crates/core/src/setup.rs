//! Telling "the environment is not ready" apart from a real failure
//! (ADR 0022).
//!
//! A verify step that fails because a dependency is not installed, a
//! program is not on the PATH, or a local service such as a test database is
//! not running, is not something a code change fixes. Verify
//! reports it as "setup needed", with the command that sets the environment
//! up, so an agent does not edit code to make it pass.
//!
//! The detection is a short list of exact messages, documented in
//! docs/verify.md. Anything else, and anything that could be a mistake in the
//! code (a module that the project itself holds), stays a normal failure.

use std::path::Path;

/// Why a failed step looks like an environment that is not set up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupProblem {
    /// What is missing, in a few words: "the Python module 'jwt' is not
    /// installed".
    pub reason: String,
    /// A program that is not on the PATH at all, such as `python` or `pnpm`.
    /// Installing the project's dependencies does not bring it back.
    pub missing_program: Option<String>,
    /// A local service that could not be reached, such as `PostgreSQL on
    /// localhost:5432`. Starting the service fixes it, not an install.
    pub missing_service: Option<String>,
    /// The command that starts the services of a Docker Compose file in the
    /// step's folder or a folder above it, to be run in the step's folder:
    /// `docker compose up -d`, or `docker compose -f ../compose.yaml up -d`.
    pub start_services: Option<String>,
}

/// Programs that a project's own install command does not provide.
const BASE_PROGRAMS: [&str; 16] = [
    "bun", "cargo", "docker", "go", "node", "npm", "npx", "pip", "pipenv", "pnpm", "poetry", "py",
    "python", "python3", "uv", "yarn",
];

/// The text between the first pair of `quote` characters after `start`, or
/// the word after it when it is not quoted.
fn name_after(text: &str, start: usize) -> Option<&str> {
    let rest = text[start..].trim_start();
    let first = rest.chars().next()?;
    let name = if matches!(first, '\'' | '"' | '`') {
        let inner = &rest[1..];
        &inner[..inner.find(first)?]
    } else {
        let end = rest
            .find(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | ')'))
            .unwrap_or(rest.len());
        &rest[..end]
    };
    let name = name.trim();
    (!name.is_empty()).then_some(name)
}

/// True when `module`'s top-level name is a file or folder of the project or
/// of a folder above it, up to the repo root. A missing module of that kind
/// may be a mistake in the code, so it is not called a setup problem.
fn is_local_module(module: &str, root: &Path, dir: &Path) -> bool {
    let top = module.split('.').next().unwrap_or(module);
    if top.is_empty() {
        return true;
    }
    let mut current = Some(dir);
    while let Some(folder) = current {
        // The flat layout and the `src/` layout.
        for base in [folder.to_path_buf(), folder.join("src")] {
            if base.join(top).is_dir() || base.join(format!("{top}.py")).is_file() {
                return true;
            }
        }
        if folder == root {
            break;
        }
        current = folder.parent().filter(|parent| parent.starts_with(root));
    }
    false
}

/// A program named by a "not found" message, without a path or `.exe`.
fn program_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let lower = base.to_lowercase();
    lower
        .strip_suffix(".exe")
        .or_else(|| lower.strip_suffix(".cmd"))
        .unwrap_or(&lower)
        .to_owned()
}

fn missing(program: &str) -> SetupProblem {
    let name = program_name(program);
    if BASE_PROGRAMS.contains(&name.as_str()) {
        SetupProblem {
            reason: format!("the program `{name}` was not found on the PATH"),
            missing_program: Some(name),
            missing_service: None,
            start_services: None,
        }
    } else {
        SetupProblem {
            reason: format!(
                "the command `{name}` was not found; the dependencies are probably not installed"
            ),
            missing_program: None,
            missing_service: None,
            start_services: None,
        }
    }
}

/// The name in a POSIX shell's "not found" line: `sh: 1: vitest: not found`,
/// `sh: vitest: command not found`, `zsh: command not found: vitest`.
fn shell_not_found(line: &str) -> Option<&str> {
    if let Some(index) = line.find("command not found: ") {
        return name_after(line, index + "command not found: ".len());
    }
    let before = line
        .strip_suffix(": command not found")
        .or_else(|| line.strip_suffix(": not found"))?;
    let name = before.rsplit(": ").next()?.trim();
    (!name.is_empty() && !name.contains(' ')).then_some(name)
}

/// True when the output reports failed tests: a line starting with `FAILED `
/// (pytest's summary), or a number followed by the word `failed` (`2 failed`
/// from pytest, `Tests  1 failed` from vitest, `Tests: 1 failed` from jest).
/// Tests that ran and failed are a code problem, even when one of them
/// failed on a missing module.
fn reports_failed_tests(output: &str) -> bool {
    output.lines().any(|line| {
        let line = line.trim();
        if line.starts_with("FAILED ") {
            return true;
        }
        let words: Vec<&str> = line
            .split(|c: char| c.is_whitespace() || matches!(c, ',' | '=' | '(' | ')'))
            .filter(|word| !word.is_empty())
            .collect();
        words
            .windows(2)
            .any(|pair| pair[1] == "failed" && pair[0].chars().all(|c| c.is_ascii_digit()))
    })
}

/// Hosts that mean this machine. A refused connection to any other host is a
/// normal failure.
const LOCAL_HOSTS: [&str; 4] = ["localhost", "127.0.0.1", "::1", "0.0.0.0"];

/// Error codes for a refused connection: ECONNREFUSED on Linux (111) and
/// macOS (61), WSAECONNREFUSED (10061) and ERROR_CONNECTION_REFUSED (1225) on
/// Windows.
const REFUSED_CODES: [&str; 4] = ["111", "61", "10061", "1225"];

/// Python's refused-connection errors that do not name the address.
const REFUSED_WITHOUT_ADDRESS: [&str; 5] = [
    "[Errno 111] Connection refused",
    "[Errno 61] Connection refused",
    "[WinError 10061] No connection could be made because the target machine actively refused it",
    "[Errno 10061] No connection could be made because the target machine actively refused it",
    "[WinError 1225] The remote computer refused the network connection",
];

/// Docker Compose file names, in the order Compose itself prefers them.
const COMPOSE_FILES: [&str; 4] = [
    "compose.yaml",
    "compose.yml",
    "docker-compose.yaml",
    "docker-compose.yml",
];

/// What a connection error names: the host, the port when known, and the
/// service when the message comes from a client of one service.
#[derive(Debug, Clone, Copy)]
struct Target<'a> {
    host: &'a str,
    port: Option<u16>,
    service: Option<&'static str>,
}

fn is_local_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    LOCAL_HOSTS
        .iter()
        .any(|local| host.eq_ignore_ascii_case(local))
}

/// The digits at the start of `text`.
fn leading_digits(text: &str) -> &str {
    let end = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    &text[..end]
}

fn port_at(text: &str) -> Option<u16> {
    leading_digits(text).parse().ok()
}

fn is_refused_code(code: &str) -> bool {
    REFUSED_CODES.contains(&code)
}

/// `host:port`, with an IPv6 host as well: `::1:6379`, `[::1]:6379`.
fn host_port(text: &str) -> Option<(&str, u16)> {
    let (host, port) = text.rsplit_once(':')?;
    if port.is_empty() || !port.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let host = host.trim_start_matches('[').trim_end_matches(']');
    (!host.is_empty()).then_some((host, port.parse().ok()?))
}

/// A Python address tuple at the start of `text`, after its `(`:
/// `'127.0.0.1', 5432)` or `'::1', 5432, 0, 0)`.
fn python_address(text: &str) -> Option<(&str, u16)> {
    let inner = text.strip_prefix('\'')?;
    let host = &inner[..inner.find('\'')?];
    let rest = inner[host.len() + 1..].strip_prefix(", ")?;
    let port = port_at(rest)?;
    (!host.is_empty() && !host.contains(char::is_whitespace)).then_some((host, port))
}

/// The last Python address tuple in a line, such as the frame argument
/// `address = ('localhost', 5432)` that pytest prints in a traceback.
fn last_python_address(line: &str) -> Option<(&str, u16)> {
    line.match_indices("('")
        .filter_map(|(index, _)| python_address(&line[index + 1..]))
        .last()
}

/// libpq 14 and later (psycopg2, psycopg, SQLAlchemy, Django):
/// `connection to server at "localhost" (127.0.0.1), port 5432 failed: Connection refused`,
/// also without the address in brackets.
fn libpq_at(line: &str) -> Option<Target<'_>> {
    const AT: &str = "connection to server at \"";
    let rest = &line[line.find(AT)? + AT.len()..];
    let host = &rest[..rest.find('"')?];
    let after = &rest[host.len()..];
    let failed = after.find(" failed: Connection refused")?;
    let port = after[..failed]
        .rfind("port ")
        .and_then(|index| port_at(&after[index + "port ".len()..]));
    Some(Target {
        host,
        port: port.or(Some(5432)),
        service: Some("PostgreSQL"),
    })
}

/// libpq 14 and later on a Unix socket, which is always local:
/// `connection to server on socket "/run/postgresql/.s.PGSQL.5432" failed: No such file or directory`
/// (or `Connection refused`).
fn libpq_socket(line: &str) -> Option<Target<'_>> {
    const ON: &str = "connection to server on socket \"";
    let rest = &line[line.find(ON)? + ON.len()..];
    let path = &rest[..rest.find('"')?];
    let after = &rest[path.len()..];
    if !after.contains(" failed: No such file or directory")
        && !after.contains(" failed: Connection refused")
    {
        return None;
    }
    let port = path
        .rfind(".s.PGSQL.")
        .and_then(|index| port_at(&path[index + ".s.PGSQL.".len()..]));
    Some(Target {
        host: "localhost",
        port: port.or(Some(5432)),
        service: Some("PostgreSQL"),
    })
}

/// libpq 13 and earlier, over three lines:
/// `could not connect to server: Connection refused`, then
/// `Is the server running on host "localhost" (127.0.0.1) and accepting`,
/// then `TCP/IP connections on port 5432?`.
fn libpq_old<'a>(lines: &[&'a str], index: usize) -> Option<Target<'a>> {
    const HOST: &str = "Is the server running on host \"";
    const PORT: &str = "connections on port ";
    if !lines[index].contains("could not connect to server: Connection refused") {
        return None;
    }
    let next = lines.iter().skip(index + 1).take(3);
    let mut host = None;
    let mut port = None;
    for line in next {
        if let Some(start) = line.find(HOST).map(|start| start + HOST.len()) {
            let rest = &line[start..];
            host = host.or_else(|| rest.find('"').map(|end| &rest[..end]));
        }
        if let Some(start) = line.find(PORT) {
            port = port.or_else(|| port_at(&line[start + PORT.len()..]));
        }
    }
    Some(Target {
        host: host?,
        port: port.or(Some(5432)),
        service: Some("PostgreSQL"),
    })
}

/// redis-py: `Error 111 connecting to localhost:6379. Connection refused.`,
/// on Windows `Error 10061 connecting to localhost:6379. No connection could
/// be made because the target machine actively refused it.`
fn redis(line: &str) -> Option<Target<'_>> {
    const CONNECTING: &str = " connecting to ";
    let index = line.find(CONNECTING)?;
    let (_, code) = line[..index].rsplit_once("Error ")?;
    if !is_refused_code(code) {
        return None;
    }
    let rest = &line[index + CONNECTING.len()..];
    let location = rest[..rest.find(". ").unwrap_or(rest.len())].trim_end_matches('.');
    let (host, port) = host_port(location)?;
    Some(Target {
        host,
        port: Some(port),
        service: Some("Redis"),
    })
}

/// MySQL clients: `Can't connect to MySQL server on 'localhost' (111)`,
/// `on 'localhost:3306' (10061)`, PyMySQL's `on 'localhost' ([Errno 111]
/// Connection refused)`; and `Can't connect to local MySQL server`.
fn mysql(line: &str) -> Option<Target<'_>> {
    const ON: &str = "Can't connect to MySQL server on ";
    if line.contains("Can't connect to local MySQL server") {
        return Some(Target {
            host: "localhost",
            port: None,
            service: Some("MySQL"),
        });
    }
    let rest = &line[line.find(ON)? + ON.len()..];
    let quote = rest.chars().next().filter(|c| matches!(c, '\'' | '"'))?;
    let inner = &rest[1..];
    let location = &inner[..inner.find(quote)?];
    let after = inner[location.len() + 1..].trim_start().strip_prefix('(')?;
    let after = after
        .strip_prefix("[Errno ")
        .or_else(|| after.strip_prefix("[WinError "))
        .unwrap_or(after);
    if !is_refused_code(leading_digits(after)) {
        return None;
    }
    let (host, port) = if is_local_host(location) {
        (location, 3306)
    } else {
        host_port(location).unwrap_or((location, 3306))
    };
    Some(Target {
        host,
        port: Some(port),
        service: Some("MySQL"),
    })
}

/// Prisma P1001: ``Can't reach database server at `localhost:5432` ``, and
/// the older ``at `localhost`:`5432` ``.
fn prisma(line: &str) -> Option<Target<'_>> {
    const AT: &str = "Can't reach database server at ";
    let rest = &line[line.find(AT)? + AT.len()..];
    let token = rest.split_whitespace().next()?;
    let token = token.trim_end_matches(['.', ',']);
    let location = token.strip_prefix('`')?.strip_suffix('`')?;
    // The older form: `localhost`:`5432`.
    let (host, port) = match location.split_once("`:`") {
        Some((host, port)) => (host, port.parse().ok()?),
        None => host_port(location)?,
    };
    Some(Target {
        host,
        port: Some(port),
        service: None,
    })
}

/// Node: `connect ECONNREFUSED 127.0.0.1:5432`, `connect ECONNREFUSED ::1:6379`.
fn node(line: &str) -> Option<Target<'_>> {
    const REFUSED: &str = "connect ECONNREFUSED ";
    let rest = &line[line.find(REFUSED)? + REFUSED.len()..];
    let end = rest
        .find(|c: char| c.is_whitespace() || matches!(c, ',' | ')' | '\'' | '"'))
        .unwrap_or(rest.len());
    let (host, port) = host_port(&rest[..end])?;
    Some(Target {
        host,
        port: Some(port),
        service: None,
    })
}

/// asyncio (asyncpg and other async clients):
/// `[Errno 111] Connect call failed ('127.0.0.1', 5432)`. With several
/// addresses, `Multiple exceptions:` lists them all; the first one counts.
fn connect_call_failed(line: &str) -> Option<Target<'_>> {
    const FAILED: &str = "] Connect call failed (";
    let index = line.find(FAILED)?;
    let (_, code) = line[..index].rsplit_once("[Errno ")?;
    if !is_refused_code(code) {
        return None;
    }
    let (host, port) = python_address(&line[index + FAILED.len()..])?;
    Some(Target {
        host,
        port: Some(port),
        service: None,
    })
}

/// `host:port: [Errno 111] Connection refused` (pymongo), on Windows
/// `host:port: [WinError 10061] No connection could be made ...`.
fn address_then_errno(line: &str) -> Option<Target<'_>> {
    for marker in [": [Errno ", ": [WinError "] {
        let Some(index) = line.find(marker) else {
            continue;
        };
        if !is_refused_code(leading_digits(&line[index + marker.len()..])) {
            continue;
        }
        let token = line[..index].rsplit(char::is_whitespace).next()?;
        let (host, port) = host_port(token)?;
        return Some(Target {
            host,
            port: Some(port),
            service: None,
        });
    }
    None
}

use crate::services::service_on_port;

/// `PostgreSQL on localhost:5432`, `a service on localhost:8000`.
fn describe(target: Target<'_>) -> String {
    let name = target
        .service
        .or_else(|| target.port.and_then(service_on_port));
    let place = match target.port {
        Some(port) => format!("localhost:{port}"),
        None => "localhost".to_owned(),
    };
    match name {
        Some(name) => format!("{name} on {place}"),
        None => format!("a service on {place}"),
    }
}

/// True for a line that starts a pytest section (`____ test_x ____`, `==== ...`).
/// A Python address seen before it belongs to another test.
fn is_section_line(line: &str) -> bool {
    line.starts_with("___") || line.starts_with("===")
}

/// The command that starts the services of the nearest Docker Compose file,
/// from `dir` up to `root`, to be run in `dir`.
fn start_services(root: &Path, dir: &Path) -> Option<String> {
    let mut current = Some(dir);
    let mut depth = 0;
    while let Some(folder) = current {
        for name in COMPOSE_FILES {
            if folder.join(name).is_file() {
                if depth == 0 {
                    return Some("docker compose up -d".to_owned());
                }
                // A command, not a path on this machine: forward slashes everywhere.
                let mut parts = vec![".."; depth];
                parts.push(name);
                return Some(format!("docker compose -f {} up -d", parts.join("/")));
            }
        }
        if folder == root {
            break;
        }
        current = folder.parent().filter(|parent| parent.starts_with(root));
        depth += 1;
    }
    None
}

/// A refused connection to a local service, found in output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusedService {
    /// The host as the message names it.
    pub host: String,
    /// The port, when the message names it or the client has a usual one.
    pub port: Option<u16>,
    /// `PostgreSQL on localhost:5432`.
    pub described: String,
}

/// The first refused connection to a service on this machine in `output`.
/// Connections to other hosts are not counted.
#[must_use]
pub fn refused_service(output: &str) -> Option<RefusedService> {
    let lines: Vec<&str> = output.lines().collect();
    let mut last_address = None;
    for (index, raw) in lines.iter().enumerate() {
        let line = raw.trim();
        if is_section_line(line) {
            last_address = None;
        }
        if let Some(address) = last_python_address(line) {
            last_address = Some(address);
        }
        let found = libpq_at(line)
            .or_else(|| libpq_socket(line))
            .or_else(|| libpq_old(&lines, index))
            .or_else(|| redis(line))
            .or_else(|| mysql(line))
            .or_else(|| prisma(line))
            .or_else(|| node(line))
            .or_else(|| connect_call_failed(line))
            .or_else(|| address_then_errno(line))
            .or_else(|| {
                // No address in the message: the nearest one before it in
                // the same traceback, if any.
                let refused = REFUSED_WITHOUT_ADDRESS
                    .iter()
                    .any(|message| line.contains(message));
                let (host, port) = last_address.filter(|_| refused)?;
                Some(Target {
                    host,
                    port: Some(port),
                    service: None,
                })
            });
        let Some(target) = found.filter(|target| is_local_host(target.host)) else {
            continue;
        };
        return Some(RefusedService {
            host: target.host.to_owned(),
            port: target.port,
            described: describe(target),
        });
    }
    None
}

/// The setup problem of a refused connection to `service`.
#[must_use]
pub fn service_down(described: &str, root: &Path, dir: &Path) -> SetupProblem {
    SetupProblem {
        reason: format!(
            "could not connect to {described}; the service is probably not running, so start it"
        ),
        missing_program: None,
        missing_service: Some(described.to_owned()),
        start_services: start_services(root, dir),
    }
}

/// A refused connection to a service on this machine: a database or cache
/// that is not running. Connections to other hosts are not counted.
fn service_problem(output: &str, root: &Path, dir: &Path) -> Option<SetupProblem> {
    refused_service(output).map(|found| service_down(&found.described, root, dir))
}

/// Looks at the output of a failed step for a sign that the environment is
/// not set up. `dir` is the folder the step ran in, `root` the repo root.
/// Returns `None` when unsure: then the step is a normal failure.
#[must_use]
pub fn setup_problem(
    output: &str,
    exit_code: Option<i32>,
    root: &Path,
    dir: &Path,
) -> Option<SetupProblem> {
    // A service that is down fails tests too, so this comes first: tests
    // that need the service cannot be judged while it is down.
    if let Some(problem) = service_problem(output, root, dir) {
        return Some(problem);
    }
    if reports_failed_tests(output) {
        return None;
    }
    for line in output.lines() {
        let line = line.trim();
        // Python: `ModuleNotFoundError: No module named 'jwt'`, and
        // `python: No module named pytest` from `python -m pytest`.
        if let Some(index) = line.find("No module named ") {
            let module = name_after(line, index + "No module named ".len())?;
            if is_local_module(module, root, dir) {
                return None;
            }
            return Some(SetupProblem {
                reason: format!("the Python module '{module}' is not installed"),
                missing_program: None,
                missing_service: None,
                start_services: None,
            });
        }
        // Windows: the Microsoft Store stand-in for a Python that is not installed.
        if line.starts_with("Python was not found") {
            return Some(missing("python"));
        }
        // cmd: 'vitest' is not recognized as an internal or external command
        if let Some(index) = line.find(" is not recognized as an internal or external command") {
            if let Some(name) = name_after(line, 0).filter(|_| index > 0) {
                return Some(missing(name));
            }
        }
        // npx --no with the tool not installed.
        if line.contains("canceled due to missing packages") {
            return Some(SetupProblem {
                reason:
                    "npx found the tool not installed; the dependencies are probably not installed"
                        .to_owned(),
                missing_program: None,
                missing_service: None,
                start_services: None,
            });
        }
        // pnpm exec: Command "oxlint" not found
        if let Some(index) = line.find("Command \"") {
            if line.ends_with("not found") || line.ends_with("not found.") {
                if let Some(name) = name_after(line, index + "Command ".len()) {
                    return Some(missing(name));
                }
            }
        }
        // poetry run: Command not found: pytest
        if let Some(index) = line.find("Command not found: ") {
            if let Some(name) = name_after(line, index + "Command not found: ".len()) {
                return Some(missing(name));
            }
        }
        // uv run: error: Failed to spawn: `pytest`
        if let Some(index) = line.find("Failed to spawn: ") {
            if let Some(name) = name_after(line, index + "Failed to spawn: ".len()) {
                return Some(missing(name));
            }
        }
        // Node ESM: Cannot find package 'vitest' imported from ...
        if line.contains("ERR_MODULE_NOT_FOUND") || line.starts_with("Error: Cannot find package") {
            if let Some(index) = line.find("Cannot find package ") {
                if let Some(name) = name_after(line, index + "Cannot find package ".len()) {
                    let relative = name.starts_with('.')
                        || name.starts_with('/')
                        || name.starts_with('\\')
                        || name.chars().nth(1) == Some(':');
                    if relative {
                        return None;
                    }
                    return Some(SetupProblem {
                        reason: format!("the package '{name}' is not installed"),
                        missing_program: None,
                        missing_service: None,
                        start_services: None,
                    });
                }
            }
        }
        // sh: exit code 127 means the shell could not find the command.
        if exit_code == Some(127) {
            if let Some(name) = shell_not_found(line) {
                return Some(missing(name));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Fixture, Track};

    fn problem(output: &str, code: i32) -> Option<SetupProblem> {
        let fixture = Fixture::new(
            &[
                ("svc/main.py", ""),
                ("svc/app/__init__.py", ""),
                ("shared/__init__.py", ""),
            ],
            Track::NoGit,
        )
        .unwrap();
        setup_problem(
            output,
            Some(code),
            fixture.root(),
            &fixture.root().join("svc"),
        )
    }

    fn reason(output: &str, code: i32) -> String {
        problem(output, code).map(|p| p.reason).unwrap_or_default()
    }

    #[test]
    fn a_missing_python_package_is_a_setup_problem() {
        let out = "ImportError while importing test module '/x/test_auth.py'.\nE   ModuleNotFoundError: No module named 'jwt'\n";
        assert_eq!(reason(out, 2), "the Python module 'jwt' is not installed");
        // `python -m pytest` with pytest missing.
        assert_eq!(
            reason("/usr/bin/python: No module named pytest\n", 1),
            "the Python module 'pytest' is not installed"
        );
        assert_eq!(
            reason(
                "E   ModuleNotFoundError: No module named 'google.protobuf'",
                2
            ),
            "the Python module 'google.protobuf' is not installed"
        );
    }

    #[test]
    fn a_missing_module_of_the_project_itself_is_a_normal_failure() {
        // Folders and files of the project or above it: maybe a code mistake.
        assert_eq!(
            problem("ModuleNotFoundError: No module named 'app.services'", 2),
            None
        );
        assert_eq!(
            problem("ModuleNotFoundError: No module named 'main'", 2),
            None
        );
        assert_eq!(
            problem("ModuleNotFoundError: No module named 'shared'", 2),
            None
        );
    }

    #[test]
    fn a_missing_module_of_a_src_layout_package_is_a_normal_failure() {
        let fixture = Fixture::new(&[("svc/src/mypkg/__init__.py", "")], Track::NoGit).unwrap();
        let dir = fixture.root().join("svc");
        let out = "ModuleNotFoundError: No module named 'mypkg.sevices'";
        assert_eq!(setup_problem(out, Some(2), fixture.root(), &dir), None);
    }

    #[test]
    fn a_missing_module_among_failed_tests_is_a_normal_failure() {
        // Tests ran and failed: a code problem, even if one failed on an import.
        let pytest = "FAILED tests/test_plugins.py::test_load - ModuleNotFoundError: No module named 'myapp_plugins'\n===== 3 failed, 2 passed in 0.12s =====\n";
        assert_eq!(problem(pytest, 1), None);
        let vitest = "Error: Cannot find package 'zod' imported from /x/a.ts\n Tests  1 failed | 4 passed (5)\n";
        assert_eq!(problem(vitest, 1), None);
        // A collection error stops pytest before any test runs: setup.
        let collection = "E   ModuleNotFoundError: No module named 'jwt'\n!!! Interrupted: 1 error during collection !!!\n===== 1 error in 0.10s =====\n";
        assert_eq!(
            reason(collection, 2),
            "the Python module 'jwt' is not installed"
        );
    }

    #[test]
    fn a_command_that_is_not_found_is_a_setup_problem() {
        let p = problem("sh: 1: vitest: not found\n", 127).unwrap();
        assert_eq!(p.missing_program, None);
        assert!(p.reason.contains("`vitest`"), "{}", p.reason);
        let p = problem("sh: line 1: pnpm: command not found\n", 127).unwrap();
        assert_eq!(p.missing_program.as_deref(), Some("pnpm"));
        let p = problem(
            "'python' is not recognized as an internal or external command,\noperable program or batch file.\n",
            9009,
        )
        .unwrap();
        assert_eq!(p.missing_program.as_deref(), Some("python"));
        let p = problem(
            "Python was not found; run without arguments to install from the Microsoft Store\n",
            9009,
        )
        .unwrap();
        assert_eq!(p.missing_program.as_deref(), Some("python"));
        let p = problem("npm error npx canceled due to missing packages and no YES option: [\"oxlint@1.0.0\"]\n", 1).unwrap();
        assert_eq!(p.missing_program, None);
        let p = problem(
            " ERR_PNPM_RECURSIVE_EXEC_FIRST_FAIL  Command \"oxlint\" not found\n",
            1,
        )
        .unwrap();
        assert!(p.reason.contains("`oxlint`"), "{}", p.reason);
        let p = problem("\nCommand not found: pytest\n", 1).unwrap();
        assert!(p.reason.contains("`pytest`"), "{}", p.reason);
        let p = problem("error: Failed to spawn: `pytest`\n  Caused by: No such file or directory (os error 2)\n", 2).unwrap();
        assert!(p.reason.contains("`pytest`"), "{}", p.reason);
        let p = problem("Error [ERR_MODULE_NOT_FOUND]: Cannot find package 'vitest' imported from /x/vitest.config.ts\n", 1).unwrap();
        assert_eq!(p.reason, "the package 'vitest' is not installed");
    }

    #[test]
    fn real_failures_are_not_setup_problems() {
        // A failing test, a lint error, a type error.
        assert_eq!(
            problem(
                "FAILED test_auth.py::test_login - assert 401 == 200\n1 failed, 3 passed\n",
                1
            ),
            None
        );
        assert_eq!(problem("src/a.ts:1:1 error no-unused-vars\n", 1), None);
        assert_eq!(
            problem(
                "error TS2322: Type 'string' is not assignable to type 'number'.\n",
                2
            ),
            None
        );
        // "not found" text with a code other than 127: a test about a 404, say.
        assert_eq!(
            problem("expected: not found\nsh: 1: x: not found\n", 1),
            None
        );
        // A relative import that is missing is a code problem.
        assert_eq!(
            problem(
                "Error [ERR_MODULE_NOT_FOUND]: Cannot find package '/x/y' imported from /x/z.js\n",
                1
            ),
            None
        );
        assert_eq!(problem("", 1), None);
    }

    /// The service named by a local-service problem, or `None`.
    fn service(output: &str) -> Option<String> {
        problem(output, 1).and_then(|p| p.missing_service)
    }

    #[test]
    fn libpq_refused_on_localhost_is_a_setup_problem() {
        // psycopg2 / psycopg / SQLAlchemy over libpq 14+.
        let out = "E   psycopg2.OperationalError: connection to server at \"localhost\" (127.0.0.1), port 5432 failed: Connection refused\n\tIs the server running on that host and accepting TCP/IP connections?\n";
        let p = problem(out, 1).unwrap();
        assert_eq!(
            p.missing_service.as_deref(),
            Some("PostgreSQL on localhost:5432")
        );
        assert_eq!(p.missing_program, None);
        assert_eq!(
            p.reason,
            "could not connect to PostgreSQL on localhost:5432; the service is probably not running, so start it"
        );
        // Windows, IPv6 loopback.
        assert_eq!(
            service("connection to server at \"localhost\" (::1), port 5432 failed: Connection refused (0x0000274D/10061)\n").as_deref(),
            Some("PostgreSQL on localhost:5432")
        );
        // psycopg 3 with an IP as the host, and a port of its own.
        assert_eq!(
            service("psycopg.OperationalError: connection failed: connection to server at \"127.0.0.1\", port 5433 failed: Connection refused\n").as_deref(),
            Some("PostgreSQL on localhost:5433")
        );
        // SQLAlchemy wrapping psycopg2.
        assert_eq!(
            service("sqlalchemy.exc.OperationalError: (psycopg2.OperationalError) connection to server at \"0.0.0.0\", port 5432 failed: Connection refused\n").as_deref(),
            Some("PostgreSQL on localhost:5432")
        );
        // On a Unix socket, which is always local.
        assert_eq!(
            service("connection to server on socket \"/var/run/postgresql/.s.PGSQL.5432\" failed: No such file or directory\n\tIs the server running locally and accepting connections on that socket?\n").as_deref(),
            Some("PostgreSQL on localhost:5432")
        );
    }

    #[test]
    fn libpq_refused_on_a_remote_host_is_a_normal_failure() {
        assert_eq!(
            problem("connection to server at \"db.internal\" (10.0.0.5), port 5432 failed: Connection refused\n", 1),
            None
        );
        assert_eq!(
            problem(
                "connection to server at \"10.0.0.5\", port 5432 failed: Connection refused\n",
                1
            ),
            None
        );
        // The server answered: not a service that is down.
        assert_eq!(
            problem("connection to server at \"localhost\" (127.0.0.1), port 5432 failed: FATAL:  password authentication failed for user \"app\"\n", 1),
            None
        );
        assert_eq!(
            problem("connection to server on socket \"/tmp/.s.PGSQL.5432\" failed: FATAL:  role \"app\" does not exist\n", 1),
            None
        );
    }

    #[test]
    fn old_libpq_refused_is_caught_on_localhost_only() {
        let local = "psycopg2.OperationalError: could not connect to server: Connection refused\n\tIs the server running on host \"localhost\" (127.0.0.1) and accepting\n\tTCP/IP connections on port 5432?\n";
        assert_eq!(
            service(local).as_deref(),
            Some("PostgreSQL on localhost:5432")
        );
        let remote = "psycopg2.OperationalError: could not connect to server: Connection refused\n\tIs the server running on host \"db.internal\" (10.0.0.5) and accepting\n\tTCP/IP connections on port 5432?\n";
        assert_eq!(problem(remote, 1), None);
        // No host named after it: unsure.
        assert_eq!(
            problem("could not connect to server: Connection refused\n", 1),
            None
        );
    }

    #[test]
    fn asyncio_connect_call_failed_is_caught_on_localhost_only() {
        assert_eq!(
            service(
                "ConnectionRefusedError: [Errno 111] Connect call failed ('127.0.0.1', 5432)\n"
            )
            .as_deref(),
            Some("PostgreSQL on localhost:5432")
        );
        assert_eq!(
            service("OSError: Multiple exceptions: [Errno 111] Connect call failed ('::1', 6379, 0, 0), [Errno 111] Connect call failed ('127.0.0.1', 6379)\n").as_deref(),
            Some("Redis on localhost:6379")
        );
        assert_eq!(
            service(
                "ConnectionRefusedError: [Errno 10061] Connect call failed ('127.0.0.1', 8000)\n"
            )
            .as_deref(),
            Some("a service on localhost:8000")
        );
        assert_eq!(
            problem(
                "ConnectionRefusedError: [Errno 111] Connect call failed ('10.0.0.5', 5432)\n",
                1
            ),
            None
        );
        // A timeout is not a refusal.
        assert_eq!(
            problem(
                "TimeoutError: [Errno 110] Connect call failed ('127.0.0.1', 5432)\n",
                1
            ),
            None
        );
    }

    #[test]
    fn a_python_refusal_without_an_address_uses_the_address_in_the_traceback() {
        // pytest prints each frame's arguments above the error.
        let windows = "address = ('localhost', 5432), timeout = 10.0, source_address = None\n>               sock.connect(sa)\nE               ConnectionRefusedError: [WinError 10061] No connection could be made because the target machine actively refused it\n";
        assert_eq!(
            service(windows).as_deref(),
            Some("PostgreSQL on localhost:5432")
        );
        let proactor = "address = ('127.0.0.1', 6379)\nE   ConnectionRefusedError: [WinError 1225] The remote computer refused the network connection\n";
        assert_eq!(
            service(proactor).as_deref(),
            Some("Redis on localhost:6379")
        );
        let linux = "address = ('localhost', 27017)\nE   ConnectionRefusedError: [Errno 111] Connection refused\n";
        assert_eq!(
            service(linux).as_deref(),
            Some("MongoDB on localhost:27017")
        );
        // A remote address, or none at all: a normal failure.
        let remote = "address = ('10.0.0.5', 5432)\nE   ConnectionRefusedError: [WinError 10061] No connection could be made because the target machine actively refused it\n";
        assert_eq!(problem(remote, 1), None);
        assert_eq!(
            problem("ConnectionRefusedError: [WinError 1225] The remote computer refused the network connection\n", 1),
            None
        );
        // An address from another test's section does not count.
        let other = "address = ('localhost', 5432)\n________ test_two ________\nE   ConnectionRefusedError: [Errno 111] Connection refused\n";
        assert_eq!(problem(other, 1), None);
    }

    #[test]
    fn redis_py_refused_is_caught_on_localhost_only() {
        assert_eq!(
            service("redis.exceptions.ConnectionError: Error 111 connecting to localhost:6379. Connection refused.\n").as_deref(),
            Some("Redis on localhost:6379")
        );
        assert_eq!(
            service("redis.exceptions.ConnectionError: Error 10061 connecting to localhost:6379. No connection could be made because the target machine actively refused it.\n").as_deref(),
            Some("Redis on localhost:6379")
        );
        assert_eq!(
            problem("redis.exceptions.ConnectionError: Error 111 connecting to db.internal:6379. Connection refused.\n", 1),
            None
        );
        assert_eq!(
            problem("redis.exceptions.TimeoutError: Error 110 connecting to localhost:6379. Connection timed out.\n", 1),
            None
        );
    }

    #[test]
    fn node_econnrefused_is_caught_on_localhost_only() {
        assert_eq!(
            service("Error: connect ECONNREFUSED 127.0.0.1:5432\n    at TCPConnectWrap.afterConnect [as oncomplete] (node:net:1607:16)\n").as_deref(),
            Some("PostgreSQL on localhost:5432")
        );
        assert_eq!(
            service("[ioredis] Unhandled error event: Error: connect ECONNREFUSED ::1:6379\n")
                .as_deref(),
            Some("Redis on localhost:6379")
        );
        assert_eq!(
            service(
                "TypeError: fetch failed\n  [cause]: Error: connect ECONNREFUSED 127.0.0.1:3000\n"
            )
            .as_deref(),
            Some("a service on localhost:3000")
        );
        assert_eq!(
            problem("Error: connect ECONNREFUSED 10.0.0.5:5432\n", 1),
            None
        );
    }

    #[test]
    fn mysql_refused_is_caught_on_localhost_only() {
        assert_eq!(
            service("MySQLdb.OperationalError: (2003, \"Can't connect to MySQL server on 'localhost' (111)\")\n").as_deref(),
            Some("MySQL on localhost:3306")
        );
        assert_eq!(
            service("mysql.connector.errors.DatabaseError: 2003 (HY000): Can't connect to MySQL server on 'localhost:3307' (10061)\n").as_deref(),
            Some("MySQL on localhost:3307")
        );
        assert_eq!(
            service("pymysql.err.OperationalError: (2003, \"Can't connect to MySQL server on 'localhost' ([Errno 111] Connection refused)\")\n").as_deref(),
            Some("MySQL on localhost:3306")
        );
        assert_eq!(
            service("pymysql.err.OperationalError: (2003, \"Can't connect to MySQL server on '127.0.0.1' ([WinError 10061] No connection could be made because the target machine actively refused it)\")\n").as_deref(),
            Some("MySQL on localhost:3306")
        );
        assert_eq!(
            service("ERROR 2002 (HY000): Can't connect to local MySQL server through socket '/var/run/mysqld/mysqld.sock' (2)\n").as_deref(),
            Some("MySQL on localhost")
        );
        assert_eq!(
            problem(
                "(2003, \"Can't connect to MySQL server on 'db.internal' (111)\")\n",
                1
            ),
            None
        );
        assert_eq!(
            problem("(2003, \"Can't connect to MySQL server on 'db.internal' ([Errno 111] Connection refused)\")\n", 1),
            None
        );
        // A timeout is not a refusal.
        assert_eq!(
            problem(
                "(2003, \"Can't connect to MySQL server on 'localhost' (110)\")\n",
                1
            ),
            None
        );
    }

    #[test]
    fn prisma_unreachable_is_caught_on_localhost_only() {
        assert_eq!(
            service("Error: P1001: Can't reach database server at `localhost:5432`\n\nPlease make sure your database server is running at `localhost:5432`.\n").as_deref(),
            Some("PostgreSQL on localhost:5432")
        );
        assert_eq!(
            service("Can't reach database server at `127.0.0.1`:`3306`\n").as_deref(),
            Some("MySQL on localhost:3306")
        );
        assert_eq!(
            problem(
                "Error: P1001: Can't reach database server at `db.internal:5432`\n",
                1
            ),
            None
        );
    }

    #[test]
    fn pymongo_refused_is_caught_on_localhost_only() {
        assert_eq!(
            service("pymongo.errors.ServerSelectionTimeoutError: localhost:27017: [Errno 111] Connection refused, Timeout: 30s, Topology Description: <...>\n").as_deref(),
            Some("MongoDB on localhost:27017")
        );
        assert_eq!(
            service("pymongo.errors.ServerSelectionTimeoutError: localhost:27017: [WinError 10061] No connection could be made because the target machine actively refused it, Timeout: 30s\n").as_deref(),
            Some("MongoDB on localhost:27017")
        );
        assert_eq!(
            problem("pymongo.errors.ServerSelectionTimeoutError: 10.0.0.5:27017: [Errno 111] Connection refused, Timeout: 30s\n", 1),
            None
        );
    }

    #[test]
    fn the_course_project_shape_is_a_setup_problem() {
        let mut out = String::from("============================= test session starts =============================\nplatform win32 -- Python 3.12.4, pytest-8.3.2\ncollected 236 items\n\ntests\\test_users.py ......EEEE\n");
        out.push_str(
            "=================================== ERRORS ====================================\n",
        );
        out.push_str("__________________ ERROR at setup of test_create_user __________________\n");
        out.push_str("E   psycopg2.OperationalError: connection to server at \"localhost\" (::1), port 5432 failed: Connection refused (0x0000274D/10061)\n");
        out.push_str(
            "E   \tIs the server running on that host and accepting TCP/IP connections?\n",
        );
        out.push_str("E   connection to server at \"localhost\" (127.0.0.1), port 5432 failed: Connection refused (0x0000274D/10061)\n");
        out.push_str(
            "=========================== short test summary info ===========================\n",
        );
        for n in 0..187 {
            out.push_str(&format!("ERROR tests/test_users.py::test_{n} - psycopg2.OperationalError: connection to server at \"localhost\" (::1), port 5432 failed: Connection refused (0x0000274D/10061)\n"));
        }
        out.push_str("============ 49 passed, 187 errors in 780.12s (0:13:00) ============\n");
        let p = problem(&out, 1).unwrap();
        assert_eq!(
            p.missing_service.as_deref(),
            Some("PostgreSQL on localhost:5432")
        );
        assert_eq!(p.missing_program, None);
        assert_eq!(p.start_services, None);
    }

    #[test]
    fn a_local_service_that_is_down_is_setup_even_when_tests_failed() {
        let out = "FAILED tests/test_cache.py::test_get - redis.exceptions.ConnectionError: Error 111 connecting to localhost:6379. Connection refused.\n===== 3 failed, 2 passed in 0.12s =====\n";
        assert_eq!(service(out).as_deref(), Some("Redis on localhost:6379"));
        let vitest =
            "Error: connect ECONNREFUSED 127.0.0.1:5432\n Tests  2 failed | 4 passed (6)\n";
        assert_eq!(
            service(vitest).as_deref(),
            Some("PostgreSQL on localhost:5432")
        );
        // A remote host among failed tests stays a normal failure.
        let remote =
            "FAILED tests/test_db.py::test_q - connect ECONNREFUSED 10.0.0.5:5432\n1 failed\n";
        assert_eq!(problem(remote, 1), None);
    }

    const REFUSED: &str = "Error: connect ECONNREFUSED 127.0.0.1:5432\n";

    #[test]
    fn a_compose_file_in_the_step_folder_is_started_there() {
        let fixture = Fixture::new(
            &[("svc/compose.yaml", ""), ("svc/main.py", "")],
            Track::NoGit,
        )
        .unwrap();
        let dir = fixture.root().join("svc");
        let p = setup_problem(REFUSED, Some(1), fixture.root(), &dir).unwrap();
        assert_eq!(p.start_services.as_deref(), Some("docker compose up -d"));
    }

    #[test]
    fn a_compose_file_above_the_step_folder_is_named_with_a_relative_path() {
        let fixture = Fixture::new(
            &[
                ("docker-compose.yml", ""),
                ("backend/audit_service/main.py", ""),
            ],
            Track::NoGit,
        )
        .unwrap();
        let dir = fixture.root().join("backend").join("audit_service");
        let p = setup_problem(REFUSED, Some(1), fixture.root(), &dir).unwrap();
        assert_eq!(
            p.start_services.as_deref(),
            Some("docker compose -f ../../docker-compose.yml up -d")
        );
        // The nearest file wins.
        let fixture = Fixture::new(
            &[
                ("docker-compose.yml", ""),
                ("backend/compose.yml", ""),
                ("backend/audit_service/main.py", ""),
            ],
            Track::NoGit,
        )
        .unwrap();
        let dir = fixture.root().join("backend").join("audit_service");
        let p = setup_problem(REFUSED, Some(1), fixture.root(), &dir).unwrap();
        assert_eq!(
            p.start_services.as_deref(),
            Some("docker compose -f ../compose.yml up -d")
        );
    }

    #[test]
    fn no_compose_file_up_to_the_root_means_no_start_command() {
        assert_eq!(problem(REFUSED, 1).unwrap().start_services, None);
        // A compose file above the root does not count.
        let fixture = Fixture::new(
            &[("compose.yaml", ""), ("repo/svc/main.py", "")],
            Track::NoGit,
        )
        .unwrap();
        let root = fixture.root().join("repo");
        let p = setup_problem(REFUSED, Some(1), &root, &root.join("svc")).unwrap();
        assert_eq!(p.start_services, None);
    }
}
