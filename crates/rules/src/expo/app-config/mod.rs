//! `expo/app-config`: the app has an Expo config (`app.json`,
//! `app.config.json` or a dynamic `app.config.*`), and a static one names
//! the app (`expo.name`, `expo.slug`) and does not set `expo.sdkVersion`,
//! which `expo-doctor` flags.

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;
use serde_json::{json, Value};

use super::{any_exists, applies, create, finding, json_with, package_json};

#[cfg(test)]
mod tests;

pub const ID: &str = "expo/app-config";

pub struct AppConfig;

/// Dynamic configs, which Expo reads before a static one (docs,
/// workflow/configuration).
pub const DYNAMIC: [&str; 6] = [
    "app.config.ts",
    "app.config.js",
    "app.config.mts",
    "app.config.cts",
    "app.config.mjs",
    "app.config.cjs",
];

/// Static configs, in the order Expo reads them.
pub const STATIC: [&str; 2] = ["app.config.json", "app.json"];

/// The app.json of a new app: the blank TypeScript template's (SDK 58)
/// without the icon, adaptive icon and favicon, which name image files the
/// starter does not have (`expo-doctor` checks every image a config names).
#[must_use]
pub fn new_app_json(name: &str) -> String {
    let value = json!({
        "expo": {
            "name": name,
            "slug": name,
            "version": "1.0.0",
            "orientation": "portrait",
            "userInterfaceStyle": "light",
            "ios": { "supportsTablet": true },
            "android": { "predictiveBackGestureEnabled": false },
        }
    });
    format!(
        "{}\n",
        serde_json::to_string_pretty(&value).unwrap_or_default()
    )
}

/// The slug a config gets: package.json's name as a slug
/// (`^[a-zA-Z0-9_-]+$`), or `expo-app`.
fn slug(ctx: &RuleContext) -> Result<String> {
    let name = match package_json(ctx)? {
        Ok(Some(json)) => json
            .get("name")
            .and_then(Value::as_str)
            .map(|n| n.rsplit('/').next().unwrap_or(n).to_owned()),
        _ => None,
    };
    let slug: String = name
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_owned();
    Ok(if slug.is_empty() {
        "expo-app".to_owned()
    } else {
        slug
    })
}

/// The static config Expo reads, with its parsed `expo` object.
fn static_config(ctx: &RuleContext) -> Result<Option<(&'static str, Option<Value>)>> {
    for name in STATIC {
        if let Some(text) = ctx.read(name)? {
            let parsed = serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')).ok();
            return Ok(Some((name, parsed)));
        }
    }
    Ok(None)
}

/// The keys `{ "expo": { … } }` lacks, as an object to merge.
fn missing_names(config: &Value, slug: &str) -> Option<Value> {
    let expo = config.get("expo")?;
    let mut add = serde_json::Map::new();
    if expo.get("name").is_none() {
        add.insert("name".to_owned(), json!(slug));
    }
    if expo.get("slug").is_none() {
        add.insert("slug".to_owned(), json!(slug));
    }
    (!add.is_empty()).then(|| json!({ "expo": add }))
}

impl BuiltinRule for AppConfig {
    fn meta(&self) -> RuleMeta {
        let mut inputs: Vec<String> = DYNAMIC
            .iter()
            .chain(STATIC.iter())
            .map(|n| (*n).to_owned())
            .collect();
        inputs.push("package.json".to_owned());
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::High,
            category: Category::Reliability,
            description: "An Expo app config exists; a static one sets expo.name and expo.slug and not expo.sdkVersion.".to_owned(),
            inputs,
            has_fix: true,
            after: vec![super::package_json::ID.to_owned()],
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        if !applies(ctx)? {
            return Ok(Vec::new());
        }
        let dynamic = any_exists(ctx, &DYNAMIC);
        let Some((file, parsed)) = static_config(ctx)? else {
            if dynamic {
                return Ok(Vec::new());
            }
            return Ok(vec![finding(
                "app.json",
                "No Expo app config (app.json or app.config.*). `fscale sync` creates app.json with the app's name and slug and the template's settings, without image files.".to_owned(),
                Fixable::Deterministic,
            )]);
        };
        // A dynamic config may build on the static one, or replace it: its
        // names are its own business.
        if dynamic {
            return Ok(Vec::new());
        }
        let Some(config) = parsed else {
            return Ok(vec![finding(
                file,
                format!("{file} is not valid JSON, so Expo cannot read the app's config. Fix it by hand."),
                Fixable::Manual,
            )]);
        };
        let mut findings = Vec::new();
        if missing_names(&config, &slug(ctx)?).is_some() {
            findings.push(finding(
                file,
                format!("{file} does not set expo.name and expo.slug, which name the app and its project. `fscale sync` adds them from package.json's name."),
                Fixable::Deterministic,
            ));
        }
        if config
            .get("expo")
            .and_then(|e| e.get("sdkVersion"))
            .is_some()
        {
            findings.push(finding(
                file,
                format!("{file} sets expo.sdkVersion. The SDK comes from the installed expo package; a stale value here is what `expo-doctor` warns about. Remove the key."),
                Fixable::Agent,
            ));
        }
        Ok(findings)
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let mut result = FixResult::default();
        if !applies(ctx)? || any_exists(ctx, &DYNAMIC) {
            return Ok(result);
        }
        let slug = slug(ctx)?;
        match static_config(ctx)? {
            None => create(ctx, "app.json", &new_app_json(&slug), &mut result)?,
            Some((file, Some(config))) => {
                if let Some(add) = missing_names(&config, &slug) {
                    if let Some(updated) = json_with(ctx, file, &add)? {
                        ctx.write(file, &updated)?;
                        result.changed_files.push(file.to_owned());
                    }
                }
            }
            Some((_, None)) => {}
        }
        Ok(result)
    }
}
