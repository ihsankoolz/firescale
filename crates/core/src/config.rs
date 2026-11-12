//! Finding, reading and validating the firescale config (SPEC §10).
//!
//! `firescale.config.json` is read here. `firescale.config.ts` is loaded by
//! the Node host; both end up as a JSON value validated by `resolve_config`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::model::{RuleMeta, Status};
use crate::node::NodeHost;
use crate::rule::{BuiltinPack, BuiltinRule, Options};
use crate::verify::{ProjectOverride, StepOverride, VerifyConfig};

pub const TS_CONFIG: &str = "firescale.config.ts";
pub const JSON_CONFIG: &str = "firescale.config.json";

/// Status of a rule that the config does not mention.
pub const DEFAULT_STATUS: Status = Status::Evaluated;

const TOP_LEVEL_KEYS: [&str; 5] = ["$schema", "packs", "rules", "protectedPaths", "verify"];

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
    /// The `verify` section: per-step settings, per-project settings and
    /// whether a skipped step fails the run.
    pub verify: VerifyConfig,
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
                None | Some(Value::Null) => DEFAULT_STATUS,
                Some(value) => value.as_str().and_then(Status::parse).ok_or_else(|| {
                    Error::user(format!(
                        "{name}: rule \"{id}\" has an invalid status. Use \"off\", \"evaluated\" or \"enforced\"."
                    ))
                })?,
            };
            let options = match object.get("options") {
                None | Some(Value::Null) => Map::new(),
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
            "{name}: unknown key \"{key}\". Allowed keys: packs, rules, protectedPaths, verify."
        )));
    }

    // A key set to null means "use the default", the same as leaving it out.
    let packs = match object.get("packs") {
        None | Some(Value::Null) => vec!["core".to_owned()],
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
        None | Some(Value::Null) => {}
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
        None | Some(Value::Null) => Vec::new(),
        Some(value) => string_list(value).ok_or_else(|| {
            Error::user(format!(
                "{name}: \"protectedPaths\" must be a list of globs."
            ))
        })?,
    };

    let verify = parse_verify(name, object.get("verify"))?;

    rules.sort_by(|a, b| a.meta.id.cmp(&b.meta.id));
    Ok(ResolvedConfig {
        file: file.map(str::to_owned),
        packs,
        rules,
        protected_paths,
        verify,
    })
}

/// Reads one step setting: a command, or `false` for "do not run this step".
fn parse_step(name: &str, key: &str, step: &str, value: &Value) -> Result<StepOverride> {
    if !crate::verify::STEPS.contains(&step) {
        return Err(Error::user(format!(
            "{name}: unknown verify step \"{step}\". Allowed steps: lint, typecheck, test."
        )));
    }
    match value {
        Value::String(command) if !command.trim().is_empty() => {
            Ok(StepOverride::Command(command.clone()))
        }
        Value::Bool(false) => Ok(StepOverride::Skip),
        _ => Err(Error::user(format!(
            "{name}: {key}.{step} must be a command (a string) or false to skip the step."
        ))),
    }
}

