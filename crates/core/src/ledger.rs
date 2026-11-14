//! `fscale ledger`: what the agent fix runs in `.firescale/ledger.jsonl`
//! add up to (M28, ADR 0048) — runs, passes, pass rate and reported cost,
//! per rule and per agent. A line that cannot be read is skipped and named,
//! never fatal: the ledger is appended to by many runs and can be cut short.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::agentrun::LedgerEntry;

/// Runs, passes and cost of one group of ledger lines.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tally {
    pub runs: usize,
    pub passed: usize,
    /// Passed runs over all runs, 0 to 1; absent with no runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pass_rate: Option<f64>,
    /// The sum of the costs the agents reported, in US dollars.
    pub cost_usd: f64,
    /// How many runs reported a cost: an agent that reports none adds
    /// nothing to `costUsd`.
    pub cost_reported_runs: usize,
    /// Failed runs by reason code.
    pub reasons: BTreeMap<String, usize>,
}

impl Tally {
    fn add(&mut self, entry: &LedgerEntry) {
        self.runs += 1;
        if entry.result == "pass" {
            self.passed += 1;
        } else {
            let reason = entry.reason.clone().unwrap_or_else(|| "unknown".to_owned());
            *self.reasons.entry(reason).or_default() += 1;
        }
        if let Some(cost) = entry.cost_usd {
            // Rounded to a millionth of a dollar, so sums of reported
            // costs print the same on every platform.
            self.cost_usd = ((self.cost_usd + cost) * 1e6).round() / 1e6;
            self.cost_reported_runs += 1;
        }
        #[allow(clippy::cast_precision_loss)]
        let rate = self.passed as f64 / self.runs as f64;
        self.pass_rate = Some(rate);
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub name: String,
    #[serde(flatten)]
    pub tally: Tally,
}

/// The whole ledger, summed.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerSummary {
    pub schema: u32,
    /// The ledger's path from the repo root, `/` separated.
    pub file: String,
    /// False when the repo has no ledger yet.
    pub found: bool,
    #[serde(flatten)]
    pub total: Tally,
    pub by_rule: Vec<Group>,
    pub by_agent: Vec<Group>,
    /// 1-based numbers of the lines that could not be read.
    pub damaged_lines: Vec<usize>,
}

/// One ledger line, or `None` when it is damaged: not JSON, a field
/// missing or of the wrong type, a result other than pass or fail, or a
/// cost that is not a number of 0 or more.
fn entry(line: &str) -> Option<LedgerEntry> {
    let entry: LedgerEntry = serde_json::from_str(line).ok()?;
    let cost_ok = entry.cost_usd.is_none_or(|c| c.is_finite() && c >= 0.0);
    let ok = entry.schema == 1
        && matches!(entry.result.as_str(), "pass" | "fail")
        && !entry.rule.is_empty()
        && !entry.agent.is_empty()
        && cost_ok;
    ok.then_some(entry)
}

/// Sums a ledger's text. `None` means there is no ledger file.
#[must_use]
pub fn summarise(file: &str, text: Option<&str>) -> LedgerSummary {
    let mut total = Tally::default();
    let mut by_rule: BTreeMap<String, Tally> = BTreeMap::new();
    let mut by_agent: BTreeMap<String, Tally> = BTreeMap::new();
    let mut damaged_lines = Vec::new();
    for (index, line) in text.unwrap_or_default().lines().enumerate() {
        let line = line.trim_start_matches('\u{feff}').trim();
        if line.is_empty() {
            continue;
        }
        let Some(entry) = entry(line) else {
            damaged_lines.push(index + 1);
            continue;
        };
        total.add(&entry);
        by_rule.entry(entry.rule.clone()).or_default().add(&entry);
        by_agent.entry(entry.agent.clone()).or_default().add(&entry);
    }
    let groups = |map: BTreeMap<String, Tally>| {
        map.into_iter()
            .map(|(name, tally)| Group { name, tally })
            .collect()
    };
    LedgerSummary {
        schema: 1,
        file: file.to_owned(),
        found: text.is_some(),
        total,
        by_rule: groups(by_rule),
        by_agent: groups(by_agent),
        damaged_lines,
    }
}

fn percent(tally: &Tally) -> String {
    tally
        .pass_rate
        .map_or_else(|| "-".to_owned(), |rate| format!("{:.0}%", rate * 100.0))
}

