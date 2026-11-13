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

use std::path::{Path, PathBuf};

/// Why a failed step looks like an environment that is not set up.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SetupProblem {
    /// What is missing, in a few words: "the Python module 'jwt' is not
    /// installed".
    pub reason: String,
    /// A program that is not on the PATH at all, such as `python` or `pnpm`.
    /// Installing the project's dependencies does not bring it back.
    pub missing_program: Option<String>,
    /// A local service that could not be reached, such as `PostgreSQL on
    /// localhost:5432`, or a local database that refused the login. Starting
    /// the service, or fixing the login settings, fixes it, not an install.
    pub missing_service: Option<String>,
    /// The command that starts the services of a Docker Compose file in the
    /// step's folder or a folder above it, to be run in the step's folder:
    /// `docker compose up -d`, or `docker compose -f ../compose.yaml up -d`.
    pub start_services: Option<String>,
    /// A Node package that is listed but not installed: the install that
    /// keeps the lockfile works only when the lockfile names it (ADR 0043).
    pub missing_package: Option<String>,
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
            missing_package: None,
        }
    } else {
        SetupProblem {
            reason: format!(
                "the command `{name}` was not found; the dependencies are probably not installed"
            ),
            missing_program: None,
            missing_service: None,
            start_services: None,
            missing_package: None,
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
        missing_package: None,
    }
}

/// A refused connection to a service on this machine: a database or cache
/// that is not running. Connections to other hosts are not counted.
fn service_problem(output: &str, root: &Path, dir: &Path) -> Option<SetupProblem> {
    refused_service(output).map(|found| service_down(&found.described, root, dir))
}

/// A failed login to a database on this machine, found in output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthFailure {
    /// The host as the message names it, when it does.
    pub host: Option<String>,
    /// The port, when the message names it.
    pub port: Option<u16>,
    /// `PostgreSQL on localhost:5433`, or `PostgreSQL` when the message names no address. Never a user name.
    pub described: String,
}

/// What PostgreSQL answers to a wrong user name or password. libpq,
/// psycopg, asyncpg and node-postgres all print it.
const PG_AUTH_FAILED: &str = "password authentication failed for user";

/// What a failed login names: the kind of database, and the host and port
/// when the message has them. Never the user name.
#[derive(Debug, Clone, Copy)]
struct Login<'a> {
    kind: &'static str,
    host: Option<&'a str>,
    port: Option<u16>,
}

/// libpq 14 and later (psycopg2, psycopg, SQLAlchemy over them):
/// `connection to server at "localhost" (127.0.0.1), port 5433 failed: FATAL:  password authentication failed for user "x"`.
fn pg_auth_at(line: &str) -> Option<Login<'_>> {
    const AT: &str = "connection to server at \"";
    let rest = &line[line.find(AT)? + AT.len()..];
    let host = &rest[..rest.find('"')?];
    let after = &rest[host.len()..];
    let failed = after.find(" failed: ")?;
    if !after[failed..].contains(PG_AUTH_FAILED) {
        return None;
    }
    let port = after[..failed]
        .rfind("port ")
        .and_then(|index| port_at(&after[index + "port ".len()..]));
    Some(Login {
        kind: "PostgreSQL",
        host: Some(host),
        port: port.or(Some(5432)),
    })
}

/// libpq 14 and later on a Unix socket, which is always local:
/// `connection to server on socket "/tmp/.s.PGSQL.5432" failed: FATAL:  password authentication failed for user "x"`.
fn pg_auth_socket(line: &str) -> Option<Login<'_>> {
    const ON: &str = "connection to server on socket \"";
    let rest = &line[line.find(ON)? + ON.len()..];
    let path = &rest[..rest.find('"')?];
    if !rest[path.len()..].contains(PG_AUTH_FAILED) {
        return None;
    }
    let port = path
        .rfind(".s.PGSQL.")
        .and_then(|index| port_at(&path[index + ".s.PGSQL.".len()..]));
    Some(Login {
        kind: "PostgreSQL",
        host: Some("localhost"),
        port: port.or(Some(5432)),
    })
}

/// PostgreSQL's message with no address, after an error or `FATAL:` label:
/// `FATAL:  password authentication failed for user "x"` (libpq 13 and
/// earlier), `asyncpg.exceptions.InvalidPasswordError: password ...`
/// (also inside SQLAlchemy's `<class '...InvalidPasswordError'>: password ...`),
/// `error: password authentication failed for user "x"` (node-postgres). The
/// label keeps out a test that only quotes the message.
fn pg_auth_plain(line: &str) -> Option<Login<'_>> {
    let index = line.find(PG_AUTH_FAILED)?;
    let before = line[..index].trim_end().strip_suffix(':')?;
    let label = before
        .rsplit(char::is_whitespace)
        .next()
        .unwrap_or(before)
        .to_ascii_lowercase();
    if !label.contains("fatal") && !label.contains("error") {
        return None;
    }
    Some(Login {
        kind: "PostgreSQL",
        host: None,
        port: None,
    })
}

/// MySQL and MariaDB, error 1045:
/// `Access denied for user 'x'@'localhost' (using password: YES)` (the
/// `mysql` client prints `ERROR 1045 (28000): ` before it; PyMySQL and
/// mysqlclient `(1045, "...")`). The host after `@` is where the server sees
/// the client come from: `localhost` or `127.0.0.1` only when both are on
/// this machine. `to database '...'` (error 1044, a missing grant) is not a
/// login failure and is not matched.
fn mysql_auth(line: &str) -> Option<Login<'_>> {
    const DENIED: &str = "Access denied for user ";
    let rest = &line[line.find(DENIED)? + DENIED.len()..];
    let account = &rest[..rest.find(" (using password: ")?];
    let host = account
        .rsplit_once('@')
        .map(|(_, host)| host.trim_matches(['\'', '"', '`']))
        .filter(|host| !host.is_empty());
    Some(Login {
        kind: "MySQL",
        host,
        port: None,
    })
}

/// Prisma P1000: ``Authentication failed against database server at `localhost`, the provided database credentials for `x` are not valid.``
/// Prisma does not say which database it is.
fn prisma_auth(line: &str) -> Option<Login<'_>> {
    const AT: &str = "Authentication failed against database server at ";
    let rest = &line[line.find(AT)? + AT.len()..];
    let token = rest.split([',', ' ']).next()?;
    let location = token.strip_prefix('`')?.strip_suffix('`')?;
    let (host, port) = if is_local_host(location) {
        (location, None)
    } else {
        match host_port(location) {
            Some((host, port)) => (host, Some(port)),
            None => (location, None),
        }
    };
    Some(Login {
        kind: "the database",
        host: Some(host),
        port,
    })
}

/// `PostgreSQL on localhost:5433`, `MySQL on localhost`, or the kind of
/// database alone when the message names no address.
fn describe_login(login: Login<'_>) -> String {
    match (login.host, login.port) {
        (Some(_), Some(port)) => format!("{} on localhost:{port}", login.kind),
        (Some(_), None) => format!("{} on localhost", login.kind),
        (None, _) => login.kind.to_owned(),
    }
}

/// The first failed login to a database on this machine in `output`. A
/// message that names another host is not counted. A message that names
/// no address counts, unless the output also has a failed login to another
/// host: then it is most likely the same one.
#[must_use]
pub fn auth_failure(output: &str) -> Option<AuthFailure> {
    let mut unaddressed = None;
    let mut remote = false;
    for raw in output.lines() {
        let line = raw.trim();
        // A test that asserts on the message quotes it; it did not log in.
        if quotes_a_message(line) {
            continue;
        }
        let found = pg_auth_at(line)
            .or_else(|| pg_auth_socket(line))
            .or_else(|| mysql_auth(line))
            .or_else(|| prisma_auth(line))
            .or_else(|| pg_auth_plain(line));
        let Some(login) = found else {
            continue;
        };
        match login.host {
            Some(host) if is_local_host(host) => return Some(auth_found(login)),
            Some(_) => remote = true,
            None => {
                unaddressed.get_or_insert(login);
            }
        }
    }
    unaddressed.filter(|_| !remote).map(auth_found)
}

