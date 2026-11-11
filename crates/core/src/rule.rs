//! The contract a built-in rule implements, and the helpers that run one.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::git::Git;
use crate::model::{Diagnostic, Finding, FixResult, Fixable, RuleMeta};

/// Per-rule options from the config file.
pub type Options = Map<String, Value>;

/// One change a fix made, or in a dry run would make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// A file was written. `before` is `None` when the file did not exist.
    Write {
        file: String,
        before: Option<String>,
        after: String,
    },
    /// A file was removed from the git index and kept on disk.
    Untrack { file: String },
}

/// What a rule's `check` and `fix` receive.
#[derive(Debug)]
pub struct RuleContext {
    root: PathBuf,
    options: Options,
    git: Git,
    /// In a dry run, changes are recorded but not applied.
    dry_run: bool,
    changes: Mutex<Vec<Change>>,
}

impl RuleContext {
    #[must_use]
    pub fn new(root: &Path, options: Options) -> Self {
        Self::for_fix(root, options, false)
    }

    /// A context for running a fix. With `dry_run`, writes and git changes
    /// are recorded but nothing is touched.
    #[must_use]
    pub fn for_fix(root: &Path, options: Options, dry_run: bool) -> Self {
        Self {
            root: root.to_path_buf(),
            options,
            git: Git::new(root),
            dry_run,
            changes: Mutex::new(Vec::new()),
        }
    }

    /// Every change made through this context, in order.
    #[must_use]
    pub fn changes(&self) -> Vec<Change> {
        self.changes
            .lock()
            .map(|changes| changes.clone())
            .unwrap_or_default()
    }

    fn record(&self, change: Change) {
        if let Ok(mut changes) = self.changes.lock() {
            changes.push(change);
        }
    }

    /// Removes paths from the git index and keeps the files on disk. Only `fix` may call this.
    pub fn untrack(&self, paths: &[String]) -> Result<()> {
        if !self.dry_run {
            self.git.untrack(paths)?;
        }
        for file in paths {
            self.record(Change::Untrack { file: file.clone() });
        }
        Ok(())
    }

    /// Absolute path of the repo root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// This rule's options from the config file.
    #[must_use]
    pub fn options(&self) -> &Options {
        &self.options
    }

    #[must_use]
    pub fn git(&self) -> &Git {
        &self.git
    }

    /// Reads a repo-relative file as UTF-8 text. `None` when it does not exist.
    pub fn read(&self, file: &str) -> Result<Option<String>> {
        let target = self.resolve(file)?;
        match fs::read_to_string(&target) {
            Ok(text) => Ok(Some(text)),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(Error::user(format!(
                "Could not read {file} ({error}). Make sure it is a readable text file."
            ))),
        }
    }

    /// Writes a repo-relative file, creating parent folders as needed. Only `fix` may call this.
    pub fn write(&self, file: &str, content: &str) -> Result<()> {
        let target = self.resolve(file)?;
        let before = self.read(file)?;
        self.record(Change::Write {
            file: file.to_owned(),
            before,
            after: content.to_owned(),
        });
        if self.dry_run {
            return Ok(());
        }
        let failed = |error: std::io::Error| {
            Error::user(format!(
                "Could not write {file} ({error}). Check the folder's permissions."
            ))
        };
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(failed)?;
        }
        fs::write(&target, content).map_err(failed)
    }

    fn resolve(&self, file: &str) -> Result<PathBuf> {
        validate_relative(file)?;
        Ok(file
            .split('/')
            .fold(self.root.clone(), |path, part| path.join(part)))
    }
}

/// Rejects paths that are absolute or leave the repo root.
pub fn validate_relative(file: &str) -> Result<()> {
    let drive =
        file.len() >= 2 && file.as_bytes()[1] == b':' && file.as_bytes()[0].is_ascii_alphabetic();
    if file.is_empty() || file.starts_with('/') || drive || file.split('/').any(|part| part == "..")
    {
        return Err(Error::internal(format!(
            "Path \"{file}\" is not a path inside the repo root. Use a path relative to the repo root."
        )));
    }
    Ok(())
}

