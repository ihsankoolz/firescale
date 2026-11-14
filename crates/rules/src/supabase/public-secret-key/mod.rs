//! `supabase/public-secret-key`: no Supabase secret or service-role key is
//! named with a prefix that ships it to the browser or the app bundle
//! (`NEXT_PUBLIC_`, `VITE_`, `EXPO_PUBLIC_`, `PUBLIC_`, …). Supabase's docs:
//! a secret key "bypasses every Row Level Security policy you have. Never
//! put one in a browser, a shipped application, or source control", and
//! "Server-only. Never prefix these, or your bundler will ship the key."

use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, LineRange, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{applies, finding};

#[cfg(test)]
mod tests;

pub const ID: &str = "supabase/public-secret-key";

pub struct PublicSecretKey;

/// Prefixes that frameworks inline into client bundles.
const PUBLIC_PREFIXES: [&str; 7] = [
    "NEXT_PUBLIC_",
    "VITE_",
    "EXPO_PUBLIC_",
    "PUBLIC_",
    "NUXT_PUBLIC_",
    "REACT_APP_",
    "GATSBY_",
];

/// Files that name env variables: source, env templates and app configs.
const EXTENSIONS: [&str; 11] = [
    "js", "jsx", "ts", "tsx", "mjs", "cjs", "mts", "cts", "vue", "svelte", "astro",
];

fn scanned(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    if name.starts_with(".env") {
        return true;
    }
    if matches!(name, "app.json" | "eas.json") {
        return true;
    }
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| EXTENSIONS.contains(&ext))
}

/// A name that ships a Supabase secret to the client.
#[must_use]
pub fn is_public_secret(name: &str) -> bool {
    PUBLIC_PREFIXES.iter().any(|p| name.starts_with(p))
        && name.contains("SUPABASE")
        && (name.contains("SECRET")
            || name.contains("SERVICE_ROLE")
            || name.contains("SERVICE_KEY"))
}

/// The first such name in a text, with its 1-based line.
fn first_public_secret(text: &str) -> Option<(usize, String)> {
    for (index, line) in text.lines().enumerate() {
        for word in line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
            if is_public_secret(word) {
                return Some((index + 1, word.to_owned()));
            }
        }
    }
    None
}

impl BuiltinRule for PublicSecretKey {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::High,
            category: Category::Security,
            description: "No Supabase secret or service-role key has a name that ships it to the client (NEXT_PUBLIC_, VITE_, EXPO_PUBLIC_, PUBLIC_, …).".to_owned(),
            inputs: super::inputs(&[
                "**/.env*",
                "**/*.{js,jsx,ts,tsx,mjs,cjs,mts,cts,vue,svelte,astro}",
                "**/app.json",
                "**/eas.json",
            ]),
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
        let files = ctx.git().tracked_files().unwrap_or_default();
        let mut findings = Vec::new();
        for file in files.iter().filter(|f| scanned(f)) {
            let too_big = ctx
                .root()
                .join(file)
                .metadata()
                .is_ok_and(|m| m.len() > 1_000_000);
            if too_big {
                continue;
            }
            let Ok(Some(text)) = ctx.read(file) else {
                continue;
            };
            if let Some((line, name)) = first_public_secret(&text) {
                let number = u32::try_from(line).unwrap_or(u32::MAX);
                let mut found = finding(
                    file,
                    format!("{file} line {line} names {name}: a Supabase secret key under a prefix that the framework puts in the client bundle, where anyone can read it and bypass row level security. Use it only in server code under a name without the prefix (for example SUPABASE_SECRET_KEY), and rotate the key in the dashboard if it was ever shipped."),
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
