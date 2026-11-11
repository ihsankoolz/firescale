//! Running the selected rules and building the report.

use std::path::Path;

use crate::config::{ConfiguredRule, RuleSource};
use crate::error::Result;
use crate::model::Status;
use crate::node::NodeHost;
use crate::report::{CheckReport, ReportedDiagnostic, RuleReport, Summary, CHECK_SCHEMA_VERSION};
use crate::rule::{check_rule, sort_diagnostics};

/// Runs each rule in order. Built-in rules run in this process; custom rules
/// go through the Node host.
pub fn run_check(
    root: &Path,
    rules: &[ConfiguredRule<'_>],
    version: &str,
    host: &mut NodeHost,
) -> Result<CheckReport> {
    let mut rule_reports = Vec::new();
    let mut diagnostics = Vec::new();

    for rule in rules.iter().filter(|rule| rule.status != Status::Off) {
        let found = match &rule.source {
            RuleSource::Builtin(builtin) => check_rule(*builtin, root, &rule.options)?,
            RuleSource::Custom { pack, .. } => {
                let mut found = host.check_rule(pack, &rule.meta.id, &rule.options)?;
                sort_diagnostics(&mut found);
                found
            }
        };
        rule_reports.push(RuleReport {
            id: rule.meta.id.clone(),
            status: rule.status,
            severity: rule.meta.severity,
            category: rule.meta.category,
            description: rule.meta.description.clone(),
            violations: found.len(),
        });
        diagnostics.extend(
            found
                .into_iter()
                .map(|d| ReportedDiagnostic::new(d, rule.status, rule.meta.severity)),
        );
    }

    let enforced_violations = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.status == Status::Enforced)
        .count();
    Ok(CheckReport {
        schema_version: CHECK_SCHEMA_VERSION,
        version: version.to_owned(),
        ok: enforced_violations == 0,
        summary: Summary {
            rules: rule_reports.len(),
            violations: diagnostics.len(),
            enforced_violations,
        },
        rules: rule_reports,
        diagnostics,
    })
}
