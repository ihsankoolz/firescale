//! Local services a repo expects to be running: databases, caches and
//! queues on this machine (ADR 0025).
//!
//! They are read from two places:
//!
//! - Docker Compose files (and their override files): each service that
//!   publishes a port on this machine.
//! - `.env.example` files: URLs such as `postgresql://user:pass@localhost:5432/app`
//!   and pairs such as `REDIS_HOST=localhost` with `REDIS_PORT=6379`.
//!
//! Only the service, the host and the port are kept. User names, passwords,
//! database names and whole connection strings are never kept, so they can
//! never be printed. Hosts that are not this machine are left out: a remote
//! database is not something a teammate starts.
//!
//! Compose files are read with a small line reader, not a YAML parser. It
//! understands `services:`, `image:` and `ports:` in the short (`"5432:5432"`)
//! and long (`target:`/`published:`) forms, which is what the files people
//! write use.

use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Docker Compose file names, in the order Compose itself prefers them,
/// each with the override file Compose merges into it.
pub(crate) const COMPOSE_FILES: [(&str, &str); 4] = [
    ("compose.yaml", "compose.override.yaml"),
    ("compose.yml", "compose.override.yml"),
    ("docker-compose.yaml", "docker-compose.override.yaml"),
    ("docker-compose.yml", "docker-compose.override.yml"),
];

/// The example env file that lists a folder's variables.
pub const ENV_EXAMPLE: &str = ".env.example";

/// How long a probe waits for a connection.
pub const PROBE_TIMEOUT: Duration = Duration::from_millis(400);

/// A service the repo expects on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    /// `PostgreSQL`, `Redis`, or the Compose service's name.
    pub name: String,
    /// The host as written: `localhost`, `127.0.0.1`, `::1` or `0.0.0.0`.
    pub host: String,
    pub port: u16,
    /// Where it was found, relative to the repo root with forward slashes:
    /// `backend/.env.example (DATABASE_URL)` or `compose.yaml (service db)`.
    pub source: String,
    /// The command that starts it, to be run from the repo root.
    pub start: Option<String>,
}

impl Service {
    /// `PostgreSQL on localhost:5432`.
    #[must_use]
    pub fn describe(&self) -> String {
        format!(
            "{} on {}:{}",
            self.name,
            display_host(&self.host),
            self.port
        )
    }

    /// What to do when it is not running: the Compose command, or how to
    /// start it by hand.
    #[must_use]
    pub fn next_step(&self) -> String {
        if let Some(start) = &self.start {
            return start.clone();
        }
        match docker_image(&self.name) {
            Some((image, extra)) => format!(
                "start {} so it listens on port {} (no Compose file names it; for example: docker run -d -p {}:{}{extra} {image})",
                self.name, self.port, self.port, default_port(&self.name).unwrap_or(self.port)
            ),
            None => format!(
                "start {} so it listens on {}:{}",
                self.name,
                display_host(&self.host),
                self.port
            ),
        }
    }
}

fn display_host(host: &str) -> &str {
    if host == "0.0.0.0" {
        "localhost"
    } else {
        host
    }
}

/// True for a host on this machine.
#[must_use]
pub fn is_local_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    ["localhost", "127.0.0.1", "::1", "0.0.0.0"]
        .iter()
        .any(|local| host.eq_ignore_ascii_case(local))
}

/// True when something accepts connections on `host:port` within `timeout`.
#[must_use]
pub fn listening(host: &str, port: u16, timeout: Duration) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let host = if host == "0.0.0.0" { "127.0.0.1" } else { host };
    let mut addresses: Vec<SocketAddr> = match (host, port).to_socket_addrs() {
        Ok(addresses) => addresses.collect(),
        Err(_) => return false,
    };
    // IPv4 first: a local service usually listens there, and on Windows a
    // probe of `::1` for a service that does not waits the whole timeout.
    addresses.sort_by_key(|address| u8::from(address.is_ipv6()));
    addresses
        .iter()
        .any(|address| TcpStream::connect_timeout(address, timeout).is_ok())
}

