//! Local-path references between projects, read from the text of the files
//! that name them: pip requirements files, `pyproject.toml`, `Cargo.toml`,
//! `package.json`, Dockerfiles and Docker Compose files.
//!
//! These are pure text readers. Nothing here touches the file system: every
//! function takes text (or parsed JSON) and returns the paths exactly as
//! written, only unquoted and cleaned of prefixes such as `file:`. Resolving
//! them against the file's folder, and into a project graph, is the caller's
//! job.
//!
//! As in the rest of the crate there is no TOML or YAML parser: TOML goes
//! through the line reader in `facts.rs`, and Compose files through a small
//! indentation-based reader like the one in `services.rs`.

use serde_json::Value;

use crate::facts::{
    array_strings, bracket_balance, option_value, parse_toml, requirement_include, strip_comment,
    unquote, TomlEntry,
};

/// The longest `text` kept for a reference, in chars.
const MAX_TEXT: usize = 120;

/// A local path named in a manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathRef {
    /// The path as written, unquoted, with extras (`[dev]`), `#egg=...` fragments and
    /// a `file:` / `file://` / `${PROJECT_ROOT}/` prefix removed. Forward or back slashes as written.
    pub path: String,
    /// What kind of reference, one of the constant strings listed per function.
    pub what: &'static str,
    /// 1-based line in the text.
    pub line: usize,
    /// That source line, trimmed, at most 120 chars (cut on a char boundary, `…` appended when cut).
    pub text: String,
}

/// A local dependency in a `package.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageRef {
    /// The dependency's key in package.json.
    pub dependency: String,
    /// The version spec as written: `file:../x`, `workspace:*`.
    pub spec: String,
    /// `"file:"`, `"link:"`, `"portal:"` or `"workspace:"`.
    pub what: &'static str,
    /// For file:/link:/portal:, and for `workspace:` followed by a relative path (starts with `.`): that path.
    pub path: Option<String>,
    /// For `workspace:` that is not a path: the package name it points at — the dependency key, or the
    /// alias in `workspace:other-name@*` (`npm:` style aliases inside are not followed).
    pub package: Option<String>,
    /// Which field: `dependencies`, `devDependencies`, `peerDependencies` or `optionalDependencies`.
    pub field: &'static str,
}

/// A `COPY` or `ADD` from the build context in a Dockerfile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyRef {
    /// `COPY` or `ADD`.
    pub instruction: &'static str,
    /// Source paths, relative to the build context, as written (unquoted).
    pub sources: Vec<String>,
    /// 1-based line where the instruction starts.
    pub line: usize,
    /// That first line, trimmed, at most 120 chars.
    pub text: String,
}

/// The build and the dependencies of one Compose service.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComposeBuild {
    /// The service key under `services:`.
    pub service: String,
    /// 1-based line of the service key.
    pub line: usize,
    /// `build: ./x`, or `context:` under a `build:` mapping (`"."` when the mapping has no context), with its line.
    /// None when the service has no `build`.
    pub context: Option<(String, usize)>,
    /// `dockerfile:` under `build:`, as written.
    pub dockerfile: Option<String>,
    /// Service names from `depends_on:` as a list (`- db`), an inline list (`[db, cache]`), or a mapping
    /// (`db:` with `condition:` below), each with its line.
    pub depends_on: Vec<(String, usize)>,
}

// ------------------------------------------------------------- shared helpers

fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

/// A source line for a reference: trimmed, at most [`MAX_TEXT`] chars.
fn snippet(line: &str) -> String {
    let line = strip_bom(line).trim();
    if line.chars().count() <= MAX_TEXT {
        return line.to_owned();
    }
    let mut cut: String = line.chars().take(MAX_TEXT - 1).collect();
    cut.push('…');
    cut
}

/// The trimmed text of the 1-based `line`.
fn line_text(lines: &[&str], line: usize) -> String {
    lines
        .get(line.saturating_sub(1))
        .map(|text| snippet(text))
        .unwrap_or_default()
}

/// True for a path that does not depend on the folder of the file naming it:
/// `/x`, `\x`, `C:\x`, `C:/x` and `~/x`.
fn is_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    let drive = bytes.first().is_some_and(u8::is_ascii_alphabetic) && bytes.get(1) == Some(&b':');
    drive || path.starts_with(['/', '\\', '~'])
}

/// `.`, `..`, or a path starting with `./`, `../`, `.\` or `..\`.
fn is_relative_path(path: &str) -> bool {
    matches!(path, "." | "..")
        || ["./", "../", ".\\", "..\\"]
            .iter()
            .any(|prefix| path.starts_with(prefix))
}

/// A version-control link such as `git+https://…` or `git@host:repo`.
fn is_vcs(target: &str) -> bool {
    ["git+", "hg+", "svn+", "bzr+", "git@"]
        .iter()
        .any(|prefix| target.starts_with(prefix))
}

/// `path` without `[extras]` at its end: `../x[dev]` gives `../x`.
fn strip_extras(path: &str) -> &str {
    if path.ends_with(']') {
        if let Some(open) = path.rfind('[') {
            return &path[..open];
        }
    }
    path
}

/// The first whitespace-separated word.
fn first_word(text: &str) -> &str {
    text.split_whitespace().next().unwrap_or_default()
}