/// True for a line that only quotes a login failure: an `assert` (pytest
/// prints the failing line, `E   assert "..." in ...`), or the message
/// inside an open string.
fn quotes_a_message(line: &str) -> bool {
    let code = line.trim_start_matches(['E', '>', ' ']);
    if code.starts_with("assert ") || line.contains(" assert ") || line.contains(" - assert ") {
        return true;
    }
    let Some(index) = line
        .find("password authentication failed")
        .or_else(|| line.find("Access denied for user"))
    else {
        return false;
    };
    let before = &line[..index];
    // `(1045, "Access denied ...` is how PyMySQL prints the error itself.
    let opens_mysql_tuple = before.trim_end().ends_with("(1045, \"");
    !opens_mysql_tuple
        && (before.matches('"').count() % 2 == 1 || before.matches('\'').count() % 2 == 1)
}

fn auth_found(login: Login<'_>) -> AuthFailure {
    AuthFailure {
        host: login.host.map(str::to_owned),
        port: login.port,
        described: describe_login(login),
    }
}

/// The setup problem of a failed login to `described`. Starting a service
/// does not fix it, so there is no start command; `missing_service` names
/// the database, which also keeps verify from offering the install command.
fn login_failed(described: &str) -> SetupProblem {
    SetupProblem {
        reason: format!(
            "could not log in to {described}: password authentication failed; the user or password in the test settings does not match the database"
        ),
        missing_program: None,
        missing_service: Some(described.to_owned()),
        start_services: None,
        missing_package: None,
    }
}

// --- Node packages that are not installed

/// The package a bare import names: `react` for `react`, `vite` for
/// `vite/client`, `@eslint/js` for `@eslint/js` and `@scope/x/y`. `None` for
/// anything that is not a package name: a relative or absolute path, a
/// drive letter, a Node built-in (`node:fs`), a subpath import (`#x`), or a
/// path alias such as `@/components`, `~/x` or `$lib/x`.
fn package_of(name: &str) -> Option<&str> {
    let name = name.trim();
    let valid = |part: &str| {
        !part.is_empty()
            && !part.starts_with('.')
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~'))
            && !part.starts_with('~')
    };
    if name.contains(':') || name.contains('\\') {
        return None;
    }
    if let Some(rest) = name.strip_prefix('@') {
        let (scope, after) = rest.split_once('/')?;
        let package = after.split('/').next().unwrap_or(after);
        return (valid(scope) && valid(package)).then(|| &name[..scope.len() + package.len() + 2]);
    }
    let package = name.split('/').next().unwrap_or(name);
    valid(package).then_some(package)
}

/// The package that ESLint loads for a shareable config named in `extends`:
/// `next` and `next/core-web-vitals` are `eslint-config-next`, `@scope` is
/// `@scope/eslint-config`, `@scope/x` is `@scope/eslint-config-x`. Names
/// ESLint resolves itself (`eslint:recommended`, `plugin:x/y`) are `None`.
fn eslint_config_package(name: &str) -> Option<String> {
    // A path (`./base.js`) or a name ESLint resolves itself.
    if name.contains(':') || (!name.starts_with('@') && package_of(name).is_none()) {
        return None;
    }
    let full = if let Some(rest) = name.strip_prefix('@') {
        match rest.split_once('/') {
            None => format!("{name}/eslint-config"),
            Some((scope, "" | "eslint-config")) => format!("@{scope}/eslint-config"),
            Some((scope, after)) if after.starts_with("eslint-config") => {
                format!("@{scope}/{after}")
            }
            Some((scope, after)) => format!("@{scope}/eslint-config-{after}"),
        }
    } else if name.starts_with("eslint-config-") {
        name.to_owned()
    } else {
        format!("eslint-config-{name}")
    };
    package_of(&full).map(str::to_owned)
}

/// What a missing Node package is about: a package to import or require, or
/// the type definitions TypeScript looks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wanted {
    Package,
    Types,
}

/// The names a package can be installed under: itself, and for TypeScript
/// its `@types` package (`@types/react`, `@types/scope__name`).
fn install_names(package: &str, typescript: bool) -> Vec<String> {
    let mut names = vec![package.to_owned()];
    if typescript && !package.starts_with("@types/") {
        let types = match package.strip_prefix('@') {
            Some(scoped) => format!("@types/{}", scoped.replacen('/', "__", 1)),
            None => format!("@types/{package}"),
        };
        names.push(types);
    }
    names
}

/// Whether a missing Node package is a setup problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PackageState {
    /// In a `node_modules` folder, or the folder uses Yarn Plug'n'Play: the
    /// install happened, so the error is about the code or the config.
    Present,
    /// Not installed, but no `package.json` lists it: a dependency missing
    /// from the manifest is a change to make in the repo.
    Undeclared,
    /// Listed and not installed: installing the dependencies fixes it.
    Missing,
}

/// Looks for any of `names` in `node_modules` and in the `package.json`
/// dependency lists of every folder from each of `starts` up to `root`. Node
/// resolves a bare import the same way, from the importing file's folder up.
fn package_state(names: &[String], root: &Path, starts: &[PathBuf]) -> PackageState {
    const LISTS: [&str; 4] = [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ];
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut manifests = false;
    let mut declared = false;
    for start in starts {
        let mut current = Some(start.as_path());
        while let Some(folder) = current {
            if !seen.iter().any(|done| done == folder) {
                seen.push(folder.to_path_buf());
                let modules = folder.join("node_modules");
                let present = names.iter().any(|name| {
                    name.split('/')
                        .fold(modules.clone(), |path, part| path.join(part))
                        .exists()
                });
                if present || folder.join(".pnp.cjs").is_file() || folder.join(".pnp.js").is_file()
                {
                    return PackageState::Present;
                }
                let manifest = std::fs::read_to_string(folder.join("package.json"))
                    .ok()
                    .and_then(|text| {
                        serde_json::from_str::<serde_json::Value>(
                            text.trim_start_matches('\u{feff}'),
                        )
                        .ok()
                    });
                if let Some(manifest) = manifest {
                    manifests = true;
                    declared |= LISTS.iter().any(|list| {
                        manifest
                            .get(list)
                            .and_then(serde_json::Value::as_object)
                            .is_some_and(|deps| names.iter().any(|name| deps.contains_key(name)))
                    });
                }
            }
            if folder == root {
                break;
            }
            current = folder.parent().filter(|parent| parent.starts_with(root));
        }
    }
    if manifests && !declared {
        PackageState::Undeclared
    } else {
        PackageState::Missing
    }
}

/// The folder of a file a message names, when it is inside the repo:
/// absolute, or relative to the step's folder `dir`.
fn folder_of_file(file: &str, root: &Path, dir: &Path) -> Option<PathBuf> {
    let file = file.trim().trim_matches(['\'', '"']);
    if file.is_empty() || file.starts_with("file://") {
        return None;
    }
    // On Windows a verbatim path (`\\?\C:\...`) does not take `/` as a
    // separator, and messages mix both.
    #[cfg(windows)]
    let file = &file.replace('/', "\\");
    let path = Path::new(file);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        file.split(['/', '\\'])
            .filter(|part| !part.is_empty() && *part != ".")
            .fold(dir.to_path_buf(), |path, part| path.join(part))
    };
    let folder = path.parent()?;
    folder.starts_with(root).then(|| folder.to_path_buf())
}

