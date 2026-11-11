//! Running the selected rules and building the report.
//!
//! Built-in rules are looked up in the cache first; the ones that miss run in
//! parallel. Custom TypeScript rules run one after another through the single
//! Node host and are never cached, because their code can change without
//! firescale knowing.

use std::path::Path;
use std::thread;

use crate::cache::{Cache, RepoSnapshot};
use crate::config::{ConfiguredRule, RuleSource};
use crate::error::{Error, Result};
use crate::model::{Diagnostic, Status};
use crate::node::NodeHost;
use crate::report::{CheckReport, ReportedDiagnostic, RuleReport, Summary, CHECK_SCHEMA_VERSION};
use crate::rule::{check_rule, sort_diagnostics};

/// What happened to one rule with respect to the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheOutcome {
    Hit,
    Miss,
    /// The cache was off, or the rule cannot be cached.
    Skipped,
}

impl CacheOutcome {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hit => "hit",
            Self::Miss => "miss",
            Self::Skipped => "skip",
        }
    }
}

/// The report plus, for each rule that ran, how the cache was used.
pub struct CheckOutcome {
    pub report: CheckReport,
    pub cache: Vec<(String, CacheOutcome)>,
    /// Problems that did not stop the check, such as a cache that cannot be written.
    pub warnings: Vec<String>,
}

enum Planned {
    /// Served from the cache.
    Cached(Vec<Diagnostic>),
    /// Must run; `key` is where to store the result, when cacheable.
    Run { key: Option<String> },
}