/// A package name, optionally with extras: `pkg`, `my_pkg[dev]`.
fn looks_like_name(text: &str) -> bool {
    let name = strip_extras(text.trim()).trim();
    name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// A hatch root URL: `{root:uri}/x` gives `x`, `{root:parent:uri}/x` gives
/// `../x`. `None` when `target` does not start with one.
fn hatch_root(target: &str) -> Option<String> {
    let rest = target.strip_prefix("{root:")?;
    let (inside, after) = rest.split_once('}')?;
    let mut parts: Vec<&str> = Vec::new();
    let mut inside = inside;
    while let Some(next) = inside.strip_prefix("parent:") {
        parts.push("..");
        inside = next;
    }
    if inside != "uri" {
        return None;
    }
    let after = after.trim_start_matches(['/', '\\']);
    if !after.is_empty() {
        parts.push(after);
    }
    Some(if parts.is_empty() {
        ".".to_owned()
    } else {
        parts.join("/")
    })
}

/// `rest` (what follows `file:` and `//`) without a leading
/// `/${PROJECT_ROOT}/` or `${PROJECT_ROOT}/`.
fn strip_project_root(rest: &str) -> String {
    let root = rest
        .strip_prefix(['/', '\\'])
        .filter(|inner| inner.starts_with("${PROJECT_ROOT}"))
        .unwrap_or(rest);
    match root.strip_prefix("${PROJECT_ROOT}") {
        Some(after) => {
            let after = after.trim_start_matches(['/', '\\']);
            if after.is_empty() {
                ".".to_owned()
            } else {
                after.to_owned()
            }
        }
        None => rest.to_owned(),
    }
}

/// The relative local path of a requirement target: a path, a `file:` URL or
/// a hatch `{root:uri}` URL, without `#fragment` and `[extras]`. `None` for
/// other URLs, version-control links, absolute paths and nothing.
fn local_target(target: &str) -> Option<String> {
    let target = target.trim();
    let target = target.split('#').next().unwrap_or(target).trim();
    let target = strip_extras(target);
    let path = if let Some(path) = hatch_root(target) {
        path
    } else if let Some(rest) = target.strip_prefix("file:") {
        let rest = rest.strip_prefix("//").unwrap_or(rest);
        if rest.starts_with("localhost/") {
            return None;
        }
        strip_project_root(rest)
    } else {
        if target.contains("://") || is_vcs(target) {
            return None;
        }
        target.to_owned()
    };
    (!path.is_empty() && !is_absolute(&path)).then_some(path)
}

/// Splits `text` on `separator` outside quotes and brackets.
fn split_top_level(text: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut quote: Option<char> = None;
    let mut depth = 0i32;
    let mut start = 0;
    for (index, c) in text.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '[' | '{' | '(') => depth += 1,
            (None, ']' | '}' | ')') => depth -= 1,
            (None, c) if c == separator && depth <= 0 => {
                parts.push(&text[start..index]);
                start = index + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

// ------------------------------------------------------------ requirements

/// Removes a pip comment: `#` at the start of the line or after whitespace.
/// A `#` inside a word, as in `#egg=`, stays.
fn pip_comment(line: &str) -> &str {
    let mut after_space = true;
    for (index, c) in line.char_indices() {
        if c == '#' && after_space {
            return &line[..index];
        }
        after_space = c.is_whitespace();
    }
    line
}

/// The logical lines of a requirements file, with `\` continuations joined
/// as pip joins them, each with the 0-based index of its first line.
fn requirement_lines(lines: &[&str]) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut pending: Option<(usize, String)> = None;
    for (index, line) in lines.iter().enumerate() {
        let comment = line.trim_start().starts_with('#');
        if line.ends_with('\\') && !comment {
            let body = line.trim_end_matches('\\');
            match &mut pending {
                Some((_, joined)) => joined.push_str(body),
                None => pending = Some((index, body.to_owned())),
            }
            continue;
        }
        // A comment line ends a continuation, and is dropped later.
        let line = if comment { "" } else { line };
        match pending.take() {
            Some((start, mut joined)) => {
                joined.push_str(line);
                out.push((start, joined));
            }
            None => out.push((index, (*line).to_owned())),
        }
    }
    out.extend(pending);
    out
}

/// The kind and the path of one requirement line, comments removed.
fn requirement_ref(line: &str) -> Option<(&'static str, String)> {
    if line.starts_with('-') {
        if let Some(target) = requirement_include(line) {
            return Some(("requirements include", target));
        }
        if let Some(target) = option_value(line, "-c", "--constraint") {
            return Some(("constraints file", target));
        }
        let target = option_value(line, "-e", "--editable")?;
        // Unquoted, the target ends at whitespace; quoted, it may hold spaces.
        let target = if line.contains(['"', '\'']) {
            target.as_str()
        } else {
            first_word(&target)
        };
        return local_target(target).map(|path| ("editable install", path));
    }
    let word = first_word(line);
    let bare = strip_extras(word.split('#').next().unwrap_or(word));
    if is_relative_path(bare) || word.starts_with("file:") {
        return local_target(word).map(|path| ("local path", path));
    }
    let (name, url) = line.split_once('@')?;
    let url = first_word(url).trim_end_matches(';');
    if looks_like_name(name) && url.starts_with("file:") {
        return local_target(url).map(|path| ("local path", path));
    }
    None
}

/// Local paths in a pip requirements file:
///
/// - `"editable install"`: `-e <path>`, `--editable <path>` or
///   `--editable=<path>` with a local target (not a URL or VCS link).
/// - `"local path"`: a line that is a relative path (`./x`, `../x[dev]`,
///   `.`), `name @ file:<path>` or a bare `file:<path>`.
/// - `"requirements include"`: the target of `-r` / `--requirement`.
/// - `"constraints file"`: the target of `-c` / `--constraint`.
///
/// Absolute paths, package names, URLs, other options and comments are left
/// out. A line continued with `\` is reported at its first line.
#[must_use]
pub fn requirements_refs(text: &str) -> Vec<PathRef> {
    let lines: Vec<&str> = strip_bom(text).lines().collect();
    let mut out = Vec::new();
    for (index, logical) in requirement_lines(&lines) {
        let line = pip_comment(&logical).trim();
        if line.is_empty() {
            continue;
        }
        if let Some((what, path)) = requirement_ref(line) {
            out.push(PathRef {
                path,
                what,
                line: index + 1,
                text: line_text(&lines, index + 1),
            });
        }
    }
    out
}

// -------------------------------------------------------------------- TOML

/// The parts of a dotted TOML key or table name, unquoted:
/// `target.'cfg(unix)'.dependencies` gives `target`, `cfg(unix)`, `dependencies`.
fn dotted(name: &str) -> Vec<String> {
    split_top_level(name, '.')
        .into_iter()
        .map(unquote)
        .collect()
}

/// The value of a TOML string: a literal string as is, a basic string with
/// `\\` and `\"` unescaped. `None` for anything that is not a string.
fn toml_string(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if let Some(inner) = raw
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
    {
        return Some(inner.to_owned());
    }
    let inner = raw.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some(next @ ('\\' | '"')) => out.push(next),
            Some(next) => {
                out.push('\\');
                out.push(next);
            }
            None => out.push('\\'),
        }
    }
    Some(out)
}

/// The outermost inline tables in a value: the value itself when it is one,
/// or each table of an array of them.
fn inline_tables(value: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut quote: Option<char> = None;
    let mut depth = 0i32;
    let mut start: Option<(usize, i32)> = None;
    for (index, c) in value.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '[' | '{') => {
                if c == '{' && start.is_none() {
                    start = Some((index, depth));
                }
                depth += 1;
            }
            (None, ']' | '}') => {
                depth -= 1;
                if c == '}' {
                    if let Some((open, at)) = start {
                        if at == depth {
                            out.push(&value[open..=index]);
                            start = None;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The raw value of `key` in an inline table such as `{ path = "../x" }`.
fn inline_value<'a>(table: &'a str, key: &str) -> Option<&'a str> {
    let inner = table.trim().strip_prefix('{')?.strip_suffix('}')?;
    split_top_level(inner, ',').into_iter().find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (unquote(name) == key).then_some(value.trim())
    })
}

/// The 1-based line of `needle` inside `entry`, which may span lines (a
/// multi-line array). The entry's first line when it is not found.
fn line_in_entry(lines: &[&str], entry: &TomlEntry, needle: &str) -> usize {
    if needle.is_empty() {
        return entry.line;
    }
    let start = entry.line.saturating_sub(1);
    let mut balance = 0;
    for (index, raw) in lines.iter().enumerate().skip(start) {
        let code = strip_comment(raw);
        let code = if index == start {
            code.split_once('=').map_or(code, |(_, value)| value)
        } else {
            code
        };
        if code.contains(needle) {
            return index + 1;
        }
        balance += bracket_balance(code);
        if balance <= 0 {
            break;
        }
    }
    entry.line
}

