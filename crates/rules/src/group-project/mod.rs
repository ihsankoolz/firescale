//! The `group-project` rule pack: what a team of students, or any small team,
//! needs to work on one repo together (SPEC §6, §17). Opt-in: add
//! "group-project" to `packs`.
//!
//! GitHub's rules for where these files live were read on 2026-11-12 from the
//! source of docs.github.com (the `github/docs` repository): a pull request
//! template and a contributing guide may be in the repo root, `docs/` or
//! `.github/`, and their file names are not case sensitive (ADR 0023).

use std::fs;
use std::path::{Path, PathBuf};

use firescale_core::model::Fixable;
use firescale_core::rule::{BuiltinPack, Options, RuleContext};
use firescale_core::{Error, Result};

#[path = "branch-name/mod.rs"]
pub mod branch_name;
#[path = "ci-verify/mod.rs"]
pub mod ci_verify;
#[path = "contributing/mod.rs"]
pub mod contributing;
#[path = "pr-template/mod.rs"]
pub mod pr_template;

/// The folders GitHub looks in, in its own order of preference.
pub(crate) const COMMUNITY_FOLDERS: [&str; 3] = [".github", ".", "docs"];

/// The branch names a team may use, unless the config says otherwise. Shared
/// by `branch-name`, which checks them, and `contributing`, which writes them
/// into the guide.
pub(crate) const DEFAULT_BRANCHES: [&str; 16] = [
    "main",
    "master",
    "develop",
    "feat/*",
    "fix/*",
    "docs/*",
    "test/*",
    "refactor/*",
    "chore/*",
    "ci/*",
    "build/*",
    "perf/*",
    "style/*",
    "release/*",
    "hotfix/*",
    "revert/*",
];

/// The `branches` option: a non-empty list of glob patterns, or the default.
/// `rule` names the rule in the error.
pub(crate) fn branch_patterns(options: &Options, rule: &str) -> Result<Vec<String>> {
    let Some(value) = options.get("branches") else {
        return Ok(DEFAULT_BRANCHES.iter().map(|b| (*b).to_owned()).collect());
    };
    let wrong = || {
        Error::user(format!(
            "{rule}: option \"branches\" must be a non-empty list of branch name patterns, for example [\"main\", \"feat/*\", \"fix/*\"]. Fix it in your firescale config."
        ))
    };
    let list = value.as_array().ok_or_else(wrong)?;
    let patterns: Vec<String> = list
        .iter()
        .map(|item| item.as_str().map(str::trim).filter(|s| !s.is_empty()))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(wrong)?
        .into_iter()
        .map(str::to_owned)
        .collect();
    if patterns.is_empty() {
        return Err(wrong());
    }
    Ok(patterns)
}

/// A glob that matches `name` in any letter case: `[pP][rR]...`. GitHub
/// does not care about case in these file names, so neither do the inputs.
pub(crate) fn any_case(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphabetic() {
                format!("[{}{}]", c.to_ascii_lowercase(), c.to_ascii_uppercase())
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// Cache inputs: `name*` and everything below `name*`, in any letter case,
/// in each community folder.
pub(crate) fn community_inputs(name: &str) -> Vec<String> {
    let glob = format!("{}*", any_case(name));
    COMMUNITY_FOLDERS
        .iter()
        .flat_map(|dir| [in_folder(dir, &glob), in_folder(dir, &format!("{glob}/**"))])
        .collect()
}

/// `dir/name` with forward slashes, or `name` for the repo root.
pub(crate) fn in_folder(dir: &str, name: &str) -> String {
    if dir == "." {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

pub(crate) fn on_disk(root: &Path, file: &str) -> PathBuf {
    file.split('/')
        .filter(|part| *part != ".")
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

/// Names of the entries of a folder, sorted. Empty when it cannot be read.
pub(crate) fn entries(root: &Path, dir: &str) -> Vec<String> {
    let Ok(read) = fs::read_dir(on_disk(root, dir)) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

/// Files in the community folders whose name `matches` (it gets the name in
/// lower case), on disk, in GitHub's order of preference.
pub(crate) fn community_files(root: &Path, matches: impl Fn(&str) -> bool) -> Vec<String> {
    COMMUNITY_FOLDERS
        .iter()
        .flat_map(|dir| {
            entries(root, dir)
                .into_iter()
                .filter(|name| matches(&name.to_lowercase()))
                .map(move |name| in_folder(dir, &name))
        })
        .filter(|file| on_disk(root, file).is_file())
        .collect()
}

/// Which of `found` count: in a git repo, the ones git tracks, because a
/// file only reaches teammates through a commit. Outside git, all of them.
pub(crate) fn counted(ctx: &RuleContext, found: &[String]) -> Result<Vec<String>> {
    if !ctx.git().is_repo()? {
        return Ok(found.to_vec());
    }
    let mut out = Vec::new();
    for file in found {
        if ctx.git().is_tracked(file)? {
            out.push(file.clone());
        }
    }
    Ok(out)
}

/// Why `file` cannot be created by a fix, as `(message, fixable)`, or `None`
/// when it can. `what` names the file for people. The check and the fix both
/// ask, so a finding is deterministic only when the fix will act.
pub(crate) fn create_blocker(
    ctx: &RuleContext,
    file: &str,
    what: &str,
) -> Result<Option<(String, Fixable)>> {
    let root = ctx.root();
    let mut folder = String::new();
    for part in file
        .split('/')
        .collect::<Vec<_>>()
        .split_last()
        .map_or(&[][..], |(_, rest)| rest)
    {
        folder = if folder.is_empty() {
            (*part).to_owned()
        } else {
            format!("{folder}/{part}")
        };
        let blocked = on_disk(root, &folder)
            .symlink_metadata()
            .is_ok_and(|meta| !meta.is_dir());
        if blocked {
            return Ok(Some((
                format!("No {what} found, and {file} cannot be created because {folder} is not a folder. Add the {what} by hand (see SKILL.md)."),
                Fixable::Manual,
            )));
        }
    }
    if on_disk(root, file).symlink_metadata().is_ok() {
        return Ok(Some((
            format!("No {what} found, and something that is not a readable file is at {file}. Replace it with the {what} (see SKILL.md)."),
            Fixable::Manual,
        )));
    }
    if ctx.git().is_repo()? {
        if ctx.git().is_tracked(file)? {
            return Ok(Some((
                format!("{file} is tracked by git but missing from disk. Restore it with `git restore {file}`, or commit its deletion."),
                Fixable::Manual,
            )));
        }
        if ctx.git().is_ignored(file)? {
            return Ok(Some((
                format!("No {what} found, and a .gitignore rule ignores {file}, so one could not be committed. Remove that rule, then run `fscale fix`."),
                Fixable::Manual,
            )));
        }
    }
    Ok(None)
}

#[must_use]
pub fn pack() -> BuiltinPack {
    BuiltinPack {
        name: "group-project",
        rules: vec![
            Box::new(pr_template::PrTemplate),
            Box::new(contributing::Contributing),
            Box::new(ci_verify::CiVerify),
            Box::new(branch_name::BranchName),
        ],
    }
}