/// Runs the rules and builds the report. `use_cache` turns the result cache on.
pub fn run_check(
    root: &Path,
    rules: &[ConfiguredRule<'_>],
    version: &str,
    host: &mut NodeHost,
    use_cache: bool,
) -> Result<CheckOutcome> {
    let rules: Vec<&ConfiguredRule<'_>> = rules
        .iter()
        .filter(|rule| rule.status != Status::Off)
        .collect();
    let cache = Cache::new(root);

    // Plan: which built-in rules can be answered from the cache.
    let cacheable = |rule: &ConfiguredRule<'_>| {
        use_cache && matches!(rule.source, RuleSource::Builtin(_)) && !rule.meta.inputs.is_empty()
    };
    let mut snapshot = if rules.iter().any(|rule| cacheable(rule)) {
        Some(RepoSnapshot::capture(root)?)
    } else {
        None
    };
    let mut plan = Vec::with_capacity(rules.len());
    for rule in &rules {
        let key = match (&mut snapshot, cacheable(rule)) {
            (Some(snapshot), true) => {
                Some(snapshot.rule_key(version, &rule.meta, &rule.options)?)
            }
            _ => None,
        };
        let cached = key.as_deref().and_then(|key| cache.get(&rule.meta.id, key));
        plan.push(match cached {
            Some(diagnostics) => Planned::Cached(diagnostics),
            None => Planned::Run { key },
        });
    }

    // Run the built-in rules that missed, in parallel.
    let mut fresh: Vec<Option<Result<Vec<Diagnostic>>>> = thread::scope(|scope| {
        let handles: Vec<_> = rules
            .iter()
            .zip(&plan)
            .map(|(rule, planned)| match (&rule.source, planned) {
                (RuleSource::Builtin(builtin), Planned::Run { .. }) => {
                    let builtin = *builtin;
                    let options = &rule.options;
                    Some(scope.spawn(move || check_rule(builtin, root, options)))
                }
                _ => None,
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle.map(|handle| {
                    handle.join().unwrap_or_else(|_| {
                        Err(Error::internal(
                            "A rule panicked while checking. Please report this.",
                        ))
                    })
                })
            })
            .collect()
    });

    let mut rule_reports = Vec::new();
    let mut diagnostics = Vec::new();
    let mut outcomes = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    for ((rule, planned), fresh) in rules.iter().zip(plan).zip(fresh.iter_mut()) {
        let (found, outcome) = match (planned, &rule.source) {
            (Planned::Cached(found), _) => (found, CacheOutcome::Hit),
            (Planned::Run { key }, RuleSource::Builtin(_)) => {
                let found = fresh.take().unwrap_or_else(|| Ok(Vec::new()))?;
                match key {
                    Some(key) => {
                        // The result is already known: a cache that cannot be
                        // written must not fail the check.
                        if let Err(error) = cache.put(&rule.meta.id, &key, &found) {
                            if warnings.is_empty() {
                                warnings.push(format!("warning: {}", error.message()));
                            }
                        }
                        (found, CacheOutcome::Miss)
                    }
                    None => (found, CacheOutcome::Skipped),
                }
            }
            (Planned::Run { .. }, RuleSource::Custom { pack, .. }) => {
                let mut found = host.check_rule(pack, &rule.meta.id, &rule.options)?;
                sort_diagnostics(&mut found);
                (found, CacheOutcome::Skipped)
            }
        };
        outcomes.push((rule.meta.id.clone(), outcome));
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
    let report = CheckReport {
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
    };
    Ok(CheckOutcome {
        report,
        cache: outcomes,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::{json, Map};

    use super::*;
    use crate::model::{Category, Finding, Fixable, RuleMeta, Severity};
    use crate::rule::{BuiltinRule, RuleContext};
    use crate::testing::{Fixture, Track};

    /// A rule that flags every tracked file with a given extension and counts its runs.
    struct CountingRule {
        id: &'static str,
        extension: &'static str,
        runs: AtomicUsize,
    }

    impl CountingRule {
        fn new(id: &'static str, extension: &'static str) -> Self {
            Self {
                id,
                extension,
                runs: AtomicUsize::new(0),
            }
        }

        fn runs(&self) -> usize {
            self.runs.load(Ordering::SeqCst)
        }
    }

    impl BuiltinRule for CountingRule {
        fn meta(&self) -> RuleMeta {
            RuleMeta {
                id: self.id.to_owned(),
                severity: Severity::Low,
                category: Category::Consistency,
                description: "test rule".to_owned(),
                inputs: vec![format!("**/*.{}", self.extension)],
                has_fix: false,
            }
        }

        fn skill(&self) -> &'static str {
            ""
        }

        fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
            self.runs.fetch_add(1, Ordering::SeqCst);
            let suffix = format!(".{}", self.extension);
            Ok(ctx
                .git()
                .tracked_files()?
                .into_iter()
                .filter(|file| file.ends_with(&suffix))
                .map(|file| Finding {
                    message: format!(
                        "{file} has {} bytes",
                        ctx.read(&file).ok().flatten().map_or(0, |t| t.len())
                    ),
                    file,
                    range: None,
                    project: None,
                    fixable: Fixable::Manual,
                })
                .collect())
        }
    }

    fn configured<'a>(rule: &'a CountingRule, options: serde_json::Value) -> ConfiguredRule<'a> {
        ConfiguredRule {
            meta: rule.meta(),
            status: Status::Enforced,
            options: options.as_object().cloned().unwrap_or_else(Map::new),
            source: RuleSource::Builtin(rule),
        }
    }

    fn run(fixture: &Fixture, rules: &[ConfiguredRule<'_>], use_cache: bool) -> CheckOutcome {
        let mut host = NodeHost::new(fixture.root());
        let outcome = run_check(fixture.root(), rules, "0.0.0-test", &mut host, use_cache).unwrap();
        assert!(!host.started(), "built-in rules must not start Node");
        outcome
    }

    fn outcomes(outcome: &CheckOutcome) -> Vec<CacheOutcome> {
        outcome.cache.iter().map(|(_, outcome)| *outcome).collect()
    }

    fn fixture() -> Fixture {
        Fixture::new(
            &[
                ("a.md", "one\n"),
                ("docs/b.md", "two\n"),
                ("src/c.rs", "three\n"),
                ("other.txt", "x\n"),
            ],
            Track::All,
        )
        .unwrap()
    }

    #[test]
    fn a_second_run_with_no_changes_is_served_from_the_cache() {
        let fixture = fixture();
        let markdown = CountingRule::new("test/markdown", "md");
        let rules = [configured(&markdown, json!({}))];

        let first = run(&fixture, &rules, true);
        assert_eq!(outcomes(&first), [CacheOutcome::Miss]);
        assert_eq!(markdown.runs(), 1);

        let second = run(&fixture, &rules, true);
        assert_eq!(outcomes(&second), [CacheOutcome::Hit]);
        assert_eq!(markdown.runs(), 1, "the rule must not run again");
        assert_eq!(
            crate::report::format_json(&second.report),
            crate::report::format_json(&first.report),
            "a cached report must be identical to a fresh one"
        );
        assert_eq!(second.report.summary.violations, 2);
    }

    #[test]
    fn changing_an_input_file_invalidates_only_that_rule() {
        let fixture = fixture();
        let markdown = CountingRule::new("test/markdown", "md");
        let rust = CountingRule::new("test/rust", "rs");
        let rules = [
            configured(&markdown, json!({})),
            configured(&rust, json!({})),
        ];

        run(&fixture, &rules, true);
        assert_eq!((markdown.runs(), rust.runs()), (1, 1));

        fixture.write("docs/b.md", "two, edited\n").unwrap();
        let after_edit = run(&fixture, &rules, true);
        assert_eq!(
            outcomes(&after_edit),
            [CacheOutcome::Miss, CacheOutcome::Hit]
        );
        assert_eq!((markdown.runs(), rust.runs()), (2, 1));
        assert!(after_edit
            .report
            .diagnostics
            .iter()
            .any(|d| d.message == "docs/b.md has 12 bytes"));

        // A file that matches neither rule's inputs invalidates nothing.
        fixture.write("other.txt", "changed\n").unwrap();
        let unrelated = run(&fixture, &rules, true);
        assert_eq!(outcomes(&unrelated), [CacheOutcome::Hit, CacheOutcome::Hit]);
        assert_eq!((markdown.runs(), rust.runs()), (2, 1));
    }

    #[test]
    fn new_deleted_and_newly_tracked_inputs_invalidate() {
        let fixture = fixture();
        let markdown = CountingRule::new("test/markdown", "md");
        let rules = [configured(&markdown, json!({}))];
        run(&fixture, &rules, true);

        // A new file that is not tracked yet still changes the fingerprint.
        fixture.write("new.md", "new\n").unwrap();
        assert_eq!(outcomes(&run(&fixture, &rules, true)), [CacheOutcome::Miss]);
        assert_eq!(outcomes(&run(&fixture, &rules, true)), [CacheOutcome::Hit]);

        // Tracking it changes the git index but not its content.
        fixture.track(&["new.md"]).unwrap();
        let tracked = run(&fixture, &rules, true);
        assert_eq!(outcomes(&tracked), [CacheOutcome::Miss]);
        assert_eq!(tracked.report.summary.violations, 3);

        std::fs::remove_file(fixture.root().join("a.md")).unwrap();
        assert_eq!(outcomes(&run(&fixture, &rules, true)), [CacheOutcome::Miss]);
    }

    #[test]
    fn options_and_version_are_part_of_the_key() {
        let fixture = fixture();
        let markdown = CountingRule::new("test/markdown", "md");
        run(&fixture, &[configured(&markdown, json!({}))], true);
        let changed = run(
            &fixture,
            &[configured(&markdown, json!({ "strict": true }))],
            true,
        );
        assert_eq!(outcomes(&changed), [CacheOutcome::Miss]);

        let rules = [configured(&markdown, json!({ "strict": true }))];
        let mut host = NodeHost::new(fixture.root());
        let other_version = run_check(fixture.root(), &rules, "9.9.9", &mut host, true).unwrap();
        assert_eq!(outcomes(&other_version), [CacheOutcome::Miss]);
    }

    #[test]
    fn no_cache_neither_reads_nor_writes() {
        let fixture = fixture();
        let markdown = CountingRule::new("test/markdown", "md");
        let rules = [configured(&markdown, json!({}))];
        assert_eq!(
            outcomes(&run(&fixture, &rules, false)),
            [CacheOutcome::Skipped]
        );
        assert!(!fixture.root().join(".firescale").exists());
        run(&fixture, &rules, true);
        assert_eq!(
            outcomes(&run(&fixture, &rules, false)),
            [CacheOutcome::Skipped]
        );
        assert_eq!(markdown.runs(), 3);
    }

    #[test]
    fn the_cache_folder_ignores_itself_and_survives_corruption() {
        let fixture = fixture();
        let markdown = CountingRule::new("test/markdown", "md");
        let rules = [configured(&markdown, json!({}))];
        run(&fixture, &rules, true);
        assert_eq!(
            fixture
                .read(".firescale/.gitignore")
                .map(|text| text.contains("\n*\n")),
            Some(true)
        );

        fixture
            .write(".firescale/cache/test__markdown.json", "not json")
            .unwrap();
        let recovered = run(&fixture, &rules, true);
        assert_eq!(outcomes(&recovered), [CacheOutcome::Miss]);
        assert_eq!(outcomes(&run(&fixture, &rules, true)), [CacheOutcome::Hit]);
    }

    #[test]
    fn a_cache_that_cannot_be_written_is_a_warning_not_a_failure() {
        let fixture = fixture();
        // A plain file where the cache folder should be makes every write fail, on every OS.
        fixture.write(".firescale", "not a folder\n").unwrap();
        let markdown = CountingRule::new("test/markdown", "md");
        let outcome = run(&fixture, &[configured(&markdown, json!({}))], true);
        assert_eq!(outcome.report.summary.violations, 2);
        assert_eq!(outcome.warnings.len(), 1);
        assert!(
            outcome.warnings[0].starts_with("warning: Could not write the cache"),
            "{}",
            outcome.warnings[0]
        );
        assert!(outcome.warnings[0].contains("--no-cache"));
    }

    #[test]
    fn rules_run_in_parallel_and_report_in_order() {
        let fixture = fixture();
        let rules_owned: Vec<CountingRule> = ["test/a", "test/b", "test/c", "test/d"]
            .into_iter()
            .map(|id| CountingRule::new(id, "md"))
            .collect();
        let rules: Vec<ConfiguredRule<'_>> = rules_owned
            .iter()
            .map(|rule| configured(rule, json!({})))
            .collect();
        let outcome = run(&fixture, &rules, true);
        let ids: Vec<&str> = outcome
            .report
            .rules
            .iter()
            .map(|rule| rule.id.as_str())
            .collect();
        assert_eq!(ids, ["test/a", "test/b", "test/c", "test/d"]);
        assert_eq!(outcome.report.summary.violations, 8);
    }
}