/// The file that imported a missing package, from the message itself or the
/// lines after it: `... imported from /x/eslint.config.js` (ESM), the first
/// entry under `Require stack:` (CommonJS), or the file a TypeScript error
/// starts with (`src/main.tsx(1,20): error TS2307` or
/// `src/main.tsx:1:20 - error TS2307`).
fn importer(lines: &[&str], index: usize, root: &Path, dir: &Path) -> Option<PathBuf> {
    let line = lines[index].trim();
    if let Some(at) = line.find(" imported from ") {
        return folder_of_file(&line[at + " imported from ".len()..], root, dir);
    }
    if let Some(at) = line.find("error TS") {
        let file = line[..at]
            .trim_end()
            .trim_end_matches(['-', ':'])
            .trim_end();
        let file = match file.strip_suffix(')') {
            Some(open) => &open[..open.rfind('(')?],
            None => file
                .trim_end_matches(|c: char| c.is_ascii_digit() || c == ':')
                .trim_end(),
        };
        return folder_of_file(file, root, dir);
    }
    let rest = lines.get(index + 1..)?;
    let stack = rest
        .iter()
        .take(3)
        .position(|next| next.trim() == "Require stack:")?;
    let first = rest.get(stack + 1)?.trim().strip_prefix("- ")?;
    folder_of_file(first, root, dir)
}

/// A missing Node package named on line `index`, and what is missing: from
/// Node (ESM and CommonJS), ESLint, and TypeScript. `Some(None)` when the
/// line is such a message about something that is not a package (a path, an
/// alias, a built-in): a mistake in the code.
fn node_missing(lines: &[&str], index: usize, has_stack: bool) -> Option<Option<(String, Wanted)>> {
    let line = lines[index].trim();
    let package = |name: &str| Some(package_of(name).map(|p| (p.to_owned(), Wanted::Package)));
    // Node ESM: `Error [ERR_MODULE_NOT_FOUND]: Cannot find package 'vitest' imported from ...`.
    if line.contains("ERR_MODULE_NOT_FOUND") || line.starts_with("Error: Cannot find package") {
        if let Some(at) = line.find("Cannot find package ") {
            return package(name_after(line, at + "Cannot find package ".len())?);
        }
    }
    // TypeScript: `error TS2307: Cannot find module 'react' or its corresponding type declarations.`
    if let Some(at) = line.find("TS2307: Cannot find module ") {
        let name = name_after(line, at + "TS2307: Cannot find module ".len())?;
        return Some(package_of(name).map(|p| (p.to_owned(), Wanted::Types)));
    }
    // TypeScript: `error TS2688: Cannot find type definition file for 'vite/client'.`
    if let Some(at) = line.find("TS2688: Cannot find type definition file for ") {
        let name = name_after(
            line,
            at + "TS2688: Cannot find type definition file for ".len(),
        )?;
        return Some(package_of(name).map(|p| (p.to_owned(), Wanted::Types)));
    }
    // Node CommonJS, with its `Require stack:` or `MODULE_NOT_FOUND` code:
    // `Error: Cannot find module 'eslint-plugin-react'`, also inside ESLint 8's
    // `Failed to load plugin 'react' declared in '.eslintrc': Cannot find module ...`.
    if let Some(at) = line.find("Cannot find module ") {
        let cjs = (has_stack && line.contains("Error"))
            || line.contains("Failed to load plugin ")
            || line.contains("Failed to load parser ");
        if cjs && !line.contains("imported from") {
            return package(name_after(line, at + "Cannot find module ".len())?);
        }
    }
    // ESLint 8: `ESLint couldn't find the plugin "eslint-plugin-react".`
    if let Some(at) = line.find("ESLint couldn't find the plugin ") {
        return package(name_after(
            line,
            at + "ESLint couldn't find the plugin ".len(),
        )?);
    }
    // ESLint 8: `ESLint couldn't find the config "next" to extend from.` (the
    // CLI) and `Failed to load config "next" to extend from.` (the message
    // itself, which `next lint` and editors show).
    for marker in ["ESLint couldn't find the config ", "Failed to load config "] {
        if let Some(at) = line.find(marker) {
            if line.contains(" to extend from") {
                let name = name_after(line, at + marker.len())?;
                return Some(eslint_config_package(name).map(|p| (p, Wanted::Package)));
            }
        }
    }
    None
}

/// The setup problem of a Node package that is missing on line `index`;
/// `Some(None)` when that line shows a mistake in the code instead.
fn node_package_problem(
    lines: &[&str],
    index: usize,
    has_stack: bool,
    root: &Path,
    dir: &Path,
) -> Option<Option<SetupProblem>> {
    let found = node_missing(lines, index, has_stack)?;
    let Some((package, wanted)) = found else {
        return Some(None);
    };
    let mut starts = vec![dir.to_path_buf()];
    starts.extend(importer(lines, index, root, dir));
    let names = install_names(&package, wanted == Wanted::Types);
    if package_state(&names, root, &starts) != PackageState::Missing {
        return Some(None);
    }
    let reason = if lines[index].contains("TS2688") {
        let name = lines[index]
            .find("file for ")
            .and_then(|at| name_after(lines[index], at + "file for ".len()))
            .unwrap_or(&package);
        format!("the type definitions for '{name}' are not installed")
    } else {
        format!("the package '{package}' is not installed")
    };
    Some(Some(SetupProblem {
        reason,
        missing_program: None,
        missing_service: None,
        start_services: None,
        missing_package: Some(package),
    }))
}

// --- Installing without rewriting the lockfile

/// True for a Yarn 2 or later project: `packageManager` names `yarn@2` or
/// later in `dir` or the root, or else there is a `.yarnrc.yml` from `dir` up
/// to the root, or the lockfile has Yarn 2's `__metadata:` header.
fn is_yarn_berry(root: &Path, dir: &Path) -> bool {
    for folder in [dir, root] {
        let declared = std::fs::read_to_string(folder.join("package.json"))
            .ok()
            .and_then(|text| {
                serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}')).ok()
            })
            .and_then(|json| {
                json.get("packageManager")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            });
        if let Some(version) = declared.as_deref().and_then(|pm| pm.strip_prefix("yarn@")) {
            return leading_digits(version)
                .parse::<u32>()
                .is_ok_and(|major| major >= 2);
        }
    }
    if file_up_to_root(".yarnrc.yml", root, dir) {
        return true;
    }
    std::fs::read_to_string(dir.join("yarn.lock"))
        .is_ok_and(|lock| lock.lines().any(|line| line.trim_end() == "__metadata:"))
}

/// True when `name` is a file in `dir` or a folder above it, up to `root`.
fn file_up_to_root(name: &str, root: &Path, dir: &Path) -> bool {
    let mut current = Some(dir);
    while let Some(folder) = current {
        if folder.join(name).is_file() {
            return true;
        }
        if folder == root {
            break;
        }
        current = folder.parent().filter(|parent| parent.starts_with(root));
    }
    false
}

