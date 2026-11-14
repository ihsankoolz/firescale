//! `expo/public-env-secret`: no `EXPO_PUBLIC_` variable is named like a
//! secret. Expo's docs: "Do not store sensitive info, such as private keys,
//! in `EXPO_PUBLIC_` variables. These variables will be visible in
//! plain-text in your compiled application."

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, LineRange, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{applies, finding};

#[cfg(test)]
mod tests;

pub const ID: &str = "expo/public-env-secret";

pub struct PublicEnvSecret;

/// Words that mark a value as a secret. Not `TOKEN` or `KEY` alone: many
/// public client tokens and keys (maps, analytics, Supabase's publishable
/// key) are meant to ship.
const SECRET_WORDS: [&str; 5] = [
    "SECRET",
    "PRIVATE",
    "PASSWORD",
    "SERVICE_ROLE",
    "SERVICE_KEY",
];

const EXTENSIONS: [&str; 8] = ["js", "jsx", "ts", "tsx", "mjs", "cjs", "mts", "cts"];

fn scanned(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.starts_with(".env")
        || matches!(name, "app.json" | "eas.json")
        || name
            .rsplit_once('.')
            .is_some_and(|(_, ext)| EXTENSIONS.contains(&ext))
}

/// An `EXPO_PUBLIC_` name that reads as a secret.
#[must_use]
pub fn is_public_secret(name: &str) -> bool {
    name.strip_prefix("EXPO_PUBLIC_")
        .is_some_and(|rest| SECRET_WORDS.iter().any(|word| rest.contains(word)))
}

fn first(text: &str) -> Option<(usize, String)> {
    for (index, line) in text.lines().enumerate() {
        for word in line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
            if is_public_secret(word) {
                return Some((index + 1, word.to_owned()));
            }
        }
    }
    None
}

impl BuiltinRule for PublicEnvSecret {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::High,
            category: Category::Security,
            description: "No EXPO_PUBLIC_ variable is named like a secret (SECRET, PRIVATE, PASSWORD, SERVICE_ROLE): Expo inlines them into the app.".to_owned(),
            inputs: vec![
                "**/.env*".to_owned(),
                "**/*.{js,jsx,ts,tsx,mjs,cjs,mts,cts}".to_owned(),
                "**/app.json".to_owned(),
                "**/eas.json".to_owned(),
                "package.json".to_owned(),
            ],
            has_fix: false,
            after: Vec::new(),
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        if !applies(ctx)? {
            return Ok(Vec::new());
        }
        let mut findings = Vec::new();
        for file in ctx.git().tracked_files().unwrap_or_default() {
            if !scanned(&file) {
                continue;
            }
            let too_big = ctx
                .root()
                .join(&file)
                .metadata()
                .is_ok_and(|m| m.len() > 1_000_000);
            if too_big {
                continue;
            }
            let Some(text) = std::fs::read(ctx.root().join(&file))
                .ok()
                .map(|b| String::from_utf8_lossy(&b).into_owned())
            else {
                continue;
            };
            if let Some((line, name)) = first(&text) {
                let number = u32::try_from(line).unwrap_or(u32::MAX);
                let mut found = finding(
                    &file,
                    format!("{file} line {line} names {name}: Expo inlines every EXPO_PUBLIC_ variable into the app, where anyone can read it. Keep secrets on a server (an API route, an Edge Function) and rotate this one if it was ever shipped."),
                    Fixable::Manual,
                );
                found.range = Some(LineRange {
                    start_line: number,
                    end_line: number,
                });
                findings.push(found);
            }
        }
        Ok(findings)
    }

    fn fix(&self, _ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        Ok(FixResult::default())
    }
}
