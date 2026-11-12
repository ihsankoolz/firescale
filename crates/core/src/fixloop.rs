//! The fix loop: run deterministic fixes in order until nothing is left to
//! fix (SPEC §15, ADR 0018).
//!
//! `fscale fix` uses it today. `init`, `adopt` and `sync` are the same loop.
//!
//! One pass runs every rule's fix once, in an order that respects the rules'
//! `after` declarations. Passes repeat until a pass finds no deterministic
//! violation. The loop refuses to go on, with an error that names the rules
//! and files, when it cannot converge:
//!
//! - a pass changes nothing while a rule still reports deterministic violations,
//! - a rule writes the same content to the same file a second time, which
//!   means another fix undid it in between,
//! - the pass limit is reached.

use std::collections::{BTreeMap, BTreeSet};

use crate::error::{Error, Result};
use crate::rule::{Change, FixOutcome};

/// How many passes the loop makes before it gives up. Rules that depend on
/// each other need one pass per link of the chain only when a dependency is
/// not declared with `after`; ten is far more than any real chain.
pub const MAX_PASSES: usize = 10;

/// What the loop needs to know about a rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopRule {
    pub id: String,
    /// Ids of rules whose fixes must run before this one's.
    pub after: Vec<String>,
}

/// One change, with the rule that made it and the pass it was made in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopChange {
    /// 1-based.
    pub pass: usize,
    pub rule: String,
    pub change: Change,
}

/// What the loop did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoopReport {
    /// Passes made, including the last one, which found nothing to do.
    pub passes: usize,
    /// Every change, in the order it was made.
    pub changes: Vec<LoopChange>,
    /// `(rule, note)` for every note a fix returned.
    pub notes: Vec<(String, String)>,
}

/// The loop stopped without converging, or a fix failed. `report` holds what
/// had been done by then, which in a real run is already on disk.
#[derive(Debug)]
pub struct LoopFailure {
    pub error: Error,
    pub report: LoopReport,
}

