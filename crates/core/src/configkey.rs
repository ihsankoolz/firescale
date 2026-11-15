//! What of the firescale config a verify step's result can depend on
//! (ADR 0058).
//!
//! A change to `firescale.config.json` used to affect every project and
//! change every step's cache key, because the config was treated as a file.
//! Here it is compared by value, setting by setting:
//!
//! - a step's **command**, and whether it is on, come out of the plan
//!   ([`crate::verify::plan_verify`]); the command is in the step's key;
//! - the settings **every step shares** ([`shared_settings`]) are a line of
//!   every step's key, and a change to one affects every project;
//! - everything else (time limits, budgets, labels, the tamper policy, the
//!   limits of `fix --agent`) is in no key and affects nothing.
//!
//! A setting this version does not know is on the "every step" side: the
//! other settings are made by taking out what is known ([`other_settings`]),
//! and a base config that cannot be read affects every project.
//!
//! This holds for a step whose key does not hold the repo root. A step that
//! runs where the config is (the root's own steps, a project at the root)
//! can read it as a file, so for such a step the config stays a file too:
//! its bytes are in the key ([`crate::stepcache::Scope::holds_root`]) and a
//! change of them runs the step's unit ([`mark_file_readers`]).
//!
//! None of this depends on git listing the file. A config that git ignores
//! is in no list of changed files and in no commit: its settings are
//! compared with the defaults all the same, its bytes are read from disk
//! for the keys that hold them, and under `--affected` they are compared
//! with what the last run that passed saw ([`ignored_config`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{Map, Value};

use crate::config::{verify_settings, JSON_CONFIG, TS_CONFIG};
use crate::facts::RepoFacts;
use crate::git::run_git;
use crate::graph::Graph;
use crate::stepcache::unit_scope;
use crate::verify::{plan_verify, Planned, ProjectPlan, VerifyConfig, VerifyPlan};

/// The top-level keys that are in no step's key: `$schema` (only editors
/// read it), `protectedPaths` and `agent` (read only by `fix --agent`).
/// `verify` is taken out too, and read setting by setting.
const NOT_IN_OTHER_SETTINGS: [&str; 4] = ["$schema", "protectedPaths", "agent", "verify"];

/// The top-level keys outside `verify` that every step's key holds under
/// their own name.
const KNOWN_OTHER_SETTINGS: [&str; 2] = ["packs", "rules"];

/// Added to the name of a top-level key this version does not know. No
/// setting's name ends with it, so an unknown key and a known setting
/// never share a name, whatever the key is called.
const UNKNOWN_KEY: &str = " (an unknown key)";