/// The service a scheme or a variable name points at, and its usual port.
fn service_for(word: &str) -> Option<(&'static str, u16)> {
    let word = word.to_ascii_lowercase();
    if word.starts_with("pgadmin") {
        return None;
    }
    let known: [(&str, &str, u16); 19] = [
        ("postgres", "PostgreSQL", 5432),
        ("postgis", "PostgreSQL", 5432),
        ("timescale", "PostgreSQL", 5432),
        ("pg", "PostgreSQL", 5432),
        ("redis", "Redis", 6379),
        ("valkey", "Redis", 6379),
        ("mysql", "MySQL", 3306),
        ("mariadb", "MySQL", 3306),
        ("mongo", "MongoDB", 27017),
        ("amqp", "RabbitMQ", 5672),
        ("rabbit", "RabbitMQ", 5672),
        ("elasticsearch", "Elasticsearch", 9200),
        ("opensearch", "OpenSearch", 9200),
        ("memcached", "Memcached", 11211),
        ("minio", "MinIO", 9000),
        ("kafka", "Kafka", 9092),
        ("nats", "NATS", 4222),
        ("mssql", "SQL Server", 1433),
        ("sqlserver", "SQL Server", 1433),
    ];
    known
        .iter()
        .find(|(prefix, _, _)| word.starts_with(prefix) || word.contains(&format!("_{prefix}")))
        .map(|(_, name, port)| (*name, *port))
}

/// The usual port of a known service.
fn default_port(name: &str) -> Option<u16> {
    service_for(name).map(|(_, port)| port)
}

/// The image and the settings `docker run` needs for a known service. The
/// password is a placeholder for the person to fill in.
fn docker_image(name: &str) -> Option<(&'static str, &'static str)> {
    match name {
        "PostgreSQL" => Some(("postgres", " -e POSTGRES_PASSWORD=<password>")),
        "Redis" => Some(("redis", "")),
        "MySQL" => Some(("mysql", " -e MYSQL_ROOT_PASSWORD=<password>")),
        "MongoDB" => Some(("mongo", "")),
        "RabbitMQ" => Some(("rabbitmq", "")),
        _ => None,
    }
}

/// The name of the service at a port, for a port no file names. The one
/// table of usual ports: setup detection uses it too.
#[must_use]
pub fn service_on_port(port: u16) -> Option<&'static str> {
    match port {
        5432 => Some("PostgreSQL"),
        6379 => Some("Redis"),
        3306 => Some("MySQL"),
        27017 => Some("MongoDB"),
        5672 => Some("RabbitMQ"),
        _ => None,
    }
}

/// Strips one pair of matching quotes.
fn unquote(value: &str) -> &str {
    let value = value.trim();
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}

/// The `KEY=value` pairs of an env file, in order. Comments and blank lines
/// are skipped.
fn env_pairs(text: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        // A comment after an unquoted value.
        let value = value.trim();
        let value = if value.starts_with('"') || value.starts_with('\'') {
            unquote(value)
        } else {
            value.split(" #").next().unwrap_or(value).trim()
        };
        pairs.push((key.to_owned(), value.to_owned()));
    }
    pairs
}

/// The host and port of a URL's authority, without the user name and
/// password: `user:secret@localhost:5432` gives `("localhost", Some(5432))`.
fn url_host_port(url: &str) -> Option<(String, String, Option<u16>)> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    // Several hosts (a replica set): the first.
    let authority = authority.split(',').next()?;
    let (host, port) = if let Some(inner) = authority.strip_prefix('[') {
        let (host, after) = inner.split_once(']')?;
        let port = after.strip_prefix(':').and_then(|port| port.parse().ok());
        (host.to_owned(), port)
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host.to_owned(), Some(port.parse().ok()?)),
            None => (authority.to_owned(), None),
        }
    };
    if host.is_empty() || host.contains('$') || host.contains('{') {
        return None;
    }
    Some((scheme.to_ascii_lowercase(), host, port))
}

