//! The check report and its two renderings: JSON (the documented, stable
//! format) and plain text for people.

use serde::Serialize;

use crate::model::{Category, Diagnostic, Fixable, LineRange, Severity, Status};

/// Version of the `fscale check --json` format. Bumped on any breaking change.
pub const CHECK_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize)]
pub struct RuleReport {
    pub id: String,
    pub status: Status,
    pub severity: Severity,
    pub category: Category,
    pub description: String,
    /// Number of diagnostics this rule produced.
    pub violations: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportedDiagnostic {
    pub rule_id: String,
    pub message: String,
    pub file: String,
    pub fixable: Fixable,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<LineRange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub status: Status,
    pub severity: Severity,
}

impl ReportedDiagnostic {
    #[must_use]
    pub fn new(diagnostic: Diagnostic, status: Status, severity: Severity) -> Self {
        Self {
            rule_id: diagnostic.rule_id,
            message: diagnostic.message,
            file: diagnostic.file,
            fixable: diagnostic.fixable,
            range: diagnostic.range,
            project: diagnostic.project,
            status,
            // A finding may carry its own severity; otherwise the rule's applies.
            severity: diagnostic.severity.unwrap_or(severity),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub rules: usize,
    pub violations: usize,
    pub enforced_violations: usize,
}

/// The `fscale check --json` document. Described by
/// `packages/cli/schemas/check-output.schema.json`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckReport {
    pub schema_version: u32,
    /// firescale version that produced the report.
    pub version: String,
    /// False when at least one enforced rule has a violation.
    pub ok: bool,
    pub summary: Summary,
    pub rules: Vec<RuleReport>,
    pub diagnostics: Vec<ReportedDiagnostic>,
}

fn plural(count: usize, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

fn fix_hint(fixable: Fixable) -> &'static str {
    match fixable {
        Fixable::Deterministic => "deterministic fix available",
        Fixable::Agent => "agent-fixable",
        Fixable::Manual => "manual fix",
    }
}

/// Renders a report for people. Plain text, no colour, forward-slash paths.
#[must_use]
pub fn format_human(report: &CheckReport) -> String {
    let mut lines: Vec<String> = Vec::new();
    for rule in report.rules.iter().filter(|rule| rule.violations > 0) {
        lines.push(format!(
            "{}  ({}, {})",
            rule.id,
            rule.status.as_str(),
            rule.severity.as_str()
        ));
        for diagnostic in report.diagnostics.iter().filter(|d| d.rule_id == rule.id) {
            let line = diagnostic
                .range
                .map_or_else(String::new, |range| format!(":{}", range.start_line));
            lines.push(format!("  {}{line}", diagnostic.file));
            // Named only when it differs from the rule's severity in the heading.
            let severity = if diagnostic.severity == rule.severity {
                String::new()
            } else {
                format!(", {} severity", diagnostic.severity.as_str())
            };
            lines.push(format!(
                "    {} [{}{severity}]",
                diagnostic.message,
                fix_hint(diagnostic.fixable)
            ));
        }
        lines.push(format!("  How to fix: fscale explain {}", rule.id));
        lines.push(String::new());
    }

    let summary = &report.summary;
    let checked = format!("{} checked", plural(summary.rules, "rule"));
    if summary.violations == 0 {
        lines.push(format!("OK: {checked}, no violations."));
    } else {
        let report_only = summary.violations - summary.enforced_violations;
        let verdict = if report.ok { "OK" } else { "FAILED" };
        lines.push(format!(
            "{verdict}: {checked}, {} ({} enforced, {report_only} report-only).",
            plural(summary.violations, "violation"),
            summary.enforced_violations
        ));
        if report.ok {
            lines.push(
                "Report-only violations do not fail the run. Set a rule to \"enforced\" to make it fail."
                    .to_owned(),
            );
        }
    }
    format!("{}\n", lines.join("\n"))
}

/// Renders a report as the documented JSON format.
#[must_use]
pub fn format_json(report: &CheckReport) -> String {
    // Serialising these plain structs cannot fail.
    let json = serde_json::to_string_pretty(report).unwrap_or_else(|_| "{}".to_owned());
    format!("{json}\n")
}
