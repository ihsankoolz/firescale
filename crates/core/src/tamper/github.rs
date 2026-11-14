//! The base of a pull request as GitHub gives it (M25, ADR 0045): from the
//! event GitHub Actions passes to the run, never from a ref in the clone, so
//! nothing the change does to the clone (a moved `origin/main`, a `git
//! replace`) moves what it is compared with.

use std::fs;
use std::path::Path;

use crate::git::run_git;
use crate::{Error, Result};

/// The commits of a pull request, from GitHub's event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    /// The tip of the base branch the change goes into: the first parent of
    /// the merge commit GitHub made for a `pull_request` run; for a
    /// `pull_request_target` run, `GITHUB_SHA` (the default branch) when the
    /// event's base commit is part of it, else that base commit.
    pub base_tip: String,
    /// The pull request's last commit (`pull_request.head.sha`).
    pub head: String,
    /// The commit the run checked out (`GITHUB_SHA`).
    pub checked_out: String,
}

/// The parents of a commit, read from its object.
fn parents(root: &Path, commit: &str) -> Option<Vec<String>> {
    let out = run_git(root, &["cat-file", "commit", commit]).ok()?;
    if out.code != 0 {
        return None;
    }
    Some(
        out.stdout
            .lines()
            .take_while(|line| !line.is_empty())
            .filter_map(|line| line.strip_prefix("parent "))
            .map(str::to_owned)
            .collect(),
    )
}

fn is_ancestor(root: &Path, older: &str, newer: &str) -> bool {
    run_git(root, &["merge-base", "--is-ancestor", older, newer]).is_ok_and(|out| out.code == 0)
}

fn full_id(text: &str) -> bool {
    (text.len() == 40 || text.len() == 64) && text.bytes().all(|b| b.is_ascii_hexdigit())
}

const FETCH: &str = "Check out the whole history (actions/checkout with fetch-depth: 0).";

/// Reads the pull request this GitHub Actions run is for. `read` reads one
/// environment variable.
///
/// # Errors
///
/// Outside GitHub Actions, for an event that is not `pull_request` or
/// `pull_request_target`, when the event names no commits, or when the
/// commits it names are not in the clone or do not fit together.
pub fn pull_request(root: &Path, read: &dyn Fn(&str) -> Option<String>) -> Result<PullRequest> {
    if read("GITHUB_ACTIONS").as_deref() != Some("true") {
        return Err(Error::user(
            "--pr-base reads the pull request from GitHub Actions' event, and this is not a GitHub Actions run. Pass --base <ref> instead.",
        ));
    }
    let event = read("GITHUB_EVENT_NAME").unwrap_or_default();
    if event != "pull_request" && event != "pull_request_target" {
        return Err(Error::user(format!(
            "--pr-base needs a pull_request or pull_request_target event, and this run is for \"{event}\". Run it only in a pull request job, or pass --base <ref>."
        )));
    }
    let payload = read("GITHUB_EVENT_PATH")
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .ok_or_else(|| {
            Error::user("--pr-base could not read the event file GITHUB_EVENT_PATH names.")
        })?;
    let sha = |side: &str| {
        payload
            .get("pull_request")
            .and_then(|pr| pr.get(side))
            .and_then(|side| side.get("sha"))
            .and_then(serde_json::Value::as_str)
            .filter(|sha| full_id(sha))
            .map(str::to_owned)
    };
    let (Some(base_sha), Some(head)) = (sha("base"), sha("head")) else {
        return Err(Error::user(
            "--pr-base: the event names no pull_request.base.sha and head.sha.",
        ));
    };
    let checked_out = read("GITHUB_SHA")
        .filter(|sha| full_id(sha))
        .ok_or_else(|| Error::user("--pr-base: GITHUB_SHA is not set to a commit id."))?;
    let Some(checked_parents) = parents(root, &checked_out) else {
        return Err(Error::user(format!(
            "--pr-base: the commit {checked_out} GitHub ran for is not in this clone. {FETCH}"
        )));
    };
    let base_tip = if event == "pull_request_target" {
        // The run is for the default branch's last commit (GitHub's docs):
        // the base branch when the pull request goes into the default
        // branch, else the event's base commit. Nothing of the pull request
        // has run in that job, so neither can have been moved.
        if is_ancestor(root, &base_sha, &checked_out) {
            checked_out.clone()
        } else {
            base_sha.clone()
        }
    } else if checked_parents.len() == 2 && checked_parents[1] == head {
        // The merge GitHub made: its first parent is the base branch.
        checked_parents[0].clone()
    } else if checked_out == head {
        base_sha.clone()
    } else {
        return Err(Error::user(format!(
            "--pr-base: GITHUB_SHA {checked_out} is neither the merge GitHub made for the pull request nor its head {head}."
        )));
    };
    if parents(root, &head).is_none() {
        return Err(Error::user(format!(
            "--pr-base: the pull request's head {head} is not in this clone. {FETCH}"
        )));
    }
    if !is_ancestor(root, &base_sha, &base_tip) {
        return Err(Error::user(format!(
            "--pr-base: the event's base {base_sha} is not part of the base branch at {base_tip}, or the history is shallow. {FETCH}"
        )));
    }
    Ok(PullRequest {
        base_tip,
        head,
        checked_out,
    })
}

/// The merge base of two commits.
///
/// # Errors
///
/// When the two commits have none in this clone.
pub fn merge_base(root: &Path, a: &str, b: &str) -> Result<String> {
    let out = run_git(root, &["merge-base", a, b])?;
    let line = out.stdout.trim();
    if out.code != 0 || line.is_empty() {
        return Err(Error::user(format!(
            "git finds no merge base of {a} and {b}. {FETCH}"
        )));
    }
    Ok(line.to_owned())
}