/// The form of a project's install command that installs exactly what its
/// lockfile says and never rewrites the lockfile, for `doctor` and for
/// verify's setup-needed text. `install` is the command `fscale facts`
/// found, to be run in `dir`. With no lockfile for its package manager, or a
/// command this does not know, `install` comes back as it is.
///
/// - `npm install` with `package-lock.json` or `npm-shrinkwrap.json`: `npm ci`.
/// - `pnpm install` with `pnpm-lock.yaml`: `pnpm install --frozen-lockfile`.
/// - `yarn install` with `yarn.lock`: `yarn install --immutable` for Yarn 2
///   and later, `yarn install --frozen-lockfile` for Yarn 1.
/// - `bun install` with `bun.lock` or `bun.lockb`: `bun install --frozen-lockfile`.
/// - `uv sync` with `uv.lock` here or above (a uv workspace keeps one at
///   its root): `uv sync --locked`.
/// - `pipenv install --dev` with `Pipfile.lock`: `pipenv sync --dev`.
/// - `poetry install` stays: it installs from `poetry.lock` and does not
///   rewrite it.
#[must_use]
pub fn locked_install(install: &str, root: &Path, dir: &Path) -> String {
    let has = |name: &str| dir.join(name).is_file();
    let locked = match install {
        "npm install" if has("package-lock.json") || has("npm-shrinkwrap.json") => "npm ci",
        "pnpm install" if has("pnpm-lock.yaml") => "pnpm install --frozen-lockfile",
        "yarn install" if has("yarn.lock") => {
            if is_yarn_berry(root, dir) {
                "yarn install --immutable"
            } else {
                "yarn install --frozen-lockfile"
            }
        }
        "bun install" if has("bun.lock") || has("bun.lockb") => "bun install --frozen-lockfile",
        "uv sync" if file_up_to_root("uv.lock", root, dir) => "uv sync --locked",
        "pipenv install --dev" if has("Pipfile.lock") => "pipenv sync --dev",
        _ => install,
    };
    locked.to_owned()
}

/// True when a TypeScript or mypy run reports an error other than a
/// package or module that cannot be found.
fn other_type_errors(lines: &[&str]) -> bool {
    lines.iter().any(|line| {
        let ts = line.contains("error TS")
            && !line.contains("error TS2307")
            && !line.contains("error TS2688");
        let mypy = line.contains(": error: ")
            && mypy_missing(line).is_none()
            && !line.contains("[import-untyped]");
        ts || mypy
    })
}

/// True when a dependency list in `dir` or a folder above it, up to the
/// root, names the Python module `module` (its top-level name, with `-`
/// and `_` alike, in any letter case): `requirements*.txt`,
/// `pyproject.toml`, `Pipfile`, `setup.cfg`.
fn python_dependency_named(module: &str, root: &Path, dir: &Path) -> bool {
    let top = module.split('.').next().unwrap_or(module);
    let wanted = top.to_ascii_lowercase().replace('_', "-");
    if wanted.is_empty() {
        return false;
    }
    let mut at = Some(dir);
    while let Some(folder) = at {
        let Ok(entries) = std::fs::read_dir(folder) else {
            break;
        };
        for entry in entries.filter_map(std::result::Result::ok) {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            let listed = (name.starts_with("requirements") && name.ends_with(".txt"))
                || matches!(name.as_str(), "pyproject.toml" | "pipfile" | "setup.cfg");
            if !listed {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            let text = text.to_ascii_lowercase().replace('_', "-");
            let named = text
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '.'))
                .any(|word| word == wanted);
            if named {
                return true;
            }
        }
        if folder == root {
            break;
        }
        at = folder.parent().filter(|up| up.starts_with(root));
    }
    false
}

/// The install to name for `problem`: the lockfile-keeping `install`, or
/// its plain form when the problem is a Node package that the lockfile
/// does not name yet (it was just added to package.json), since `npm ci`
/// and the frozen installs refuse a lockfile that is out of date
/// (ADR 0043).
#[must_use]
pub fn install_for(problem: &SetupProblem, install: &str, root: &Path, dir: &Path) -> String {
    let Some(package) = &problem.missing_package else {
        return install.to_owned();
    };
    let plain = match install {
        "npm ci" => "npm install",
        "pnpm install --frozen-lockfile" => "pnpm install",
        "yarn install --immutable" | "yarn install --frozen-lockfile" => "yarn install",
        "bun install --frozen-lockfile" => "bun install",
        _ => return install.to_owned(),
    };
    let lockfiles = [
        "package-lock.json",
        "npm-shrinkwrap.json",
        "pnpm-lock.yaml",
        "yarn.lock",
        "bun.lock",
        "bun.lockb",
    ];
    let mut at = Some(dir);
    while let Some(folder) = at {
        for name in lockfiles {
            if let Ok(bytes) = std::fs::read(folder.join(name)) {
                let text = String::from_utf8_lossy(&bytes);
                return if names_package(&text, package) {
                    install.to_owned()
                } else {
                    plain.to_owned()
                };
            }
        }
        if folder == root {
            break;
        }
        at = folder.parent().filter(|up| up.starts_with(root));
    }
    install.to_owned()
}

/// True when a lockfile's text names `package` as a whole name:
/// `"node_modules/react"`, `react@18`, `'react':`, `/react@`.
fn names_package(text: &str, package: &str) -> bool {
    text.match_indices(package).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + package.len()..].chars().next();
        let starts = before.is_none_or(|c| matches!(c, '"' | '\'' | '/' | ' ' | '\n' | '\t'));
        let ends = after.is_none_or(|c| matches!(c, '"' | '\'' | '@' | ':'));
        starts && ends
    })
}

