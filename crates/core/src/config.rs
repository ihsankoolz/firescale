//! Finding, reading and validating the firescale config (SPEC §10).
//!
//! `firescale.config.json` is read here. `firescale.config.ts` is loaded by
//! the Node host; both end up as a JSON value validated by `resolve_config`.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::model::{RuleMeta, Status};
use crate::node::NodeHost;
use crate::rule::{BuiltinPack, BuiltinRule, Options};

pub const TS_CONFIG: &str = "firescale.config.ts";
pub const JSON_CONFIG: &str = "firescale.config.json";

/// Status of a rule that the config does not mention.
pub const DEFAULT_STATUS: Status = Status::Evaluated;

const TOP_LEVEL_KEYS: [&str; 4] = ["$schema", "packs", "rules", "protectedPaths"];

/// Where a rule's code lives.
pub enum RuleSource<'a> {
    /// Compiled into the binary.
    Builtin(&'a dyn BuiltinRule),
    /// A TypeScript rule, run through the Node host.
    Custom {
        /// The pack's module path as written in the config.
        pack: String,
        /// Absolute path of the rule's SKILL.md.
        skill_file: PathBuf,
    },
}

/// A rule with its settings from the config applied.
pub struct ConfiguredRule<'a> {
    pub meta: RuleMeta,
    pub status: Status,
    pub options: Options,
    pub source: RuleSource<'a>,
}

pub struct ResolvedConfig<'a> {
    /// Name of the config file, or `None` when defaults are used.
    pub file: Option<String>,
    pub packs: Vec<String>,
    /// Every rule of every enabled pack, including ones that are off. Sorted by id.
    pub rules: Vec<ConfiguredRule<'a>>,
    pub protected_paths: Vec<String>,
}

/// One custom rule, as described by the Node host.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomRule {
    #[serde(flatten)]
    pub meta: RuleMeta,
    pub skill_file: String,
}

/// One custom pack, as described by the Node host.
#[derive(Debug, Clone, Deserialize)]
pub struct PackDescription {
    /// The module path as written in the config.
    pub spec: String,
    pub name: String,
    pub rules: Vec<CustomRule>,
}

/// True for a `packs` entry that is a path to a custom pack module.
#[must_use]
pub fn is_custom_pack(entry: &str) -> bool {
    entry.starts_with("./") || entry.starts_with("../")
}

/// Finds the repo root for `cwd`: the nearest folder, walking up, that holds
/// a firescale config or a `.git` entry. The search never goes above the git
/// root, so a stray config higher up cannot claim the repo. With neither,
/// the root is `cwd` itself.
#[must_use]
pub fn find_root(cwd: &Path) -> PathBuf {
    let mut dir = cwd;
    loop {
        if [TS_CONFIG, JSON_CONFIG, ".git"]
            .iter()
            .any(|marker| dir.join(marker).exists())
        {
            return dir.to_path_buf();
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => return cwd.to_path_buf(),
        }
    }
}

fn string_list(value: &Value) -> Option<Vec<String>> {
    value
        .as_array()?
        .iter()
        .map(|item| item.as_str().map(str::to_owned))
        .collect()
}