/// Paths from dependency tables that `is_table` recognises by their dotted
/// name: inline tables `x = { path = "../x" }` (or an array of them), dotted
/// keys `x.path = "../x"`, and the table form `[<table>.x]` with
/// `path = "../x"`. Each path comes with its line.
fn table_paths(
    lines: &[&str],
    entry: &TomlEntry,
    is_table: fn(&[String]) -> bool,
) -> Vec<(String, usize)> {
    let section = dotted(&entry.section);
    let value = entry.value.trim();
    if is_table(&section) {
        if value.starts_with(['{', '[']) {
            return inline_tables(value)
                .into_iter()
                .filter_map(|table| inline_value(table, "path"))
                .filter_map(|raw| Some((toml_string(raw)?, line_in_entry(lines, entry, raw))))
                .collect();
        }
        let key = dotted(&entry.key);
        if key.len() >= 2 && key.last().is_some_and(|last| last == "path") {
            return toml_string(value)
                .map(|path| (path, entry.line))
                .into_iter()
                .collect();
        }
        return Vec::new();
    }
    let parent = section.split_last().map(|(_, parent)| parent);
    if entry.key == "path" && parent.is_some_and(|parent| !parent.is_empty() && is_table(parent)) {
        return toml_string(value)
            .map(|path| (path, entry.line))
            .into_iter()
            .collect();
    }
    Vec::new()
}

fn names_are(section: &[String], names: &[&str]) -> bool {
    section.len() == names.len() && section.iter().zip(names).all(|(a, b)| a == b)
}

/// `[tool.uv.sources]`.
fn is_uv_sources(section: &[String]) -> bool {
    names_are(section, &["tool", "uv", "sources"])
}

/// `[tool.poetry.dependencies]`, `[tool.poetry.dev-dependencies]` and
/// `[tool.poetry.group.<g>.dependencies]`.
fn is_poetry_dependencies(section: &[String]) -> bool {
    names_are(section, &["tool", "poetry", "dependencies"])
        || names_are(section, &["tool", "poetry", "dev-dependencies"])
        || (section.len() == 5
            && names_are(&section[..3], &["tool", "poetry", "group"])
            && section[4] == "dependencies")
}

/// Whether `key` in `section` is a list of PEP 508 requirement strings.
fn is_requirement_list(section: &[String], key: &str) -> bool {
    (names_are(section, &["project"]) && key == "dependencies")
        || (names_are(section, &["tool", "uv"]) && key == "dev-dependencies")
        || names_are(section, &["project", "optional-dependencies"])
        || names_are(section, &["dependency-groups"])
        || names_are(section, &["tool", "pdm", "dev-dependencies"])
}

/// The relative path of a PEP 508 requirement that points at a file URL:
/// `x @ file:../x`, `x @ file:///${PROJECT_ROOT}/../x`, `x @ {root:uri}/../x`,
/// and PDM's `-e file:///${PROJECT_ROOT}/x#egg=x`.
fn file_url(spec: &str) -> Option<String> {
    let spec = spec.trim();
    let spec = spec
        .strip_prefix("-e")
        .filter(|rest| rest.starts_with(char::is_whitespace))
        .map_or(spec, str::trim_start);
    let target = match spec.split_once('@') {
        Some((name, url)) if looks_like_name(name) => url,
        _ => spec,
    };
    let target = target.split(';').next().unwrap_or(target);
    let target = first_word(target);
    if target.starts_with("file:") || target.starts_with("{root:") {
        local_target(target)
    } else {
        None
    }
}

/// Local paths in a `pyproject.toml`:
///
/// - `"uv path source"`: `[tool.uv.sources]` entries with `path`, inline
///   (`x = { path = "../x" }`) or as `[tool.uv.sources.x]` tables.
/// - `"poetry path dependency"`: `path` dependencies under
///   `[tool.poetry.dependencies]`, `[tool.poetry.dev-dependencies]` and
///   `[tool.poetry.group.<g>.dependencies]`, inline or as tables.
/// - `"file URL"`: PEP 508 strings pointing at a `file:` or hatch
///   `{root:uri}` URL in `[project] dependencies`,
///   `[project.optional-dependencies]`, `[dependency-groups]`,
///   `[tool.pdm.dev-dependencies]` and `[tool.uv] dev-dependencies`.
///
/// Workspace, git and URL sources are left out.
#[must_use]
pub fn pyproject_refs(text: &str) -> Vec<PathRef> {
    let text = strip_bom(text);
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut push = |path: String, what: &'static str, line: usize| {
        out.push(PathRef {
            path,
            what,
            line,
            text: line_text(&lines, line),
        });
    };
    for entry in parse_toml(text) {
        for (path, line) in table_paths(&lines, &entry, is_uv_sources) {
            push(path, "uv path source", line);
        }
        for (path, line) in table_paths(&lines, &entry, is_poetry_dependencies) {
            push(path, "poetry path dependency", line);
        }
        if is_requirement_list(&dotted(&entry.section), &entry.key) {
            for spec in array_strings(&entry.value) {
                if let Some(path) = file_url(&spec) {
                    push(path, "file URL", line_in_entry(&lines, &entry, &spec));
                }
            }
        }
    }
    out
}

/// `[dependencies]`, `[dev-dependencies]`, `[build-dependencies]`, their
/// `[target.<cfg>.…]` variants and `[workspace.dependencies]`.
fn is_cargo_dependencies(section: &[String]) -> bool {
    let kind = |name: &String| {
        matches!(
            name.as_str(),
            "dependencies" | "dev-dependencies" | "build-dependencies"
        )
    };
    match section {
        [name] => kind(name),
        [workspace, name] => workspace == "workspace" && name == "dependencies",
        [target, _, name] => target == "target" && kind(name),
        _ => false,
    }
}

/// Path dependencies in a `Cargo.toml` (`what` is `"cargo path dependency"`):
/// `x = { path = "../x" }`, `x.path = "../x"` and `[dependencies.x]` tables
/// with `path`, under the dependency tables, their `[target.…]` variants
/// and `[workspace.dependencies]`.
#[must_use]
pub fn cargo_path_refs(text: &str) -> Vec<PathRef> {
    let text = strip_bom(text);
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    for entry in parse_toml(text) {
        for (path, line) in table_paths(&lines, &entry, is_cargo_dependencies) {
            out.push(PathRef {
                path,
                what: "cargo path dependency",
                line,
                text: line_text(&lines, line),
            });
        }
    }
    out
}

// ------------------------------------------------------------ package.json

/// The dependency fields of a package.json, in the order they are reported.
const PACKAGE_FIELDS: [&str; 4] = [
    "dependencies",
    "devDependencies",
    "peerDependencies",
    "optionalDependencies",
];

/// The protocols that name a local path.
const PATH_PROTOCOLS: [&str; 3] = ["file:", "link:", "portal:"];

fn package_ref(dependency: &str, spec: &str, field: &'static str) -> Option<PackageRef> {
    let trimmed = spec.trim();
    let reference = |what, path: Option<String>, package: Option<String>| PackageRef {
        dependency: dependency.to_owned(),
        spec: spec.to_owned(),
        what,
        path,
        package,
        field,
    };
    for protocol in PATH_PROTOCOLS {
        if let Some(rest) = trimmed.strip_prefix(protocol) {
            let rest = rest.strip_prefix("//").unwrap_or(rest).trim();
            if rest.is_empty() {
                return None;
            }
            return Some(reference(protocol, Some(rest.to_owned()), None));
        }
    }
    let rest = trimmed.strip_prefix("workspace:")?.trim();
    if rest.starts_with('.') {
        return Some(reference("workspace:", Some(rest.to_owned()), None));
    }
    // `workspace:other@*` and `workspace:@scope/other@^1` name another
    // package; `workspace:*`, `workspace:^1` and `npm:` aliases do not.
    let alias = rest
        .get(1..)
        .and_then(|after| after.rfind('@'))
        .map(|at| &rest[..=at])
        .filter(|name| !name.starts_with("npm:"));
    let package = alias.unwrap_or(dependency);
    Some(reference("workspace:", None, Some(package.to_owned())))
}

