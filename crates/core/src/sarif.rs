//! SARIF 2.1.0 rendering of a check report (`fscale check --sarif`).

use serde_json::{json, Value};

use crate::model::{Severity, Status};
use crate::report::CheckReport;

const SCHEMA_URL: &str =
    "https://docs.oasis-open.org/sarif/sarif/v2.1.0/errata01/os/schemas/sarif-schema-2.1.0.json";
const INFORMATION_URI: &str = "https://github.com/ihsankoolz/firescale";

/// SARIF level for a finding: report-only rules are notes, enforced rules
/// follow their severity.
#[must_use]
pub fn level(status: Status, severity: Severity) -> &'static str {
    match (status, severity) {
        (Status::Enforced, Severity::Blocker | Severity::High) => "error",
        (Status::Enforced, Severity::Medium) => "warning",
        _ => "note",
    }
}

/// Percent-encodes a repo-relative path as a URI reference. Only unreserved
/// characters and `/` are kept as they are.
fn encode_uri(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for byte in path.replace('\\', "/").bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Renders a report as SARIF 2.1.0: pretty JSON, 2-space indent, trailing newline.
#[must_use]
pub fn format_sarif(report: &CheckReport) -> String {
    let rules: Vec<Value> = report
        .rules
        .iter()
        .map(|rule| {
            json!({
                "id": rule.id,
                "shortDescription": { "text": rule.description },
                "defaultConfiguration": { "level": level(rule.status, rule.severity) },
                "properties": {
                    "category": rule.category.as_str(),
                    "severity": rule.severity.as_str(),
                    "status": rule.status.as_str(),
                },
            })
        })
        .collect();

    let results: Vec<Value> = report
        .diagnostics
        .iter()
        .map(|d| {
            let mut physical = json!({
                "artifactLocation": { "uri": encode_uri(&d.file), "uriBaseId": "%SRCROOT%" },
            });
            if let (Some(range), Some(map)) = (d.range, physical.as_object_mut()) {
                map.insert(
                    "region".to_owned(),
                    json!({ "startLine": range.start_line, "endLine": range.end_line }),
                );
            }
            let mut result = json!({
                "ruleId": d.rule_id,
                "level": level(d.status, d.severity),
                "message": { "text": d.message },
                "locations": [{ "physicalLocation": physical }],
                "properties": { "fixable": d.fixable.as_str(), "status": d.status.as_str() },
            });
            if let (Some(index), Some(map)) = (
                report.rules.iter().position(|r| r.id == d.rule_id),
                result.as_object_mut(),
            ) {
                map.insert("ruleIndex".to_owned(), json!(index));
            }
            result
        })
        .collect();

    let doc = json!({
        "$schema": SCHEMA_URL,
        "version": "2.1.0",
        "runs": [{
            "tool": { "driver": {
                "name": "firescale",
                "version": report.version,
                "informationUri": INFORMATION_URI,
                "rules": rules,
            } },
            "results": results,
        }],
    });
    // Serialising a `Value` cannot fail.
    let text = serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".to_owned());
    format!("{text}\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Category, Fixable, LineRange};
    use crate::report::{ReportedDiagnostic, RuleReport, Summary};

    fn report(
        status: Status,
        severity: Severity,
        diagnostics: Vec<ReportedDiagnostic>,
    ) -> CheckReport {
        CheckReport {
            schema_version: 1,
            version: "1.2.3".to_owned(),
            ok: diagnostics.is_empty(),
            summary: Summary {
                rules: 1,
                violations: diagnostics.len(),
                enforced_violations: 0,
            },
            rules: vec![RuleReport {
                id: "core/rule".to_owned(),
                status,
                severity,
                category: Category::Security,
                description: "Something must hold.".to_owned(),
                violations: diagnostics.len(),
            }],
            diagnostics,
        }
    }

    fn diagnostic(
        file: &str,
        range: Option<LineRange>,
        status: Status,
        severity: Severity,
    ) -> ReportedDiagnostic {
        ReportedDiagnostic {
            rule_id: "core/rule".to_owned(),
            message: "bad".to_owned(),
            file: file.to_owned(),
            fixable: Fixable::Manual,
            range,
            project: None,
            status,
            severity,
        }
    }

    fn parse(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn empty_report_has_one_run_and_no_results() {
        let text = format_sarif(&report(Status::Enforced, Severity::High, vec![]));
        assert!(text.ends_with("}\n"));
        let value = parse(&text);
        assert_eq!(value["version"], "2.1.0");
        assert_eq!(value["runs"].as_array().unwrap().len(), 1);
        assert_eq!(value["runs"][0]["tool"]["driver"]["version"], "1.2.3");
        assert_eq!(
            value["runs"][0]["tool"]["driver"]["rules"][0]["id"],
            "core/rule"
        );
        assert_eq!(value["runs"][0]["results"], json!([]));
    }

    #[test]
    fn ranged_diagnostic_with_encoded_path() {
        let range = Some(LineRange {
            start_line: 3,
            end_line: 5,
        });
        let d = diagnostic(
            "dir/my file [é]#1%.env",
            range,
            Status::Enforced,
            Severity::High,
        );
        let value = parse(&format_sarif(&report(
            Status::Enforced,
            Severity::High,
            vec![d],
        )));
        let result = &value["runs"][0]["results"][0];
        let physical = &result["locations"][0]["physicalLocation"];
        assert_eq!(
            physical["artifactLocation"]["uri"],
            "dir/my%20file%20%5B%C3%A9%5D%231%25.env"
        );
        assert_eq!(physical["artifactLocation"]["uriBaseId"], "%SRCROOT%");
        assert_eq!(physical["region"], json!({ "startLine": 3, "endLine": 5 }));
        assert_eq!(result["ruleIndex"], 0);
        assert_eq!(result["level"], "error");
    }

    #[test]
    fn no_region_without_range() {
        let d = diagnostic("a.txt", None, Status::Enforced, Severity::Low);
        let value = parse(&format_sarif(&report(
            Status::Enforced,
            Severity::Low,
            vec![d],
        )));
        assert!(
            value["runs"][0]["results"][0]["locations"][0]["physicalLocation"]
                .get("region")
                .is_none()
        );
    }

    #[test]
    fn levels_follow_status_and_severity() {
        assert_eq!(level(Status::Evaluated, Severity::Blocker), "note");
        assert_eq!(level(Status::Enforced, Severity::Blocker), "error");
        assert_eq!(level(Status::Enforced, Severity::High), "error");
        assert_eq!(level(Status::Enforced, Severity::Medium), "warning");
        assert_eq!(level(Status::Enforced, Severity::Low), "note");
    }
}