/// Services named by one `.env.example`. `file` is its path relative to the
/// repo root, for the `source`.
fn from_env_example(text: &str, file: &str) -> Vec<Service> {
    let pairs = env_pairs(text);
    let mut found = Vec::new();
    // URLs with a service scheme.
    for (key, value) in &pairs {
        let Some((scheme, host, port)) = url_host_port(value) else {
            continue;
        };
        let base = scheme.split('+').next().unwrap_or(&scheme);
        // `mongodb+srv` looks the hosts up in DNS: never a local port.
        if scheme == "mongodb+srv" {
            continue;
        }
        let Some((name, usual)) = service_for(base) else {
            continue;
        };
        if !is_local_host(&host) {
            continue;
        }
        found.push(Service {
            name: name.to_owned(),
            host,
            port: port.unwrap_or(usual),
            source: format!("{file} ({key})"),
            start: None,
        });
    }
    // `<PREFIX>_HOST` (or `_SERVER`, `_HOSTNAME`) with `<PREFIX>_PORT`.
    for (key, value) in &pairs {
        let Some(prefix) = ["_HOST", "_SERVER", "_HOSTNAME"]
            .iter()
            .find_map(|suffix| key.strip_suffix(suffix))
        else {
            continue;
        };
        let host = unquote(value);
        if !is_local_host(host) {
            continue;
        }
        let port_key = format!("{prefix}_PORT");
        let port = pairs
            .iter()
            .find(|(key, _)| *key == port_key)
            .and_then(|(_, value)| unquote(value).parse::<u16>().ok());
        let by_name = service_for(prefix);
        let (name, port) = match (by_name, port) {
            (Some((name, _)), Some(port)) => (name.to_owned(), port),
            (Some((name, usual)), None) => (name.to_owned(), usual),
            (None, Some(port)) => match service_on_port(port) {
                Some(name) => (name.to_owned(), port),
                None => continue,
            },
            (None, None) => continue,
        };
        found.push(Service {
            name,
            host: host.to_owned(),
            port,
            source: format!("{file} ({key})"),
            start: None,
        });
    }
    found
}

/// The number of spaces before a line's text.
fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// A published port on this machine from a short-form port entry:
/// `"5432:5432"`, `127.0.0.1:6379:6379`, `${PG_PORT:-5432}:5432`. A
/// container port alone (`"5432"`) is published on a random port: `None`.
fn short_port(entry: &str) -> Option<(String, u16)> {
    let entry = unquote(entry);
    if entry.ends_with("/udp") {
        return None;
    }
    let entry = entry.trim_end_matches("/tcp");
    // `${VAR:-5432}` becomes its default, before splitting on ':'.
    let mut plain = String::new();
    let mut rest = entry;
    while let Some(start) = rest.find("${") {
        plain.push_str(&rest[..start]);
        let inner_end = rest[start..].find('}')? + start;
        let inner = &rest[start + 2..inner_end];
        let default = inner.split_once(":-").or_else(|| inner.split_once('-'))?.1;
        plain.push_str(default);
        rest = &rest[inner_end + 1..];
    }
    plain.push_str(rest);
    // An IPv6 host in brackets: `[::1]:5432:5432`.
    let (host, ports) = if let Some(inner) = plain.strip_prefix('[') {
        let (host, after) = inner.split_once(']')?;
        (host.to_owned(), after.strip_prefix(':')?.to_owned())
    } else {
        let parts: Vec<&str> = plain.split(':').collect();
        match parts.len() {
            2 => ("localhost".to_owned(), parts.join(":")),
            3 => (parts[0].to_owned(), format!("{}:{}", parts[1], parts[2])),
            _ => return None,
        }
    };
    let (published, _) = ports.split_once(':')?;
    // A range of ports: not one service to probe.
    let port = published.parse::<u16>().ok()?;
    let host = if host.is_empty() || host == "0.0.0.0" {
        "localhost".to_owned()
    } else {
        host
    };
    Some((host, port))
}

/// A service in a Compose file, while it is read.
#[derive(Default)]
struct ComposeService {
    key: String,
    image: Option<String>,
    ports: Vec<(String, u16)>,
}