fn quoted(ids: &[&str]) -> String {
    ids.iter()
        .map(|id| format!("\"{id}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Works out the order in which fixes run.
///
/// `enabled` is every rule the config turns on; `known` is every rule id that
/// exists, enabled or not. An `after` entry that names a rule which exists but
/// is not enabled places no constraint. An entry that names no rule at all is
/// an error, and so is a cycle. Both are found before any fix runs.
///
/// The order is stable: among rules that are free to run, the lowest id goes
/// first. Returns indices into `enabled`.
pub fn plan_order(enabled: &[LoopRule], known: &BTreeSet<String>) -> Result<Vec<usize>> {
    let index_of: BTreeMap<&str, usize> = enabled
        .iter()
        .enumerate()
        .map(|(index, rule)| (rule.id.as_str(), index))
        .collect();

    for rule in enabled {
        for dependency in &rule.after {
            if dependency == &rule.id {
                return Err(Error::user(format!(
                    "Rule \"{}\" lists itself in \"after\". Remove it from the rule's \"after\" list.",
                    rule.id
                )));
            }
            if !known.contains(dependency) && !index_of.contains_key(dependency.as_str()) {
                return Err(Error::user(format!(
                    "Rule \"{}\" must run after \"{dependency}\", but no rule has that id. Fix the id in the rule's \"after\" list.",
                    rule.id
                )));
            }
        }
    }

    // Kahn's algorithm, always taking the lowest id that is ready.
    let mut waiting_on: Vec<BTreeSet<usize>> = enabled
        .iter()
        .map(|rule| {
            rule.after
                .iter()
                .filter_map(|dependency| index_of.get(dependency.as_str()).copied())
                .collect()
        })
        .collect();
    let mut ready: BTreeSet<(&str, usize)> = enabled
        .iter()
        .enumerate()
        .filter(|(index, _)| waiting_on[*index].is_empty())
        .map(|(index, rule)| (rule.id.as_str(), index))
        .collect();
    let mut placed = vec![false; enabled.len()];
    let mut order = Vec::with_capacity(enabled.len());
    while let Some(next) = ready.iter().next().copied() {
        ready.remove(&next);
        let (_, done) = next;
        placed[done] = true;
        order.push(done);
        for (index, waiting) in waiting_on.iter_mut().enumerate() {
            if waiting.remove(&done) && waiting.is_empty() && !placed[index] {
                ready.insert((enabled[index].id.as_str(), index));
            }
        }
    }

    if order.len() < enabled.len() {
        return Err(Error::user(format!(
            "These rules must each run after another of them, so there is no order to fix them in: {}. Remove one of the \"after\" entries to break the cycle.",
            cycle_path(enabled, &placed, &index_of)
        )));
    }
    Ok(order)
}

/// Describes one cycle among the rules that could not be placed, as
/// `"a" -> "b" -> "a"`.
fn cycle_path(enabled: &[LoopRule], placed: &[bool], index_of: &BTreeMap<&str, usize>) -> String {
    let unplaced_dependency = |index: usize| {
        enabled[index]
            .after
            .iter()
            .filter_map(|dependency| index_of.get(dependency.as_str()).copied())
            .find(|dependency| !placed[*dependency])
    };
    // Every unplaced rule waits on another unplaced rule, so walking those
    // links from the lowest id must come back to a rule already seen.
    let Some(start) = (0..enabled.len())
        .filter(|index| !placed[*index])
        .min_by_key(|index| enabled[*index].id.as_str())
    else {
        return String::new();
    };
    let mut path = vec![start];
    let mut current = start;
    while let Some(next) = unplaced_dependency(current) {
        if let Some(position) = path.iter().position(|seen| *seen == next) {
            let mut cycle: Vec<&str> = path[position..]
                .iter()
                .map(|index| enabled[*index].id.as_str())
                .collect();
            cycle.push(enabled[next].id.as_str());
            // The links were followed from a rule to what it waits on, so
            // reversed they read in the order the fixes would have to run.
            cycle.reverse();
            return cycle
                .iter()
                .map(|id| format!("\"{id}\""))
                .collect::<Vec<_>>()
                .join(" -> ");
        }
        path.push(next);
        current = next;
    }
    quoted(&[enabled[start].id.as_str()])
}

fn file_of(change: &Change) -> &str {
    match change {
        Change::Write { file, .. } | Change::Untrack { file } | Change::Track { file } => file,
    }
}

/// Runs the loop over `rules`, which must already be in the order from
/// `plan_order`. `fix` runs one rule's deterministic fix and says what it did.
///
/// Stops successfully after the first pass in which no rule has a
/// deterministic violation left.
pub fn run_fix_loop(
    rules: &[&LoopRule],
    max_passes: usize,
    mut fix: impl FnMut(&LoopRule) -> Result<FixOutcome>,
) -> std::result::Result<LoopReport, LoopFailure> {
    let mut report = LoopReport::default();
    // Rules whose fix changed nothing in the pass before, though they had
    // violations to repair.
    let mut stuck_before: Vec<String> = Vec::new();

    for pass in 1..=max_passes {
        report.passes = pass;
        let mut stuck: Vec<String> = Vec::new();
        let mut changed_in_pass: Vec<(&str, String)> = Vec::new();

        for rule in rules {
            let outcome = match fix(rule) {
                Ok(outcome) => outcome,
                Err(error) => return Err(LoopFailure { error, report }),
            };
            if outcome.attempted > 0 && outcome.changes.is_empty() {
                stuck.push(rule.id.clone());
            }
            for note in outcome.result.notes {
                report.notes.push((rule.id.clone(), note));
            }
            for change in outcome.changes {
                let file = file_of(&change).to_owned();
                changed_in_pass.push((&rule.id, file.clone()));
                let fight: Option<Vec<String>> = fighters(&report.changes, &rule.id, &change)
                    .map(|ids| ids.into_iter().map(str::to_owned).collect());
                report.changes.push(LoopChange {
                    pass,
                    rule: rule.id.clone(),
                    change,
                });
                if let Some(fighters) = fight {
                    let error = Error::user(format!(
                        "The fixes do not settle: in pass {pass}, {} wrote the same content to {file} that it had already written, because another fix changed the file in between. Rules that write {file}: {}. Turn one of them off, or change one fix so that both can be satisfied at once.",
                        quoted(&[rule.id.as_str()]),
                        quoted(&fighters.iter().map(String::as_str).collect::<Vec<_>>())
                    ));
                    return Err(LoopFailure { error, report });
                }
            }
        }

        if changed_in_pass.is_empty() {
            // A fix may repair things without reporting a change (a custom
            // fix that writes files by itself). So a rule only counts as
            // stuck when a second pass finds the same violations still there.
            let still: Vec<&str> = stuck
                .iter()
                .filter(|id| stuck_before.contains(id))
                .map(String::as_str)
                .collect();
            if stuck.is_empty() {
                return Ok(report);
            }
            if !still.is_empty() {
                let error = Error::user(format!(
                    "The fix of {} changed nothing, but the rule still reports violations it says its fix can repair. This is a bug in the rule: its fix does not do what its check asks for. Run `fscale check --rules {}` to see the violations, and fix them by hand.",
                    quoted(&still),
                    still[0]
                ));
                return Err(LoopFailure { error, report });
            }
        }

        if pass == max_passes {
            let rules_in_pass: BTreeSet<&str> = changed_in_pass
                .iter()
                .map(|(rule, _)| *rule)
                .chain(stuck.iter().map(String::as_str))
                .collect();
            let files_in_pass: BTreeSet<&str> = changed_in_pass
                .iter()
                .map(|(_, file)| file.as_str())
                .collect();
            let files = if files_in_pass.is_empty() {
                "none reported".to_owned()
            } else {
                files_in_pass.into_iter().collect::<Vec<_>>().join(", ")
            };
            let error = Error::user(format!(
                "The fixes did not settle after {max_passes} passes. Still changing files in the last pass: {} (files: {files}). A fix that is run twice must change nothing the second time; one of these does not. Turn the rule off, or repair its fix.",
                quoted(&rules_in_pass.into_iter().collect::<Vec<_>>()),
            ));
            return Err(LoopFailure { error, report });
        }
        stuck_before = stuck;
    }
    Ok(report)
}

/// Decides whether `change`, about to be made by `rule`, shows that fixes are
/// undoing each other: the rule writes content to a file that it has already
/// written there, and since then another rule has written the same file.
/// Returns the rules that write the file.
///
/// A rule that rewrites its own file, with nobody else touching it, is not a
/// fight. If it never settles, the pass limit stops it.
fn fighters<'a>(earlier: &'a [LoopChange], rule: &'a str, change: &Change) -> Option<Vec<&'a str>> {
    let Change::Write { file, after, .. } = change else {
        return None;
    };
    let writes_of_file = || {
        earlier.iter().filter_map(move |entry| match &entry.change {
            Change::Write {
                file: other, after, ..
            } if other == file => Some((entry.rule.as_str(), after)),
            _ => None,
        })
    };
    let last_same = writes_of_file()
        .enumerate()
        .filter(|(_, (writer, content))| *writer == rule && *content == after)
        .map(|(position, _)| position)
        .last()?;
    let others_since: BTreeSet<&str> = writes_of_file()
        .skip(last_same + 1)
        .map(|(writer, _)| writer)
        .filter(|writer| *writer != rule)
        .collect();
    if others_since.is_empty() {
        return None;
    }
    let mut all: BTreeSet<&str> = writes_of_file().map(|(writer, _)| writer).collect();
    all.insert(rule);
    Some(all.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FixResult;

    fn rule(id: &str, after: &[&str]) -> LoopRule {
        LoopRule {
            id: id.to_owned(),
            after: after.iter().map(|id| (*id).to_owned()).collect(),
        }
    }

    fn known(rules: &[LoopRule], extra: &[&str]) -> BTreeSet<String> {
        rules
            .iter()
            .map(|rule| rule.id.clone())
            .chain(extra.iter().map(|id| (*id).to_owned()))
            .collect()
    }

    fn ordered(rules: &[LoopRule]) -> Vec<&str> {
        plan_order(rules, &known(rules, &[]))
            .unwrap()
            .into_iter()
            .map(|index| rules[index].id.as_str())
            .collect()
    }

    fn write(file: &str, before: Option<&str>, after: &str) -> Change {
        Change::Write {
            file: file.to_owned(),
            before: before.map(str::to_owned),
            after: after.to_owned(),
        }
    }

    fn outcome(changes: Vec<Change>, attempted: usize) -> FixOutcome {
        FixOutcome {
            result: FixResult::default(),
            changes,
            attempted,
        }
    }

    #[test]
    fn without_dependencies_rules_run_in_id_order() {
        let rules = [rule("p/c", &[]), rule("p/a", &[]), rule("p/b", &[])];
        assert_eq!(ordered(&rules), ["p/a", "p/b", "p/c"]);
    }

    #[test]
    fn a_rule_runs_after_the_rules_it_names() {
        let rules = [
            rule("p/a", &["p/c"]),
            rule("p/b", &[]),
            rule("p/c", &["p/d"]),
            rule("p/d", &[]),
        ];
        assert_eq!(ordered(&rules), ["p/b", "p/d", "p/c", "p/a"]);
    }

    #[test]
    fn a_dependency_that_exists_but_is_not_enabled_places_no_constraint() {
        let rules = [rule("p/a", &["other/off"]), rule("p/b", &[])];
        let order = plan_order(&rules, &known(&rules, &["other/off"])).unwrap();
        assert_eq!(order, [0, 1]);
    }

    #[test]
    fn an_unknown_dependency_is_an_error_that_names_both_ids() {
        let rules = [rule("p/a", &["p/nope"])];
        let error = plan_order(&rules, &known(&rules, &[])).unwrap_err();
        assert_eq!(
            error.message(),
            "Rule \"p/a\" must run after \"p/nope\", but no rule has that id. Fix the id in the rule's \"after\" list."
        );
    }

    #[test]
    fn a_cycle_is_an_error_that_names_the_rules_in_it() {
        let rules = [
            rule("p/a", &["p/c"]),
            rule("p/b", &["p/a"]),
            rule("p/c", &["p/b"]),
            rule("p/free", &[]),
        ];
        let message = plan_order(&rules, &known(&rules, &[]))
            .unwrap_err()
            .message()
            .to_owned();
        assert!(
            message.contains("\"p/a\" -> \"p/b\" -> \"p/c\" -> \"p/a\""),
            "{message}"
        );
        assert!(!message.contains("p/free"), "{message}");

        let own = [rule("p/a", &["p/a"])];
        assert!(plan_order(&own, &known(&own, &[]))
            .unwrap_err()
            .message()
            .contains("lists itself"));
    }

    #[test]
    fn the_loop_stops_after_a_pass_with_nothing_to_do() {
        let (a, b) = (rule("p/a", &[]), rule("p/b", &["p/a"]));
        let mut calls: Vec<String> = Vec::new();
        // b can only do its work once a has written its file.
        let mut a_done = false;
        let mut b_done = false;
        let report = run_fix_loop(&[&a, &b], MAX_PASSES, |rule| {
            calls.push(rule.id.clone());
            Ok(match rule.id.as_str() {
                "p/a" if !a_done => {
                    a_done = true;
                    outcome(vec![write("a.txt", None, "a\n")], 1)
                }
                "p/b" if a_done && !b_done => {
                    b_done = true;
                    outcome(vec![write("b.txt", None, "b\n")], 1)
                }
                _ => outcome(Vec::new(), 0),
            })
        })
        .unwrap();
        assert_eq!(report.passes, 2);
        assert_eq!(calls, ["p/a", "p/b", "p/a", "p/b"]);
        let made: Vec<(usize, &str, &str)> = report
            .changes
            .iter()
            .map(|change| (change.pass, change.rule.as_str(), file_of(&change.change)))
            .collect();
        assert_eq!(made, [(1, "p/a", "a.txt"), (1, "p/b", "b.txt")]);
    }

    #[test]
    fn an_undeclared_dependency_still_converges_in_a_later_pass() {
        // Id order runs p/a first, but p/a needs what p/z writes.
        let (a, z) = (rule("p/a", &[]), rule("p/z", &[]));
        let mut z_done = false;
        let mut a_done = false;
        let report = run_fix_loop(&[&a, &z], MAX_PASSES, |rule| {
            Ok(match rule.id.as_str() {
                "p/z" if !z_done => {
                    z_done = true;
                    outcome(vec![write("z.txt", None, "z\n")], 1)
                }
                "p/a" if z_done && !a_done => {
                    a_done = true;
                    outcome(vec![write("a.txt", None, "a\n")], 1)
                }
                _ => outcome(Vec::new(), 0),
            })
        })
        .unwrap();
        assert_eq!(report.passes, 3);
        assert_eq!(report.changes.len(), 2);
        assert_eq!(report.changes[1].pass, 2);
    }

    #[test]
    fn two_fixes_that_undo_each_other_stop_the_loop_and_both_are_named() {
        let (a, b) = (rule("p/a", &[]), rule("p/b", &[]));
        let mut content: Option<String> = None;
        let failure = run_fix_loop(&[&a, &b], MAX_PASSES, |rule| {
            let wanted = if rule.id == "p/a" {
                "tabs\n"
            } else {
                "spaces\n"
            };
            if content.as_deref() == Some(wanted) {
                return Ok(outcome(Vec::new(), 0));
            }
            let change = write("style.txt", content.as_deref(), wanted);
            content = Some(wanted.to_owned());
            Ok(outcome(vec![change], 1))
        })
        .unwrap_err();
        let message = failure.error.message();
        assert!(message.contains("The fixes do not settle"), "{message}");
        assert!(message.contains("in pass 2"), "{message}");
        assert!(
            message.contains("another fix changed the file in between"),
            "{message}"
        );
        assert!(
            message.contains("Rules that write style.txt: \"p/a\", \"p/b\""),
            "{message}"
        );
        // What was done before the loop gave up is reported, not lost.
        assert_eq!(failure.report.changes.len(), 3);
    }

    #[test]
    fn a_fix_that_never_settles_hits_the_pass_limit() {
        let a = rule("p/counter", &[]);
        let mut count = 0;
        let failure = run_fix_loop(&[&a], 4, |_| {
            count += 1;
            Ok(outcome(
                vec![write("count.txt", None, &format!("{count}\n"))],
                1,
            ))
        })
        .unwrap_err();
        assert_eq!(count, 4);
        assert_eq!(failure.report.passes, 4);
        assert_eq!(
            failure.error.message(),
            "The fixes did not settle after 4 passes. Still changing files in the last pass: \"p/counter\" (files: count.txt). A fix that is run twice must change nothing the second time; one of these does not. Turn the rule off, or repair its fix."
        );
    }

    #[test]
    fn a_fix_that_does_nothing_about_its_violations_is_an_error() {
        let (a, b) = (rule("p/lazy", &[]), rule("p/ok", &[]));
        let failure = run_fix_loop(&[&a, &b], MAX_PASSES, |rule| {
            Ok(outcome(Vec::new(), usize::from(rule.id == "p/lazy")))
        })
        .unwrap_err();
        // One pass to see it do nothing, one more to see the violations remain.
        assert_eq!(failure.report.passes, 2);
        let message = failure.error.message();
        assert!(
            message.starts_with("The fix of \"p/lazy\" changed nothing"),
            "{message}"
        );
        assert!(message.contains("fscale check --rules p/lazy"), "{message}");
    }

    #[test]
    fn a_fix_that_repairs_without_reporting_changes_is_not_an_error() {
        // The fix did its work by a route the loop cannot see: the first
        // call had a violation and reported no change, the second has none.
        let a = rule("p/quiet", &[]);
        let mut calls = 0;
        let report = run_fix_loop(&[&a], MAX_PASSES, |_| {
            calls += 1;
            Ok(outcome(Vec::new(), usize::from(calls == 1)))
        })
        .unwrap();
        assert_eq!(report.passes, 2);
        assert!(report.changes.is_empty());
    }

    #[test]
    fn one_rule_rewriting_its_own_file_is_not_a_fight() {
        // The same rule writes the same content twice in one pass (once per
        // violation, say), and nobody else touches the file.
        let a = rule("p/twice", &[]);
        let mut done = false;
        let report = run_fix_loop(&[&a], MAX_PASSES, |_| {
            if done {
                return Ok(outcome(Vec::new(), 0));
            }
            done = true;
            Ok(outcome(
                vec![
                    write("list.txt", None, "a\nb\n"),
                    write("list.txt", Some("a\nb\n"), "a\nb\n"),
                ],
                2,
            ))
        })
        .unwrap();
        assert_eq!(report.passes, 2);
    }

    #[test]
    fn a_second_rule_editing_a_file_compatibly_is_not_a_fight() {
        // p/a creates the file; p/b appends to it; p/a is still satisfied.
        let (a, b) = (rule("p/a", &[]), rule("p/b", &[]));
        let mut state = 0;
        let report = run_fix_loop(&[&a, &b], MAX_PASSES, |rule| {
            Ok(match (rule.id.as_str(), state) {
                ("p/a", 0) => {
                    state = 1;
                    outcome(vec![write("f.txt", None, "a\n")], 1)
                }
                ("p/b", 1) => {
                    state = 2;
                    outcome(vec![write("f.txt", Some("a\n"), "a\nb\n")], 1)
                }
                _ => outcome(Vec::new(), 0),
            })
        })
        .unwrap();
        assert_eq!(report.changes.len(), 2);
    }

    #[test]
    fn a_failing_fix_keeps_the_report_of_what_was_already_done() {
        let (a, b) = (rule("p/a", &[]), rule("p/b", &[]));
        let failure = run_fix_loop(&[&a, &b], MAX_PASSES, |rule| {
            if rule.id == "p/b" {
                return Err(Error::user("disk full"));
            }
            Ok(outcome(vec![write("a.txt", None, "a\n")], 1))
        })
        .unwrap_err();
        assert_eq!(failure.error.message(), "disk full");
        assert_eq!(failure.report.changes.len(), 1);
    }

    /// Built-in rules for the tests below. `Needs` wants `file` to hold
    /// `"<file> from <source content>"`, and can only write it once the
    /// source exists. With no source it wants a fixed line.
    struct Needs {
        id: &'static str,
        after: &'static [&'static str],
        file: &'static str,
        source: Option<&'static str>,
    }

    impl Needs {
        fn wanted(&self, ctx: &crate::rule::RuleContext) -> Result<Option<String>> {
            match self.source {
                None => Ok(Some(format!("{}\n", self.file))),
                Some(source) => Ok(ctx
                    .read(source)?
                    .map(|content| format!("{} from {content}", self.file))),
            }
        }
    }

    impl crate::rule::BuiltinRule for Needs {
        fn meta(&self) -> crate::model::RuleMeta {
            crate::model::RuleMeta {
                id: self.id.to_owned(),
                severity: crate::model::Severity::Low,
                category: crate::model::Category::Consistency,
                description: "Test rule.".to_owned(),
                inputs: vec!["**".to_owned()],
                has_fix: true,
                after: self.after.iter().map(|id| (*id).to_owned()).collect(),
            }
        }

        fn skill(&self) -> &'static str {
            ""
        }

        fn check(&self, ctx: &crate::rule::RuleContext) -> Result<Vec<crate::model::Finding>> {
            let fixable = match self.wanted(ctx)? {
                Some(wanted) if ctx.read(self.file)?.as_deref() == Some(wanted.as_str()) => {
                    return Ok(Vec::new())
                }
                Some(_) => crate::model::Fixable::Deterministic,
                None => crate::model::Fixable::Manual,
            };
            Ok(vec![crate::model::Finding {
                message: format!("{} is missing or out of date.", self.file),
                file: self.file.to_owned(),
                range: None,
                project: None,
                fixable,
                severity: None,
            }])
        }

        fn fix(
            &self,
            ctx: &crate::rule::RuleContext,
            _diagnostics: &[crate::model::Diagnostic],
        ) -> Result<FixResult> {
            let Some(wanted) = self.wanted(ctx)? else {
                return Ok(FixResult::default());
            };
            ctx.write(self.file, &wanted)?;
            Ok(FixResult {
                changed_files: vec![self.file.to_owned()],
                notes: Vec::new(),
            })
        }
    }

    /// `p/a-top` needs `p/m-middle`, which needs `p/z-base`: id order is the
    /// reverse of the order the fixes have to run in.
    const CHAIN: [Needs; 3] = [
        Needs {
            id: "p/a-top",
            after: &["p/m-middle"],
            file: "top.txt",
            source: Some("middle.txt"),
        },
        Needs {
            id: "p/m-middle",
            after: &["p/z-base"],
            file: "middle.txt",
            source: Some("base.txt"),
        },
        Needs {
            id: "p/z-base",
            after: &[],
            file: "base.txt",
            source: None,
        },
    ];

    /// Runs the loop over built-in rules in `root`, as `fscale fix` does.
    fn run_builtin(
        root: &std::path::Path,
        rules: &[Needs],
    ) -> std::result::Result<LoopReport, LoopFailure> {
        use crate::rule::BuiltinRule;
        let metas: Vec<LoopRule> = rules
            .iter()
            .map(|rule| LoopRule {
                id: rule.meta().id,
                after: rule.meta().after,
            })
            .collect();
        let order = plan_order(&metas, &known(&metas, &[])).unwrap();
        let ordered: Vec<&LoopRule> = order.iter().map(|index| &metas[*index]).collect();
        run_fix_loop(&ordered, MAX_PASSES, |entry| {
            let rule = rules.iter().find(|rule| rule.id == entry.id).unwrap();
            crate::rule::fix_rule_with(rule, root, &crate::rule::Options::new(), false)
        })
    }

    #[test]
    fn built_in_rules_are_fixed_in_dependency_order_in_one_pass() {
        use crate::testing::{Fixture, Track};
        let fixture = Fixture::new(&[], Track::None).unwrap();
        let report = run_builtin(fixture.root(), &CHAIN).unwrap();
        let made: Vec<(usize, &str)> = report
            .changes
            .iter()
            .map(|change| (change.pass, change.rule.as_str()))
            .collect();
        assert_eq!(made, [(1, "p/z-base"), (1, "p/m-middle"), (1, "p/a-top")]);
        assert_eq!(report.passes, 2, "one pass of work, one that finds nothing");
        assert_eq!(
            fixture.read("top.txt").as_deref(),
            Some("top.txt from middle.txt from base.txt\n")
        );
        // Run again: nothing changes.
        let again = run_builtin(fixture.root(), &CHAIN).unwrap();
        assert!(again.changes.is_empty());
        assert_eq!(again.passes, 1);
    }

    #[test]
    fn a_dry_run_in_a_scratch_copy_finds_the_changes_the_real_run_makes() {
        use crate::scratch::Scratch;
        use crate::testing::{Fixture, Track};
        let fixture = Fixture::new(&[("README.md", "# x\n")], Track::All).unwrap();

        let preview = {
            let scratch = Scratch::copy_of(fixture.root()).unwrap();
            run_builtin(scratch.root(), &CHAIN).unwrap()
        };
        // The dry run changed nothing in the repo itself.
        for file in ["base.txt", "middle.txt", "top.txt"] {
            assert_eq!(fixture.read(file), None, "{file}");
        }
        assert_eq!(fixture.tracked().unwrap(), ["README.md"]);

        let real = run_builtin(fixture.root(), &CHAIN).unwrap();
        assert_eq!(preview, real);
        assert_eq!(real.changes.len(), 3);
    }

    #[test]
    fn untracking_the_same_file_twice_is_not_treated_as_a_fight() {
        let a = rule("p/a", &[]);
        let mut runs = 0;
        let report = run_fix_loop(&[&a], MAX_PASSES, |_| {
            runs += 1;
            Ok(if runs <= 2 {
                outcome(
                    vec![Change::Untrack {
                        file: ".env".to_owned(),
                    }],
                    1,
                )
            } else {
                outcome(Vec::new(), 0)
            })
        })
        .unwrap();
        assert_eq!(report.passes, 3);
    }
}