fn cost(tally: &Tally) -> String {
    if tally.cost_reported_runs == 0 {
        "-".to_owned()
    } else if tally.cost_reported_runs < tally.runs {
        format!(
            "${:.2} ({} of {} runs)",
            tally.cost_usd, tally.cost_reported_runs, tally.runs
        )
    } else {
        format!("${:.2}", tally.cost_usd)
    }
}

fn reasons(tally: &Tally) -> String {
    tally
        .reasons
        .iter()
        .map(|(reason, count)| format!("{reason} {count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn table(out: &mut String, heading: &str, groups: &[Group]) {
    let width = groups
        .iter()
        .map(|group| group.name.chars().count())
        .chain([heading.chars().count()])
        .max()
        .unwrap_or(0);
    out.push_str(&format!("\n{heading:<width$}  runs  passed  rate  cost\n"));
    for group in groups {
        let tally = &group.tally;
        let mut line = format!(
            "{:<width$}  {:>4}  {:>6}  {:>4}  {}",
            group.name,
            tally.runs,
            tally.passed,
            percent(tally),
            cost(tally),
        );
        if !tally.reasons.is_empty() {
            line.push_str(&format!("  failed: {}", reasons(tally)));
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
}

/// The summary as text.
#[must_use]
pub fn format_human(summary: &LedgerSummary) -> String {
    let mut out = String::new();
    if !summary.found {
        out.push_str(&format!(
            "No agent runs yet: {} does not exist. `fscale fix --agent <name>` adds one line per run.\n",
            summary.file
        ));
        return out;
    }
    let total = &summary.total;
    out.push_str(&format!(
        "{}: {} run{}, {} passed ({}), cost {}\n",
        summary.file,
        total.runs,
        if total.runs == 1 { "" } else { "s" },
        total.passed,
        percent(total),
        cost(total),
    ));
    if !summary.by_rule.is_empty() {
        table(&mut out, "rule", &summary.by_rule);
        table(&mut out, "agent", &summary.by_agent);
    }
    if !summary.damaged_lines.is_empty() {
        let lines: Vec<String> = summary
            .damaged_lines
            .iter()
            .map(ToString::to_string)
            .collect();
        out.push_str(&format!(
            "\nSkipped {count} line{s} that could not be read (line{s} {}). Each line must be one run as docs/schemas/ledger-entry.schema.json describes; delete or repair the line to count it.\n",
            lines.join(", "),
            count = lines.len(),
            s = if lines.len() == 1 { "" } else { "s" },
        ));
    }
    out.push_str("\nCost is what each agent reported; - means none did.\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASS: &str = r#"{"schema":1,"time":"2026-11-14T00:00:00Z","rule":"core/env-example","agent":"claude","result":"pass","filesChanged":1,"linesChanged":2,"durationMs":10,"costUsd":0.25,"turns":3}"#;
    const FAIL: &str = r#"{"schema":1,"time":"2026-11-14T00:00:00Z","rule":"core/readme-setup","agent":"codex","result":"fail","reason":"tamper","filesChanged":0,"linesChanged":0,"durationMs":10}"#;

    #[test]
    fn sums_per_rule_and_agent() {
        let text = format!("{PASS}\n{FAIL}\n{PASS}\n");
        let summary = summarise(".firescale/ledger.jsonl", Some(&text));
        assert_eq!(summary.total.runs, 3);
        assert_eq!(summary.total.passed, 2);
        assert!((summary.total.cost_usd - 0.5).abs() < 1e-9);
        assert_eq!(summary.total.cost_reported_runs, 2);
        assert_eq!(summary.by_rule.len(), 2);
        assert_eq!(summary.by_agent[1].name, "codex");
        assert_eq!(summary.by_agent[1].tally.reasons.get("tamper"), Some(&1));
    }

    #[test]
    fn skips_and_names_damaged_lines() {
        let negative = PASS.replace("0.25", "-1");
        let wrong = PASS.replace("\"pass\"", "\"maybe\"");
        let text = format!(
            "{PASS}\nnot json\n\n{negative}\n{wrong}\n{{\"schema\":1}}\n{}",
            &PASS[..40]
        );
        let summary = summarise("ledger", Some(&text));
        assert_eq!(summary.total.runs, 1);
        assert_eq!(summary.damaged_lines, vec![2, 4, 5, 6, 7]);
    }

    #[test]
    fn no_ledger_is_not_an_error() {
        let summary = summarise("ledger", None);
        assert!(!summary.found);
        assert!(format_human(&summary).starts_with("No agent runs yet"));
    }
}