/// Services from one Compose file's text: `(key, image, host, port)` for
/// each published port, and port 0 for a service that publishes none.
fn compose_services(text: &str) -> Vec<(String, Option<String>, String, u16)> {
    let mut services: Vec<ComposeService> = Vec::new();
    let mut in_services = false;
    let mut service_indent = None;
    let mut ports_indent: Option<usize> = None;
    // The long form spreads one port over several lines.
    let mut long_host: Option<String> = None;
    for raw in text.lines() {
        let line = raw.split(" #").next().unwrap_or(raw).trim_end();
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let depth = indent(line);
        if depth == 0 {
            in_services = trimmed == "services:";
            service_indent = None;
            ports_indent = None;
            continue;
        }
        if !in_services {
            continue;
        }
        let service_depth = *service_indent.get_or_insert(depth);
        if depth == service_depth {
            ports_indent = None;
            if let Some(key) = trimmed.strip_suffix(':') {
                services.push(ComposeService {
                    key: unquote(key).to_owned(),
                    ..ComposeService::default()
                });
            }
            continue;
        }
        let Some(service) = services.last_mut() else {
            continue;
        };
        if let Some(ports_depth) = ports_indent {
            if depth > ports_depth || depth == ports_depth && trimmed.starts_with("- ") {
                let item = trimmed.strip_prefix("- ");
                let field = item.unwrap_or(trimmed);
                if item.is_some() {
                    long_host = None;
                }
                if let Some(value) = field.strip_prefix("host_ip:") {
                    long_host = Some(unquote(value).to_owned());
                } else if let Some(value) = field.strip_prefix("published:") {
                    if let Ok(port) = unquote(value).parse::<u16>() {
                        let host = long_host.clone().unwrap_or_else(|| "localhost".to_owned());
                        service.ports.push((host, port));
                    }
                } else if !field.contains(": ") && !field.ends_with(':') && item.is_some() {
                    if let Some(port) = short_port(field) {
                        service.ports.push(port);
                    }
                }
                continue;
            }
            ports_indent = None;
        }
        if let Some(value) = trimmed.strip_prefix("image:") {
            service.image = Some(unquote(value).to_owned());
        } else if let Some(value) = trimmed.strip_prefix("ports:") {
            let value = value.trim();
            if let Some(inline) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
                for entry in inline.split(',') {
                    if let Some(port) = short_port(entry) {
                        service.ports.push(port);
                    }
                }
            } else {
                ports_indent = Some(depth);
            }
        }
    }
    let mut found = Vec::new();
    for service in services {
        if service.ports.is_empty() {
            // Kept with port 0, so an override file's ports can find the image.
            found.push((service.key, service.image, String::new(), 0));
            continue;
        }
        for (host, port) in service.ports {
            found.push((service.key.clone(), service.image.clone(), host, port));
        }
    }
    found
}

/// The command that starts a Compose service, run from the repo root.
fn compose_start(compose_dir: &str, file: &str, key: &str) -> String {
    if compose_dir.is_empty() {
        format!("docker compose up -d {key}")
    } else {
        format!("docker compose -f {compose_dir}/{file} up -d {key}")
    }
}

/// `folder/name` relative to the repo, with forward slashes; `name` at the root.
fn join(folder: &str, name: &str) -> String {
    if folder.is_empty() {
        name.to_owned()
    } else {
        format!("{folder}/{name}")
    }
}

/// Services named by the Compose files in `folder` (relative to `root`).
/// The override file is read with its main file, so an image in one and the
/// ports in the other make one service.
fn compose_in(root: &Path, folder: &str) -> Vec<Service> {
    let dir = root.join(folder);
    let mut found = Vec::new();
    for (main, override_file) in COMPOSE_FILES {
        if !dir.join(main).is_file() {
            continue;
        }
        let mut entries = Vec::new();
        for file in [main, override_file] {
            if let Ok(text) = std::fs::read_to_string(dir.join(file)) {
                entries.push((file, compose_services(&text)));
            }
        }
        let image_of = |key: &str| {
            entries
                .iter()
                .flat_map(|(_, services)| services.iter())
                .find(|(other, image, _, _)| other == key && image.is_some())
                .and_then(|(_, image, _, _)| image.clone())
        };
        for (file, services) in &entries {
            for (key, _, host, port) in services {
                if *port == 0 || !is_local_host(host) {
                    continue;
                }
                let image = image_of(key);
                let image_name = image
                    .as_deref()
                    .map(|image| image.rsplit('/').next().unwrap_or(image));
                let name = image_name
                    .and_then(service_for)
                    .or_else(|| service_for(key))
                    .map_or_else(
                        || format!("the Compose service {key}"),
                        |(name, _)| name.to_owned(),
                    );
                found.push(Service {
                    name,
                    host: host.clone(),
                    port: *port,
                    source: format!("{} (service {key})", join(folder, file)),
                    start: Some(compose_start(folder, main, key)),
                });
            }
        }
        // Compose reads only the first main file it finds.
        break;
    }
    found
}