/// Local dependencies of a `package.json`: `file:`, `link:`, `portal:` and
/// `workspace:` specs in `dependencies`, `devDependencies`,
/// `peerDependencies` and `optionalDependencies`, in that order and then in
/// the order of the keys. Non-string values are skipped; `file:` tarballs
/// are returned too.
#[must_use]
pub fn package_json_refs(json: &Value) -> Vec<PackageRef> {
    let mut out = Vec::new();
    for field in PACKAGE_FIELDS {
        let Some(dependencies) = json.get(field).and_then(Value::as_object) else {
            continue;
        };
        for (dependency, value) in dependencies {
            if let Some(spec) = value.as_str() {
                out.extend(package_ref(dependency, spec, field));
            }
        }
    }
    out
}

// -------------------------------------------------------------- Dockerfile

/// The escape character set by a `# escape=` parser directive at the top
/// of a Dockerfile; `\` without one.
fn escape_char(lines: &[&str]) -> char {
    let mut escape = '\\';
    for line in lines {
        let Some(body) = line.trim().strip_prefix('#') else {
            break;
        };
        let Some((key, value)) = body.split_once('=') else {
            break;
        };
        let key = key.trim();
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric()) {
            break;
        }
        if key.eq_ignore_ascii_case("escape") {
            if let Some(c @ ('`' | '\\')) = value.trim().chars().next() {
                escape = c;
            }
        }
    }
    escape
}

/// The terminator of a heredoc in a word such as `<<EOF`, `<<-"EOF"` or
/// `cat<<'END'`, and whether leading tabs are stripped from its body.
fn heredoc(word: &str) -> Option<(&str, bool)> {
    let at = word.find("<<")?;
    let rest = &word[at + 2..];
    let (rest, tabs) = match rest.strip_prefix('-') {
        Some(rest) => (rest, true),
        None => (rest, false),
    };
    let name = rest.trim_matches(['"', '\'']);
    let valid = name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    valid.then_some((name, tabs))
}

/// A URL or git source of `ADD`, which is not in the build context.
fn is_remote_source(source: &str) -> bool {
    source.contains("://")
        || source.starts_with("git@")
        || source.ends_with(".git")
        || source.contains(".git#")
}

/// The words of a shell-form instruction, split on whitespace outside
/// quotes, with the quotes removed: `"a b" c` gives `a b` and `c`.
fn shell_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    for c in text.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            (None, c) => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(word);
    }
    words
}

/// The `COPY` or `ADD` an instruction line is, with its sources.
fn copy_sources(instruction: &str) -> Option<(&'static str, Vec<String>)> {
    let instruction = instruction.trim();
    let (word, rest) = instruction
        .split_once(char::is_whitespace)
        .unwrap_or((instruction, ""));
    let name = if word.eq_ignore_ascii_case("COPY") {
        "COPY"
    } else if word.eq_ignore_ascii_case("ADD") {
        "ADD"
    } else {
        return None;
    };
    let mut rest = rest.trim();
    while rest.starts_with("--") {
        let (flag, after) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        if flag.split('=').next() == Some("--from") {
            // From another stage or image, not the build context.
            return None;
        }
        rest = after.trim_start();
    }
    let args = if rest.starts_with('[') {
        serde_json::from_str::<Vec<String>>(rest).unwrap_or_else(|_| shell_words(rest))
    } else {
        shell_words(rest)
    };
    let (_, sources) = args.split_last()?;
    if sources.is_empty() {
        return None;
    }
    let sources: Vec<String> = sources
        .iter()
        .filter(|source| name == "COPY" || !is_remote_source(source))
        .cloned()
        .collect();
    (!sources.is_empty()).then_some((name, sources))
}

/// The `COPY` and `ADD` instructions of a Dockerfile that copy from the
/// build context.
///
/// Instructions are case-insensitive and may continue over lines with the
/// escape character (`\`, or a backtick after an `escape` parser directive). Flags such as
/// `--chown=` are skipped, and an instruction with `--from=` is left out.
/// Both the shell form and the JSON array form are read; the last argument
/// is the destination. `ADD` sources that are URLs or git repositories are
/// dropped. Heredocs are skipped with their bodies.
#[must_use]
pub fn dockerfile_copies(text: &str) -> Vec<CopyRef> {
    let lines: Vec<&str> = strip_bom(text).lines().collect();
    let escape = escape_char(&lines);
    let mut out = Vec::new();
    let mut next = 0;
    while let Some(first) = lines.get(next) {
        let start = next;
        next += 1;
        let first_trimmed = first.trim();
        if first_trimmed.is_empty() || first_trimmed.starts_with('#') {
            continue;
        }
        let mut joined = String::new();
        let mut current = *first;
        loop {
            let Some(body) = current.trim_end().strip_suffix(escape) else {
                joined.push_str(current.trim_end());
                break;
            };
            joined.push_str(body);
            // Comment and blank lines inside a continuation are dropped.
            while lines.get(next).is_some_and(|line| {
                let line = line.trim();
                line.is_empty() || line.starts_with('#')
            }) {
                next += 1;
            }
            let Some(line) = lines.get(next).copied() else {
                break;
            };
            current = line;
            next += 1;
        }
        let heredocs: Vec<(&str, bool)> = joined.split_whitespace().filter_map(heredoc).collect();
        if !heredocs.is_empty() {
            for (terminator, tabs) in heredocs {
                while let Some(line) = lines.get(next).copied() {
                    next += 1;
                    let line = if tabs {
                        line.trim_start_matches('\t')
                    } else {
                        line
                    };
                    if line.trim_end() == terminator {
                        break;
                    }
                }
            }
            continue;
        }
        if let Some((instruction, sources)) = copy_sources(&joined) {
            out.push(CopyRef {
                instruction,
                sources,
                line: start + 1,
                text: snippet(first),
            });
        }
    }
    out
}

// ----------------------------------------------------------------- Compose

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// The key and the value of a `key: value` or `key:` line.
fn yaml_pair(line: &str) -> Option<(String, &str)> {
    if let Some((key, value)) = line.split_once(": ") {
        return Some((unquote(key), value.trim()));
    }
    let key = line.strip_suffix(':')?;
    Some((unquote(key), ""))
}

/// A value without a leading anchor (`&name`) or tag (`!reset`).
fn without_node_props(value: &str) -> &str {
    let mut value = value.trim();
    while value.starts_with(['&', '!']) {
        value = value
            .split_once(char::is_whitespace)
            .map_or("", |(_, rest)| rest.trim_start());
    }
    value
}

/// The block a Compose reader is inside.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Block {
    Build,
    DependsOn,
}

