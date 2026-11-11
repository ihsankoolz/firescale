//! `core/single-lockfile`: exactly one lockfile exists, matching the declared package manager.

use firescale_core::model::{Category, Finding, Fixable, RuleMeta, Severity};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;
use serde_json::Value;

#[cfg(test)]
mod tests;

pub const ID: &str = "core/single-lockfile";

const PACKAGE_JSON: &str = "package.json";

/// Lockfile name and the package manager that writes it.
const LOCKFILES: [(&str, &str); 5] = [
    ("pnpm-lock.yaml", "pnpm"),
    ("package-lock.json", "npm"),
    ("yarn.lock", "yarn"),
    ("bun.lock", "bun"),
    ("bun.lockb", "bun"),
];

pub struct SingleLockfile;

/// The manager name from `packageManager` (`pnpm@9.1.0` -> `pnpm`).
fn declared_manager(package_json: &str) -> Option<String> {
    let value: Value = serde_json::from_str(package_json.trim_start_matches('\u{feff}')).ok()?;
    let declared = value.get("packageManager")?.as_str()?;
    let name = declared.split('@').next().unwrap_or(declared).trim();
    (!name.is_empty()).then(|| name.to_owned())
}

fn finding(file: &str, message: String) -> Finding {
    Finding {
        message,
        file: file.to_owned(),
        range: None,
        project: None,
        fixable: Fixable::Manual,
    }
}

impl BuiltinRule for SingleLockfile {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::High,
            category: Category::Reliability,
            description: "Exactly one lockfile exists, matching the declared package manager."
                .to_owned(),
            inputs: [
                PACKAGE_JSON,
                "pnpm-lock.yaml",
                "package-lock.json",
                "yarn.lock",
                "bun.lock",
                "bun.lockb",
            ]
            .iter()
            .map(|name| (*name).to_owned())
            .collect(),
            has_fix: false,
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        let Some(package_json) = ctx.read(PACKAGE_JSON)? else {
            return Ok(Vec::new());
        };
        let mut present = Vec::new();
        for (name, manager) in LOCKFILES {
            if root_has(ctx, name) {
                present.push((name, manager));
            }
        }
        let declared = declared_manager(&package_json);

        if present.is_empty() {
            let manager = declared.as_deref().unwrap_or("npm");
            return Ok(vec![finding(
                PACKAGE_JSON,
                format!("There is no lockfile. Run \"{manager} install\" and commit the lockfile it creates."),
            )]);
        }
        if present.len() > 1 {
            let names: Vec<&str> = present.iter().map(|(name, _)| *name).collect();
            return Ok(match declared.as_deref() {
                None => present
                    .iter()
                    .map(|(name, _)| {
                        finding(
                            name,
                            format!(
                                "\"{name}\" is one of several lockfiles ({}). Keep one, delete the others, and declare the manager in \"packageManager\" in package.json.",
                                names.join(", ")
                            ),
                        )
                    })
                    .collect(),
                Some(manager) => present
                    .iter()
                    .filter(|(_, owner)| *owner != manager)
                    .map(|(name, owner)| {
                        finding(
                            name,
                            format!(
                                "\"{name}\" is an extra lockfile: package.json declares {manager}, but this one belongs to {owner}. Delete it, or change \"packageManager\" if you meant to switch."
                            ),
                        )
                    })
                    .chain(
                        // Several lockfiles of the declared manager (bun.lock and bun.lockb).
                        present
                            .iter()
                            .filter(|(_, owner)| *owner == manager)
                            .skip(1)
                            .map(|(name, _)| {
                                finding(
                                    name,
                                    format!("\"{name}\" is an extra {manager} lockfile. Keep only one and delete this one."),
                                )
                            }),
                    )
                    .collect(),
            });
        }
        let (name, owner) = present[0];
        Ok(match declared {
            Some(manager) if manager != owner => vec![finding(
                name,
                format!(
                    "package.json declares {manager} but the lockfile \"{name}\" belongs to {owner}. Make them agree: change \"packageManager\", or run \"{manager} install\" and delete \"{name}\"."
                ),
            )],
            _ => Vec::new(),
        })
    }
}

fn root_has(ctx: &RuleContext, name: &str) -> bool {
    // `bun.lockb` is binary, so check that the file exists instead of reading it.
    ctx.root().join(name).is_file()
}