/// Adds `service` unless one on the same port is known. A service from an
/// env file takes the start command of a Compose service on its port.
fn merge(all: &mut Vec<Service>, service: Service) {
    if let Some(known) = all.iter_mut().find(|known| known.port == service.port) {
        if known.start.is_none() {
            known.start.clone_from(&service.start);
        }
        return;
    }
    all.push(service);
}

/// The folder a path is in, relative to `root`, with forward slashes.
fn relative(root: &Path, dir: &Path) -> String {
    dir.strip_prefix(root)
        .unwrap_or(dir)
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// The folders from `dir` up to `root`, nearest first.
#[must_use]
pub fn folders_up(root: &Path, dir: &Path) -> Vec<PathBuf> {
    let mut folders = Vec::new();
    let mut current = Some(dir);
    while let Some(folder) = current {
        folders.push(folder.to_path_buf());
        if folder == root || !folder.starts_with(root) {
            break;
        }
        current = folder.parent();
    }
    folders
}

/// The services a step in `dir` may need: from the `.env.example` and the
/// Compose files in `dir` and every folder above it up to `root`.
#[must_use]
pub fn services_for_folder(root: &Path, dir: &Path) -> Vec<Service> {
    services_in(root, &folders_up(root, dir))
}

/// The services named in the given folders (absolute, inside `root`). A
/// Compose service and an env URL on the same port are one service.
///
/// A Compose service counts only when it is a known database, cache, queue
/// or search service, or an `.env.example` names its port: the app's own
/// containers (a backend, a proxy, an admin page) are not something the
/// tests need running.
#[must_use]
pub fn services_in(root: &Path, folders: &[PathBuf]) -> Vec<Service> {
    let mut compose = Vec::new();
    let mut env = Vec::new();
    for folder in folders {
        let rel = relative(root, folder);
        compose.extend(compose_in(root, &rel));
        let file = folder.join(ENV_EXAMPLE);
        if let Ok(text) = std::fs::read_to_string(&file) {
            env.extend(from_env_example(&text, &join(&rel, ENV_EXAMPLE)));
        }
    }
    let env_ports: Vec<u16> = env.iter().map(|service| service.port).collect();
    compose.retain(|service| {
        !service.name.starts_with("the Compose service ") || env_ports.contains(&service.port)
    });
    let mut all = Vec::new();
    for service in compose.into_iter().chain(env) {
        merge(&mut all, service);
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_give_host_and_port_and_never_the_password() {
        let text = "\
# Database
DATABASE_URL=postgresql+asyncpg://app:s3cr3t-pw@localhost:5433/app_db
REDIS_URL=\"redis://:hunter2@127.0.0.1:6379/0\"
CACHE_URL=redis://cache.internal:6379
MONGO_URL=mongodb+srv://u:p@cluster0.example.net/db
API_URL=http://localhost:8000
SECRET_KEY=changeme
";
        let found = from_env_example(text, "backend/.env.example");
        assert_eq!(
            found,
            vec![
                Service {
                    name: "PostgreSQL".into(),
                    host: "localhost".into(),
                    port: 5433,
                    source: "backend/.env.example (DATABASE_URL)".into(),
                    start: None,
                },
                Service {
                    name: "Redis".into(),
                    host: "127.0.0.1".into(),
                    port: 6379,
                    source: "backend/.env.example (REDIS_URL)".into(),
                    start: None,
                },
            ]
        );
        let debug = format!("{found:?}");
        for secret in ["s3cr3t-pw", "hunter2", "app_db", "app:"] {
            assert!(!debug.contains(secret), "{secret} leaked: {debug}");
        }
    }

    #[test]
    fn host_and_port_pairs_are_read() {
        let text = "POSTGRES_SERVER=localhost\nPOSTGRES_PORT=5432\nPOSTGRES_PASSWORD=x\nREDIS_HOST=localhost\nDB_HOST=127.0.0.1\nDB_PORT=3306\nMAIL_HOST=smtp.example.com\nOTHER_HOST=localhost\n";
        let found = from_env_example(text, ".env.example");
        let described: Vec<String> = found.iter().map(Service::describe).collect();
        assert_eq!(
            described,
            vec![
                "PostgreSQL on localhost:5432",
                "Redis on localhost:6379",
                "MySQL on 127.0.0.1:3306"
            ]
        );
    }

    #[test]
    fn compose_ports_in_every_form() {
        let text = r#"
services:
  db:
    image: postgres:16
    environment:
      POSTGRES_PASSWORD: example
    ports:
      - "5432:5432"
  cache:
    image: "docker.io/library/redis:7"
    ports: ["127.0.0.1:6380:6379"]
  queue:
    image: rabbitmq:3
    ports:
      - target: 5672
        published: "5673"
        host_ip: 127.0.0.1
  search:
    image: elasticsearch:8
    ports:
      - ${ES_PORT:-9200}:9200
  random:
    image: nginx
    ports:
      - "80"
  remote:
    image: nginx
    ports:
      - "10.0.0.5:8080:80"
volumes:
  data:
"#;
        let found: Vec<_> = compose_services(text)
            .into_iter()
            .filter(|(_, _, _, port)| *port != 0)
            .collect();
        let ports: Vec<(String, u16)> = found
            .iter()
            .map(|(key, _, _, port)| (key.clone(), *port))
            .collect();
        assert_eq!(
            ports,
            vec![
                ("db".to_owned(), 5432),
                ("cache".to_owned(), 6380),
                ("queue".to_owned(), 5673),
                ("search".to_owned(), 9200),
                ("remote".to_owned(), 8080),
            ]
        );
        assert_eq!(found[4].2, "10.0.0.5");
    }

    #[test]
    fn short_ports_are_read() {
        assert_eq!(
            short_port("\"5432:5432\""),
            Some(("localhost".into(), 5432))
        );
        assert_eq!(short_port("[::1]:6379:6379"), Some(("::1".into(), 6379)));
        assert_eq!(
            short_port("0.0.0.0:5432:5432/tcp"),
            Some(("localhost".into(), 5432))
        );
        assert_eq!(short_port("5432"), None);
        assert_eq!(short_port("53:53/udp"), None);
        assert_eq!(short_port("8000-8010:8000-8010"), None);
    }

    #[test]
    fn a_folder_sees_its_own_files_and_those_above() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("backend/orders")).expect("mkdir");
        std::fs::write(
            root.join("compose.yaml"),
            "services:\n  db:\n    image: postgres:16\n",
        )
        .expect("write");
        std::fs::write(
            root.join("compose.override.yaml"),
            "services:\n  db:\n    ports:\n      - \"5432:5432\"\n",
        )
        .expect("write");
        std::fs::write(
            root.join("backend/orders/.env.example"),
            "DATABASE_URL=postgresql://u:p@localhost:5432/orders\nREDIS_URL=redis://localhost:6379\n",
        )
        .expect("write");
        let found = services_for_folder(root, &root.join("backend/orders"));
        let shown: Vec<(String, Option<String>)> = found
            .iter()
            .map(|service| (service.describe(), service.start.clone()))
            .collect();
        // The override file has the port; the main file has the image.
        assert_eq!(
            shown,
            vec![
                (
                    "PostgreSQL on localhost:5432".to_owned(),
                    Some("docker compose up -d db".to_owned())
                ),
                ("Redis on localhost:6379".to_owned(), None),
            ]
        );
        assert!(found[1]
            .next_step()
            .contains("docker run -d -p 6379:6379 redis"));
    }

    #[test]
    fn compose_in_a_subfolder_is_started_with_its_path() {
        assert_eq!(
            compose_start("infra", "compose.yaml", "db"),
            "docker compose -f infra/compose.yaml up -d db"
        );
    }

    #[test]
    fn a_port_with_nothing_on_it_is_not_listening_and_an_open_one_is() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        assert!(listening("127.0.0.1", port, PROBE_TIMEOUT));
        drop(listener);
        // Tests run in parallel and another one may take the freed port at
        // once: one of a few freed ports must read as closed.
        let closed = (0..5).any(|_| {
            let other = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            let port = other.local_addr().expect("addr").port();
            drop(other);
            !listening("127.0.0.1", port, PROBE_TIMEOUT)
        });
        assert!(closed);
    }
}