/// A line inside a `build:` or `depends_on:` block. `child` is true for a
/// line at the block's first indentation level.
fn block_line(block: Block, service: &mut ComposeBuild, trimmed: &str, child: bool, line: usize) {
    if !child {
        return;
    }
    match block {
        Block::Build => {
            let Some((key, value)) = yaml_pair(trimmed) else {
                return;
            };
            let value = unquote(without_node_props(value));
            match key.as_str() {
                "context" if !value.is_empty() => service.context = Some((value, line)),
                "dockerfile" if !value.is_empty() => service.dockerfile = Some(value),
                _ => {}
            }
        }
        Block::DependsOn => {
            let name = match trimmed.strip_prefix('-') {
                Some(item) => unquote(item),
                None => match yaml_pair(trimmed) {
                    Some((key, _)) => key,
                    None => return,
                },
            };
            if !name.is_empty() && name != "<<" {
                service.depends_on.push((name, line));
            }
        }
    }
}

/// `build: { context: ./x, dockerfile: D }`.
fn flow_build(service: &mut ComposeBuild, inner: &str, line: usize) {
    service.context = Some((".".to_owned(), line));
    for pair in split_top_level(inner, ',') {
        let Some((key, value)) = pair.split_once(':') else {
            continue;
        };
        let value = unquote(value);
        match unquote(key).as_str() {
            "context" if !value.is_empty() => service.context = Some((value, line)),
            "dockerfile" if !value.is_empty() => service.dockerfile = Some(value),
            _ => {}
        }
    }
}

/// `depends_on: [db, cache]` or `depends_on: { db: { condition: … } }`.
fn flow_depends_on(service: &mut ComposeBuild, value: &str, line: usize) {
    let (inner, mapping) = if let Some(inner) = value
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    {
        (inner, false)
    } else if let Some(inner) = value
        .strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
    {
        (inner, true)
    } else {
        return;
    };
    for item in split_top_level(inner, ',') {
        let name = if mapping {
            unquote(item.split_once(':').map_or(item, |(key, _)| key))
        } else {
            unquote(item)
        };
        if !name.is_empty() {
            service.depends_on.push((name, line));
        }
    }
}