/// mypy: `error: Cannot find implementation or library stub for module named "requests"  [import-not-found]`.
fn mypy_missing(line: &str) -> Option<&str> {
    const STUB: &str = "Cannot find implementation or library stub for module named ";
    let at = line.find(STUB)?;
    name_after(line, at + STUB.len())
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
    // A database that refuses the test settings' login fails tests the same
    // way, and no code change fixes it either.
    if let Some(found) = auth_failure(output) {
        // With failed tests, only when every failed test failed on the login
        // itself: a real assertion failure beside it is a code failure.
        let failed: Vec<&str> = output
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with("FAILED "))
            .collect();
        let only_logins =
            !failed.is_empty() && failed.iter().all(|line| auth_failure(line).is_some());
        if !reports_failed_tests(output) || only_logins {
            return Some(login_failed(&found.described));
        }
    }
    if reports_failed_tests(output) {
        return None;
    }
    let lines: Vec<&str> = output.lines().collect();
    let has_stack = output.contains("Require stack:") || output.contains("MODULE_NOT_FOUND");
    for (index, line) in lines.iter().enumerate() {
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
                missing_package: None,
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
                missing_package: None,
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
        // mypy, with the module not installed in its environment. Not when
        // mypy also found other errors (a missing package must not hide a
        // type error), and only for a module a dependency list names, so a
        // typo in an import stays a failure (ADR 0043).
        if let Some(module) = mypy_missing(line) {
            if is_local_module(module, root, dir)
                || other_type_errors(&lines)
                || !python_dependency_named(module, root, dir)
            {
                return None;
            }
            return Some(SetupProblem {
                reason: format!("the Python module '{module}' is not installed"),
                missing_program: None,
                missing_service: None,
                start_services: None,
                missing_package: None,
            });
        }
        // Node, ESLint and TypeScript: a package that is listed in a
        // package.json and is in no node_modules folder. A TypeScript run
        // with other errors beside it is a failure: the missing package
        // must not hide a type error (ADR 0043).
        if let Some(found) = node_package_problem(&lines, index, has_stack, root, dir) {
            if line.contains("error TS") && other_type_errors(&lines) {
                return None;
            }
            return found;
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
        // The server answered: not a service that is down. (A failed
        // password login is setup too, but of its own kind: see
        // `a_failed_login_to_a_local_database_is_a_setup_problem`.)
        assert_eq!(
            refused_service("connection to server at \"localhost\" (127.0.0.1), port 5432 failed: FATAL:  password authentication failed for user \"app\"\n"),
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

    /// The database named by a failed-login problem, or `None`. Checks that
    /// it is the login kind: no start command, and the user name `app_user`
    /// nowhere in it.
    fn login(output: &str) -> Option<String> {
        let found = auth_failure(output);
        let p = problem(output, 1)?;
        if !p.reason.starts_with("could not log in to ") {
            return None;
        }
        assert_eq!(p.start_services, None, "{output}");
        assert_eq!(p.missing_program, None, "{output}");
        assert_eq!(
            p.missing_service,
            found.as_ref().map(|f| f.described.clone())
        );
        let shown = format!("{p:?} {found:?}");
        assert!(!shown.contains("app_user"), "user name leaked: {shown}");
        p.missing_service
    }

    #[test]
    fn a_failed_login_to_a_local_database_is_a_setup_problem() {
        // libpq 14+ (psycopg2, psycopg).
        let libpq = "E   psycopg2.OperationalError: connection to server at \"localhost\" (127.0.0.1), port 5433 failed: FATAL:  password authentication failed for user \"app_user\"\n";
        let p = problem(libpq, 1).unwrap();
        assert_eq!(
            p.reason,
            "could not log in to PostgreSQL on localhost:5433: password authentication failed; the user or password in the test settings does not match the database"
        );
        assert_eq!(
            auth_failure(libpq),
            Some(AuthFailure {
                host: Some("localhost".into()),
                port: Some(5433),
                described: "PostgreSQL on localhost:5433".into(),
            })
        );
        assert_eq!(
            login(libpq).as_deref(),
            Some("PostgreSQL on localhost:5433")
        );
        // psycopg 3 with an IP and no address in brackets.
        assert_eq!(
            login("psycopg.OperationalError: connection failed: connection to server at \"127.0.0.1\", port 5432 failed: FATAL:  password authentication failed for user \"app_user\"\n").as_deref(),
            Some("PostgreSQL on localhost:5432")
        );
        // On a Unix socket.
        assert_eq!(
            login("connection to server on socket \"/tmp/.s.PGSQL.5434\" failed: FATAL:  password authentication failed for user \"app_user\"\n").as_deref(),
            Some("PostgreSQL on localhost:5434")
        );
        // SQLAlchemy wrapping psycopg2.
        assert_eq!(
            login("sqlalchemy.exc.OperationalError: (psycopg2.OperationalError) connection to server at \"localhost\" (::1), port 5433 failed: FATAL:  password authentication failed for user \"app_user\"\n").as_deref(),
            Some("PostgreSQL on localhost:5433")
        );
        // libpq 13 and earlier: no address.
        assert_eq!(
            login("psycopg2.OperationalError: FATAL:  password authentication failed for user \"app_user\"\n").as_deref(),
            Some("PostgreSQL")
        );
        assert_eq!(
            auth_failure("FATAL:  password authentication failed for user \"app_user\"\n"),
            Some(AuthFailure {
                host: None,
                port: None,
                described: "PostgreSQL".into(),
            })
        );
        // asyncpg, alone and inside SQLAlchemy.
        assert_eq!(
            login("E   asyncpg.exceptions.InvalidPasswordError: password authentication failed for user \"app_user\"\n").as_deref(),
            Some("PostgreSQL")
        );
        assert_eq!(
            login("sqlalchemy.exc.DBAPIError: (sqlalchemy.dialects.postgresql.asyncpg.Error) <class 'asyncpg.exceptions.InvalidPasswordError'>: password authentication failed for user \"app_user\"\n").as_deref(),
            Some("PostgreSQL")
        );
        // node-postgres.
        assert_eq!(
            login("error: password authentication failed for user \"app_user\"\n    at Parser.parseErrorMessage (node_modules/pg-protocol/dist/parser.js:283:98)\n").as_deref(),
            Some("PostgreSQL")
        );
        // MySQL: the client, PyMySQL, mysql2.
        assert_eq!(
            login("ERROR 1045 (28000): Access denied for user 'app_user'@'localhost' (using password: YES)\n").as_deref(),
            Some("MySQL on localhost")
        );
        assert_eq!(
            login("pymysql.err.OperationalError: (1045, \"Access denied for user 'app_user'@'127.0.0.1' (using password: YES)\")\n").as_deref(),
            Some("MySQL on localhost")
        );
        assert_eq!(
            login("Error: Access denied for user 'app_user'@'localhost' (using password: NO)\n")
                .as_deref(),
            Some("MySQL on localhost")
        );
        // Prisma P1000.
        assert_eq!(
            login("Error: P1000: Authentication failed against database server at `localhost`, the provided database credentials for `app_user` are not valid.\n").as_deref(),
            Some("the database on localhost")
        );
    }

    #[test]
    fn a_failed_login_is_setup_even_when_tests_failed() {
        let out = "FAILED tests/test_db.py::test_q - asyncpg.exceptions.InvalidPasswordError: password authentication failed for user \"app_user\"\n===== 3 failed in 0.40s =====\n";
        assert_eq!(login(out).as_deref(), Some("PostgreSQL"));
        // A refused connection in the same output comes first.
        let both = "Error: connect ECONNREFUSED 127.0.0.1:6379\nerror: password authentication failed for user \"app_user\"\n";
        assert_eq!(
            problem(both, 1).and_then(|p| p.missing_service).as_deref(),
            Some("Redis on localhost:6379")
        );
    }

    #[test]
    fn a_test_that_quotes_the_message_beside_a_real_failure_is_a_normal_failure() {
        // From the review of 0.2.1: a test asserting on the message, and a
        // plain assertion failure, must not read as "do not change code".
        let out = "E   assert \"FATAL:  password authentication failed for user\" in caplog.text\nFAILED tests/test_login.py::test_login_error_is_shown_to_the_user - assert \"FATAL:  password authentication failed for user\" in ''\nFAILED tests/test_math.py::test_sum - assert 41 == 42\n2 failed in 0.1s\n";
        assert_eq!(auth_failure(out), None);
        assert_eq!(problem(out, 1), None);
        // A real login failure beside an assertion failure: a failure.
        let mixed = "FAILED tests/test_db.py::test_q - asyncpg.exceptions.InvalidPasswordError: password authentication failed for user \"app\"\nFAILED tests/test_math.py::test_sum - assert 41 == 42\n2 failed\n";
        assert_eq!(problem(mixed, 1), None);
        // PyMySQL's own tuple is not a quote.
        let pymysql = "E   pymysql.err.OperationalError: (1045, \"Access denied for user 'app'@'localhost' (using password: YES)\")\n";
        assert!(auth_failure(pymysql).is_some());
    }

    #[test]
    fn a_failed_login_to_a_remote_database_is_a_normal_failure() {
        for out in [
            "connection to server at \"db.internal\" (10.0.0.5), port 5432 failed: FATAL:  password authentication failed for user \"app_user\"\n",
            "connection to server at \"10.0.0.5\", port 5433 failed: FATAL:  password authentication failed for user \"app_user\"\n",
            "ERROR 1045 (28000): Access denied for user 'app_user'@'10.0.0.7' (using password: YES)\n",
            "Error: P1000: Authentication failed against database server at `db.example.com`, the provided database credentials for `app_user` are not valid.\n",
            // The same remote failure, also printed without the address.
            "E   connection to server at \"db.internal\" (10.0.0.5), port 5432 failed: FATAL:  password authentication failed for user \"app_user\"\nFATAL:  password authentication failed for user \"app_user\"\n",
        ] {
            assert_eq!(auth_failure(out), None, "{out}");
            assert_eq!(problem(out, 1), None, "{out}");
        }
    }

    #[test]
    fn other_database_errors_and_quoted_messages_are_not_failed_logins() {
        for out in [
            // A missing grant, a missing role: not a wrong password.
            "ERROR 1044 (42000): Access denied for user 'app_user'@'localhost' to database 'shop'\n",
            "connection to server at \"localhost\" (127.0.0.1), port 5432 failed: FATAL:  role \"app_user\" does not exist\n",
            // A test that only quotes the message.
            "    assert \"password authentication failed for user\" in str(err)\n",
            "",
        ] {
            assert_eq!(auth_failure(out), None, "{out}");
        }
    }

    #[test]
    fn a_failed_login_never_names_the_user() {
        let user = "orders_admin_7";
        for out in [
            format!("connection to server at \"localhost\" (127.0.0.1), port 5433 failed: FATAL:  password authentication failed for user \"{user}\""),
            format!("FATAL:  password authentication failed for user \"{user}\""),
            format!("asyncpg.exceptions.InvalidPasswordError: password authentication failed for user \"{user}\""),
            format!("error: password authentication failed for user \"{user}\""),
            format!("ERROR 1045 (28000): Access denied for user '{user}'@'localhost' (using password: YES)"),
            format!("Access denied for user '{user}@corp'@'localhost' (using password: YES)"),
            format!("Error: P1000: Authentication failed against database server at `localhost`, the provided database credentials for `{user}` are not valid."),
        ] {
            let found = auth_failure(&out).expect(&out);
            let p = problem(&out, 1).expect(&out);
            for text in [format!("{found:?}"), found.described.clone(), p.reason.clone(), format!("{p:?}")] {
                assert!(!text.contains(user), "user name in {text}");
            }
        }
    }

    /// A frontend whose package.json lists react, vite, ESLint and its
    /// plugins, with `installed` packages in `frontend/node_modules`.
    fn frontend(installed: &[&str]) -> Fixture {
        let manifest = r#"{"name":"web","dependencies":{"react":"18.3.1"},"devDependencies":{"@eslint/js":"9.9.0","eslint":"9.9.0","eslint-plugin-react":"7.35.0","eslint-config-next":"14.2.0","@scope/eslint-config":"1.0.0","@scope/eslint-config-strict":"1.0.0","@types/react":"18.3.3","@types/node":"20.0.0","vite":"5.4.0","lodash":"4.17.21"}}"#;
        let markers: Vec<(String, &str)> = installed
            .iter()
            .map(|name| (format!("frontend/node_modules/{name}/package.json"), "{}"))
            .collect();
        let mut files: Vec<(&str, &str)> = vec![
            ("frontend/package.json", manifest),
            ("frontend/src/main.tsx", ""),
        ];
        files.extend(markers.iter().map(|(path, text)| (path.as_str(), *text)));
        Fixture::new(&files, Track::NoGit).unwrap()
    }

    fn in_frontend(output: &str, installed: &[&str]) -> Option<SetupProblem> {
        let fixture = frontend(installed);
        let dir = fixture.root().join("frontend");
        let output = output.replace("{dir}", &dir.to_string_lossy());
        setup_problem(&output, Some(2), fixture.root(), &dir)
    }

    /// The real messages, with `{dir}` for the frontend's folder.
    const ESLINT_FLAT: &str = "\nOops! Something went wrong! :(\n\nESLint: 9.9.0\n\nError [ERR_MODULE_NOT_FOUND]: Cannot find package '@eslint/js' imported from {dir}/eslint.config.js\n    at packageResolve (node:internal/modules/esm/resolve:854:9)\n";
    const ESLINT_CJS: &str = "\nOops! Something went wrong! :(\n\nESLint: 9.9.0\n\nError: Cannot find module 'eslint-plugin-react'\nRequire stack:\n- {dir}/eslint.config.cjs\n    at Module._resolveFilename (node:internal/modules/cjs/loader:1225:15)\n";
    const ESLINT8_PLUGIN: &str = "\nOops! Something went wrong! :(\n\nESLint: 8.57.0\n\nESLint couldn't find the plugin \"eslint-plugin-react\".\n\n(The package \"eslint-plugin-react\" was not found when loaded as a Node module from the directory \"{dir}\".)\n\nIt's likely that the plugin isn't installed correctly. Try reinstalling by running the following:\n\n    npm install eslint-plugin-react@latest --save-dev\n\nThe plugin \"eslint-plugin-react\" was referenced from the config file in \".eslintrc.cjs\".\n";
    const ESLINT8_FAILED_PLUGIN: &str = "Failed to load plugin 'react' declared in '.eslintrc.json': Cannot find module 'eslint-plugin-react'\nRequire stack:\n- {dir}/__placeholder__.js\n";
    const ESLINT8_CONFIG: &str = "\nOops! Something went wrong! :(\n\nESLint: 8.57.0\n\nESLint couldn't find the config \"next/core-web-vitals\" to extend from. Please check that the name of the config is correct.\n\nThe config \"next/core-web-vitals\" was referenced from the config file in \"{dir}/.eslintrc.json\".\n";
    const NEXT_LINT_CONFIG: &str =
        "Failed to load config \"next\" to extend from.\nReferenced from: {dir}/.eslintrc.json\n";
    const TS2307: &str = "src/main.tsx(1,19): error TS2307: Cannot find module 'react' or its corresponding type declarations.\n";
    const TS2307_PRETTY: &str = "src/main.tsx:1:19 - error TS2307: Cannot find module 'react' or its corresponding type declarations.\n\n1 import React from 'react';\n                    ~~~~~~~\n\nFound 1 error in src/main.tsx:1\n";
    const TS2688: &str = "error TS2688: Cannot find type definition file for 'vite/client'.\n  The file is in the program because:\n    Entry point of type library 'vite/client' specified in compilerOptions\n";

    #[test]
    fn lint_and_typecheck_on_a_package_that_is_not_installed_are_setup_problems() {
        for (output, reason) in [
            (ESLINT_FLAT, "the package '@eslint/js' is not installed"),
            (
                ESLINT_CJS,
                "the package 'eslint-plugin-react' is not installed",
            ),
            (
                ESLINT8_PLUGIN,
                "the package 'eslint-plugin-react' is not installed",
            ),
            (
                ESLINT8_FAILED_PLUGIN,
                "the package 'eslint-plugin-react' is not installed",
            ),
            (
                ESLINT8_CONFIG,
                "the package 'eslint-config-next' is not installed",
            ),
            (
                NEXT_LINT_CONFIG,
                "the package 'eslint-config-next' is not installed",
            ),
            (TS2307, "the package 'react' is not installed"),
            (TS2307_PRETTY, "the package 'react' is not installed"),
            (
                TS2688,
                "the type definitions for 'vite/client' are not installed",
            ),
        ] {
            let found = in_frontend(output, &[]);
            assert_eq!(
                found.as_ref().map(|p| p.reason.as_str()),
                Some(reason),
                "{output}"
            );
            let found = found.unwrap();
            assert_eq!(found.missing_program, None);
            assert_eq!(found.missing_service, None);
        }
        // With no node_modules folder at all, as on a fresh clone, and on Windows.
        let windows = "Error [ERR_MODULE_NOT_FOUND]: Cannot find package 'vite' imported from C:\\Users\\me\\web\\vite.config.ts.timestamp-1.mjs\n";
        assert_eq!(
            in_frontend(windows, &[]).map(|p| p.reason),
            Some("the package 'vite' is not installed".to_owned())
        );
    }

    #[test]
    fn a_package_that_is_installed_is_a_normal_failure() {
        // The package is there: its error is about the code or the config.
        for (output, installed) in [
            (ESLINT_FLAT, "@eslint/js"),
            (ESLINT_CJS, "eslint-plugin-react"),
            (ESLINT8_PLUGIN, "eslint-plugin-react"),
            (ESLINT8_FAILED_PLUGIN, "eslint-plugin-react"),
            (ESLINT8_CONFIG, "eslint-config-next"),
            (NEXT_LINT_CONFIG, "eslint-config-next"),
            (TS2307, "react"),
            // Its type definitions are there.
            (TS2307_PRETTY, "@types/react"),
            (TS2688, "vite"),
        ] {
            assert_eq!(in_frontend(output, &[installed]), None, "{output}");
        }
        // Not installed, but a type error beside it: a failure, so the
        // missing package does not hide the type error (ADR 0043).
        let mixed = format!("{TS2307}src/main.tsx(9,3): error TS2322: Type 'number' is not assignable to type 'string'.\n");
        assert_eq!(in_frontend(&mixed, &[]), None);
        // In node_modules of a folder above the project, up to the root.
        let fixture = Fixture::new(
            &[
                ("package.json", r#"{"devDependencies":{"@eslint/js":"9"}}"#),
                ("node_modules/@eslint/js/package.json", "{}"),
                ("apps/web/package.json", "{}"),
            ],
            Track::NoGit,
        )
        .unwrap();
        let dir = fixture.root().join("apps").join("web");
        let out = ESLINT_FLAT.replace("{dir}", &dir.to_string_lossy());
        assert_eq!(setup_problem(&out, Some(2), fixture.root(), &dir), None);
        // Yarn Plug'n'Play keeps no node_modules: unsure, so a failure.
        let fixture = Fixture::new(
            &[
                ("package.json", r#"{"devDependencies":{"@eslint/js":"9"}}"#),
                (".pnp.cjs", ""),
            ],
            Track::NoGit,
        )
        .unwrap();
        let out = ESLINT_FLAT.replace("{dir}", &fixture.root().to_string_lossy());
        assert_eq!(
            setup_problem(&out, Some(2), fixture.root(), fixture.root()),
            None
        );
    }

    #[test]
    fn the_importing_files_folder_counts_for_a_run_from_the_root() {
        // `pnpm -r lint` runs from the root; the package is in the member's
        // node_modules, and the member's file imported it.
        let fixture = Fixture::new(
            &[
                ("package.json", r#"{"private":true}"#),
                (
                    "packages/web/package.json",
                    r#"{"devDependencies":{"@eslint/js":"9","react":"18"}}"#,
                ),
                ("packages/web/node_modules/@eslint/js/package.json", "{}"),
                ("packages/web/node_modules/react/package.json", "{}"),
            ],
            Track::NoGit,
        )
        .unwrap();
        let web = fixture.root().join("packages").join("web");
        let out = ESLINT_FLAT.replace("{dir}", &web.to_string_lossy());
        assert_eq!(
            setup_problem(&out, Some(2), fixture.root(), fixture.root()),
            None
        );
        let ts = "packages/web/src/a.tsx(1,19): error TS2307: Cannot find module 'react' or its corresponding type declarations.\n";
        assert_eq!(
            setup_problem(ts, Some(2), fixture.root(), fixture.root()),
            None
        );
        // Not installed there either, but listed: setup.
        let fixture = Fixture::new(
            &[
                ("package.json", r#"{"private":true}"#),
                (
                    "packages/web/package.json",
                    r#"{"devDependencies":{"@eslint/js":"9"}}"#,
                ),
            ],
            Track::NoGit,
        )
        .unwrap();
        let web = fixture.root().join("packages").join("web");
        let out = ESLINT_FLAT.replace("{dir}", &web.to_string_lossy());
        assert_eq!(
            setup_problem(&out, Some(2), fixture.root(), fixture.root()).map(|p| p.reason),
            Some("the package '@eslint/js' is not installed".to_owned())
        );
    }

    #[test]
    fn a_missing_import_that_is_not_a_listed_package_is_a_normal_failure() {
        for output in [
            // Relative and absolute paths, a drive letter.
            "src/main.tsx(2,20): error TS2307: Cannot find module './App' or its corresponding type declarations.\n",
            "Error: Cannot find module '../shared/config'\nRequire stack:\n- {dir}/eslint.config.cjs\n",
            "Error: Cannot find module '{dir}/eslint.config.cjs'\n  code: 'MODULE_NOT_FOUND',\n",
            "Error [ERR_MODULE_NOT_FOUND]: Cannot find module '{dir}/src/util.js' imported from {dir}/src/main.js\n",
            "Error: Cannot find module 'C:\\x\\eslint.config.cjs'\nRequire stack:\n- C:\\x\\y.js\n",
            "ESLint couldn't find the config \"./base.js\" to extend from.\n",
            // Path aliases and built-ins.
            "src/main.tsx(3,20): error TS2307: Cannot find module '@/components/Button' or its corresponding type declarations.\n",
            "src/main.tsx(3,20): error TS2307: Cannot find module '~/lib' or its corresponding type declarations.\n",
            "src/main.tsx(3,20): error TS2307: Cannot find module '$lib/x' or its corresponding type declarations.\n",
            "src/main.tsx(3,20): error TS2307: Cannot find module '#internal' or its corresponding type declarations.\n",
            "Error: Cannot find module 'node:fs'\nRequire stack:\n- {dir}/a.js\n",
            // A bare name that no package.json lists: a dependency the code
            // forgot, or a path alias such as `@components/x`.
            "src/main.tsx(3,20): error TS2307: Cannot find module 'left-pad' or its corresponding type declarations.\n",
            "src/main.tsx(3,20): error TS2307: Cannot find module '@components/Button' or its corresponding type declarations.\n",
            "Error [ERR_MODULE_NOT_FOUND]: Cannot find package 'zod' imported from {dir}/src/main.js\n",
            "ESLint couldn't find the plugin \"eslint-plugin-vue\".\n",
            // `Cannot find module` with no sign of Node's loader: a test's own text.
            "expected error: Cannot find module 'react'\n",
        ] {
            assert_eq!(in_frontend(output, &[]), None, "{output}");
        }
    }

    #[test]
    fn scoped_eslint_configs_and_subpaths_name_their_package() {
        assert_eq!(package_of("@eslint/js"), Some("@eslint/js"));
        assert_eq!(package_of("@scope/x/y"), Some("@scope/x"));
        assert_eq!(package_of("lodash/fp"), Some("lodash"));
        assert_eq!(package_of("vite/client"), Some("vite"));
        assert_eq!(package_of("@scope"), None);
        assert_eq!(package_of("@/x"), None);
        assert_eq!(package_of("./x"), None);
        assert_eq!(
            eslint_config_package("next/core-web-vitals").as_deref(),
            Some("eslint-config-next")
        );
        assert_eq!(
            eslint_config_package("@scope").as_deref(),
            Some("@scope/eslint-config")
        );
        assert_eq!(
            eslint_config_package("@scope/strict").as_deref(),
            Some("@scope/eslint-config-strict")
        );
        assert_eq!(
            eslint_config_package("@scope/eslint-config-strict").as_deref(),
            Some("@scope/eslint-config-strict")
        );
        assert_eq!(
            eslint_config_package("eslint-config-airbnb").as_deref(),
            Some("eslint-config-airbnb")
        );
        assert_eq!(eslint_config_package("plugin:react/recommended"), None);
        assert_eq!(eslint_config_package("./base.js"), None);
        assert_eq!(
            install_names("@scope/x", true),
            vec!["@scope/x".to_owned(), "@types/scope__x".to_owned()]
        );
        // The scoped config is listed in the fixture, so its message is setup.
        let out = "ESLint couldn't find the config \"@scope/strict\" to extend from.\n";
        assert_eq!(
            in_frontend(out, &[]).map(|p| p.reason),
            Some("the package '@scope/eslint-config-strict' is not installed".to_owned())
        );
    }

    #[test]
    fn mypy_on_a_module_that_is_not_installed_is_a_setup_problem() {
        let out = "app/api.py:3: error: Cannot find implementation or library stub for module named \"requests\"  [import-not-found]\napp/api.py:3: note: See https://mypy.readthedocs.io/en/stable/running_mypy.html#missing-imports\nFound 1 error in 1 file (checked 4 source files)\n";
        let fixture = Fixture::new(
            &[
                ("svc/requirements.txt", "Requests==2.32.0\nmypy\n"),
                ("svc/main.py", ""),
            ],
            Track::NoGit,
        )
        .unwrap();
        let svc = fixture.root().join("svc");
        let found = setup_problem(out, Some(1), fixture.root(), &svc).map(|found| found.reason);
        assert_eq!(
            found.as_deref(),
            Some("the Python module 'requests' is not installed")
        );
        // A module no dependency list names: maybe a typo in an import.
        let typo = out.replace("\"requests\"", "\"requets\"");
        assert_eq!(setup_problem(&typo, Some(1), fixture.root(), &svc), None);
        // A type error beside it is a failure: the missing module must not hide it.
        let mixed = format!("{out}app/api.py:9: error: Incompatible return value type (got \"int\", expected \"str\")  [return-value]\n");
        assert_eq!(setup_problem(&mixed, Some(1), fixture.root(), &svc), None);
        assert_eq!(problem(out, 1), None);
        // A module of the project itself: maybe a typo in the code.
        let local = "main.py:1: error: Cannot find implementation or library stub for module named \"app.servics\"  [import-not-found]\n";
        assert_eq!(problem(local, 1), None);
        // Installed without types is another message, and a normal failure.
        let untyped = "main.py:1: error: Skipping analyzing \"yaml\": module is installed, but missing library stubs or py.typed marker  [import-untyped]\n";
        assert_eq!(problem(untyped, 1), None);
    }

    #[test]
    fn a_package_the_lockfile_does_not_name_yet_gets_the_plain_install() {
        let fixture = Fixture::new(
            &[(
                "web/package-lock.json",
                r#"{"packages":{"node_modules/react":{"version":"18.3.1"}}}"#,
            )],
            Track::NoGit,
        )
        .unwrap();
        let dir = fixture.root().join("web");
        let problem = |package: &str| SetupProblem {
            reason: format!("the package '{package}' is not installed"),
            missing_package: Some(package.to_owned()),
            ..SetupProblem::default()
        };
        // Named in the lockfile: npm ci installs it without rewriting it.
        assert_eq!(
            install_for(&problem("react"), "npm ci", fixture.root(), &dir),
            "npm ci"
        );
        // Just added to package.json: npm ci would refuse the lockfile.
        assert_eq!(
            install_for(&problem("zod"), "npm ci", fixture.root(), &dir),
            "npm install"
        );
        // A prefix of a named package is not that package.
        assert_eq!(
            install_for(&problem("reac"), "npm ci", fixture.root(), &dir),
            "npm install"
        );
        // Not a package problem: the locked install as it is.
        let other = SetupProblem {
            reason: "the Python module 'jwt' is not installed".to_owned(),
            ..SetupProblem::default()
        };
        assert_eq!(
            install_for(&other, "npm ci", fixture.root(), &dir),
            "npm ci"
        );
    }

    fn locked(install: &str, files: &[(&str, &str)]) -> String {
        let fixture = Fixture::new(files, Track::NoGit).unwrap();
        let dir = fixture.root().join("app");
        std::fs::create_dir_all(&dir).unwrap();
        locked_install(install, fixture.root(), &dir)
    }

    #[test]
    fn npm_with_a_lockfile_installs_with_npm_ci() {
        assert_eq!(
            locked("npm install", &[("app/package-lock.json", "{}")]),
            "npm ci"
        );
        assert_eq!(
            locked("npm install", &[("app/npm-shrinkwrap.json", "{}")]),
            "npm ci"
        );
        assert_eq!(locked("npm install", &[]), "npm install");
        // The lockfile of another folder does not count.
        assert_eq!(
            locked("npm install", &[("package-lock.json", "{}")]),
            "npm install"
        );
    }

    #[test]
    fn pnpm_with_a_lockfile_installs_with_a_frozen_lockfile() {
        assert_eq!(
            locked("pnpm install", &[("app/pnpm-lock.yaml", "")]),
            "pnpm install --frozen-lockfile"
        );
        assert_eq!(locked("pnpm install", &[]), "pnpm install");
        // A lockfile of another manager does not count.
        assert_eq!(
            locked("pnpm install", &[("app/package-lock.json", "{}")]),
            "pnpm install"
        );
    }

    #[test]
    fn yarn_with_a_lockfile_installs_with_its_versions_flag() {
        let classic = "# THIS IS AN AUTOGENERATED FILE. DO NOT EDIT THIS FILE DIRECTLY.\n# yarn lockfile v1\n";
        assert_eq!(
            locked("yarn install", &[("app/yarn.lock", classic)]),
            "yarn install --frozen-lockfile"
        );
        assert_eq!(
            locked(
                "yarn install",
                &[
                    ("app/yarn.lock", classic),
                    ("app/package.json", r#"{"packageManager":"yarn@1.22.22"}"#)
                ]
            ),
            "yarn install --frozen-lockfile"
        );
        let berry = "# This file is generated by running \"yarn install\" inside your project.\n\n__metadata:\n  version: 8\n";
        assert_eq!(
            locked("yarn install", &[("app/yarn.lock", berry)]),
            "yarn install --immutable"
        );
        assert_eq!(
            locked(
                "yarn install",
                &[
                    ("app/yarn.lock", ""),
                    ("app/package.json", r#"{"packageManager":"yarn@4.5.0"}"#)
                ]
            ),
            "yarn install --immutable"
        );
        assert_eq!(
            locked(
                "yarn install",
                &[
                    ("app/yarn.lock", ""),
                    ("package.json", r#"{"packageManager":"yarn@3.8.0"}"#)
                ]
            ),
            "yarn install --immutable"
        );
        assert_eq!(
            locked(
                "yarn install",
                &[("app/yarn.lock", ""), (".yarnrc.yml", "")]
            ),
            "yarn install --immutable"
        );
        assert_eq!(locked("yarn install", &[]), "yarn install");
    }

    #[test]
    fn bun_with_a_lockfile_installs_with_a_frozen_lockfile() {
        assert_eq!(
            locked("bun install", &[("app/bun.lock", "")]),
            "bun install --frozen-lockfile"
        );
        assert_eq!(
            locked("bun install", &[("app/bun.lockb", "")]),
            "bun install --frozen-lockfile"
        );
        assert_eq!(locked("bun install", &[]), "bun install");
    }

    #[test]
    fn python_managers_with_a_lockfile_install_from_it() {
        assert_eq!(
            locked("uv sync", &[("app/uv.lock", "")]),
            "uv sync --locked"
        );
        // A uv workspace keeps its lockfile at the workspace root.
        assert_eq!(locked("uv sync", &[("uv.lock", "")]), "uv sync --locked");
        assert_eq!(locked("uv sync", &[]), "uv sync");
        assert_eq!(
            locked("pipenv install --dev", &[("app/Pipfile.lock", "{}")]),
            "pipenv sync --dev"
        );
        assert_eq!(locked("pipenv install --dev", &[]), "pipenv install --dev");
        // poetry install never rewrites an existing poetry.lock.
        assert_eq!(
            locked("poetry install", &[("app/poetry.lock", "")]),
            "poetry install"
        );
        // pip has no lockfile of its own.
        assert_eq!(
            locked(
                ".venv/bin/python -m pip install -r requirements.txt",
                &[("app/requirements.txt", "")]
            ),
            ".venv/bin/python -m pip install -r requirements.txt"
        );
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
