//! Lint and type-check rules turned off or made weaker in config files
//! (M25, ADR 0045): ESLint, Biome, Ruff, Flake8, Pylint, mypy, Pyright,
//! Cargo `[lints]` and `tsconfig.json`.
//!
//! Every setting becomes a [`Measure`] of kind [`Kind::RuleDisabled`],
//! keyed by tool, folder (a nested config is its own), scope (the globs or
//! modules a block applies to, `*` for all) and name:
//!
//! - **Rule levels** (ESLint, Biome, Pyright `report*`, Cargo lints) are
//!   `Measure::max`, error `2`, warn `1`, off `0`, and a rule no file sets
//!   counts as `1`. So `error → warn`, `warn → off`, a new `off` (also in a
//!   new block for some files) and a removed `error` are findings; a new
//!   `warn` is not (it may as well turn a rule on). Within one file the last
//!   setting of a rule and scope wins, as in ESLint's flat config.
//! - **Ignore lists** (Ruff and Flake8 `ignore` and per-file ignores,
//!   Pylint `disable`, mypy `disable_error_code`) count `-1` per code.
//! - **Switches** (`strict`, mypy's and tsconfig's checks, Biome's
//!   `linter.enabled`, Pyright's `typeCheckingMode`) are `Measure::max`
//!   with the tool's default as `absent`. A strict-family switch turned off
//!   explicitly while `strict` is on counts below its default, and so does
//!   any `false` in a tsconfig that `extends` another.
//! - **Selections** (Ruff `select`, mypy `enable_error_code`) count `1` per
//!   code; [`implied`] keeps a code that a broader selection, or `strict`,
//!   still covers from reading as gone.
//!
//! Files an ignore pattern leaves out of linting (`.eslintignore`,
//! `ignores`, Ruff `exclude`) are not read here.

use std::collections::BTreeMap;

use super::config::{Located, Measure};
use super::formats::{self, is_index, Entry, Format, Scalar};
use super::Kind;
use crate::imports::JsDialect;

const KIND: Kind = Kind::RuleDisabled;
/// The scope of a setting that applies to every file.
const ALL: &str = "*";
/// Between the parts of a key.
const SEP: &str = " | ";

/// The tools a config file may hold settings of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    Eslint,
    EslintPackageJson,
    Biome,
    Ruff,
    RuffPyproject,
    Flake8,
    Pylint,
    PylintPyproject,
    Mypy,
    MypyPyproject,
    Pyright,
    PyrightPyproject,
    Cargo,
    Tsconfig,
}

fn file_name(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, name)| name)
}

fn folder(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn tools(path: &str) -> &'static [Tool] {
    match file_name(path) {
        ".eslintrc" | ".eslintrc.json" | ".eslintrc.js" | ".eslintrc.cjs" | ".eslintrc.yml"
        | ".eslintrc.yaml" | "eslint.config.js" | "eslint.config.mjs" | "eslint.config.cjs"
        | "eslint.config.ts" | "eslint.config.mts" | "eslint.config.cts" => &[Tool::Eslint],
        "package.json" => &[Tool::EslintPackageJson],
        "biome.json" | "biome.jsonc" => &[Tool::Biome],
        "ruff.toml" | ".ruff.toml" => &[Tool::Ruff],
        "pyproject.toml" => &[
            Tool::RuffPyproject,
            Tool::PylintPyproject,
            Tool::MypyPyproject,
            Tool::PyrightPyproject,
        ],
        ".flake8" | "tox.ini" => &[Tool::Flake8],
        "setup.cfg" => &[Tool::Flake8, Tool::Pylint, Tool::Mypy],
        ".pylintrc" | "pylintrc" => &[Tool::Pylint],
        "mypy.ini" | ".mypy.ini" => &[Tool::Mypy],
        "pyrightconfig.json" => &[Tool::Pyright],
        "Cargo.toml" => &[Tool::Cargo],
        "tsconfig.json" | "jsconfig.json" => &[Tool::Tsconfig],
        name if name.starts_with("tsconfig.") && name.ends_with(".json") => &[Tool::Tsconfig],
        _ => &[],
    }
}

/// True for a config file that may turn lint or type-check rules off.
pub(crate) fn reads(path: &str) -> bool {
    !tools(path).is_empty()
}

/// The rule levels, ignore lists and strictness switches a config file
/// holds, or the line where it cannot be read.
pub(crate) fn measures(path: &str, text: &str) -> Result<Vec<Measure>, usize> {
    let name = file_name(path);
    let dir = folder(path);
    let mut out = Vec::new();
    for tool in tools(path) {
        match tool {
            Tool::Eslint => {
                let roots = if name == ".eslintrc" {
                    formats::read_roots(Format::Json, text)
                        .or_else(|line| formats::read_roots(Format::Yaml, text).map_err(|_| line))?
                } else if name.ends_with(".json") {
                    formats::read_roots(Format::Json, text)?
                } else if name.ends_with(".yml") || name.ends_with(".yaml") {
                    formats::read_roots(Format::Yaml, text)?
                } else {
                    formats::read_roots(Format::Js(JsDialect::TypeScript), text)?
                };
                eslint(dir, &roots, &mut out);
            }
            Tool::EslintPackageJson => {
                let entries = under(&formats::read(Format::Json, text)?, &["eslintConfig"]);
                eslint(dir, &[entries], &mut out);
            }
            Tool::Biome => biome(dir, &formats::read(Format::Json, text)?, &mut out),
            Tool::Ruff => ruff(dir, &formats::read(Format::Toml, text)?, true, &mut out),
            Tool::RuffPyproject => {
                let entries = under(&formats::read(Format::Toml, text)?, &["tool", "ruff"]);
                ruff(dir, &entries, !entries.is_empty(), &mut out);
            }
            Tool::Flake8 => flake8(dir, &formats::read(Format::Ini, text)?, &mut out),
            Tool::Pylint => {
                let entries: Vec<Entry> = formats::read(Format::Ini, text)?
                    .into_iter()
                    .filter(|entry| {
                        entry.path.first().is_some_and(|section| {
                            let section = section.to_ascii_lowercase().replace(['_', '-'], " ");
                            let section = section.strip_prefix("pylint.").unwrap_or(&section);
                            section == "messages control"
                        })
                    })
                    .collect();
                pylint(dir, &entries, Shown::Ini, &mut out);
            }
            Tool::PylintPyproject => {
                let entries = formats::read(Format::Toml, text)?;
                let mut picked = under(&entries, &["tool", "pylint", "messages control"]);
                picked.extend(under(&entries, &["tool", "pylint", "messages_control"]));
                pylint(dir, &picked, Shown::Toml, &mut out);
            }
            Tool::Mypy => mypy(dir, &mypy_ini(&formats::read(Format::Ini, text)?), &mut out),
            Tool::MypyPyproject => {
                let entries = under(&formats::read(Format::Toml, text)?, &["tool", "mypy"]);
                mypy(dir, &mypy_toml(&entries), &mut out);
            }
            Tool::Pyright => pyright(
                dir,
                "pyright",
                &formats::read(Format::Json, text)?,
                &mut out,
            ),
            Tool::PyrightPyproject => {
                let entries = formats::read(Format::Toml, text)?;
                for tool in ["pyright", "basedpyright"] {
                    pyright(dir, tool, &under(&entries, &["tool", tool]), &mut out);
                }
            }
            Tool::Cargo => cargo(dir, &formats::read(Format::Toml, text)?, &mut out),
            Tool::Tsconfig => tsconfig(path, &formats::read(Format::Json, text)?, &mut out),
        }
    }
    Ok(out)
}