/// Reads the `verify` section of the config.
fn parse_verify(name: &str, value: Option<&Value>) -> Result<VerifyConfig> {
    let mut verify = VerifyConfig::default();
    let entries = match value {
        None | Some(Value::Null) => return Ok(verify),
        Some(Value::Object(entries)) => entries,
        Some(_) => {
            return Err(Error::user(format!(
                "{name}: \"verify\" must be an object, for example {{ \"test\": \"pnpm test\" }}."
            )));
        }
    };
    for (key, value) in entries {
        match key.as_str() {
            "failOnSkipped" => {
                verify.fail_on_skipped = value.as_bool().ok_or_else(|| {
                    Error::user(format!(
                        "{name}: verify.failOnSkipped must be true or false."
                    ))
                })?;
            }
            "projects" => {
                let Value::Object(projects) = value else {
                    return Err(Error::user(format!(
                        "{name}: verify.projects must be an object keyed by project folder, for example {{ \"backend/api\": {{ \"test\": \"pytest\" }} }}."
                    )));
                };
                for (folder, setting) in projects {
                    let clean = folder.trim_end_matches('/');
                    // "." names the projects at the repo root that the root's
                    // own commands do not run.
                    let plain = clean == "."
                        || !clean.is_empty()
                            && !clean.starts_with('/')
                            && !clean.contains('\\')
                            && !clean.contains(':')
                            && clean
                                .split('/')
                                .all(|part| !part.is_empty() && part != "." && part != "..");
                    if !plain {
                        return Err(Error::user(format!(
                            "{name}: verify.projects key \"{folder}\" must be a folder inside the repo, relative to its root and written with forward slashes, for example \"backend/api\"."
                        )));
                    }
                    let parsed = match setting {
                        Value::Bool(false) => ProjectOverride::Off,
                        Value::Object(steps) => {
                            let mut map = BTreeMap::new();
                            for (step, value) in steps {
                                let key = format!("verify.projects[\"{folder}\"]");
                                map.insert(step.clone(), parse_step(name, &key, step, value)?);
                            }
                            ProjectOverride::Steps(map)
                        }
                        _ => {
                            return Err(Error::user(format!(
                                "{name}: verify.projects[\"{folder}\"] must be false, to leave the project out, or an object with lint, typecheck and test."
                            )));
                        }
                    };
                    if verify.projects.insert(clean.to_owned(), parsed).is_some() {
                        return Err(Error::user(format!(
                            "{name}: verify.projects lists the folder \"{clean}\" twice, once with a trailing slash. Keep one entry."
                        )));
                    }
                }
            }
            step if crate::verify::STEPS.contains(&step) => {
                verify
                    .steps
                    .insert(step.to_owned(), parse_step(name, "verify", step, value)?);
            }
            other => {
                // A misspelt option must not read as a misspelt step.
                let lower = other.to_lowercase();
                let meant = if lower.starts_with("fail") || lower.contains("skip") {
                    Some("failOnSkipped")
                } else if lower.starts_with("proj") {
                    Some("projects")
                } else {
                    None
                };
                return Err(Error::user(match meant {
                    Some(meant) => format!(
                        "{name}: unknown key \"{other}\" in \"verify\". Did you mean \"{meant}\"?"
                    ),
                    None => format!(
                        "{name}: unknown verify step \"{other}\". Allowed steps: lint, typecheck, test."
                    ),
                }));
            }
        }
    }
    Ok(verify)
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

/// The text of a new `firescale.config.json` that turns on `packs`, and the
/// config it resolves to. Only built-in packs are accepted, so this never
/// starts Node. `init` and `adopt` write the text when a repo has no config.
pub fn starter_config<'a>(
    packs: &[String],
    builtin: &'a [BuiltinPack],
) -> Result<(String, ResolvedConfig<'a>)> {
    let raw = serde_json::json!({ "packs": packs });
    let config = resolve_config(&raw, Some(JSON_CONFIG), builtin, &mut |specs| {
        Err(Error::user(format!(
            "A new config can only list built-in packs, not \"{}\". Add custom packs to {JSON_CONFIG} by hand afterwards.",
            specs.join("\", \"")
        )))
    })?;
    let list = packs
        .iter()
        .map(|pack| format!("\"{pack}\""))
        .collect::<Vec<_>>()
        .join(", ");
    Ok((format!("{{\n  \"packs\": [{list}]\n}}\n"), config))
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

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::*;
    use crate::model::{Category, Finding, Severity};
    use crate::rule::RuleContext;

    const RULE: &str = "core/no-committed-env";

    fn meta(id: &str) -> RuleMeta {
        RuleMeta {
            id: id.to_owned(),
            severity: Severity::Blocker,
            category: Category::Security,
            description: "stub".to_owned(),
            inputs: Vec::new(),
            has_fix: false,
            after: Vec::new(),
        }
    }

    struct StubRule(&'static str);

    impl BuiltinRule for StubRule {
        fn meta(&self) -> RuleMeta {
            meta(self.0)
        }
        fn skill(&self) -> &'static str {
            "stub skill"
        }
        fn check(&self, _ctx: &RuleContext) -> Result<Vec<Finding>> {
            Ok(Vec::new())
        }
    }

    fn builtin() -> Vec<BuiltinPack> {
        vec![BuiltinPack {
            name: "core",
            rules: vec![Box::new(StubRule(RULE))],
        }]
    }

    /// A `describe` that must never run: built-in-only configs must not start Node.
    fn no_node(_specs: &[String]) -> Result<Vec<PackDescription>> {
        panic!("describe was called, but the config has no custom packs");
    }

    fn resolve<'a>(raw: &Value, packs: &'a [BuiltinPack]) -> Result<ResolvedConfig<'a>> {
        resolve_config(raw, Some("firescale.config.json"), packs, &mut no_node)
    }

    fn rejection(raw: &Value) -> Error {
        let packs = builtin();
        resolve(raw, &packs)
            .err()
            .expect("config should have been rejected")
    }

    fn description(value: Value) -> PackDescription {
        serde_json::from_value(value).expect("valid pack description")
    }

    fn pack_json(spec: &str, name: &str, ids: &[&str]) -> Value {
        let rules: Vec<Value> = ids
            .iter()
            .map(|id| {
                json!({
                    "id": id,
                    "severity": "high",
                    "category": "consistency",
                    "description": "custom",
                    "skillFile": format!("/repo/tools/{id}/SKILL.md"),
                })
            })
            .collect();
        json!({ "spec": spec, "name": name, "rules": rules })
    }

    #[test]
    fn defaults_to_the_core_pack_with_every_rule_evaluated() {
        let packs = builtin();
        let config = resolve_config(&json!({}), None, &packs, &mut no_node).unwrap();
        assert_eq!(config.file, None);
        assert_eq!(config.packs, vec!["core"]);
        assert!(config.protected_paths.is_empty());
        assert_eq!(config.rules.len(), 1);
        assert_eq!(config.rules[0].meta.id, RULE);
        assert_eq!(config.rules[0].status, Status::Evaluated);
        assert!(config.rules[0].options.is_empty());
        assert!(matches!(config.rules[0].source, RuleSource::Builtin(_)));
    }

    #[test]
    fn accepts_a_status_string_or_a_status_and_options_object() {
        let packs = builtin();
        let enforced = resolve(&json!({ "rules": { RULE: "enforced" } }), &packs).unwrap();
        assert_eq!(enforced.rules[0].status, Status::Enforced);

        let with_options = resolve(
            &json!({ "rules": { RULE: { "status": "off", "options": { "allow": [".env.example"] } } } }),
            &packs,
        )
        .unwrap();
        assert_eq!(with_options.rules[0].status, Status::Off);
        assert_eq!(
            Value::Object(with_options.rules[0].options.clone()),
            json!({ "allow": [".env.example"] })
        );

        let options_only =
            resolve(&json!({ "rules": { RULE: { "options": {} } } }), &packs).unwrap();
        assert_eq!(options_only.rules[0].status, Status::Evaluated);
    }

    #[test]
    fn keeps_protected_paths_and_allows_schema() {
        let packs = builtin();
        let config = resolve(
            &json!({ "$schema": "x", "protectedPaths": ["infra/**"] }),
            &packs,
        )
        .unwrap();
        assert_eq!(config.protected_paths, vec!["infra/**"]);
        assert_eq!(config.file.as_deref(), Some("firescale.config.json"));
    }

    #[test]
    fn an_empty_packs_list_yields_no_rules() {
        let packs = builtin();
        let config = resolve(&json!({ "packs": [] }), &packs).unwrap();
        assert!(config.packs.is_empty());
        assert!(config.rules.is_empty());
    }

    #[test]
    fn rejects_bad_configs_with_a_message_that_says_what_to_do() {
        let unknown_rule = format!("Rules in the enabled packs: {RULE}");
        let cases: Vec<(&str, Value, String)> = vec![
            ("a non-object", json!([]), "must contain an object".into()),
            (
                "an unknown top-level key",
                json!({ "rulez": {} }),
                "unknown key \"rulez\"".into(),
            ),
            (
                "a bad packs value",
                json!({ "packs": "core" }),
                "\"packs\" must be a list".into(),
            ),
            (
                "an unknown pack",
                json!({ "packs": ["nope"] }),
                "Available packs: core".into(),
            ),
            (
                "an unknown rule",
                json!({ "rules": { "core/nope": "off" } }),
                unknown_rule,
            ),
            (
                "a bad status",
                json!({ "rules": { RULE: "on" } }),
                "must be \"off\", \"evaluated\", \"enforced\"".into(),
            ),
            (
                "a bad status in an object",
                json!({ "rules": { RULE: { "status": "on" } } }),
                "invalid status".into(),
            ),
            (
                "an unknown rule key",
                json!({ "rules": { RULE: { "level": "x" } } }),
                "unknown key \"level\"".into(),
            ),
            (
                "bad options",
                json!({ "rules": { RULE: { "options": [] } } }),
                "options must be an object".into(),
            ),
            (
                "bad protectedPaths",
                json!({ "protectedPaths": "x" }),
                "\"protectedPaths\" must be a list".into(),
            ),
        ];
        for (name, raw, message) in cases {
            let error = rejection(&raw);
            assert!(matches!(error, Error::User(_)), "{name}: {error:?}");
            assert!(
                error.message().contains(&message),
                "{name}: expected \"{message}\" in \"{}\"",
                error.message()
            );
            assert!(
                error.message().starts_with("firescale.config.json"),
                "{name}: {}",
                error.message()
            );
        }
    }

    #[test]
    fn describe_is_never_called_without_custom_packs() {
        let packs = builtin();
        let mut called = false;
        let mut describe = |_: &[String]| -> Result<Vec<PackDescription>> {
            called = true;
            Ok(Vec::new())
        };
        for raw in [
            json!({}),
            json!({ "packs": ["core"] }),
            json!({ "packs": [], "protectedPaths": ["a"] }),
            json!({ "rules": { RULE: "enforced" } }),
        ] {
            resolve_config(&raw, None, &packs, &mut describe).unwrap();
        }
        assert!(!called);
    }

    #[test]
    fn custom_pack_rules_are_merged_sorted_and_marked_custom() {
        let packs = builtin();
        let mut asked: Vec<Vec<String>> = Vec::new();
        let mut describe = |specs: &[String]| -> Result<Vec<PackDescription>> {
            asked.push(specs.to_vec());
            Ok(vec![description(pack_json(
                "./tools/pack.ts",
                "team",
                &["team/zeta", "team/alpha"],
            ))])
        };
        let raw = json!({
            "packs": ["core", "./tools/pack.ts", "./tools/pack.ts"],
            "rules": { "team/alpha": "enforced", "team/zeta": { "status": "off", "options": { "n": 1 } } },
        });
        let config =
            resolve_config(&raw, Some("firescale.config.json"), &packs, &mut describe).unwrap();

        let ids: Vec<&str> = config.rules.iter().map(|r| r.meta.id.as_str()).collect();
        assert_eq!(ids, vec![RULE, "team/alpha", "team/zeta"]);
        assert!(matches!(config.rules[0].source, RuleSource::Builtin(_)));
        match &config.rules[1].source {
            RuleSource::Custom { pack, skill_file } => {
                assert_eq!(pack, "./tools/pack.ts");
                assert_eq!(
                    skill_file,
                    &PathBuf::from("/repo/tools/team/alpha/SKILL.md")
                );
            }
            RuleSource::Builtin(_) => panic!("expected a custom rule"),
        }
        assert!(matches!(config.rules[2].source, RuleSource::Custom { .. }));
        assert_eq!(config.rules[0].status, Status::Evaluated);
        assert_eq!(config.rules[1].status, Status::Enforced);
        assert_eq!(config.rules[2].status, Status::Off);
        assert_eq!(config.rules[2].options.get("n"), Some(&json!(1)));
        // Called once, with the de-duplicated specs.
        assert_eq!(asked, vec![vec!["./tools/pack.ts".to_owned()]]);
    }

    #[test]
    fn a_custom_pack_named_core_is_rejected() {
        let packs = builtin();
        let mut describe = |_: &[String]| -> Result<Vec<PackDescription>> {
            Ok(vec![description(pack_json(
                "./p.ts",
                "core",
                &["core/other"],
            ))])
        };
        let error = resolve_config(&json!({ "packs": ["./p.ts"] }), None, &packs, &mut describe)
            .err()
            .expect("rejected");
        assert!(error.message().contains("built-in pack"), "{error}");
        assert!(error.message().contains("./p.ts"), "{error}");
    }

    #[test]
    fn a_duplicate_rule_id_is_rejected() {
        let packs = builtin();
        let mut describe = |_: &[String]| -> Result<Vec<PackDescription>> {
            Ok(vec![description(pack_json("./p.ts", "team", &[RULE]))])
        };
        let error = resolve_config(
            &json!({ "packs": ["core", "./p.ts"] }),
            None,
            &packs,
            &mut describe,
        )
        .err()
        .expect("rejected");
        assert!(error.message().contains("defined twice"), "{error}");
        assert!(error.message().contains(RULE), "{error}");

        // The same id in two custom packs is rejected too.
        let mut two = |_: &[String]| -> Result<Vec<PackDescription>> {
            Ok(vec![
                description(pack_json("./a.ts", "a", &["x/same"])),
                description(pack_json("./b.ts", "b", &["x/same"])),
            ])
        };
        let error = resolve_config(
            &json!({ "packs": ["./a.ts", "./b.ts"] }),
            None,
            &packs,
            &mut two,
        )
        .err()
        .expect("rejected");
        assert!(error.message().contains("defined twice"), "{error}");
    }

    #[test]
    fn describe_errors_are_passed_through() {
        let packs = builtin();
        let mut describe = |_: &[String]| -> Result<Vec<PackDescription>> {
            Err(Error::user("Node is needed for ./p.ts."))
        };
        let error = resolve_config(&json!({ "packs": ["./p.ts"] }), None, &packs, &mut describe)
            .err()
            .expect("rejected");
        assert_eq!(error.message(), "Node is needed for ./p.ts.");
    }

    #[test]
    fn recognises_custom_pack_paths() {
        assert!(is_custom_pack("./x.ts"));
        assert!(is_custom_pack("../x.ts"));
        assert!(!is_custom_pack("core"));
        assert!(!is_custom_pack("x.ts"));
    }

    fn canonical_temp() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        (dir, root)
    }

    fn canonical(path: &Path) -> PathBuf {
        path.canonicalize().unwrap()
    }

    #[test]
    fn find_root_prefers_the_nearest_config_inside_the_git_root() {
        let (_guard, root) = canonical_temp();
        fs::create_dir(root.join(".git")).unwrap();
        let nested = root.join("a").join("b");
        fs::create_dir_all(&nested).unwrap();
        assert_eq!(canonical(&find_root(&nested)), root);

        fs::write(root.join("a").join(JSON_CONFIG), "{}").unwrap();
        assert_eq!(canonical(&find_root(&nested)), root.join("a"));

        fs::write(nested.join(TS_CONFIG), "export default {};\n").unwrap();
        assert_eq!(canonical(&find_root(&nested)), nested);
    }

    #[test]
    fn find_root_does_not_look_for_a_config_above_the_git_root() {
        let (_guard, outer) = canonical_temp();
        fs::write(outer.join(JSON_CONFIG), "{}").unwrap();
        let inner = outer.join("inner");
        fs::create_dir_all(inner.join(".git")).unwrap();
        assert_eq!(canonical(&find_root(&inner)), inner);
        let deeper = inner.join("src");
        fs::create_dir(&deeper).unwrap();
        assert_eq!(canonical(&find_root(&deeper)), inner);
    }

    #[test]
    fn find_root_uses_the_folder_itself_when_there_is_no_marker() {
        let (_guard, root) = canonical_temp();
        // Assumes no config or .git exists above the OS temp folder.
        assert_eq!(canonical(&find_root(&root)), root);
    }

    #[test]
    fn null_values_mean_the_default() {
        let packs = builtin();
        let raw = json!({
            "packs": null,
            "rules": { "core/no-committed-env": { "status": null, "options": null } },
            "protectedPaths": null
        });
        let config = resolve(&raw, &packs).expect("nulls are accepted");
        assert_eq!(config.packs, ["core"]);
        assert_eq!(config.rules[0].status, Status::Evaluated);
        assert!(config.rules[0].options.is_empty());
        assert!(config.protected_paths.is_empty());
        let config = resolve(&json!({ "rules": null }), &packs).expect("null rules are accepted");
        assert_eq!(config.rules.len(), 1);
    }

    #[test]
    fn the_first_unknown_key_in_file_order_is_reported() {
        let raw: Value = serde_json::from_str(r#"{"zzz": 1, "aaa": 2}"#).expect("valid JSON");
        let error = rejection(&raw);
        assert!(
            error.message().contains("unknown key \"zzz\""),
            "{}",
            error.message()
        );
    }
}