/// Every service of a Docker Compose file with its build context, its
/// Dockerfile and the services it depends on.
///
/// The file is read line by line by indentation, like the port reader in
/// `services.rs`: `services:` at column 0, any consistent indent width,
/// comments, quotes and CRLF. `build:` is read as a path or as a mapping
/// with `context:` and `dockerfile:`; `depends_on:` as a list, an inline
/// list or a mapping. `extends`, `include` and merge keys (`<<: *x`) are
/// not followed.
#[must_use]
pub fn compose_builds(text: &str) -> Vec<ComposeBuild> {
    let mut services: Vec<ComposeBuild> = Vec::new();
    let mut in_services = false;
    let mut service_indent: Option<usize> = None;
    let mut field_indent: Option<usize> = None;
    let mut current = false;
    let mut block: Option<(Block, usize)> = None;
    let mut child_indent: Option<usize> = None;
    for (index, raw) in strip_bom(text).lines().enumerate() {
        let number = index + 1;
        let line = raw.split(" #").next().unwrap_or(raw).trim_end();
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let depth = indent(line);
        if depth == 0 {
            in_services = trimmed == "services:";
            service_indent = None;
            current = false;
            continue;
        }
        if !in_services {
            continue;
        }
        let service_depth = *service_indent.get_or_insert(depth);
        if depth <= service_depth {
            field_indent = None;
            block = None;
            current = false;
            if depth == service_depth {
                if let Some((key, _)) = yaml_pair(trimmed) {
                    if key != "<<" && !key.is_empty() && !trimmed.starts_with('-') {
                        services.push(ComposeBuild {
                            service: key,
                            line: number,
                            ..ComposeBuild::default()
                        });
                        current = true;
                    }
                }
            }
            continue;
        }
        let Some(service) = services.last_mut().filter(|_| current) else {
            continue;
        };
        if let Some((kind, block_depth)) = block {
            if depth > block_depth || depth == block_depth && trimmed.starts_with('-') {
                let child = *child_indent.get_or_insert(depth);
                block_line(kind, service, trimmed, depth == child, number);
                continue;
            }
            block = None;
        }
        if depth != *field_indent.get_or_insert(depth) {
            continue;
        }
        let Some((key, value)) = yaml_pair(trimmed) else {
            continue;
        };
        let value = without_node_props(value);
        match key.as_str() {
            "build" => {
                if value.is_empty() {
                    service.context = Some((".".to_owned(), number));
                    block = Some((Block::Build, depth));
                    child_indent = None;
                } else if let Some(inner) = value
                    .strip_prefix('{')
                    .and_then(|rest| rest.strip_suffix('}'))
                {
                    flow_build(service, inner, number);
                } else if !value.starts_with('*') {
                    service.context = Some((unquote(value), number));
                }
            }
            "depends_on" => {
                if value.is_empty() {
                    block = Some((Block::DependsOn, depth));
                    child_indent = None;
                } else {
                    flow_depends_on(service, value, number);
                }
            }
            _ => {}
        }
    }
    services
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(refs: &[PathRef]) -> Vec<(&str, &str, usize)> {
        refs.iter()
            .map(|r| (r.what, r.path.as_str(), r.line))
            .collect()
    }

    // ------------------------------------------------------- requirements

    #[test]
    fn an_editable_install_of_a_sibling_is_a_reference() {
        let refs = requirements_refs(
            "-e ../shared\n--editable ../core[dev]\n--editable=./libs/x#egg=x\n-e.\n-e \"../with space\"\n",
        );
        assert_eq!(
            paths(&refs),
            [
                ("editable install", "../shared", 1),
                ("editable install", "../core", 2),
                ("editable install", "./libs/x", 3),
                ("editable install", ".", 4),
                ("editable install", "../with space", 5),
            ]
        );
        assert_eq!(refs[0].text, "-e ../shared");
    }

    #[test]
    fn editable_installs_from_version_control_or_urls_are_not_references() {
        let refs = requirements_refs(
            "-e git+https://github.com/o/r.git#egg=r\n-e hg+https://h/r\n-e https://x/y.zip\n-e git@github.com:o/r.git\n-e /abs/path\n-e C:\\abs\n-e file:///abs/x\n",
        );
        assert_eq!(paths(&refs), []);
    }

    #[test]
    fn relative_path_lines_are_local_paths() {
        let refs = requirements_refs(
            "./pkg\n../other[extra1,extra2]\n.\\win\n..\\up\n.\n..\n.[dev]\n./hashed --hash=sha256:abc\n",
        );
        assert_eq!(
            paths(&refs),
            [
                ("local path", "./pkg", 1),
                ("local path", "../other", 2),
                ("local path", ".\\win", 3),
                ("local path", "..\\up", 4),
                ("local path", ".", 5),
                ("local path", "..", 6),
                ("local path", ".", 7),
                ("local path", "./hashed", 8),
            ]
        );
    }

    #[test]
    fn file_urls_in_requirements_are_local_paths() {
        let refs = requirements_refs(
            "lib @ file:../lib\nother[x] @ file://../other ; python_version >= \"3.9\"\nfile:../bare\nabs @ file:///opt/abs\nfile:///C:/abs\n",
        );
        assert_eq!(
            paths(&refs),
            [
                ("local path", "../lib", 1),
                ("local path", "../other", 2),
                ("local path", "../bare", 3),
            ]
        );
    }

    #[test]
    fn includes_and_constraints_are_returned_whatever_they_look_like() {
        let refs = requirements_refs(
            "-r base.txt\n--requirement ../common/reqs.txt\n--requirement=dev.txt\n-c constraints.txt\n--constraint=/abs/c.txt\n-rinline.txt\n",
        );
        assert_eq!(
            paths(&refs),
            [
                ("requirements include", "base.txt", 1),
                ("requirements include", "../common/reqs.txt", 2),
                ("requirements include", "dev.txt", 3),
                ("constraints file", "constraints.txt", 4),
                ("constraints file", "/abs/c.txt", 5),
                ("requirements include", "inline.txt", 6),
            ]
        );
    }

    #[test]
    fn names_urls_options_and_comments_are_not_references() {
        let refs = requirements_refs(
            "# -e ../commented\nrequests>=2\nDjango[argon2]==5.0\nhttps://example.com/x.whl\npkg @ https://example.com/p.zip\ngit+https://github.com/o/r\n--index-url https://pypi.org/simple\n--extra-index-url https://x\n-i https://x\n--require-hashes\n--config-settings x=y\n   \nflask  # ./not-a-path\n/abs/pkg\nC:\\pkg\n",
        );
        assert_eq!(paths(&refs), []);
    }

    #[test]
    fn a_comment_after_a_path_is_dropped() {
        let refs = requirements_refs("./x # the x lib\n-e ../y\t# editable\n");
        assert_eq!(
            paths(&refs),
            [("local path", "./x", 1), ("editable install", "../y", 2)]
        );
    }

    #[test]
    fn continued_lines_are_joined_and_report_their_first_line() {
        let text = "\u{feff}requests \\\r\n  >=2\r\n-e \\\r\n../joined\r\n# c \\\r\n./after\r\n";
        let refs = requirements_refs(text);
        assert_eq!(
            paths(&refs),
            [
                ("editable install", "../joined", 3),
                ("local path", "./after", 6)
            ]
        );
        assert_eq!(refs[0].text, "-e \\");
    }

    #[test]
    fn a_continuation_at_the_end_of_the_file_is_still_read() {
        let refs = requirements_refs("-r \\\nbase.txt \\");
        assert_eq!(paths(&refs), [("requirements include", "base.txt", 1)]);
    }

    #[test]
    fn long_lines_are_cut_on_a_char_boundary() {
        let line = format!("-e ./{}", "é".repeat(200));
        let refs = requirements_refs(&line);
        let text = &refs[0].text;
        assert_eq!(text.chars().count(), MAX_TEXT);
        assert!(text.ends_with('…'));
        assert!(text.starts_with("-e ./é"));
    }

    #[test]
    fn requirements_readers_do_not_panic_on_odd_input() {
        for text in [
            "",
            "\u{feff}",
            "\\",
            "-e",
            "-e ",
            "-r",
            "--editable=",
            "@",
            "x @",
            "@ file:",
            "[",
            "]",
            "-c\n\\\n\\",
            "file:",
            "file://",
            "é @ file:é",
            "#",
            ".[",
            "{root:uri}",
        ] {
            let _ = requirements_refs(text);
        }
        assert_eq!(paths(&requirements_refs("-e\nfile:\n")), []);
    }

    // ---------------------------------------------------------- pyproject

    #[test]
    fn uv_path_sources_inline_and_as_tables_are_references() {
        let text = r#"[project]
name = "app"

[tool.uv.sources]
lib = { path = "../lib", editable = true }
ws = { workspace = true }
remote = { git = "https://github.com/o/r" }
url = { url = "https://x/y.whl" }
multi = [
  { path = "../multi-a", marker = "sys_platform == 'linux'" },
  { index = "pytorch" },
]
other.path = "../dotted"

[tool.uv.sources.tabled]
path = '..\win'
editable = true
"#;
        let refs = pyproject_refs(text);
        assert_eq!(
            paths(&refs),
            [
                ("uv path source", "../lib", 5),
                ("uv path source", "../multi-a", 10),
                ("uv path source", "../dotted", 13),
                ("uv path source", "..\\win", 16),
            ]
        );
        assert_eq!(refs[0].text, "lib = { path = \"../lib\", editable = true }");
    }

    #[test]
    fn poetry_path_dependencies_are_references() {
        let text = r#"[tool.poetry.dependencies]
python = "^3.11"
core = { path = "../core", develop = true }
requests = "^2"
gitdep = { git = "https://github.com/o/r.git" }

[tool.poetry.dev-dependencies]
devlib = {path = "../devlib"}

[tool.poetry.group.test.dependencies]
testlib = { path = "libs/test" }

[tool.poetry.dependencies.tabled]
path = "../tabled"
develop = true
"#;
        assert_eq!(
            paths(&pyproject_refs(text)),
            [
                ("poetry path dependency", "../core", 3),
                ("poetry path dependency", "../devlib", 8),
                ("poetry path dependency", "libs/test", 11),
                ("poetry path dependency", "../tabled", 14),
            ]
        );
    }

    #[test]
    fn file_urls_in_dependency_lists_are_references() {
        let text = r#"[project]
name = "app"
dependencies = [
    "requests>=2",
    "lib @ file:///${PROJECT_ROOT}/../lib",
    "other @ file:../other ; python_version >= '3.10'",
    "hatched @ {root:uri}/../hatched",
    "up @ {root:parent:uri}/sibling",
    "remote @ https://example.com/r.whl",
    "abs @ file:///opt/abs",
]

[project.optional-dependencies]
extra = ["x[dev] @ file:${PROJECT_ROOT}/libs/x"]

[dependency-groups]
dev = ["dg @ file:../dg", {include-group = "test"}]

[tool.pdm.dev-dependencies]
test = ["-e file:///${PROJECT_ROOT}/../pdm#egg=pdm"]

[tool.uv]
dev-dependencies = ["uvdev @ file:../uvdev"]
"#;
        assert_eq!(
            paths(&pyproject_refs(text)),
            [
                ("file URL", "../lib", 5),
                ("file URL", "../other", 6),
                ("file URL", "../hatched", 7),
                ("file URL", "../sibling", 8),
                ("file URL", "libs/x", 14),
                ("file URL", "../dg", 17),
                ("file URL", "../pdm", 20),
                ("file URL", "../uvdev", 23),
            ]
        );
    }

    #[test]
    fn file_urls_outside_dependency_lists_are_not_references() {
        let text =
            "[tool.other]\nlist = [\"x @ file:../x\"]\n[project]\nkeywords = [\"y @ file:../y\"]\n";
        assert_eq!(paths(&pyproject_refs(text)), []);
    }

    #[test]
    fn pyproject_lines_are_right_with_crlf_comments_and_a_bom() {
        let text = "\u{feff}# top\r\n[tool.uv.sources]\r\n# lib = { path = \"../no\" }\r\nlib = { path = \"../lib\" } # yes\r\n";
        assert_eq!(
            paths(&pyproject_refs(text)),
            [("uv path source", "../lib", 4)]
        );
    }

    #[test]
    fn pyproject_readers_do_not_panic_on_odd_input() {
        for text in [
            "",
            "[",
            "[tool.uv.sources]\nx = {",
            "[tool.uv.sources]\nx = { path = }",
            "[tool.uv.sources]\nx = { path = \" }",
            "[project]\ndependencies = [\"@\", \"x @\", \"x @ file:\"",
            "[tool.uv.sources.x]\npath = 1",
            "[tool.poetry.group]\nx = 1",
            "=\n==\n[.]\n.=.",
        ] {
            let _ = pyproject_refs(text);
        }
    }

    // ------------------------------------------------------------- Cargo

    #[test]
    fn cargo_path_dependencies_in_every_table_are_references() {
        let text = r#"[package]
name = "app"
path = "not-a-dep"

[dependencies]
core = { path = "../core", version = "0.1", features = ["x"] }
serde = "1"
dotted.path = "../dotted"
remote = { git = "https://github.com/o/r" }
ws = { workspace = true }

[dev-dependencies]
fixtures = { path = "tests/fixtures" }

[build-dependencies]
gen = { path = "../gen" }

[target.'cfg(windows)'.dependencies]
win = { path = "../win" }

[target."cfg(target_os = \"linux\")".dev-dependencies]
linux = { path = "../linux" }

[workspace.dependencies]
shared = { path = "crates/shared", version = "0.1.0" }

[dependencies.tabled]
version = "1"
path = "../tabled"

[target.x86_64-pc-windows-msvc.build-dependencies.winbuild]
path = '..\winbuild'

[bin]
path = "src/main.rs"
"#;
        let refs = cargo_path_refs(text);
        assert!(refs.iter().all(|r| r.what == "cargo path dependency"));
        assert_eq!(
            refs.iter()
                .map(|r| (r.path.as_str(), r.line))
                .collect::<Vec<_>>(),
            [
                ("../core", 6),
                ("../dotted", 8),
                ("tests/fixtures", 13),
                ("../gen", 16),
                ("../win", 19),
                ("../linux", 22),
                ("crates/shared", 25),
                ("../tabled", 29),
                ("..\\winbuild", 32),
            ]
        );
        assert_eq!(refs[1].text, "dotted.path = \"../dotted\"");
    }

    #[test]
    fn cargo_basic_strings_are_unescaped() {
        let text = "[dependencies]\nx = { path = \"..\\\\x\" }\n";
        assert_eq!(paths(&cargo_path_refs(text))[0].1, "..\\x");
    }

    #[test]
    fn a_dependency_named_path_is_not_a_path_dependency() {
        let text = "[dependencies]\npath = \"1.0\"\n[dependencies.path]\nversion = \"1\"\n";
        assert_eq!(paths(&cargo_path_refs(text)), []);
    }

    #[test]
    fn cargo_readers_do_not_panic_on_odd_input() {
        for text in [
            "",
            "[dependencies]\nx = {",
            "[dependencies]\nx.path = ",
            "[target]\npath = \"x\"",
            "[[bin]]\npath = \"x\"",
            "[dependencies.]\npath = \"x\"",
        ] {
            let _ = cargo_path_refs(text);
        }
        assert_eq!(
            paths(&cargo_path_refs("[[bin]]\npath = \"src/x.rs\"\n")),
            []
        );
    }

    // ------------------------------------------------------- package.json

    fn package(
        dependency: &str,
        spec: &str,
        what: &'static str,
        field: &'static str,
    ) -> PackageRef {
        PackageRef {
            dependency: dependency.to_owned(),
            spec: spec.to_owned(),
            what,
            path: None,
            package: None,
            field,
        }
    }

    #[test]
    fn local_package_json_specs_are_references_in_field_and_key_order() {
        let json: Value = serde_json::from_str(
            r#"{
                "optionalDependencies": { "opt": "link:../opt" },
                "dependencies": {
                    "zeta": "file:../zeta",
                    "alpha": "workspace:*",
                    "react": "^18.0.0",
                    "tarball": "file:../dist/x-1.0.0.tgz",
                    "aliased": "workspace:real-name@^1.0.0",
                    "scoped": "workspace:@scope/real@*",
                    "byPath": "workspace:../by-path",
                    "npmAlias": "workspace:npm:other@*",
                    "notString": 3
                },
                "devDependencies": { "portal": "portal:./portal", "caret": "workspace:^" },
                "peerDependencies": { "peer": "file://../peer" }
            }"#,
        )
        .unwrap_or(Value::Null);
        let with = |mut r: PackageRef, path: Option<&str>, name: Option<&str>| {
            r.path = path.map(str::to_owned);
            r.package = name.map(str::to_owned);
            r
        };
        assert_eq!(
            package_json_refs(&json),
            [
                with(
                    package("zeta", "file:../zeta", "file:", "dependencies"),
                    Some("../zeta"),
                    None
                ),
                with(
                    package("alpha", "workspace:*", "workspace:", "dependencies"),
                    None,
                    Some("alpha")
                ),
                with(
                    package(
                        "tarball",
                        "file:../dist/x-1.0.0.tgz",
                        "file:",
                        "dependencies"
                    ),
                    Some("../dist/x-1.0.0.tgz"),
                    None
                ),
                with(
                    package(
                        "aliased",
                        "workspace:real-name@^1.0.0",
                        "workspace:",
                        "dependencies"
                    ),
                    None,
                    Some("real-name")
                ),
                with(
                    package(
                        "scoped",
                        "workspace:@scope/real@*",
                        "workspace:",
                        "dependencies"
                    ),
                    None,
                    Some("@scope/real")
                ),
                with(
                    package(
                        "byPath",
                        "workspace:../by-path",
                        "workspace:",
                        "dependencies"
                    ),
                    Some("../by-path"),
                    None
                ),
                with(
                    package(
                        "npmAlias",
                        "workspace:npm:other@*",
                        "workspace:",
                        "dependencies"
                    ),
                    None,
                    Some("npmAlias")
                ),
                with(
                    package("portal", "portal:./portal", "portal:", "devDependencies"),
                    Some("./portal"),
                    None
                ),
                with(
                    package("caret", "workspace:^", "workspace:", "devDependencies"),
                    None,
                    Some("caret")
                ),
                with(
                    package("peer", "file://../peer", "file:", "peerDependencies"),
                    Some("../peer"),
                    None
                ),
                with(
                    package("opt", "link:../opt", "link:", "optionalDependencies"),
                    Some("../opt"),
                    None
                ),
            ]
        );
    }

    #[test]
    fn package_json_readers_do_not_panic_on_odd_input() {
        for text in [
            "null",
            "[]",
            "{}",
            r#"{"dependencies": []}"#,
            r#"{"dependencies": {"x": "file:", "y": "workspace:", "z": "workspace:@", "w": "@"}}"#,
        ] {
            let json: Value = serde_json::from_str(text).unwrap_or(Value::Null);
            let _ = package_json_refs(&json);
        }
        let json: Value =
            serde_json::from_str(r#"{"dependencies": {"x": "file:"}}"#).unwrap_or(Value::Null);
        assert_eq!(package_json_refs(&json), []);
    }

    // --------------------------------------------------------- Dockerfile

    fn copies(text: &str) -> Vec<(&'static str, Vec<String>, usize)> {
        dockerfile_copies(text)
            .into_iter()
            .map(|c| (c.instruction, c.sources, c.line))
            .collect()
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn copy_and_add_sources_from_the_context_are_references() {
        let text = "FROM node:20\ncopy package.json pnpm-lock.yaml ./\nAdd ../shared /app/shared\nCOPY --chown=app:app --chmod=644 --link --parents --exclude=*.md --checksum=sha256:x --keep-git-dir=true src/ /app/src/\nCOPY [\"a b\", \"c\", \"/dest/\"]\nCOPY --chown=1:1 [\"json\", \"/d\"]\nCOPY \"quoted dir\" /q\n";
        assert_eq!(
            copies(text),
            [
                ("COPY", strings(&["package.json", "pnpm-lock.yaml"]), 2),
                ("ADD", strings(&["../shared"]), 3),
                ("COPY", strings(&["src/"]), 4),
                ("COPY", strings(&["a b", "c"]), 5),
                ("COPY", strings(&["json"]), 6),
                ("COPY", strings(&["quoted dir"]), 7),
            ]
        );
    }

    #[test]
    fn copies_from_stages_urls_and_heredocs_are_not_references() {
        let text = "FROM a AS build\nCOPY --from=build /out /app\nCOPY --link --from=node:20 /usr/bin/node /usr/bin/\nADD https://example.com/x.tgz /x\nADD git@github.com:o/r.git /r\nADD https://github.com/o/r.git#v1 /r\nADD ./repo.git /r\nADD http://x/y local.txt /dest/\nCOPY <<EOF /etc/conf\nCOPY not-a-copy /x\nEOF\nRUN <<-'END' bash\n\tCOPY inside /run\n\tEND\nCOPY only-one\nCOPY\nRUN echo $((1<<2)) && cp a b\nCOPY after /x\n";
        assert_eq!(
            copies(text),
            [
                ("ADD", strings(&["local.txt"]), 8),
                ("COPY", strings(&["after"]), 18),
            ]
        );
    }

    #[test]
    fn continued_instructions_report_their_first_line() {
        let text = "FROM x\r\nCOPY a \\\r\n  # a comment\r\n\r\n  b \\\r\n  /dest\r\nCOPY c /d\r\n";
        let found = dockerfile_copies(text);
        assert_eq!(found[0].sources, strings(&["a", "b"]));
        assert_eq!(found[0].line, 2);
        assert_eq!(found[0].text, "COPY a \\");
        assert_eq!(found[1].line, 7);
    }

    #[test]
    fn the_escape_directive_sets_the_continuation_character() {
        let text = "# escape=`\n\nFROM mcr.microsoft.com/windows\nCOPY app\\ `\n  C:\\app\\\nCOPY x\\ C:\\y\\\n";
        assert_eq!(
            copies(text),
            [
                ("COPY", strings(&["app\\"]), 4),
                ("COPY", strings(&["x\\"]), 6),
            ]
        );
    }

    #[test]
    fn an_escape_directive_after_an_instruction_is_a_comment() {
        let text = "FROM x\n# escape=`\nCOPY a `\nb /c\n";
        assert_eq!(copies(text), [("COPY", strings(&["a"]), 3)]);
    }

    #[test]
    fn dockerfile_readers_do_not_panic_on_odd_input() {
        for text in [
            "",
            "\\",
            "COPY \\",
            "COPY <<",
            "COPY <<EOF",
            "ADD [",
            "COPY [\"a\"",
            "COPY --from",
            "# escape=",
            "# escape=`\nCOPY a `",
            "copy é é",
            "COPY --chown=x",
        ] {
            let _ = dockerfile_copies(text);
        }
        assert_eq!(copies("COPY [\"a\""), []);
    }

    // ------------------------------------------------------------ Compose

    fn build(service: &str, line: usize) -> ComposeBuild {
        ComposeBuild {
            service: service.to_owned(),
            line,
            ..ComposeBuild::default()
        }
    }

    #[test]
    fn compose_builds_in_every_form() {
        let text = r#"name: app
x-common: &common
  build: ./ignored

services:
  web:
    build: ./web
    depends_on:
      - db
      - "cache"
  api:
    build:
      context: ../api # the API
      dockerfile: Dockerfile.dev
      args:
        context: not-this
    depends_on:
      db:
        condition: service_healthy
      'cache':
        condition: service_started
  worker:
    build:
      dockerfile: worker.Dockerfile
    depends_on: [db, "cache"]
  inline:
    build: { context: "./inline", dockerfile: D }
    depends_on: { db: { condition: service_started } }
  db:
    image: postgres
    <<: *common
  cache:
    image: redis
volumes:
  data:
"#;
        let mut web = build("web", 6);
        web.context = Some(("./web".to_owned(), 7));
        web.depends_on = vec![("db".to_owned(), 9), ("cache".to_owned(), 10)];
        let mut api = build("api", 11);
        api.context = Some(("../api".to_owned(), 13));
        api.dockerfile = Some("Dockerfile.dev".to_owned());
        api.depends_on = vec![("db".to_owned(), 18), ("cache".to_owned(), 20)];
        let mut worker = build("worker", 22);
        worker.context = Some((".".to_owned(), 23));
        worker.dockerfile = Some("worker.Dockerfile".to_owned());
        worker.depends_on = vec![("db".to_owned(), 25), ("cache".to_owned(), 25)];
        let mut inline = build("inline", 26);
        inline.context = Some(("./inline".to_owned(), 27));
        inline.dockerfile = Some("D".to_owned());
        inline.depends_on = vec![("db".to_owned(), 28)];
        assert_eq!(
            compose_builds(text),
            [
                web,
                api,
                worker,
                inline,
                build("db", 29),
                build("cache", 32)
            ]
        );
    }

    #[test]
    fn compose_lists_at_the_key_indent_and_other_indent_widths_are_read() {
        let text = "\u{feff}# c\r\nservices:\r\n    \"web\":\r\n        build: \"./web\"\r\n        depends_on:\r\n        - db\r\n        image: x\r\n    db:\r\n        image: postgres\r\n";
        let mut web = build("web", 3);
        web.context = Some(("./web".to_owned(), 4));
        web.depends_on = vec![("db".to_owned(), 6)];
        assert_eq!(compose_builds(text), [web, build("db", 8)]);
    }

    #[test]
    fn keys_nested_deeper_than_the_service_fields_are_not_builds() {
        let text = "services:\n  web:\n    image: x\n    labels:\n      build: ./no\n      depends_on: [no]\n    deploy:\n      depends_on:\n        - no\n";
        assert_eq!(compose_builds(text), [build("web", 2)]);
    }

    #[test]
    fn anchors_aliases_and_tags_do_not_confuse_the_reader() {
        let text = "services:\n  base: &base\n    build: &b ./base\n  web:\n    <<: *base\n    build: *b\n    depends_on: !reset []\n  api: *base\n";
        let mut base = build("base", 2);
        base.context = Some(("./base".to_owned(), 3));
        assert_eq!(
            compose_builds(text),
            [base, build("web", 4), build("api", 8)]
        );
    }

    #[test]
    fn compose_readers_do_not_panic_on_odd_input() {
        for text in [
            "",
            "services:",
            "services:\n  web:\n    build:",
            "services:\n  web:\n    depends_on:\n  - x",
            "services:\n    a:\n  b:\n      build: x",
            "services:\n  :\n  - x\n  web: {build: .}",
            "services:\n  web:\n    build: {",
            "services:\n  web:\n    depends_on: [",
            "  services:\n web:",
        ] {
            let _ = compose_builds(text);
        }
        assert_eq!(compose_builds("services:\n  - x\n"), []);
    }
}
