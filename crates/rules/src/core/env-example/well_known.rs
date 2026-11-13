//! Well-known system and tooling variables that `core/env-example` never
//! asks a project to list.
//!
//! This file is the only place the list lives. `SKILL.md` and `docs/rules.md`
//! repeat it for readers, and a test fails when they fall out of step.
//!
//! Only add a name that the operating system, a shell, a CI service or a
//! development tool sets by itself. Never add a name a project could plausibly
//! use for its own configuration, such as `PORT`, `HOST` or `DATABASE_URL`.

/// Exact names.
pub const NAMES: &[&str] = &[
    // Operating system and shell.
    "PATH",
    "HOME",
    "PWD",
    "OLDPWD",
    "USER",
    "LOGNAME",
    "SHELL",
    "SHLVL",
    "TERM",
    "TERM_PROGRAM",
    "COLORTERM",
    "COLUMNS",
    "LINES",
    "DISPLAY",
    "EDITOR",
    "VISUAL",
    "PAGER",
    "LANG",
    "LANGUAGE",
    "TZ",
    "TMPDIR",
    "TEMP",
    "TMP",
    "SSH_AUTH_SOCK",
    // Windows.
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMDATA",
    "PROGRAMFILES",
    "SYSTEMROOT",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "HOMEDRIVE",
    "HOMEPATH",
    "COMPUTERNAME",
    // Docker.
    "DOCKER_HOST",
    "DOCKER_TLS_VERIFY",
    "DOCKER_CERT_PATH",
    "DOCKER_BUILDKIT",
    "DOCKER_CONFIG",
    "DOCKER_CONTEXT",
    // CI.
    "CI",
    "CONTINUOUS_INTEGRATION",
    "GITLAB_CI",
    // Terminal output.
    "NO_COLOR",
    "FORCE_COLOR",
    "CLICOLOR",
    "CLICOLOR_FORCE",
    "DEBUG",
    // Proxies and certificates.
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    // Node.js.
    "NODE_ENV",
    "NODE_OPTIONS",
    "NODE_PATH",
    "NODE_DEBUG",
    "NODE_NO_WARNINGS",
    "NODE_EXTRA_CA_CERTS",
    "JEST_WORKER_ID",
    // Python.
    "PYTHONPATH",
    "PYTHONHOME",
    "PYTHONUNBUFFERED",
    "PYTHONDONTWRITEBYTECODE",
    "PYTHONIOENCODING",
    "PYTHONHASHSEED",
    "VIRTUAL_ENV",
    // Rust, Go and Java.
    "RUST_BACKTRACE",
    "GOPATH",
    "GOROOT",
    "JAVA_HOME",
];

/// Name prefixes. A name that starts with one of these is ignored.
pub const PREFIXES: &[&str] = &[
    "LC_", "XDG_", "npm_", "GITHUB_", "RUNNER_", "FSCALE_", "CARGO_", "RUSTC_", "RUSTUP_",
    "PYTEST_", "VITEST",
];

/// True when the rule never asks for `name`.
pub fn is_well_known(name: &str) -> bool {
    NAMES.contains(&name) || PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}