/// The entries under `prefix`, with it taken off their paths.
fn under(entries: &[Entry], prefix: &[&str]) -> Vec<Entry> {
    entries
        .iter()
        .filter(|entry| {
            entry.path.len() > prefix.len() && entry.path.iter().zip(prefix).all(|(a, b)| a == b)
        })
        .map(|entry| Entry {
            path: entry.path[prefix.len()..].to_vec(),
            value: entry.value.clone(),
            line: entry.line,
        })
        .collect()
}

fn key(tool: &str, place: &str, scope: &str, name: &str) -> String {
    [tool, place, scope, name].join(SEP)
}

/// How a value is shown: quoted as JSON, TOML and JS write strings, or as
/// an INI file writes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shown {
    Toml,
    Ini,
}

fn quoted(value: &Scalar) -> String {
    match value {
        Scalar::Str(text) => format!("\"{text}\""),
        other => other.as_text(),
    }
}

fn shown(value: &Scalar, how: Shown) -> String {
    match how {
        Shown::Toml => quoted(value),
        Shown::Ini => value.as_text(),
    }
}

/// A glob as a scope: `./`, `**/` and `\` dropped, so ESLint's legacy
/// `*.test.js` and the flat config's `**/*.test.js` are the same scope.
fn scope_of(globs: impl IntoIterator<Item = String>) -> String {
    let mut globs: Vec<String> = globs
        .into_iter()
        .map(|glob| {
            let mut glob = glob.trim().replace('\\', "/");
            loop {
                if let Some(rest) = glob.strip_prefix("./") {
                    glob = rest.to_owned();
                } else if let Some(rest) = glob.strip_prefix("**/") {
                    glob = rest.to_owned();
                } else {
                    break;
                }
            }
            glob
        })
        .filter(|glob| !glob.is_empty())
        .collect();
    globs.sort();
    globs.dedup();
    if globs.is_empty() {
        ALL.to_owned()
    } else {
        globs.join(", ")
    }
}