fn sorted_ids(rules: &[ConfiguredRule<'_>]) -> String {
    if rules.is_empty() {
        return "(none)".to_owned();
    }
    let mut ids: Vec<&str> = rules.iter().map(|rule| rule.meta.id.as_str()).collect();
    ids.sort_unstable();
    ids.join(", ")
}

fn rule_setting(name: &str, id: &str, value: &Value) -> Result<(Status, Options)> {
    let expected = "\"off\", \"evaluated\", \"enforced\" or { status, options }";
    match value {
        Value::String(text) => Status::parse(text)
            .map(|status| (status, Map::new()))
            .ok_or_else(|| Error::user(format!("{name}: rule \"{id}\" must be {expected}."))),
        Value::Object(object) => {
            if let Some(extra) = object
                .keys()
                .find(|key| *key != "status" && *key != "options")
            {
                return Err(Error::user(format!(
                    "{name}: rule \"{id}\" has unknown key \"{extra}\". Use {expected}."
                )));
            }
            let status = match object.get("status") {
                None => DEFAULT_STATUS,
                Some(value) => value.as_str().and_then(Status::parse).ok_or_else(|| {
                    Error::user(format!(
                        "{name}: rule \"{id}\" has an invalid status. Use \"off\", \"evaluated\" or \"enforced\"."
                    ))
                })?,
            };
            let options = match object.get("options") {
                None => Map::new(),
                Some(Value::Object(options)) => options.clone(),
                Some(_) => {
                    return Err(Error::user(format!(
                        "{name}: rule \"{id}\" options must be an object."
                    )));
                }
            };
            Ok((status, options))
        }
        _ => Err(Error::user(format!(
            "{name}: rule \"{id}\" must be {expected}."
        ))),
    }
}

/// Validates a raw config value and applies it to the available packs.
///
/// `describe` is called at most once, and only when the config lists custom
/// packs. It is how the Node host is reached, so a config without custom
/// packs never starts Node.
pub fn resolve_config<'a>(
    raw: &Value,
    file: Option<&str>,
    builtin: &'a [BuiltinPack],
    describe: &mut dyn FnMut(&[String]) -> Result<Vec<PackDescription>>,
) -> Result<ResolvedConfig<'a>> {
    let name = file.unwrap_or("config");
    let Some(object) = raw.as_object() else {
        return Err(Error::user(format!(
            "{name} must contain an object with \"packs\" and \"rules\"."
        )));
    };
    if let Some(key) = object
        .keys()
        .find(|key| !TOP_LEVEL_KEYS.contains(&key.as_str()))
    {
        return Err(Error::user(format!(
            "{name}: unknown key \"{key}\". Allowed keys: packs, rules, protectedPaths."
        )));
    }

    let packs = match object.get("packs") {
        None => vec!["core".to_owned()],
        Some(value) => string_list(value).ok_or_else(|| {
            Error::user(format!(
                "{name}: \"packs\" must be a list of pack names, for example [\"core\"]."
            ))
        })?,
    };

    let mut rules: Vec<ConfiguredRule<'a>> = Vec::new();
    let mut custom: Vec<String> = Vec::new();
    for entry in &packs {
        if is_custom_pack(entry) {
            if !custom.contains(entry) {
                custom.push(entry.clone());
            }
            continue;
        }
        let Some(pack) = builtin.iter().find(|pack| pack.name == entry) else {
            let available: Vec<&str> = builtin.iter().map(|pack| pack.name).collect();
            return Err(Error::user(format!(
                "{name}: unknown rule pack \"{entry}\". Available packs: {}. A custom pack is a path that starts with \"./\".",
                available.join(", ")
            )));
        };
        for rule in &pack.rules {
            let meta = rule.meta();
            if !rules.iter().any(|existing| existing.meta.id == meta.id) {
                rules.push(ConfiguredRule {
                    meta,
                    status: DEFAULT_STATUS,
                    options: Map::new(),
                    source: RuleSource::Builtin(rule.as_ref()),
                });
            }
        }
    }

    if !custom.is_empty() {
        for pack in describe(&custom)? {
            if builtin
                .iter()
                .any(|builtin_pack| builtin_pack.name == pack.name)
            {
                return Err(Error::user(format!(
                    "{name}: custom pack \"{}\" is named \"{}\", which is a built-in pack. Rename the pack.",
                    pack.spec, pack.name
                )));
            }
            for rule in pack.rules {
                if rules
                    .iter()
                    .any(|existing| existing.meta.id == rule.meta.id)
                {
                    return Err(Error::user(format!(
                        "{name}: rule \"{}\" is defined twice. Rename one of the rules.",
                        rule.meta.id
                    )));
                }
                rules.push(ConfiguredRule {
                    meta: rule.meta,
                    status: DEFAULT_STATUS,
                    options: Map::new(),
                    source: RuleSource::Custom {
                        pack: pack.spec.clone(),
                        skill_file: PathBuf::from(rule.skill_file),
                    },
                });
            }
        }
    }

    match object.get("rules") {
        None => {}
        Some(Value::Object(settings)) => {
            for (id, value) in settings {
                let known = sorted_ids(&rules);
                let Some(rule) = rules.iter_mut().find(|rule| rule.meta.id == *id) else {
                    return Err(Error::user(format!(
                        "{name}: unknown rule \"{id}\". Rules in the enabled packs: {known}."
                    )));
                };
                let (status, options) = rule_setting(name, id, value)?;
                rule.status = status;
                rule.options = options;
            }
        }
        Some(_) => {
            return Err(Error::user(format!(
                "{name}: \"rules\" must be an object keyed by rule id."
            )));
        }
    }

    let protected_paths = match object.get("protectedPaths") {
        None => Vec::new(),
        Some(value) => string_list(value).ok_or_else(|| {
            Error::user(format!(
                "{name}: \"protectedPaths\" must be a list of globs."
            ))
        })?,
    };

    rules.sort_by(|a, b| a.meta.id.cmp(&b.meta.id));
    Ok(ResolvedConfig {
        file: file.map(str::to_owned),
        packs,
        rules,
        protected_paths,
    })
}

fn read_json(root: &Path) -> Result<Value> {
    let text = fs::read_to_string(root.join(JSON_CONFIG)).map_err(|error| {
        Error::user(format!(
            "Could not read {JSON_CONFIG} ({error}). Make sure it is a readable file."
        ))
    })?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|error| {
        Error::user(format!(
            "{JSON_CONFIG} is not valid JSON ({error}). Fix the syntax and try again."
        ))
    })
}

/// Loads the config from the repo root. Defaults apply when no file exists.
/// Node is started only for a `.ts` config or for custom packs.
pub fn load_config<'a>(
    root: &Path,
    builtin: &'a [BuiltinPack],
    host: &mut NodeHost,
) -> Result<ResolvedConfig<'a>> {
    let has_ts = root.join(TS_CONFIG).exists();
    let has_json = root.join(JSON_CONFIG).exists();
    if has_ts && has_json {
        return Err(Error::user(format!(
            "Found both {TS_CONFIG} and {JSON_CONFIG}. Keep one config file and delete the other."
        )));
    }
    let (raw, file) = if has_ts {
        (host.load_config(TS_CONFIG)?, Some(TS_CONFIG))
    } else if has_json {
        (read_json(root)?, Some(JSON_CONFIG))
    } else {
        (Value::Object(Map::new()), None)
    };
    resolve_config(&raw, file, builtin, &mut |specs| host.describe_packs(specs))
}