/// The value with the keys of every object sorted, so the order in which a
/// person wrote them never matters.
fn sorted(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let ordered: BTreeMap<&String, Value> = object
                .iter()
                .map(|(key, value)| (key, sorted(value)))
                .collect();
            Value::Object(
                ordered
                    .into_iter()
                    .map(|(key, value)| (key.clone(), value))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

/// The settings outside `verify` that every step's key holds, as JSON with
/// sorted keys: the raw config without the keys of
/// [`NOT_IN_OTHER_SETTINGS`]. `packs`, `rules` and any top-level key this
/// version does not know are in it, because it is made by taking out what
/// is known.
///
/// A key set to `null` is the same as one left out, `packs` left out is
/// `["core"]` and `rules` left out is `{}`, as [`crate::config::resolve_config`]
/// reads them.
#[must_use]
pub fn other_settings(raw: &Value) -> String {
    let Some(object) = raw.as_object() else {
        return raw.to_string();
    };
    let mut rest: Map<String, Value> = object
        .iter()
        .filter(|(key, value)| !NOT_IN_OTHER_SETTINGS.contains(&key.as_str()) && !value.is_null())
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    rest.entry("packs".to_owned())
        .or_insert_with(|| Value::Array(vec![Value::String("core".to_owned())]));
    rest.entry("rules".to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    sorted(&Value::Object(rest)).to_string()
}

/// The settings every step's result can depend on, by name, each with its
/// resolved value as text.
///
/// `VerifyConfig` is taken apart with no `..`: a new field does not compile
/// until it is put on a side here (and in the table of ADR 0058).
fn shared_parts(config: &VerifyConfig) -> BTreeMap<String, String> {
    let VerifyConfig {
        // In, per step: the plan turns them into each step's command, which
        // is in its key, or into "off".
        steps: _,
        projects: _,
        // In, every step.
        fail_on_skipped,
        no_ci_variable,
        keep_running_when_service_down,
        stop_after_errors: _,
        other_settings,
        // Out: time limits and budgets. A step they stop is never kept as
        // passed or failed.
        timeout: _,
        idle_timeout: _,
        step_timeouts: _,
        project_timeouts: _,
        base_budget: _,
        // Out: which passing steps are judged on the base. The label is
        // worked out again in every run.
        label_fixed: _,
        // Out of a head step's key; in the base store's key
        // (`environment` in `blame::compare`).
        base_install: _,
        // Out: the tamper guard reads the base commit's policy and is
        // never cached.
        tamper: _,
    } = config;
    let mut parts = BTreeMap::new();
    parts.insert("verify.ci".to_owned(), (!no_ci_variable).to_string());
    parts.insert(
        "verify.failOnSkipped".to_owned(),
        fail_on_skipped.to_string(),
    );
    parts.insert(
        "verify.stopWhenServiceDown".to_owned(),
        (!keep_running_when_service_down).to_string(),
    );
    // The resolved count: the default written out in full is no change.
    parts.insert(
        "verify.stopAfterErrors".to_owned(),
        config
            .errors_before_stop()
            .map_or_else(|| "false".to_owned(), |count| count.to_string()),
    );
    match serde_json::from_str::<Value>(other_settings) {
        Ok(Value::Object(other)) => {
            for (key, value) in other {
                // A key this version does not know gets a name no setting
                // has, so one called `verify.ci` in a base commit's config
                // cannot take the place of the real `verify.ci`.
                let name = if KNOWN_OTHER_SETTINGS.contains(&key.as_str()) {
                    key
                } else {
                    format!("{key}{UNKNOWN_KEY}")
                };
                parts.insert(name, value.to_string());
            }
        }
        // Not made from a config's value (a `VerifyConfig` built in code).
        _ => {
            parts.insert("(other settings)".to_owned(), other_settings.clone());
        }
    }
    parts
}

/// A digest of the settings every step's result can depend on: a line of
/// every step's cache key (ADR 0058).
#[must_use]
pub fn shared_settings(config: &VerifyConfig) -> String {
    let mut hasher = blake3::Hasher::new();
    for (name, value) in shared_parts(config) {
        hasher.update(format!("{name}\0{value}\n").as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

/// The config of a commit, as far as it can be compared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaseConfig {
    /// Its `verify` settings and other settings; the defaults when the
    /// commit has no config.
    Settings(Box<VerifyConfig>),
    /// It cannot be compared, with why: a `firescale.config.ts`, JSON that
    /// does not parse, a setting this version does not know.
    Unreadable(String),
}

/// Reads a config's text as [`BaseConfig`].
#[must_use]
pub fn base_config_from(text: Option<&str>) -> BaseConfig {
    let raw = match text {
        None => Value::Object(Map::new()),
        Some(text) => match serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')) {
            Ok(raw) => raw,
            Err(error) => return BaseConfig::Unreadable(format!("it is not valid JSON ({error})")),
        },
    };
    if !raw.is_object() {
        return BaseConfig::Unreadable("it does not hold an object".to_owned());
    }
    match verify_settings(JSON_CONFIG, &raw) {
        Ok(settings) => BaseConfig::Settings(Box::new(settings)),
        Err(error) => BaseConfig::Unreadable(error.message().to_owned()),
    }
}

/// The firescale config of `commit`, read with git and never run: the
/// defaults when the commit has none.
#[must_use]
pub fn base_config(root: &Path, commit: &str) -> BaseConfig {
    let unreadable = |why: &str| BaseConfig::Unreadable(why.to_owned());
    // Names relative to `root`, also when it is a folder inside a larger repo.
    let listed = match run_git(
        root,
        &[
            "ls-tree",
            "--name-only",
            "-z",
            commit,
            "--",
            JSON_CONFIG,
            TS_CONFIG,
        ],
    ) {
        Ok(out) if out.code == 0 => out.stdout,
        _ => return unreadable("git could not list the files of that commit"),
    };
    let names: Vec<&str> = listed.split('\0').filter(|name| !name.is_empty()).collect();
    if names.contains(&TS_CONFIG) {
        return unreadable("it is a firescale.config.ts, which is code and is not run for this");
    }
    if !names.contains(&JSON_CONFIG) {
        return base_config_from(None);
    }
    match run_git(root, &["show", &format!("{commit}:./{JSON_CONFIG}")]) {
        Ok(out) if out.code == 0 => base_config_from(Some(&out.stdout)),
        _ => unreadable("git could not read it from that commit"),
    }
}

/// The commit `--affected` measures from: the merge base with `base`, or
/// `HEAD` when no base is given. `None` when git cannot say.
#[must_use]
pub fn comparison_commit(root: &Path, base: Option<&str>) -> Option<String> {
    let out = match base {
        Some(base) => run_git(root, &["merge-base", "HEAD", base]),
        None => run_git(root, &["rev-parse", "--verify", "--quiet", "HEAD"]),
    }
    .ok()?;
    let commit = out.stdout.trim();
    (out.code == 0 && !commit.is_empty()).then(|| commit.to_owned())
}

/// A unit of a plan: the project's folder and kind, or `(None, None)` for
/// the repo root's own steps.
pub type UnitId = (Option<String>, Option<String>);

/// What the difference between two configs affects.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigEffect {
    /// Set when every step is affected, with the reason: a setting every
    /// step shares changed, or the base's config cannot be read.
    pub everything: Option<String>,
    /// Otherwise: the units with a step whose command is new or changed,
    /// or that is no longer off, and those steps.
    pub units: BTreeMap<UnitId, Vec<String>>,
}

impl ConfigEffect {
    /// True when the config change affects no step.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.everything.is_none() && self.units.is_empty()
    }

    /// True when the change can alter the result of this step.
    #[must_use]
    pub fn reaches(&self, folder: Option<&str>, kind: Option<&str>, step: &str) -> bool {
        self.everything.is_some()
            || self
                .units
                .get(&(folder.map(str::to_owned), kind.map(str::to_owned)))
                .is_some_and(|steps| steps.iter().any(|name| name == step))
    }

    /// Marks the units of an affected plan that the config change reaches
    /// and that no changed file already runs.
    pub fn mark(&self, plan: &mut VerifyPlan) {
        let Some(affected) = plan.affected.as_mut() else {
            return;
        };
        for unit in affected.units.iter_mut().filter(|unit| !unit.affected) {
            let Some(steps) = self.units.get(&(unit.project.clone(), unit.kind.clone())) else {
                continue;
            };
            let steps: Vec<&str> = steps.iter().map(String::as_str).collect();
            let listed = match steps.as_slice() {
                [] => continue,
                [one] => format!("{one} step runs"),
                [first @ .., last] => format!("{} and {last} steps run", first.join(", ")),
            };
            unit.affected = true;
            unit.reason = format!("{JSON_CONFIG} changes what its {listed}");
        }
    }
}

/// The note of an affected run whose config changed in nothing that can
/// change a step's result, when no unit of the plan can read the config as
/// a file ([`mark_file_readers`]).
pub const NOTHING_TO_RUN_NOTE: &str = "firescale.config.json changed, but only in settings that cannot change a step's result (time limits, budgets and the like), so the change runs no project.";

/// The reason of a unit that runs only because its steps can read the
/// changed config as a file.
pub const READ_AS_A_FILE_REASON: &str = "firescale.config.json changed, and these steps can read every file of the repo, the config among them";

/// The same, for a config that git ignores: no commit has a copy of it, so
/// "changed" is measured from the last verify run that passed.
pub const IGNORED_READ_AS_A_FILE_REASON: &str = "firescale.config.json, which git ignores, changed since the last run that passed, and these steps can read every file of the repo, the config among them";

/// The digest of `firescale.config.json` when it is on disk and git ignores
/// it; `None` for a config git tracks or lists as untracked, and when there
/// is none.
///
/// Git lists an ignored file nowhere and no commit has a copy of it, so
/// `verify --affected` cannot tell from git whether its bytes changed. It
/// compares this digest with the one the last run that passed wrote down
/// ([`crate::stepcache::ignored_config_changed`]).
#[must_use]
pub fn ignored_config(root: &Path) -> Option<String> {
    let bytes = std::fs::read(root.join(JSON_CONFIG)).ok()?;
    let ignored = run_git(root, &["check-ignore", "--quiet", "--", JSON_CONFIG])
        .is_ok_and(|out| out.code == 0);
    ignored.then(|| blake3::hash(&bytes).to_hex().to_string())
}

/// Marks the units of an affected plan whose steps can read a changed
/// `firescale.config.json` as a file and that nothing else already runs:
/// those whose key holds the repo root
/// ([`crate::stepcache::Scope::holds_root`]: the root's
/// own steps, a project at the root, a folder the graph cannot place, a
/// project that uses one of those). For them a change of the file's bytes
/// is a change, whatever its settings say (ADR 0058). `reason` is what a
/// unit marked here is told. Returns true when the plan has such a unit,
/// marked here or before.
pub fn mark_file_readers(plan: &mut VerifyPlan, graph: &Graph, reason: &str) -> bool {
    let Some(affected) = plan.affected.as_mut() else {
        return false;
    };
    let mut any = false;
    for unit in &mut affected.units {
        let reads = unit_scope(Some(graph), unit.project.as_deref(), unit.kind.as_deref())
            .is_ok_and(|scope| scope.holds_root());
        if !reads {
            continue;
        }
        any = true;
        if !unit.affected {
            unit.affected = true;
            reason.clone_into(&mut unit.reason);
        }
    }
    any
}

/// The steps of `now` that the config change reaches: one with a command
/// that it did not have, with that exact text, in `before`, and one that
/// is now skipped for want of a command and was not before (it was off).
/// A skipped step runs nothing, but the run must count it: it makes the
/// run partial, or failed with `verify.failOnSkipped`.
fn reached(now: &[(String, Planned)], before: Option<&[(String, Planned)]>) -> Vec<String> {
    now.iter()
        .filter(|(step, planned)| {
            let old = before.and_then(|steps| {
                steps
                    .iter()
                    .find(|(name, _)| name == step)
                    .map(|(_, planned)| planned)
            });
            match planned {
                Planned::Command(command) => {
                    !matches!(old, Some(Planned::Command(other)) if other == command)
                }
                Planned::Skipped(_) => !matches!(old, Some(Planned::Skipped(_))),
                Planned::NotNeeded(_) => false,
            }
        })
        .map(|(step, _)| step.clone())
        .collect()
}

/// What the difference between the config of a base commit and the loaded
/// one affects. `plan` is the plan made from `head`; `base_plan` the plan
/// made the same way, from the same facts, with the base's settings.
#[must_use]
pub fn effect_between(
    base: &BaseConfig,
    head: &VerifyConfig,
    base_plan: impl FnOnce(&VerifyConfig) -> VerifyPlan,
    plan: &VerifyPlan,
) -> ConfigEffect {
    let base = match base {
        BaseConfig::Settings(settings) => settings,
        BaseConfig::Unreadable(why) => {
            return ConfigEffect {
                everything: Some(format!(
                    "the {JSON_CONFIG} of the commit compared with could not be read by this version ({why}), so its settings cannot be compared"
                )),
                units: BTreeMap::new(),
            };
        }
    };
    // The same settings, as when the config did not change at all: no plan
    // needs to be made to see that nothing is reached.
    if **base == *head {
        return ConfigEffect::default();
    }
    let (before, after) = (shared_parts(base), shared_parts(head));
    let names: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    let differ: Vec<&str> = names
        .into_iter()
        .filter(|name| before.get(*name) != after.get(*name))
        .map(String::as_str)
        .collect();
    if !differ.is_empty() {
        return ConfigEffect {
            everything: Some(format!(
                "a setting of {JSON_CONFIG} that every step's result can depend on changed ({})",
                differ.join(", ")
            )),
            units: BTreeMap::new(),
        };
    }
    let old = base_plan(base);
    let mut units: BTreeMap<UnitId, Vec<String>> = BTreeMap::new();
    let root_steps = reached(&plan.steps, Some(&old.steps));
    if !root_steps.is_empty() {
        units.insert((None, None), root_steps);
    }
    for project in &plan.projects {
        let same_unit =
            |other: &&ProjectPlan| other.root == project.root && other.kind == project.kind;
        let before = old.projects.iter().find(same_unit);
        let steps = reached(&project.steps, before.map(|plan| plan.steps.as_slice()));
        if !steps.is_empty() {
            units.insert((Some(project.root.clone()), project.kind.clone()), steps);
        }
    }
    ConfigEffect {
        everything: None,
        units,
    }
}

/// What the difference between the config at `commit` and the loaded one
/// affects, for the plan made from the loaded one. `affected` and `base`
/// are what [`plan_verify`] was given.
#[must_use]
pub fn effect_since(
    root: &Path,
    commit: &str,
    facts: &RepoFacts,
    head: &VerifyConfig,
    plan: &VerifyPlan,
    affected: bool,
    base: Option<&str>,
) -> ConfigEffect {
    effect_between(
        &base_config(root, commit),
        head,
        |settings| plan_verify(root, facts, settings, affected, base),
        plan,
    )
}

/// For `verify --affected`: what the difference between the loaded config
/// and the config of the commit the run compares with affects.
///
/// It does not ask whether `firescale.config.json` is among the changed
/// files: git lists a config it ignores nowhere, and its settings count all
/// the same. A config with no copy in that commit (new, untracked or
/// ignored) is compared with the defaults.
///
/// `None` when that commit is not known, and for a `firescale.config.ts`
/// that is there while no `.json` changed: then the whole-file rule holds,
/// and a changed config file affects every project
/// ([`crate::graph::widening_reason`]). The units that can read the config
/// as a file are not in the answer: [`mark_file_readers`] marks them.
#[must_use]
pub fn effect_for_affected(
    root: &Path,
    base: Option<&str>,
    changed: &crate::affected::Changed,
    facts: &RepoFacts,
    head: &VerifyConfig,
    plan: &VerifyPlan,
) -> Option<ConfigEffect> {
    let crate::affected::Changed::Files(files) = changed else {
        return None;
    };
    if !files.contains(JSON_CONFIG) && root.join(TS_CONFIG).exists() {
        return None;
    }
    let commit = comparison_commit(root, base)?;
    Some(effect_since(root, &commit, facts, head, plan, true, base))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::*;
    use crate::config::{AgentLimits, TOP_LEVEL_KEYS};
    use crate::facts::detect_facts;
    use crate::testing::course_project;
    use crate::verify::ProjectTimeouts;

    /// The side of a setting (ADR 0058).
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Side {
        /// In no key; a change affects nothing.
        Out,
        /// In every step's key; a change affects every project.
        InAll,
        /// In the key of these steps only: `(project folder, step)`, with
        /// `""` for the repo root's own steps.
        InSteps(&'static [(&'static str, &'static str)]),
    }

    const LEDGER: &str = "backend/ledger_service";

    /// The table of ADR 0058: every setting, a change of it (from the first
    /// value to the second) and the side it must land on.
    fn table() -> Vec<(&'static str, Value, Value, Side)> {
        let project = |setting: Value| json!({ "verify": { "projects": { LEDGER: setting } } });
        vec![
            (
                "$schema",
                json!({}),
                json!({ "$schema": "./schema.json" }),
                Side::Out,
            ),
            (
                "packs",
                json!({}),
                json!({ "packs": ["core", "agent-ready"] }),
                Side::InAll,
            ),
            (
                "rules.<id>",
                json!({}),
                json!({ "rules": { "core/readme-setup": "off" } }),
                Side::InAll,
            ),
            (
                "rules.<id>.status",
                json!({ "rules": { "core/readme-setup": { "status": "off" } } }),
                json!({ "rules": { "core/readme-setup": { "status": "enforced" } } }),
                Side::InAll,
            ),
            (
                "rules.<id>.options",
                json!({ "rules": { "core/env-example": { "options": { "excludePaths": ["a/"] } } } }),
                json!({ "rules": { "core/env-example": { "options": { "excludePaths": ["b/"] } } } }),
                Side::InAll,
            ),
            (
                "protectedPaths",
                json!({}),
                json!({ "protectedPaths": ["infra/**"] }),
                Side::Out,
            ),
            (
                "agent.maxTurns",
                json!({}),
                json!({ "agent": { "maxTurns": 5 } }),
                Side::Out,
            ),
            (
                "agent.maxCostUsd",
                json!({}),
                json!({ "agent": { "maxCostUsd": 1.5 } }),
                Side::Out,
            ),
            (
                "agent.timeoutSeconds",
                json!({}),
                json!({ "agent": { "timeoutSeconds": 60 } }),
                Side::Out,
            ),
            // A root command runs once, in place of every project's own.
            (
                "verify.<step>",
                json!({}),
                json!({ "verify": { "test": "python -m pytest backend" } }),
                Side::InSteps(&[("", "test")]),
            ),
            (
                "verify.<step>.command",
                json!({ "verify": { "test": "python -m pytest backend" } }),
                json!({ "verify": { "test": { "command": "python -m pytest -x backend" } } }),
                Side::InSteps(&[("", "test")]),
            ),
            // Turned off: nothing to run. Turned on again: the projects'
            // own test steps are new.
            (
                "verify.<step>: false",
                json!({ "verify": { "test": false } }),
                json!({}),
                Side::InSteps(&[
                    ("backend/audit_service", "test"),
                    ("backend/auth_service", "test"),
                    (LEDGER, "test"),
                    ("backend/market_service", "test"),
                    ("backend/realtime_service", "test"),
                    ("frontend", "test"),
                ]),
            ),
            (
                "verify.<step>.timeout",
                json!({}),
                json!({ "verify": { "test": { "timeout": "2h" } } }),
                Side::Out,
            ),
            (
                "verify.timeout",
                json!({}),
                json!({ "verify": { "timeout": "2h" } }),
                Side::Out,
            ),
            (
                "verify.idleTimeout",
                json!({}),
                json!({ "verify": { "idleTimeout": false } }),
                Side::Out,
            ),
            (
                "verify.ci",
                json!({}),
                json!({ "verify": { "ci": false } }),
                Side::InAll,
            ),
            (
                "verify.failOnSkipped",
                json!({}),
                json!({ "verify": { "failOnSkipped": true } }),
                Side::InAll,
            ),
            (
                "verify.stopWhenServiceDown",
                json!({}),
                json!({ "verify": { "stopWhenServiceDown": false } }),
                Side::InAll,
            ),
            (
                "verify.stopAfterErrors",
                json!({}),
                json!({ "verify": { "stopAfterErrors": false } }),
                Side::InAll,
            ),
            (
                "verify.labelFixed",
                json!({}),
                json!({ "verify": { "labelFixed": true } }),
                Side::Out,
            ),
            (
                "verify.baseBudget",
                json!({}),
                json!({ "verify": { "baseBudget": "1m" } }),
                Side::Out,
            ),
            (
                "verify.baseInstall",
                json!({}),
                json!({ "verify": { "baseInstall": "npm ci" } }),
                Side::Out,
            ),
            (
                "verify.tamper",
                json!({}),
                json!({ "verify": { "tamper": { "policy": "enforced", "testPaths": ["checks/**"], "allow": ["a/**"] } } }),
                Side::Out,
            ),
            (
                "verify.projects.<folder>: false",
                project(json!(false)),
                json!({}),
                Side::InSteps(&[(LEDGER, "lint"), (LEDGER, "typecheck"), (LEDGER, "test")]),
            ),
            (
                "verify.projects.<folder>.<step>",
                json!({}),
                project(json!({ "test": "python -m pytest -q" })),
                Side::InSteps(&[(LEDGER, "test")]),
            ),
            (
                "verify.projects.<folder>.<step>.command",
                project(json!({ "lint": "ruff check ." })),
                project(json!({ "lint": { "command": "ruff check --fix ." } })),
                Side::InSteps(&[(LEDGER, "lint")]),
            ),
            (
                "verify.projects.<folder>.<step>: false",
                project(json!({ "test": false })),
                json!({}),
                Side::InSteps(&[(LEDGER, "test")]),
            ),
            // No linter was found for this project: with the step no longer
            // off, it is skipped, which a run must report.
            (
                "verify.projects.<folder>.<step>: false, with no command found",
                project(json!({ "lint": false })),
                json!({}),
                Side::InSteps(&[(LEDGER, "lint")]),
            ),
            (
                "verify.projects.<folder>.timeout",
                json!({}),
                project(json!({ "timeout": "2h" })),
                Side::Out,
            ),
            (
                "verify.projects.<folder>.<step>.timeout",
                project(json!({ "test": { "timeout": "1h" } })),
                project(json!({ "test": { "timeout": "2h" } })),
                Side::Out,
            ),
            // A top-level key of a newer firescale, in the base commit.
            (
                "(unknown)",
                json!({ "future": true }),
                json!({}),
                Side::InAll,
            ),
        ]
    }

    fn settings(raw: &Value) -> VerifyConfig {
        verify_settings(JSON_CONFIG, raw).expect("a valid config")
    }

    #[test]
    fn every_setting_lands_on_the_side_the_adr_gives_it() {
        let fixture = course_project().unwrap();
        let root = fixture.root();
        let facts = detect_facts(root).unwrap();
        for affected in [false, true] {
            for (name, before, after, side) in table() {
                // The unknown key is refused in the working tree; the base
                // side goes through the same reader as a commit's config.
                let base = base_config_from(Some(&before.to_string()));
                let head = settings(&after);
                let plan = plan_verify(root, &facts, &head, affected, None);
                let effect = effect_between(
                    &base,
                    &head,
                    |old| plan_verify(root, &facts, old, affected, None),
                    &plan,
                );
                let BaseConfig::Settings(old) = &base else {
                    panic!("{name}: the base config was not read");
                };
                let same_key = shared_settings(old) == shared_settings(&head);
                match side {
                    Side::Out => {
                        assert!(effect.is_empty(), "{name}: {effect:?}");
                        assert!(same_key, "{name}: the shared settings changed");
                        assert_eq!(
                            plan,
                            plan_verify(root, &facts, old, affected, None),
                            "{name}: the plan changed"
                        );
                    }
                    Side::InAll => {
                        assert!(effect.everything.is_some(), "{name}: {effect:?}");
                        assert!(!same_key, "{name}: the shared settings did not change");
                    }
                    Side::InSteps(steps) => {
                        assert!(effect.everything.is_none(), "{name}: {effect:?}");
                        assert!(same_key, "{name}: the shared settings changed");
                        let found: Vec<(String, String)> = effect
                            .units
                            .iter()
                            .flat_map(|((folder, _), steps)| {
                                steps.iter().map(move |step| {
                                    (folder.clone().unwrap_or_default(), step.clone())
                                })
                            })
                            .collect();
                        let expected: Vec<(String, String)> = steps
                            .iter()
                            .map(|(folder, step)| ((*folder).to_owned(), (*step).to_owned()))
                            .collect();
                        assert_eq!(found, expected, "{name}");
                    }
                }
            }
        }
    }

    /// The settings each field of the config carries, by the names of the
    /// table. No `..`: a new field does not compile until it is named here,
    /// and the test below then asks for its row.
    fn settings_carried() -> Vec<&'static str> {
        let VerifyConfig {
            steps: _,
            fail_on_skipped: _,
            projects: _,
            timeout: _,
            idle_timeout: _,
            label_fixed: _,
            step_timeouts: _,
            project_timeouts: _,
            no_ci_variable: _,
            keep_running_when_service_down: _,
            stop_after_errors: _,
            base_budget: _,
            base_install: _,
            tamper: _,
            other_settings: _,
        } = VerifyConfig::default();
        let ProjectTimeouts { all: _, steps: _ } = ProjectTimeouts::default();
        let AgentLimits {
            max_turns: _,
            max_cost_usd: _,
            timeout_seconds: _,
        } = AgentLimits::default();
        vec![
            // steps
            "verify.<step>",
            "verify.<step>.command",
            "verify.<step>: false",
            // fail_on_skipped
            "verify.failOnSkipped",
            // projects
            "verify.projects.<folder>: false",
            "verify.projects.<folder>.<step>",
            "verify.projects.<folder>.<step>.command",
            "verify.projects.<folder>.<step>: false",
            "verify.projects.<folder>.<step>: false, with no command found",
            // timeout, idle_timeout, label_fixed, step_timeouts
            "verify.timeout",
            "verify.idleTimeout",
            "verify.labelFixed",
            "verify.<step>.timeout",
            // project_timeouts: all, steps
            "verify.projects.<folder>.timeout",
            "verify.projects.<folder>.<step>.timeout",
            // no_ci_variable, keep_running_when_service_down, stop_after_errors
            "verify.ci",
            "verify.stopWhenServiceDown",
            "verify.stopAfterErrors",
            // base_budget, base_install, tamper
            "verify.baseBudget",
            "verify.baseInstall",
            "verify.tamper",
            // other_settings
            "packs",
            "rules.<id>",
            "rules.<id>.status",
            "rules.<id>.options",
            "(unknown)",
            // Top-level keys that are in no key, and the agent limits.
            "$schema",
            "protectedPaths",
            "agent.maxTurns",
            "agent.maxCostUsd",
            "agent.timeoutSeconds",
        ]
    }

    #[test]
    fn the_table_names_every_setting_and_nothing_else() {
        let mut rows: Vec<&str> = table().iter().map(|(name, ..)| *name).collect();
        let mut carried = settings_carried();
        rows.sort_unstable();
        carried.sort_unstable();
        assert_eq!(rows, carried);
        // Every top-level key the config reader accepts has a row.
        for key in TOP_LEVEL_KEYS {
            assert!(
                rows.iter().any(|row| {
                    *row == key
                        || row.starts_with(&format!("{key}."))
                        || row.starts_with(&format!("{key}:"))
                }),
                "no row for the top-level key {key}"
            );
        }
        // Every key the verify reader accepts is in the table: its error
        // for an unknown key lists them all.
        let error = verify_settings(JSON_CONFIG, &json!({ "verify": { "zzz": 1 } }))
            .expect_err("an unknown verify key is refused");
        let listed = error
            .message()
            .split_once("other keys: ")
            .map(|(_, keys)| keys.trim_end_matches('.').to_owned())
            .expect("the error lists the keys");
        for key in listed.split(", ") {
            let row = if key == "projects" {
                "verify.projects.<folder>: false".to_owned()
            } else {
                format!("verify.{key}")
            };
            assert!(rows.contains(&row.as_str()), "no row for verify.{key}");
        }
    }

    fn git(root: &Path, args: &[&str]) {
        let mut full = vec![
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.autocrlf=false",
        ];
        full.extend_from_slice(args);
        let out = run_git(root, &full).unwrap();
        assert_eq!(out.code, 0, "git {args:?}: {}", out.stderr);
    }

    fn commit(root: &Path, message: &str) {
        git(root, &["add", "--all"]);
        git(root, &["commit", "--quiet", "--no-verify", "-m", message]);
    }

    #[test]
    fn the_config_of_a_commit_is_read_with_git_from_the_firescale_root() {
        use crate::testing::{Fixture, Track};
        // The firescale root is a folder inside a larger git repo, and the
        // repo's own root has another config that must not be read.
        let fixture = Fixture::new(
            &[
                (
                    "firescale.config.json",
                    "{ \"verify\": { \"ci\": false } }\n",
                ),
                ("service/a.py", "x = 1\n"),
            ],
            Track::NoGit,
        )
        .unwrap();
        let top = fixture.root();
        let root = top.join("service");
        git(top, &["init", "--quiet", "--initial-branch=main"]);
        commit(top, "no config in service");
        let defaults = BaseConfig::Settings(Box::new(settings(&json!({}))));
        assert_eq!(base_config(&root, "HEAD"), defaults);

        // With a byte order mark and CRLF line ends, as an editor on
        // Windows may write it.
        fs::write(
            root.join(JSON_CONFIG),
            "\u{feff}{\r\n  \"verify\": { \"timeout\": \"2h\", \"failOnSkipped\": true }\r\n}\r\n",
        )
        .unwrap();
        commit(top, "a config");
        let BaseConfig::Settings(read) = base_config(&root, "HEAD") else {
            panic!("the committed config was not read");
        };
        assert!(read.fail_on_skipped);
        assert!(!read.no_ci_variable);
        // The commit before still has none, whatever the working tree holds.
        assert_eq!(base_config(&root, "HEAD~1"), defaults);
        assert_eq!(
            comparison_commit(&root, None),
            comparison_commit(&root, Some("HEAD"))
        );
        assert!(comparison_commit(&root, Some("no-such-ref")).is_none());

        // A TypeScript config is never run for this, and a commit git does
        // not know gives no settings.
        fs::remove_file(root.join(JSON_CONFIG)).unwrap();
        fs::write(root.join(TS_CONFIG), "export default {};\n").unwrap();
        commit(top, "a ts config");
        for (commit, why) in [
            ("HEAD", "it is a firescale.config.ts"),
            ("no-such-ref", "git could not list the files of that commit"),
        ] {
            match base_config(&root, commit) {
                BaseConfig::Unreadable(reason) => assert!(reason.contains(why), "{reason}"),
                BaseConfig::Settings(_) => panic!("{commit} gave settings"),
            }
        }
    }

    #[test]
    fn a_base_config_this_version_cannot_read_affects_everything() {
        let head = settings(&json!({}));
        let plan = VerifyPlan::default();
        for (text, why) in [
            ("{ not json", "it is not valid JSON"),
            ("[]", "it does not hold an object"),
            (
                r#"{ "verify": { "retries": 2 } }"#,
                "unknown verify step \"retries\"",
            ),
            (
                r#"{ "verify": { "timeout": "soon" } }"#,
                "verify.timeout must be a time",
            ),
            (
                r#"{ "verify": { "test": { "retries": 2 } } }"#,
                "unknown key \"retries\" in verify.test",
            ),
            (
                r#"{ "verify": { "tamper": { "level": 1 } } }"#,
                "unknown key \"level\" in verify.tamper",
            ),
        ] {
            let base = base_config_from(Some(text));
            let effect = effect_between(&base, &head, |_| VerifyPlan::default(), &plan);
            let reason = effect.everything.expect(text);
            assert!(reason.contains(why), "{text}: {reason}");
            assert!(effect.units.is_empty());
        }
        // A top-level key this version does not know is one of the other
        // settings, so it is in the key and its change is named.
        let base = base_config_from(Some(r#"{ "future": { "x": 1 }, "packs": ["core"] }"#));
        let effect = effect_between(&base, &head, |_| VerifyPlan::default(), &plan);
        assert_eq!(
            effect.everything.as_deref(),
            Some("a setting of firescale.config.json that every step's result can depend on changed (future (an unknown key))")
        );
    }

    /// After the review: an unknown top-level key of the base's config
    /// named like a resolved setting took that setting's place, so the
    /// change of the real one was not seen.
    #[test]
    fn an_unknown_key_named_like_a_setting_cannot_hide_that_setting() {
        let head = settings(&json!({}));
        let plan = VerifyPlan::default();
        for name in [
            "verify.ci",
            "verify.failOnSkipped",
            "verify.stopWhenServiceDown",
            "verify.stopAfterErrors",
        ] {
            // The real setting differs from the default; the look-alike
            // key holds the default's text.
            let real = match name {
                "verify.ci" => json!({ "ci": false }),
                "verify.failOnSkipped" => json!({ "failOnSkipped": true }),
                "verify.stopWhenServiceDown" => json!({ "stopWhenServiceDown": false }),
                _ => json!({ "stopAfterErrors": 25 }),
            };
            let default = match name {
                "verify.failOnSkipped" => json!(false),
                "verify.stopAfterErrors" => json!(crate::verify::DEFAULT_ERRORS_BEFORE_STOP),
                _ => json!(true),
            };
            let text = json!({ name: default, "verify": real }).to_string();
            let base = base_config_from(Some(&text));
            let BaseConfig::Settings(old) = &base else {
                panic!("{name}: the base config was not read");
            };
            assert_ne!(shared_settings(old), shared_settings(&head), "{name}");
            let effect = effect_between(&base, &head, |_| VerifyPlan::default(), &plan);
            let reason = effect.everything.expect(name);
            assert!(
                reason.contains(&format!("({name}, {name} (an unknown key))")),
                "{reason}"
            );
        }
    }

    /// After the second review: a config that git ignores is in no list of
    /// changed files, so its settings were never compared under
    /// `--affected` and in the stop hook.
    #[test]
    fn a_config_that_git_ignores_is_compared_though_no_list_names_it() {
        use crate::affected::{changed_files, Changed};
        use crate::config::load_verify_config;
        use crate::testing::{Fixture, Track};
        let package = |name: &str| {
            format!("{{\"name\":\"{name}\",\"scripts\":{{\"test\":\"node t.js\"}}}}\n")
        };
        let fixture = Fixture::new(
            &[
                (".gitignore", "firescale.config.json\n"),
                ("api/package.json", &package("api")),
                ("api/t.js", "\n"),
                ("web/package.json", &package("web")),
                ("web/t.js", "\n"),
            ],
            Track::NoGit,
        )
        .unwrap();
        let root = fixture.root();
        git(root, &["init", "--quiet", "--initial-branch=main"]);
        commit(root, "two projects");
        fs::write(root.join("api").join("t.js"), "// changed\n").unwrap();
        let effect_of = |config: &str| {
            fs::write(root.join(JSON_CONFIG), config).unwrap();
            let changed = changed_files(root, None).unwrap();
            // Git lists the other file only: it ignores the config.
            assert_eq!(
                changed,
                Changed::Files(BTreeSet::from(["api/t.js".to_owned()]))
            );
            let facts = detect_facts(root).unwrap();
            let head = load_verify_config(root).unwrap();
            let plan = plan_verify(root, &facts, &head, true, None);
            effect_for_affected(root, None, &changed, &facts, &head, &plan)
        };

        // A command of one project: that project's step is reached.
        let effect = effect_of(
            r#"{ "verify": { "projects": { "web": { "test": "node -e process.exit(1)" } } } }"#,
        )
        .expect("the ignored config is compared with the defaults");
        assert_eq!(effect.everything, None);
        assert_eq!(
            effect.units,
            BTreeMap::from([(
                (Some("web".to_owned()), Some("node".to_owned())),
                vec!["test".to_owned()]
            )])
        );
        // A setting every step shares: every project.
        for (config, name) in [
            (r#"{ "verify": { "ci": false } }"#, "verify.ci"),
            (
                r#"{ "verify": { "failOnSkipped": true } }"#,
                "verify.failOnSkipped",
            ),
        ] {
            let effect = effect_of(config).expect(name);
            let reason = effect.everything.expect(name);
            assert!(reason.contains(&format!("({name})")), "{reason}");
        }
        // Only time limits: compared, and nothing is reached.
        let effect = effect_of(r#"{ "verify": { "test": { "timeout": "2h" } } }"#)
            .expect("the ignored config is compared with the defaults");
        assert!(effect.is_empty(), "{effect:?}");

        // A `firescale.config.ts` that did not change keeps the whole-file
        // rule: nothing is compared, and it does not count as unreadable.
        fs::remove_file(root.join(JSON_CONFIG)).unwrap();
        fs::write(root.join(TS_CONFIG), "export default {};\n").unwrap();
        commit(root, "a ts config");
        fs::write(root.join("api").join("t.js"), "// changed again\n").unwrap();
        let changed = changed_files(root, None).unwrap();
        let facts = detect_facts(root).unwrap();
        let head = settings(&json!({}));
        let plan = plan_verify(root, &facts, &head, true, None);
        assert_eq!(
            effect_for_affected(root, None, &changed, &facts, &head, &plan),
            None
        );
    }

    /// No commit has a copy of an ignored config, so `--affected` compares
    /// its bytes with what the last run that passed wrote down.
    #[test]
    fn only_a_config_that_git_ignores_has_a_digest_and_a_record() {
        use crate::stepcache::{ignored_config_changed, remember_ignored_config};
        use crate::testing::{Fixture, Track};
        let fixture = Fixture::new(&[("a.py", "x = 1\n")], Track::NoGit).unwrap();
        let root = fixture.root();
        git(root, &["init", "--quiet", "--initial-branch=main"]);
        commit(root, "one file");
        // No config, and one git lists as untracked: nothing to remember.
        assert_eq!(ignored_config(root), None);
        fs::write(root.join(JSON_CONFIG), "{}\n").unwrap();
        assert_eq!(ignored_config(root), None);

        fs::write(root.join(".gitignore"), "firescale.config.json\n").unwrap();
        let first = ignored_config(root).expect("git ignores the config");
        // No run wrote anything down yet: it counts as changed.
        assert!(ignored_config_changed(root, &first));
        remember_ignored_config(root, &first);
        assert!(!ignored_config_changed(root, &first));
        fs::write(root.join(JSON_CONFIG), "{}").unwrap();
        let second = ignored_config(root).expect("git ignores the config");
        assert_ne!(second, first);
        assert!(ignored_config_changed(root, &second));
        // The record is firescale's own file, which git never lists.
        assert!(root
            .join(".firescale")
            .join("ignored-config.digest")
            .is_file());
        let listed = run_git(root, &["status", "--porcelain"]).unwrap().stdout;
        assert!(!listed.contains(".firescale"), "{listed}");

        // Tracked all the same (`git add --force`): git answers for it.
        git(root, &["add", "--force", JSON_CONFIG]);
        assert_eq!(ignored_config(root), None);
    }

    /// After the review: a step whose key holds the repo root can read the
    /// config as a file, so a change of its bytes runs that step's unit.
    #[test]
    fn the_units_that_can_read_the_config_as_a_file_are_marked() {
        use crate::graph::build_graph;
        use crate::testing::{Fixture, Track};
        use crate::verify::{AffectedPlan, AffectedUnit};
        let unit = |project: Option<&str>, kind: Option<&str>, affected: bool| AffectedUnit {
            project: project.map(str::to_owned),
            kind: kind.map(str::to_owned),
            affected,
            reason: "before".to_owned(),
        };
        let plan_of = |units: Vec<AffectedUnit>| VerifyPlan {
            affected: Some(AffectedPlan {
                units,
                ..AffectedPlan::default()
            }),
            ..VerifyPlan::default()
        };
        let marked = |plan: VerifyPlan| -> Vec<(bool, String)> {
            plan.affected
                .map(|affected| affected.units)
                .unwrap_or_default()
                .into_iter()
                .map(|unit| (unit.affected, unit.reason))
                .collect()
        };
        let fixture = Fixture::new(
            &[
                ("package.json", "{\"name\":\"app\"}\n"),
                ("web/package.json", "{\"name\":\"web\"}\n"),
                ("api/requirements.txt", "fastapi\n"),
                ("api/main.py", "app = None\n"),
            ],
            Track::NoGit,
        )
        .unwrap();
        let facts = detect_facts(fixture.root()).unwrap();
        let graph = build_graph(fixture.root(), &facts);

        // Only projects in folders below the root: none can read it.
        let mut plan = plan_of(vec![
            unit(Some("web"), Some("node"), false),
            unit(Some("api"), Some("python"), false),
        ]);
        assert!(!mark_file_readers(&mut plan, &graph, READ_AS_A_FILE_REASON));
        assert_eq!(
            marked(plan),
            [(false, "before".to_owned()), (false, "before".to_owned())]
        );

        // The root's own steps, a project at the root and a folder only
        // named in the config can; one that already runs keeps its reason.
        let mut plan = plan_of(vec![
            unit(None, None, false),
            unit(Some("."), Some("node"), true),
            unit(Some("tools"), None, false),
            unit(Some("web"), Some("node"), false),
        ]);
        assert!(mark_file_readers(&mut plan, &graph, READ_AS_A_FILE_REASON));
        assert_eq!(
            marked(plan),
            [
                (true, READ_AS_A_FILE_REASON.to_owned()),
                (true, "before".to_owned()),
                (true, READ_AS_A_FILE_REASON.to_owned()),
                (false, "before".to_owned()),
            ]
        );
        // A plan that is not an affected one has no unit to mark.
        assert!(!mark_file_readers(
            &mut VerifyPlan::default(),
            &graph,
            READ_AS_A_FILE_REASON
        ));
    }

    #[test]
    fn the_other_settings_do_not_depend_on_how_the_config_is_written() {
        let plain = other_settings(&json!({}));
        assert_eq!(plain, r#"{"packs":["core"],"rules":{}}"#);
        for same in [
            json!({ "packs": null, "rules": null }),
            json!({ "packs": ["core"], "rules": {} }),
            json!({ "$schema": "x", "protectedPaths": ["a/**"], "agent": { "maxTurns": 3 } }),
            json!({ "verify": { "ci": false, "test": "x" } }),
        ] {
            assert_eq!(other_settings(&same), plain, "{same}");
        }
        // Key order does not matter; a list's order does.
        let a: Value = serde_json::from_str(
            r#"{ "rules": { "b": "off", "a": { "options": { "y": 1, "x": 2 } } } }"#,
        )
        .unwrap();
        let b: Value = serde_json::from_str(
            r#"{ "rules": { "a": { "options": { "x": 2, "y": 1 } }, "b": "off" } }"#,
        )
        .unwrap();
        assert_eq!(other_settings(&a), other_settings(&b));
        assert_ne!(
            other_settings(&json!({ "packs": ["core", "agent-ready"] })),
            other_settings(&json!({ "packs": ["agent-ready", "core"] }))
        );
        // A value inside a rule's options is the rule's to read: null stays.
        assert_ne!(
            other_settings(&json!({ "rules": { "a": { "options": { "x": null } } } })),
            other_settings(&json!({ "rules": { "a": { "options": {} } } }))
        );
    }

    #[test]
    fn a_default_written_out_in_full_is_no_change() {
        let plain = shared_settings(&settings(&json!({})));
        let full = settings(&json!({
            "packs": ["core"],
            "verify": {
                "ci": true,
                "failOnSkipped": false,
                "stopWhenServiceDown": true,
                "stopAfterErrors": crate::verify::DEFAULT_ERRORS_BEFORE_STOP,
            }
        }));
        assert_eq!(shared_settings(&full), plain);
    }

    #[test]
    fn the_units_a_change_reaches_are_marked_with_the_steps_it_changes() {
        let mut effect = ConfigEffect::default();
        effect.units.insert(
            (Some("api".to_owned()), Some("python".to_owned())),
            vec!["lint".to_owned(), "typecheck".to_owned(), "test".to_owned()],
        );
        effect.units.insert((None, None), vec!["test".to_owned()]);
        assert!(effect.reaches(Some("api"), Some("python"), "lint"));
        assert!(!effect.reaches(Some("api"), Some("node"), "lint"));
        assert!(!effect.reaches(Some("web"), Some("node"), "lint"));
        assert!(effect.reaches(None, None, "test"));
        assert!(!effect.reaches(None, None, "lint"));
        let unit = |project: Option<&str>, kind: Option<&str>, affected: bool| {
            crate::verify::AffectedUnit {
                project: project.map(str::to_owned),
                kind: kind.map(str::to_owned),
                affected,
                reason: "before".to_owned(),
            }
        };
        let mut plan = VerifyPlan {
            affected: Some(crate::verify::AffectedPlan {
                units: vec![
                    unit(None, None, false),
                    unit(Some("api"), Some("python"), false),
                    unit(Some("web"), Some("node"), false),
                ],
                ..crate::verify::AffectedPlan::default()
            }),
            ..VerifyPlan::default()
        };
        effect.mark(&mut plan);
        let units = plan.affected.unwrap().units;
        assert_eq!(
            units
                .iter()
                .map(|unit| (unit.affected, unit.reason.as_str()))
                .collect::<Vec<_>>(),
            [
                (
                    true,
                    "firescale.config.json changes what its test step runs"
                ),
                (
                    true,
                    "firescale.config.json changes what its lint, typecheck and test steps run"
                ),
                (false, "before"),
            ]
        );
    }
}