/// A rule compiled into the binary.
pub trait BuiltinRule: Send + Sync {
    fn meta(&self) -> RuleMeta;

    /// The rule's SKILL.md, embedded at build time.
    fn skill(&self) -> &'static str;

    /// Read-only. Returns one finding per violation.
    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>>;

    /// Deterministic and idempotent. Only called when `meta().has_fix` is true,
    /// with the diagnostics marked `Fixable::Deterministic`.
    fn fix(&self, _ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        Ok(FixResult::default())
    }
}

/// A named set of built-in rules.
pub struct BuiltinPack {
    /// Equals the `<pack>` part of every rule id in it.
    pub name: &'static str,
    pub rules: Vec<Box<dyn BuiltinRule>>,
}

fn to_diagnostic(rule_id: &str, finding: Finding) -> Result<Diagnostic> {
    let invalid = |what: &str| {
        Error::internal(format!(
            "Rule \"{rule_id}\" returned an invalid finding: {what}. This is a bug in the rule; please report it."
        ))
    };
    if finding.message.is_empty() {
        return Err(invalid("message is missing"));
    }
    validate_relative(&finding.file)?;
    if let Some(range) = finding.range {
        if range.start_line < 1 || range.end_line < range.start_line {
            return Err(invalid("range must be 1-based with endLine >= startLine"));
        }
    }
    Ok(Diagnostic {
        rule_id: rule_id.to_owned(),
        message: finding.message,
        file: finding.file,
        fixable: finding.fixable,
        range: finding.range,
        project: finding.project,
    })
}

/// Orders diagnostics by file, then line, then message, so output is stable.
pub fn sort_diagnostics(diagnostics: &mut [Diagnostic]) {
    diagnostics.sort_by(|a, b| {
        let line = |d: &Diagnostic| d.range.map_or(0, |range| range.start_line);
        a.file
            .cmp(&b.file)
            .then(line(a).cmp(&line(b)))
            .then(a.message.cmp(&b.message))
    });
}

/// Runs one built-in rule's check and returns its diagnostics in a stable order.
pub fn check_rule(
    rule: &dyn BuiltinRule,
    root: &Path,
    options: &Options,
) -> Result<Vec<Diagnostic>> {
    let id = rule.meta().id;
    let ctx = RuleContext::new(root, options.clone());
    let mut diagnostics = rule
        .check(&ctx)?
        .into_iter()
        .map(|finding| to_diagnostic(&id, finding))
        .collect::<Result<Vec<_>>>()?;
    sort_diagnostics(&mut diagnostics);
    Ok(diagnostics)
}

/// The result of running one rule's fix: what it reported and what it changed.
#[derive(Debug, Clone, Default)]
pub struct FixOutcome {
    pub result: FixResult,
    pub changes: Vec<Change>,
}

/// Runs one built-in rule's deterministic fix on its current diagnostics.
/// Returns an empty result when the rule has no fix or nothing is fixable.
pub fn fix_rule(rule: &dyn BuiltinRule, root: &Path, options: &Options) -> Result<FixResult> {
    Ok(fix_rule_with(rule, root, options, false)?.result)
}

/// Like `fix_rule`, and also returns every change. With `dry_run`, the
/// changes are worked out but nothing is written and git is not touched.
pub fn fix_rule_with(
    rule: &dyn BuiltinRule,
    root: &Path,
    options: &Options,
    dry_run: bool,
) -> Result<FixOutcome> {
    if !rule.meta().has_fix {
        return Ok(FixOutcome::default());
    }
    let diagnostics: Vec<Diagnostic> = check_rule(rule, root, options)?
        .into_iter()
        .filter(|diagnostic| diagnostic.fixable == Fixable::Deterministic)
        .collect();
    if diagnostics.is_empty() {
        return Ok(FixOutcome::default());
    }
    let ctx = RuleContext::for_fix(root, options.clone(), dry_run);
    let mut result = rule.fix(&ctx, &diagnostics)?;
    result.changed_files.sort();
    result.changed_files.dedup();
    Ok(FixOutcome {
        result,
        changes: ctx.changes(),
    })
}