/// The text values at `prefix` + `name` (+ list indexes).
fn values_at(entries: &[Entry], prefix: &[String], name: &str) -> Vec<String> {
    entries
        .iter()
        .filter(|entry| {
            entry.path.len() > prefix.len()
                && entry.path[..prefix.len()] == *prefix
                && entry.path[prefix.len()] == name
                && entry.path[prefix.len() + 1..].iter().all(|s| is_index(s))
        })
        .filter_map(|entry| match &entry.value {
            Scalar::Str(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// A rule level, where the last setting of a key in a file wins.
struct Levels {
    tool: &'static str,
    place: String,
    absent: i64,
    last: BTreeMap<(String, String), (i64, usize, String)>,
}

impl Levels {
    fn new(tool: &'static str, place: &str, absent: i64) -> Self {
        Self {
            tool,
            place: place.to_owned(),
            absent,
            last: BTreeMap::new(),
        }
    }

    fn set(&mut self, scope: &str, name: &str, value: i64, line: usize, what: String) {
        self.last
            .insert((scope.to_owned(), name.to_owned()), (value, line, what));
    }

    fn into_measures(self, out: &mut Vec<Measure>) {
        for ((scope, name), (value, line, what)) in self.last {
            out.push(Measure::max(
                KIND,
                key(self.tool, &self.place, &scope, &name),
                value,
                self.absent,
                line,
                what,
            ));
        }
    }
}

// ----------------------------------------------------------------- ESLint

/// ESLint's level: `off`/`0`, `warn`/`1`, `error`/`2`.
fn eslint_level(value: &Scalar) -> Option<i64> {
    match value {
        Scalar::Num(number) if [0.0, 1.0, 2.0].contains(number) => Some(*number as i64),
        Scalar::Str(text) => match text.trim().to_ascii_lowercase().as_str() {
            "off" | "0" => Some(0),
            "warn" | "1" => Some(1),
            "error" | "2" => Some(2),
            _ => None,
        },
        _ => None,
    }
}

/// A config block's path: the root, a flat-config array item, or a legacy
/// `overrides` entry.
fn is_block(prefix: &[String]) -> bool {
    prefix.iter().all(|s| is_index(s) || s == "overrides")
}

fn eslint(dir: &str, roots: &[Vec<Entry>], out: &mut Vec<Measure>) {
    let mut levels = Levels::new("eslint", dir, 1);
    let mut options = Levels::new("eslint", dir, 0);
    let mut directives = Levels::new("eslint", dir, 1);
    for entries in roots {
        let mut scopes: BTreeMap<Vec<String>, String> = BTreeMap::new();
        let mut scope = |prefix: &[String]| -> String {
            scopes
                .entry(prefix.to_vec())
                .or_insert_with(|| scope_of(values_at(entries, prefix, "files")))
                .clone()
        };
        for entry in entries {
            let path = &entry.path;
            // `rules.<rule>` or `rules.<rule>.[0]` under a block.
            let at = (0..path.len()).find(|&k| path[k] == "rules" && is_block(&path[..k]));
            if let Some(k) = at {
                let rest = &path[k + 1..];
                let (rule, first) = match rest {
                    [rule] => (rule, false),
                    [rule, index] if index == "[0]" => (rule, true),
                    _ => continue,
                };
                let Some(level) = eslint_level(&entry.value) else {
                    continue;
                };
                let written = quoted(&entry.value);
                let what = if first {
                    format!("\"{rule}\": [{written}, …]")
                } else {
                    format!("\"{rule}\": {written}")
                };
                let scope = scope(&path[..k]);
                levels.set(&scope, rule, level, entry.line, what);
                continue;
            }
            let Some((option, before)) = path.split_last() else {
                continue;
            };
            let prefix = before
                .strip_suffix(&["linterOptions".to_owned()])
                .unwrap_or(before);
            if !is_block(prefix) {
                continue;
            }
            let what = format!("{option}: {}", quoted(&entry.value));
            match option.as_str() {
                "noInlineConfig" => {
                    if let Some(on) = entry.value.as_bool() {
                        let scope = scope(prefix);
                        options.set(&scope, option, i64::from(on), entry.line, what);
                    }
                }
                "reportUnusedDisableDirectives" => {
                    let level = match &entry.value {
                        Scalar::Bool(on) => Some(i64::from(*on)),
                        other => eslint_level(other),
                    };
                    if let Some(level) = level {
                        let scope = scope(prefix);
                        directives.set(&scope, option, level, entry.line, what);
                    }
                }
                _ => {}
            }
        }
    }
    levels.into_measures(out);
    options.into_measures(out);
    directives.into_measures(out);
}

// ------------------------------------------------------------------ Biome

/// Biome's level: `error`, then `warn`, `info` and `on`, then `off`.
fn biome_level(value: &Scalar) -> Option<i64> {
    match value {
        Scalar::Str(text) => match text.trim() {
            "off" => Some(0),
            "warn" | "info" | "on" => Some(1),
            "error" => Some(2),
            _ => None,
        },
        _ => None,
    }
}

fn biome(dir: &str, entries: &[Entry], out: &mut Vec<Measure>) {
    let mut levels = Levels::new("biome", dir, 1);
    let mut switches = Levels::new("biome", dir, 1);
    for entry in entries {
        let path = &entry.path;
        let prefix_len = match path.as_slice() {
            [overrides, index, ..] if overrides == "overrides" && is_index(index) => 2,
            _ => 0,
        };
        let prefix = &path[..prefix_len];
        let rest: Vec<&str> = path[prefix_len..].iter().map(String::as_str).collect();
        let scope = || {
            let mut globs = values_at(entries, prefix, "include");
            globs.extend(values_at(entries, prefix, "includes"));
            scope_of(globs)
        };
        let switch = |name: &str| -> Option<(String, i64)> {
            entry
                .value
                .as_bool()
                .map(|on| (name.to_owned(), i64::from(on)))
        };
        let what = |name: &str| format!("\"{name}\": {}", quoted(&entry.value));
        match rest.as_slice() {
            ["linter", "enabled"] => {
                if let Some((name, value)) = switch("linter.enabled") {
                    switches.set(&scope(), &name, value, entry.line, what("enabled"));
                }
            }
            ["linter", "rules", "recommended"] => {
                if let Some((name, value)) = switch("recommended") {
                    switches.set(&scope(), &name, value, entry.line, what("recommended"));
                }
            }
            ["linter", "rules", group, "recommended"] => {
                if let Some((name, value)) = switch(&format!("{group}.recommended")) {
                    switches.set(&scope(), &name, value, entry.line, what("recommended"));
                }
            }
            ["linter", "rules", group] | ["linter", "rules", group, "level"] => {
                if let Some(level) = biome_level(&entry.value) {
                    let name = format!("group {group}");
                    levels.set(&scope(), &name, level, entry.line, what(group));
                }
            }
            ["linter", "rules", _, rule] | ["linter", "rules", _, rule, "level"]
                if *rule != "all" =>
            {
                // Keyed by the rule alone: Biome moves rules between groups.
                if let Some(level) = biome_level(&entry.value) {
                    levels.set(&scope(), rule, level, entry.line, what(rule));
                }
            }
            _ => {}
        }
    }
    levels.into_measures(out);
    switches.into_measures(out);
}

// ------------------------------------------------------------------- Ruff

/// What Ruff selects when a config selects nothing.
const RUFF_DEFAULT_SELECT: [&str; 4] = ["E4", "E7", "E9", "F"];

fn ruff(dir: &str, entries: &[Entry], configured: bool, out: &mut Vec<Measure>) {
    let mut selected: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut selects = false;
    for entry in entries {
        let path: Vec<&str> = entry.path.iter().map(String::as_str).collect();
        let path = match path.split_first() {
            Some((&"lint", rest)) => rest,
            _ => path.as_slice(),
        };
        let Scalar::Str(code) = &entry.value else {
            continue;
        };
        let code = code.trim().to_ascii_uppercase();
        match path {
            [option @ ("ignore" | "extend-ignore"), index] if is_index(index) => {
                out.push(Measure::count(
                    KIND,
                    key("ruff", dir, ALL, &format!("ignore {code}")),
                    -1,
                    entry.line,
                    format!("{option} = [\"{code}\"]"),
                ));
            }
            [option @ ("per-file-ignores" | "extend-per-file-ignores"), glob, index]
                if is_index(index) =>
            {
                out.push(Measure::count(
                    KIND,
                    key(
                        "ruff",
                        dir,
                        &scope_of([(*glob).to_owned()]),
                        &format!("ignore {code}"),
                    ),
                    -1,
                    entry.line,
                    format!("{option} \"{glob}\" = [\"{code}\"]"),
                ));
            }
            [option @ ("select" | "extend-select"), index] if is_index(index) => {
                selects |= *option == "select";
                selected
                    .entry(code.clone())
                    .or_insert((entry.line, format!("{option} = [\"{code}\"]")));
            }
            _ => {}
        }
    }
    if configured && !selects {
        let line = entries.first().map_or(1, |entry| entry.line);
        for code in RUFF_DEFAULT_SELECT {
            selected
                .entry(code.to_owned())
                .or_insert((line, format!("select (Ruff's default) \"{code}\"")));
        }
    }
    for (code, (line, what)) in selected {
        out.push(Measure::max(
            KIND,
            key("ruff", dir, ALL, &format!("select {code}")),
            1,
            0,
            line,
            what,
        ));
    }
}

/// Codes in a comma or white-space separated INI value.
fn codes(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|code| !code.is_empty())
}

// ----------------------------------------------------------------- Flake8

fn flake8(dir: &str, entries: &[Entry], out: &mut Vec<Measure>) {
    for entry in entries {
        let [section, option] = entry.path.as_slice() else {
            continue;
        };
        if !section.eq_ignore_ascii_case("flake8") {
            continue;
        }
        let option = option.trim().to_ascii_lowercase().replace('_', "-");
        let text = entry.value.as_text();
        match option.as_str() {
            "ignore" | "extend-ignore" => {
                for code in codes(&text) {
                    out.push(Measure::count(
                        KIND,
                        key("flake8", dir, ALL, &format!("ignore {code}")),
                        -1,
                        entry.line,
                        format!("{option} = {code}"),
                    ));
                }
            }
            "per-file-ignores" | "extend-per-file-ignores" => {
                // `glob: codes` pairs, over one line or several.
                let spaced = text.replace(':', " : ");
                let tokens: Vec<&str> = codes(&spaced).collect();
                let mut file: Option<&str> = None;
                for (at, token) in tokens.iter().enumerate() {
                    if *token == ":" {
                        continue;
                    }
                    if tokens.get(at + 1) == Some(&":") {
                        file = Some(token);
                        continue;
                    }
                    let Some(file) = file else { continue };
                    out.push(Measure::count(
                        KIND,
                        key(
                            "flake8",
                            dir,
                            &scope_of([file.to_owned()]),
                            &format!("ignore {token}"),
                        ),
                        -1,
                        entry.line,
                        format!("{option} = {file}: {token}"),
                    ));
                }
            }
            _ => {}
        }
    }
}

// ----------------------------------------------------------------- Pylint

fn pylint(dir: &str, entries: &[Entry], how: Shown, out: &mut Vec<Measure>) {
    for entry in entries {
        let Some(option) = entry.path.iter().rev().find(|s| !is_index(s)) else {
            continue;
        };
        if option != "disable" {
            continue;
        }
        let text = entry.value.as_text();
        for code in codes(&text) {
            let what = match how {
                Shown::Ini => format!("disable = {code}"),
                Shown::Toml => format!("disable = [\"{code}\"]"),
            };
            out.push(Measure::count(
                KIND,
                key("pylint", dir, ALL, &format!("disable {code}")),
                -1,
                entry.line,
                what,
            ));
        }
    }
}

// ------------------------------------------------------------------- mypy

/// mypy's boolean options: name, whether `true` is the weaker value, the
/// default as strictness (`1` strict), and whether `strict` turns it on.
const MYPY_FLAGS: [(&str, bool, i64, bool); 27] = [
    ("disallow_untyped_defs", false, 0, true),
    ("disallow_incomplete_defs", false, 0, true),
    ("check_untyped_defs", false, 0, true),
    ("disallow_untyped_calls", false, 0, true),
    ("disallow_untyped_decorators", false, 0, true),
    ("disallow_any_generics", false, 0, true),
    ("disallow_subclassing_any", false, 0, true),
    ("warn_return_any", false, 0, true),
    ("warn_unused_ignores", false, 0, true),
    ("warn_redundant_casts", false, 0, true),
    ("warn_unused_configs", false, 0, true),
    ("strict_equality", false, 0, true),
    ("extra_checks", false, 0, true),
    ("strict_bytes", false, 0, true),
    ("no_implicit_reexport", false, 0, true),
    ("implicit_reexport", true, 0, true),
    ("disallow_any_unimported", false, 0, false),
    ("disallow_any_expr", false, 0, false),
    ("disallow_any_decorated", false, 0, false),
    ("disallow_any_explicit", false, 0, false),
    ("warn_unreachable", false, 0, false),
    ("strict_optional", false, 1, false),
    ("warn_no_return", false, 1, false),
    ("no_implicit_optional", false, 1, false),
    ("implicit_optional", true, 1, false),
    ("ignore_errors", true, 1, false),
    ("allow_untyped_globals", true, 1, false),
];

fn implied_by_strict(tool: &str, name: &str) -> bool {
    match tool {
        "mypy" => MYPY_FLAGS
            .iter()
            .any(|(flag, _, _, family)| *family && *flag == name),
        "tsc" => TS_STRICT_FAMILY.contains(&name),
        _ => false,
    }
}

/// One mypy setting: its scope (`*` or `module <patterns>`), name, value,
/// line, and how it is shown.
struct MypySetting {
    scope: String,
    name: String,
    value: Scalar,
    line: usize,
    shown: Shown,
}

fn mypy_ini(entries: &[Entry]) -> Vec<MypySetting> {
    entries
        .iter()
        .filter_map(|entry| {
            let [section, name] = entry.path.as_slice() else {
                return None;
            };
            let section = section.trim();
            let scope = if section == "mypy" {
                ALL.to_owned()
            } else {
                let modules = section.strip_prefix("mypy-")?;
                module_scope(modules.split(',').map(str::to_owned))
            };
            Some(MypySetting {
                scope,
                name: name.trim().replace('-', "_"),
                value: entry.value.clone(),
                line: entry.line,
                shown: Shown::Ini,
            })
        })
        .collect()
}

fn module_scope(modules: impl IntoIterator<Item = String>) -> String {
    let mut modules: Vec<String> = modules
        .into_iter()
        .map(|module| module.trim().to_owned())
        .filter(|module| !module.is_empty())
        .collect();
    modules.sort();
    modules.dedup();
    format!("module {}", modules.join(", "))
}

fn mypy_toml(entries: &[Entry]) -> Vec<MypySetting> {
    let mut settings = Vec::new();
    for entry in entries {
        let (scope, rest) = match entry.path.as_slice() {
            [overrides, index, rest @ ..] if overrides == "overrides" && is_index(index) => {
                let prefix = [overrides.clone(), index.clone()];
                (module_scope(values_at(entries, &prefix, "module")), rest)
            }
            rest => (ALL.to_owned(), rest),
        };
        let name = match rest {
            [name] => name,
            [name, index] if is_index(index) => name,
            _ => continue,
        };
        settings.push(MypySetting {
            scope,
            name: name.replace('-', "_"),
            value: entry.value.clone(),
            line: entry.line,
            shown: Shown::Toml,
        });
    }
    settings
}

fn mypy(dir: &str, settings: &[MypySetting], out: &mut Vec<Measure>) {
    let on = |scope: &str, name: &str| {
        settings
            .iter()
            .filter(|s| s.scope == scope && s.name == name)
            .rev()
            .find_map(|s| s.value.as_bool())
    };
    let strict = on(ALL, "strict") == Some(true);
    for setting in settings {
        let name = setting.name.as_str();
        let what = format!("{name} = {}", shown(&setting.value, setting.shown));
        if name == "strict" && setting.scope == ALL {
            if let Some(value) = setting.value.as_bool() {
                out.push(Measure::max(
                    KIND,
                    key("mypy", dir, ALL, name),
                    i64::from(value),
                    0,
                    setting.line,
                    what,
                ));
            }
            continue;
        }
        if name == "disable_error_code" || name == "enable_error_code" {
            for code in codes(&setting.value.as_text()) {
                let what = format!("{name} = {code}");
                let measure = if name == "disable_error_code" {
                    Measure::count(
                        KIND,
                        key("mypy", dir, &setting.scope, &format!("disable {code}")),
                        -1,
                        setting.line,
                        what,
                    )
                } else {
                    Measure::max(
                        KIND,
                        key("mypy", dir, &setting.scope, &format!("enable {code}")),
                        1,
                        0,
                        setting.line,
                        what,
                    )
                };
                out.push(measure);
            }
            continue;
        }
        let Some(&(_, weaker_when_on, default, family)) =
            MYPY_FLAGS.iter().find(|(flag, ..)| *flag == name)
        else {
            continue;
        };
        let Some(value) = setting.value.as_bool() else {
            continue;
        };
        let strictness = |value: bool| i64::from(value != weaker_when_on);
        let mut level = strictness(value);
        // Turned off where it is on for the whole program: by `strict`, or
        // by the global section for a module.
        let global_on = on(ALL, name).map(strictness) == Some(1);
        if level == 0 && ((family && strict) || (setting.scope != ALL && global_on)) {
            level = -1;
        }
        out.push(Measure::max(
            KIND,
            key("mypy", dir, &setting.scope, name),
            level,
            default,
            setting.line,
            what,
        ));
    }
}

// ---------------------------------------------------------------- Pyright

/// Pyright's level of a `report*` rule.
fn pyright_level(value: &Scalar) -> Option<i64> {
    match value {
        Scalar::Bool(on) => Some(if *on { 2 } else { 0 }),
        Scalar::Str(text) => match text.trim() {
            "none" | "false" => Some(0),
            "information" | "warning" | "hint" => Some(1),
            "error" | "true" => Some(2),
            _ => None,
        },
        _ => None,
    }
}

fn pyright(dir: &str, tool: &'static str, entries: &[Entry], out: &mut Vec<Measure>) {
    // basedpyright adds stricter modes and defaults to `recommended`.
    let (modes, default): (&[&str], i64) = if tool == "basedpyright" {
        (
            &["off", "basic", "standard", "strict", "recommended", "all"],
            4,
        )
    } else {
        (&["off", "basic", "standard", "strict"], 2)
    };
    let mut levels = Levels::new(tool, dir, 1);
    let mut mode = Levels::new(tool, dir, default);
    for entry in entries {
        let (scope, rest) = match entry.path.as_slice() {
            [envs, index, rest @ ..] if envs == "executionEnvironments" && is_index(index) => {
                let prefix = [envs.clone(), index.clone()];
                let root = values_at(entries, &prefix, "root");
                (format!("root {}", scope_of(root)), rest)
            }
            rest => (ALL.to_owned(), rest),
        };
        let what = |name: &str| format!("\"{name}\": {}", quoted(&entry.value));
        match rest {
            [name] if name == "typeCheckingMode" => {
                let text = entry.value.as_text();
                if let Some(at) = modes.iter().position(|m| *m == text.trim()) {
                    mode.set(&scope, name, at as i64, entry.line, what(name));
                }
            }
            [name] if name.starts_with("report") => {
                if let Some(level) = pyright_level(&entry.value) {
                    levels.set(&scope, name, level, entry.line, what(name));
                }
            }
            [name, index] if name == "strict" && is_index(index) => {
                // Paths checked in strict mode.
                let path = scope_of([entry.value.as_text()]);
                out.push(Measure::max(
                    KIND,
                    key(tool, dir, ALL, &format!("strict {path}")),
                    1,
                    0,
                    entry.line,
                    format!("\"strict\": [\"{}\"]", entry.value.as_text()),
                ));
            }
            _ => {}
        }
    }
    levels.into_measures(out);
    mode.into_measures(out);
}

// ------------------------------------------------------------------ Cargo

/// A Rust lint level: `forbid`, `deny`, `warn`, `allow` (and `expect`).
fn cargo_level(text: &str) -> Option<i64> {
    match text.trim() {
        "allow" | "expect" => Some(0),
        "warn" => Some(1),
        "deny" => Some(2),
        "forbid" => Some(3),
        _ => None,
    }
}

fn cargo(dir: &str, entries: &[Entry], out: &mut Vec<Measure>) {
    let mut levels = Levels::new("cargo", dir, 1);
    for entry in entries {
        let path: Vec<&str> = entry.path.iter().map(String::as_str).collect();
        let (scope, rest) = match path.as_slice() {
            ["workspace", "lints", rest @ ..] => ("workspace", rest),
            ["lints", rest @ ..] => (ALL, rest),
            _ => continue,
        };
        let (tool, lint) = match rest {
            [tool, lint] | [tool, lint, "level"] if *tool != "workspace" => (*tool, *lint),
            _ => continue,
        };
        let Scalar::Str(text) = &entry.value else {
            continue;
        };
        let Some(level) = cargo_level(text) else {
            continue;
        };
        let name = if tool == "rust" {
            lint.to_owned()
        } else {
            format!("{tool}::{lint}")
        };
        levels.set(
            scope,
            &name,
            level,
            entry.line,
            format!("{lint} = \"{text}\""),
        );
    }
    levels.into_measures(out);
}

// --------------------------------------------------------------- tsconfig

/// The checks `strict` turns on.
const TS_STRICT_FAMILY: [&str; 9] = [
    "noImplicitAny",
    "noImplicitThis",
    "alwaysStrict",
    "strictBindCallApply",
    "strictNullChecks",
    "strictFunctionTypes",
    "strictPropertyInitialization",
    "useUnknownInCatchVariables",
    "strictBuiltinIteratorReturn",
];

/// The other checks, off by default.
const TS_CHECKS: [&str; 10] = [
    "strict",
    "noUnusedLocals",
    "noUnusedParameters",
    "noImplicitReturns",
    "noFallthroughCasesInSwitch",
    "noUncheckedIndexedAccess",
    "exactOptionalPropertyTypes",
    "noImplicitOverride",
    "noPropertyAccessFromIndexSignature",
    "checkJs",
];

fn tsconfig(path: &str, entries: &[Entry], out: &mut Vec<Measure>) {
    let extends = entries
        .iter()
        .any(|entry| entry.path.first().is_some_and(|key| key == "extends"));
    let option = |entry: &Entry| match entry.path.as_slice() {
        [options, name] if options == "compilerOptions" => Some(name.clone()),
        _ => None,
    };
    let strict = entries
        .iter()
        .filter(|entry| option(entry).as_deref() == Some("strict"))
        .rev()
        .find_map(|entry| entry.value.as_bool())
        == Some(true);
    let mut levels = Levels::new("tsc", path, 0);
    for entry in entries {
        let Some(name) = option(entry) else { continue };
        let family = TS_STRICT_FAMILY.contains(&name.as_str());
        if !family && !TS_CHECKS.contains(&name.as_str()) {
            continue;
        }
        let Scalar::Bool(on) = entry.value else {
            continue;
        };
        // `false` may turn off what `strict` or an extended config turns on.
        let value = match on {
            true => 1,
            false if extends || (family && strict) => -1,
            false => 0,
        };
        levels.set(ALL, &name, value, entry.line, format!("\"{name}\": {on}"));
    }
    levels.into_measures(out);
}

// ---------------------------------------------------------------- implied

/// Measures the working tree holds by implication, for keys on the base
/// that it no longer sets itself: a Ruff code still selected by a broader
/// prefix or `ALL`, a mypy or tsconfig check `strict` now turns on. Without
/// them, `select = ["E501"]` → `["E"]` or a redundant flag dropped next to
/// `strict` would read as a weakening.
pub(crate) fn implied(base: &[Located], head: &[Located]) -> Vec<Located> {
    let parts = |located: &Located| -> Option<[String; 4]> {
        if located.measure.kind != KIND {
            return None;
        }
        let mut parts = located.measure.key.splitn(4, SEP).map(str::to_owned);
        Some([parts.next()?, parts.next()?, parts.next()?, parts.next()?])
    };
    let head_parts: Vec<(&Located, [String; 4])> = head
        .iter()
        .filter_map(|located| parts(located).map(|p| (located, p)))
        .collect();
    let mut out: Vec<Located> = Vec::new();
    for located in base {
        let Some([tool, place, _scope, name]) = parts(located) else {
            continue;
        };
        if located.measure.value <= 0
            || head
                .iter()
                .chain(out.iter())
                .any(|h| h.measure.kind == KIND && h.measure.key == located.measure.key)
        {
            continue;
        }
        let cover = head_parts.iter().find(|(h, [t, p, s, n])| {
            if *t != tool || *p != place || h.measure.value <= 0 {
                return false;
            }
            if tool == "ruff" {
                let (Some(code), Some(by)) =
                    (name.strip_prefix("select "), n.strip_prefix("select "))
                else {
                    return false;
                };
                by == "ALL" || code.starts_with(by)
            } else {
                s == ALL && n == "strict" && implied_by_strict(&tool, &name)
            }
        });
        if let Some((by, _)) = cover {
            out.push(Located {
                file: by.file.clone(),
                measure: Measure {
                    key: located.measure.key.clone(),
                    value: located.measure.value,
                    ..by.measure.clone()
                },
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::config::compare;
    use super::*;

    fn side(files: &[(&str, &str)]) -> Vec<Located> {
        files
            .iter()
            .flat_map(|(path, text)| {
                measures(path, text)
                    .expect("readable")
                    .into_iter()
                    .map(|measure| Located {
                        file: (*path).to_owned(),
                        measure,
                    })
            })
            .collect()
    }

    /// The findings, as `file:line what`.
    fn findings(base: &[(&str, &str)], head: &[(&str, &str)]) -> Vec<String> {
        let base = side(base);
        let mut head = side(head);
        head.extend(implied(&base, &head));
        compare(&base, &head)
            .into_iter()
            .map(|finding| {
                assert_eq!(finding.kind, Kind::RuleDisabled);
                format!("{}:{} {}", finding.file, finding.line, finding.what)
            })
            .collect()
    }

    const ESLINTRC: &str = "{\n  \"extends\": [\"eslint:recommended\"],\n  \"rules\": {\n    \"no-console\": \"error\",\n    \"eqeqeq\": [\"error\", \"always\"]\n  },\n  \"overrides\": [\n    { \"files\": [\"*.test.js\"], \"rules\": { \"no-unused-expressions\": \"off\" } }\n  ]\n}\n";

    #[test]
    fn reads_the_files_it_knows() {
        for path in [
            ".eslintrc",
            "web/.eslintrc.cjs",
            "eslint.config.mjs",
            "package.json",
            "biome.jsonc",
            "ruff.toml",
            "pyproject.toml",
            "setup.cfg",
            "tox.ini",
            ".flake8",
            ".pylintrc",
            "mypy.ini",
            "pyrightconfig.json",
            "crates/a/Cargo.toml",
            "tsconfig.json",
            "packages/a/tsconfig.build.json",
            "jsconfig.json",
        ] {
            assert!(reads(path), "{path}");
        }
        for path in [
            ".eslintignore",
            "clippy.toml",
            "tsconfig.json.bak",
            "eslint.js",
        ] {
            assert!(!reads(path), "{path}");
        }
    }

    #[test]
    fn eslint_rule_turned_off_or_to_warn_is_a_finding() {
        let head = ESLINTRC.replace("\"no-console\": \"error\"", "\"no-console\": \"off\"");
        assert_eq!(
            findings(&[(".eslintrc.json", ESLINTRC)], &[(".eslintrc.json", &head)]),
            [".eslintrc.json:4 \"no-console\": \"off\" (on the base: \"no-console\": \"error\", .eslintrc.json:4)"]
        );
        let warned = ESLINTRC.replace("[\"error\", \"always\"]", "[\"warn\", \"always\"]");
        assert_eq!(
            findings(
                &[(".eslintrc.json", ESLINTRC)],
                &[(".eslintrc.json", &warned)]
            )
            .len(),
            1
        );
        // A new override that turns a rule off for some files.
        let scoped = ESLINTRC.replace(
            "\"no-unused-expressions\": \"off\" }",
            "\"no-unused-expressions\": \"off\", \"no-console\": 0 }",
        );
        assert_eq!(
            findings(
                &[(".eslintrc.json", ESLINTRC)],
                &[(".eslintrc.json", &scoped)]
            ),
            [".eslintrc.json:8 \"no-console\": 0 (not set on the base)"]
        );
    }

    #[test]
    fn eslint_flat_config_block_appended_to_turn_a_rule_off_is_a_finding() {
        let base = "import js from \"@eslint/js\";\n\nexport default [\n  js.configs.recommended,\n  { rules: { \"no-console\": \"error\" } },\n];\n";
        let head = "import js from \"@eslint/js\";\n\nexport default [\n  js.configs.recommended,\n  { rules: { \"no-console\": \"error\" } },\n  { rules: { \"no-console\": \"off\" } },\n];\n";
        assert_eq!(
            findings(&[("eslint.config.js", base)], &[("eslint.config.js", head)]).len(),
            1
        );
        let tests_off = "export default tseslint.config(\n  { rules: { \"no-console\": \"error\" } },\n  { files: [\"**/*.test.ts\"], rules: { \"no-console\": [\"off\"] } },\n);\n";
        assert_eq!(
            findings(
                &[("eslint.config.js", base)],
                &[("eslint.config.ts", tests_off)]
            ),
            ["eslint.config.ts:3 \"no-console\": [\"off\", …] (not set on the base)"]
        );
    }

    #[test]
    fn eslint_migrated_to_flat_config_stricter_or_reformatted_is_not_a_finding() {
        let flat = "import js from \"@eslint/js\";\n\nexport default [\n  js.configs.recommended,\n  {\n    rules: {\n      \"no-console\": \"error\",\n      eqeqeq: [\"error\", \"always\"],\n    },\n  },\n  {\n    files: [\"**/*.test.js\"],\n    rules: { \"no-unused-expressions\": \"off\" },\n  },\n];\n";
        assert!(findings(
            &[(".eslintrc.json", ESLINTRC)],
            &[("eslint.config.js", flat)]
        )
        .is_empty());
        let yaml = "extends: eslint:recommended\nrules:\n  no-console: 2\n  eqeqeq: [error, always]\noverrides:\n  - files: \"*.test.js\"\n    rules:\n      no-unused-expressions: 0\n";
        assert!(findings(&[(".eslintrc.json", ESLINTRC)], &[(".eslintrc.yml", yaml)]).is_empty());
        let stricter = ESLINTRC.replace(
            "\"no-unused-expressions\": \"off\"",
            "\"no-unused-expressions\": \"warn\", \"no-var\": \"warn\"",
        );
        assert!(findings(
            &[(".eslintrc.json", ESLINTRC)],
            &[(".eslintrc.json", &stricter)]
        )
        .is_empty());
        let package = format!("{{\n  \"name\": \"a\",\n  \"eslintConfig\": {ESLINTRC}}}\n");
        assert!(findings(
            &[(".eslintrc.json", ESLINTRC)],
            &[("package.json", &package)]
        )
        .is_empty());
        // A rule level in another folder's config is its own.
        let off = "{ \"rules\": { \"no-console\": \"off\" } }";
        assert_eq!(
            findings(
                &[(".eslintrc.json", ESLINTRC)],
                &[(".eslintrc.json", ESLINTRC), ("web/.eslintrc.json", off)]
            )
            .len(),
            1
        );
    }

    #[test]
    fn eslint_inline_config_turned_back_on_is_a_finding() {
        let base = "export default [{ linterOptions: { noInlineConfig: true, reportUnusedDisableDirectives: \"error\" } }];\n";
        let head =
            "export default [{ linterOptions: { reportUnusedDisableDirectives: \"off\" } }];\n";
        assert_eq!(
            findings(&[("eslint.config.js", base)], &[("eslint.config.js", head)]).len(),
            2
        );
    }

    #[test]
    fn biome_rule_off_linter_off_and_recommended_off_are_findings() {
        let base = "{\n  \"linter\": {\n    \"enabled\": true,\n    \"rules\": {\n      \"recommended\": true,\n      \"suspicious\": { \"noExplicitAny\": \"error\" }\n    }\n  }\n}\n";
        let off = base.replace(
            "\"noExplicitAny\": \"error\"",
            "\"noExplicitAny\": { \"level\": \"off\" }",
        );
        assert_eq!(
            findings(&[("biome.json", base)], &[("biome.json", &off)]).len(),
            1
        );
        let disabled = base.replace("\"enabled\": true", "\"enabled\": false");
        assert_eq!(
            findings(&[("biome.json", base)], &[("biome.json", &disabled)]).len(),
            1
        );
        let unrecommended = base.replace("\"recommended\": true", "\"recommended\": false");
        assert_eq!(
            findings(&[("biome.json", base)], &[("biome.json", &unrecommended)]).len(),
            1
        );
        let overridden = base.replace(
            "\n}\n",
            ",\n  \"overrides\": [{ \"includes\": [\"src/legacy/**\"], \"linter\": { \"rules\": { \"suspicious\": { \"noExplicitAny\": \"off\" } } } }]\n}\n",
        );
        assert_eq!(
            findings(&[("biome.json", base)], &[("biome.json", &overridden)]).len(),
            1
        );
        // Moved to biome.jsonc, and a rule made stricter: not findings.
        let moved = base.replace("\"error\"", "\"error\" /* keep */");
        assert!(findings(&[("biome.json", base)], &[("biome.jsonc", &moved)]).is_empty());
        let stricter = base.replace("\"error\" }", "\"error\", \"noDebugger\": \"error\" }");
        assert!(findings(&[("biome.json", base)], &[("biome.json", &stricter)]).is_empty());
    }

    #[test]
    fn ruff_ignores_and_unselected_codes_are_findings() {
        let base = "[tool.ruff]\nline-length = 100\n\n[tool.ruff.lint]\nselect = [\"E\", \"F\", \"B\"]\nignore = [\"E501\"]\n";
        let ignored = base.replace("[\"E501\"]", "[\"E501\", \"B008\"]");
        assert_eq!(
            findings(&[("pyproject.toml", base)], &[("pyproject.toml", &ignored)]),
            ["pyproject.toml:6 ignore = [\"B008\"]"]
        );
        let per_file =
            format!("{base}\n[tool.ruff.lint.per-file-ignores]\n\"tests/*\" = [\"S101\"]\n");
        assert_eq!(
            findings(
                &[("pyproject.toml", base)],
                &[("pyproject.toml", &per_file)]
            ),
            ["pyproject.toml:9 per-file-ignores \"tests/*\" = [\"S101\"]"]
        );
        let unselected = base.replace("\"E\", \"F\", \"B\"", "\"E\", \"F\"");
        assert_eq!(
            findings(
                &[("pyproject.toml", base)],
                &[("pyproject.toml", &unselected)]
            ),
            ["pyproject.toml:5 select = [\"B\"] is gone"]
        );
        // Selecting nothing falls back to Ruff's default, which drops B.
        let default = "[tool.ruff]\nline-length = 100\n";
        assert_eq!(
            findings(&[("pyproject.toml", base)], &[("pyproject.toml", default)]).len(),
            2
        );
    }

    #[test]
    fn ruff_moved_broadened_or_stricter_is_not_a_finding() {
        let base = "[tool.ruff.lint]\nselect = [\"E501\", \"F401\", \"B\"]\nignore = [\"E741\"]\n";
        let moved = "line-length = 100\n\n[lint]\nselect = [\"E501\", \"F401\", \"B\"]\nextend-ignore = [\"E741\"]\n";
        assert!(findings(&[("pyproject.toml", base)], &[("ruff.toml", moved)]).is_empty());
        let broader =
            "[tool.ruff.lint]\nselect = [\"E\", \"F\", \"B\", \"UP\"]\nignore = [\"E741\"]\n";
        assert!(findings(&[("pyproject.toml", base)], &[("pyproject.toml", broader)]).is_empty());
        let all = "[tool.ruff.lint]\nselect = [\"ALL\"]\n";
        assert!(findings(&[("pyproject.toml", base)], &[("pyproject.toml", all)]).is_empty());
        // A pyproject.toml without Ruff settings holds none.
        assert!(measures("pyproject.toml", "[project]\nname = \"a\"\n")
            .expect("readable")
            .is_empty());
    }

    #[test]
    fn flake8_ignores_are_findings_and_a_move_is_not() {
        let base = "[flake8]\nmax-line-length = 100\nextend-ignore = E203\n";
        let head = "[flake8]\nmax-line-length = 100\nextend-ignore = E203, W503\nper-file-ignores =\n    tests/*: S101\n    app/legacy.py:E501,F401\n";
        assert_eq!(
            findings(&[(".flake8", base)], &[(".flake8", head)]),
            [
                ".flake8:3 extend-ignore = W503",
                ".flake8:4 per-file-ignores = app/legacy.py: E501",
                ".flake8:4 per-file-ignores = app/legacy.py: F401",
                ".flake8:4 per-file-ignores = tests/*: S101",
            ]
        );
        let moved = "[metadata]\nname = a\n\n[flake8]\nextend_ignore =\n    E203\n";
        assert!(findings(&[(".flake8", base)], &[("setup.cfg", moved)]).is_empty());
    }

    #[test]
    fn pylint_disables_are_findings() {
        let base = "[MESSAGES CONTROL]\ndisable=\n    missing-module-docstring,\n    C0115\n";
        let head = "[MESSAGES CONTROL]\ndisable=\n    missing-module-docstring,\n    C0115,\n    unused-import\n";
        assert_eq!(
            findings(&[(".pylintrc", base)], &[(".pylintrc", head)]),
            [".pylintrc:2 disable = unused-import"]
        );
        let toml = "[tool.pylint.\"messages control\"]\ndisable = [\"missing-module-docstring\", \"C0115\"]\n";
        assert!(findings(&[(".pylintrc", base)], &[("pyproject.toml", toml)]).is_empty());
        let cfg = "[pylint.messages_control]\ndisable = missing-module-docstring\n";
        assert!(findings(&[(".pylintrc", base)], &[("setup.cfg", cfg)]).is_empty());
    }

    #[test]
    fn mypy_strict_off_flags_relaxed_and_errors_ignored_are_findings() {
        let base = "[tool.mypy]\nstrict = true\nwarn_unreachable = true\n";
        let not_strict = "[tool.mypy]\nstrict = false\nwarn_unreachable = true\n";
        assert_eq!(
            findings(
                &[("pyproject.toml", base)],
                &[("pyproject.toml", not_strict)]
            ),
            ["pyproject.toml:2 strict = false (on the base: strict = true, pyproject.toml:2)"]
        );
        let relaxed =
            "[tool.mypy]\nstrict = true\nwarn_unreachable = true\ndisallow_untyped_defs = false\n";
        assert_eq!(
            findings(&[("pyproject.toml", base)], &[("pyproject.toml", relaxed)]).len(),
            1
        );
        let ignored = format!("{base}\n[[tool.mypy.overrides]]\nmodule = [\"app.legacy.*\"]\nignore_errors = true\ndisable_error_code = [\"union-attr\"]\n");
        assert_eq!(
            findings(&[("pyproject.toml", base)], &[("pyproject.toml", &ignored)]).len(),
            2
        );
        let ini = "[mypy]\ndisallow_untyped_defs = True\n\n[mypy-app.legacy.*]\ndisallow_untyped_defs = False\n";
        assert_eq!(
            findings(
                &[("mypy.ini", "[mypy]\ndisallow_untyped_defs = True\n")],
                &[("mypy.ini", ini)]
            ),
            ["mypy.ini:5 disallow_untyped_defs = False (not set on the base)"]
        );
    }

    #[test]
    fn mypy_moved_or_folded_into_strict_is_not_a_finding() {
        let ini = "[mypy]\nstrict = True\nwarn_unreachable = True\n\n[mypy-tests.*]\nignore_errors = True\n";
        let toml = "[tool.mypy]\nstrict = true\nwarn_unreachable = true\n\n[[tool.mypy.overrides]]\nmodule = \"tests.*\"\nignore_errors = true\n";
        assert!(findings(&[("mypy.ini", ini)], &[("pyproject.toml", toml)]).is_empty());
        let flags = "[mypy]\ndisallow_untyped_defs = True\ncheck_untyped_defs = True\n";
        let strict = "[mypy]\nstrict = True\n";
        assert!(findings(&[("mypy.ini", flags)], &[("mypy.ini", strict)]).is_empty());
        assert_eq!(
            findings(&[("mypy.ini", strict)], &[("mypy.ini", flags)]).len(),
            1
        );
    }

    #[test]
    fn pyright_mode_lowered_and_rules_off_are_findings() {
        let base =
            "{\n  \"typeCheckingMode\": \"strict\",\n  \"reportMissingImports\": \"error\"\n}\n";
        let basic = base.replace("\"strict\"", "\"basic\"");
        assert_eq!(
            findings(
                &[("pyrightconfig.json", base)],
                &[("pyrightconfig.json", &basic)]
            )
            .len(),
            1
        );
        let none = base.replace("\"error\"", "\"none\"");
        assert_eq!(
            findings(
                &[("pyrightconfig.json", base)],
                &[("pyrightconfig.json", &none)]
            )
            .len(),
            1
        );
        let off = "[tool.pyright]\ntypeCheckingMode = \"strict\"\nreportMissingImports = false\n";
        assert_eq!(
            findings(&[("pyrightconfig.json", base)], &[("pyproject.toml", off)]).len(),
            1
        );
        // Removing `standard` is pyright's default; moved to pyproject.toml.
        let standard = "{ \"typeCheckingMode\": \"standard\" }";
        assert!(findings(
            &[("pyrightconfig.json", standard)],
            &[("pyrightconfig.json", "{}")]
        )
        .is_empty());
        let moved = "[tool.pyright]\ntypeCheckingMode = \"strict\"\nreportMissingImports = \"error\"\nreportUnnecessaryTypeIgnoreComment = \"warning\"\n";
        assert!(findings(
            &[("pyrightconfig.json", base)],
            &[("pyproject.toml", moved)]
        )
        .is_empty());
    }

    #[test]
    fn cargo_lints_allowed_or_lowered_are_findings() {
        let base = "[package]\nname = \"a\"\n\n[lints.clippy]\nunwrap_used = \"deny\"\npedantic = { level = \"warn\", priority = -1 }\n";
        let allowed = base.replace("\"deny\"", "\"allow\"");
        assert_eq!(
            findings(&[("crates/a/Cargo.toml", base)], &[("crates/a/Cargo.toml", &allowed)]),
            ["crates/a/Cargo.toml:5 unwrap_used = \"allow\" (on the base: unwrap_used = \"deny\", crates/a/Cargo.toml:5)"]
        );
        let new_allow = format!("{base}\n[lints.rust]\ndead_code = \"allow\"\n");
        assert_eq!(
            findings(
                &[("crates/a/Cargo.toml", base)],
                &[("crates/a/Cargo.toml", &new_allow)]
            )
            .len(),
            1
        );
        let pedantic_off = base.replace("level = \"warn\"", "level = \"allow\"");
        assert_eq!(
            findings(
                &[("crates/a/Cargo.toml", base)],
                &[("crates/a/Cargo.toml", &pedantic_off)]
            )
            .len(),
            1
        );
        // A crate that allows what the workspace denies.
        let workspace = "[workspace]\nmembers = [\"crates/*\"]\n\n[workspace.lints.clippy]\nunwrap_used = \"deny\"\n";
        let crate_allows = "[package]\nname = \"b\"\n\n[lints.clippy]\nunwrap_used = \"allow\"\n";
        assert_eq!(
            findings(
                &[("Cargo.toml", workspace)],
                &[
                    ("Cargo.toml", workspace),
                    ("crates/b/Cargo.toml", crate_allows)
                ]
            )
            .len(),
            1
        );
    }

    #[test]
    fn cargo_lints_new_warn_or_deny_stricter_or_reformatted_is_not_a_finding() {
        let base = "[lints.clippy]\nunwrap_used = \"warn\"\n";
        let head = "[lints.clippy]\nunwrap_used = { level = \"deny\", priority = 1 }\nexpect_used = \"warn\"\ntodo = \"deny\"\n\n[lints.rust]\nunsafe_code = \"forbid\"\n";
        assert!(findings(&[("Cargo.toml", base)], &[("Cargo.toml", head)]).is_empty());
        let workspace = "[lints]\nworkspace = true\n";
        assert!(measures("Cargo.toml", workspace)
            .expect("readable")
            .is_empty());
    }

    #[test]
    fn tsconfig_checks_turned_off_are_findings() {
        let base = "{\n  // the app\n  \"compilerOptions\": {\n    \"strict\": true,\n    \"noUncheckedIndexedAccess\": true\n  }\n}\n";
        let loose = base.replace("\"strict\": true", "\"strict\": false");
        assert_eq!(
            findings(&[("tsconfig.json", base)], &[("tsconfig.json", &loose)]).len(),
            1
        );
        let removed = "{ \"compilerOptions\": { \"noUncheckedIndexedAccess\": true } }";
        assert_eq!(
            findings(&[("tsconfig.json", base)], &[("tsconfig.json", removed)]),
            ["tsconfig.json:4 \"strict\": true is gone"]
        );
        let family_off = base.replace(
            "\"strict\": true,",
            "\"strict\": true,\n    \"strictNullChecks\": false,",
        );
        assert_eq!(
            findings(
                &[("tsconfig.json", base)],
                &[("tsconfig.json", &family_off)]
            ),
            ["tsconfig.json:5 \"strictNullChecks\": false (not set on the base)"]
        );
        let child = "{ \"extends\": \"./tsconfig.json\", \"compilerOptions\": { \"noImplicitAny\": false } }";
        assert_eq!(
            findings(
                &[(
                    "tsconfig.build.json",
                    "{ \"extends\": \"./tsconfig.json\" }"
                )],
                &[("tsconfig.build.json", child)]
            )
            .len(),
            1
        );
    }

    #[test]
    fn tsconfig_stricter_folded_into_strict_or_another_file_looser_is_not_a_finding() {
        let flags =
            "{ \"compilerOptions\": { \"noImplicitAny\": true, \"strictNullChecks\": true } }";
        let strict = "{ \"compilerOptions\": { \"strict\": true, \"noUnusedLocals\": true } }";
        assert!(findings(&[("tsconfig.json", flags)], &[("tsconfig.json", strict)]).is_empty());
        // Each tsconfig is its own: a new, looser one is not a weakening.
        let other = "{ \"compilerOptions\": { \"checkJs\": false } }";
        assert!(findings(
            &[("tsconfig.json", strict)],
            &[("tsconfig.json", strict), ("tsconfig.scripts.json", other)]
        )
        .is_empty());
    }

    #[test]
    fn an_unreadable_file_names_its_line() {
        assert_eq!(measures("tsconfig.json", "{\n  \"a\": \n}"), Err(3));
        assert!(measures(".eslintrc", "rules:\n  no-console: off\n").is_ok());
    }
}
