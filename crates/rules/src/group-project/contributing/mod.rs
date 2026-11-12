//! `group-project/contributing`: a contributing guide exists.

use firescale_core::facts::{detect_facts, source, Commands, Project, RepoFacts};
use firescale_core::model::{
    Category, Diagnostic, Finding, FixResult, Fixable, RuleMeta, Severity,
};
use firescale_core::rule::{BuiltinRule, RuleContext};
use firescale_core::Result;

use super::{branch_patterns, community_files, community_inputs, counted, create_blocker};

#[cfg(test)]
mod tests;

pub const ID: &str = "group-project/contributing";

/// The only file the fix creates.
pub const GUIDE: &str = "CONTRIBUTING.md";

pub struct Contributing;

/// `contributing` with any one extension, in any letter case.
fn is_guide_name(lower: &str) -> bool {
    lower == "contributing"
        || lower
            .strip_prefix("contributing.")
            .is_some_and(|extension| !extension.is_empty() && !extension.contains('.'))
}

/// A command as a teammate types it. A Python command that starts with the
/// interpreter of this machine (a `.venv` path, an active environment) is
/// written with plain `python -m`, which is the same command in the
/// teammate's own activated environment.
fn portable(command: &str, project: Option<&Project>) -> String {
    let Some(interpreter) = project.and_then(|project| project.interpreter.as_ref()) else {
        return command.to_owned();
    };
    if matches!(interpreter.source.as_str(), "venv" | "active" | "path") {
        if let Some(rest) = command.strip_prefix(&interpreter.command) {
            return format!("python -m{rest}");
        }
    }
    command.to_owned()
}

/// One place with commands: its folder (`.` for the root), its commands,
/// and the project they belong to.
struct Place<'a> {
    folder: String,
    commands: Commands,
    project: Option<&'a Project>,
}

/// The places to list: the repo root, then each project that runs by itself
/// (found in a subfolder, or at the root). Workspace members are covered by
/// the root's commands.
fn places(facts: &RepoFacts) -> Vec<Place<'_>> {
    let mut out: Vec<Place<'_>> = Vec::new();
    let root_node = facts
        .projects
        .iter()
        .find(|project| project.root == "." && project.kind == "node");
    if root_node.is_none() && facts.commands != Commands::default() {
        out.push(Place {
            folder: ".".to_owned(),
            commands: facts.commands.clone(),
            project: None,
        });
    }
    let mut own: Vec<&Project> = facts
        .projects
        .iter()
        .filter(|project| project.source == source::ROOT || project.source == source::DISCOVERED)
        .collect();
    own.sort_by(|a, b| (&a.root, &a.kind).cmp(&(&b.root, &b.kind)));
    for project in own {
        out.push(Place {
            folder: project.root.clone(),
            commands: project.commands.clone(),
            project: Some(project),
        });
    }
    out
}

fn place_name(folder: &str) -> String {
    if folder == "." {
        "the repo root".to_owned()
    } else {
        format!("`{folder}`")
    }
}

fn code(command: &Option<String>, project: Option<&Project>) -> String {
    command.as_deref().map_or_else(
        || "-".to_owned(),
        |command| format!("`{}`", portable(command, project)),
    )
}

/// The guide the fix writes. LF line endings. Built from `fscale facts` and
/// the branch patterns, so it names this repo's real commands.
#[must_use]
pub fn guide(facts: &RepoFacts, branches: &[String]) -> String {
    let places = places(facts);
    let mut out = String::from(
        "# Contributing\n\n<!-- Created by firescale (group-project/contributing). It is your team's to edit. -->\n\nHow we work on this repo together. Read it before your first pull request.\n\n## Set up\n\n",
    );
    let installs: Vec<String> = places
        .iter()
        .filter_map(|place| {
            place.commands.install.as_ref().map(|install| {
                format!(
                    "- In {}: `{}`\n",
                    place_name(&place.folder),
                    portable(install, place.project)
                )
            })
        })
        .collect();
    if installs.is_empty() {
        out.push_str("Describe how to install the project here.\n");
    } else {
        out.push_str("Install the dependencies. Run each command in the folder it names:\n\n");
        out.extend(installs);
    }
    if places.iter().any(|place| {
        place
            .project
            .is_some_and(|project| project.kind == "python")
    }) {
        out.push_str("\nFor a Python project, first make a virtual environment in its folder and activate it: `python -m venv .venv`. `fscale verify` uses the `.venv` in a project's folder by itself.\n");
    }

    out.push_str("\n## Check your work\n\nBefore you push, run the same check that CI runs:\n\n```sh\nnpx firescale verify\n```\n\nIt runs the firescale rules, then lint, typecheck and tests");
    out.push_str(if places.len() > 1 {
        " in every project"
    } else {
        ""
    });
    out.push_str(". Fix what it reports before you ask for a review. When it says `SETUP NEEDED`, your machine is missing something: run the command it shows, and do not change code for it.\n");
    let rows: Vec<String> = places
        .iter()
        .filter(|place| {
            [
                &place.commands.lint,
                &place.commands.typecheck,
                &place.commands.test,
            ]
            .iter()
            .any(|command| command.is_some())
        })
        .map(|place| {
            format!(
                "| {} | {} | {} | {} |\n",
                if place.folder == "." {
                    "repo root".to_owned()
                } else {
                    format!("`{}`", place.folder)
                },
                code(&place.commands.lint, place.project),
                code(&place.commands.typecheck, place.project),
                code(&place.commands.test, place.project),
            )
        })
        .collect();
    if !rows.is_empty() {
        out.push_str("\nTo run one step on its own, run its command in that folder:\n\n| Folder | Lint | Typecheck | Test |\n| --- | --- | --- | --- |\n");
        out.extend(rows);
    }

    let (kinds, fixed): (Vec<&String>, Vec<&String>) = branches
        .iter()
        .partition(|pattern| pattern.contains(['*', '?', '[']));
    out.push_str("\n## Branches\n\n");
    if kinds.is_empty() {
        out.push_str("Branch names must be one of these:\n\n");
        for name in &fixed {
            out.push_str(&format!("- `{name}`\n"));
        }
    } else {
        out.push_str("Do not commit to the main branch directly. Make a branch for each change, with a name that matches one of these patterns (`*` is any text):\n\n");
        for pattern in &kinds {
            out.push_str(&format!("- `{pattern}`\n"));
        }
        if !fixed.is_empty() {
            let names: Vec<String> = fixed.iter().map(|name| format!("`{name}`")).collect();
            out.push_str(&format!(
                "\nThe long-lived branches are {}.\n",
                names.join(", ")
            ));
        }
    }
    if branches.iter().any(|pattern| pattern == "feat/*") {
        out.push_str("\nFor example: `git switch -c feat/login-page`.\n");
    }
    out.push_str("\n## Pull requests\n\n- Keep each pull request to one change, small enough to review in one sitting.\n- Fill in the pull request template.\n- CI runs `npx firescale verify`. It must pass before the pull request is merged.\n- A teammate reviews and approves it before it is merged.\n");
    out
}

/// What is wrong, as `(message, file, fixable)`, or `None` when a guide counts.
fn problem(ctx: &RuleContext) -> Result<Option<(String, String, Fixable)>> {
    let found = community_files(ctx.root(), is_guide_name);
    if !counted(ctx, &found)?.is_empty() {
        return Ok(None);
    }
    if let Some(untracked) = found.first() {
        return Ok(Some((
            format!(
                "\"{untracked}\" exists but is not tracked by git, so teammates do not get it. Run `git add {untracked}` and commit it."
            ),
            untracked.clone(),
            Fixable::Manual,
        )));
    }
    if let Some((message, fixable)) = create_blocker(ctx, GUIDE, "contributing guide")? {
        return Ok(Some((message, GUIDE.to_owned(), fixable)));
    }
    Ok(Some((
        format!(
            "No contributing guide. Add {GUIDE} with how to set up, check and hand in work; `fscale fix` can create one from the commands `fscale facts` finds."
        ),
        GUIDE.to_owned(),
        Fixable::Deterministic,
    )))
}

impl BuiltinRule for Contributing {
    fn meta(&self) -> RuleMeta {
        RuleMeta {
            id: ID.to_owned(),
            severity: Severity::Medium,
            category: Category::Maintainability,
            description: "A contributing guide exists.".to_owned(),
            inputs: community_inputs("contributing"),
            has_fix: true,
            after: Vec::new(),
        }
    }

    fn skill(&self) -> &'static str {
        include_str!("SKILL.md")
    }

    fn check(&self, ctx: &RuleContext) -> Result<Vec<Finding>> {
        // A bad option is reported even when the guide exists.
        branch_patterns(ctx.options(), ID)?;
        Ok(problem(ctx)?
            .map(|(message, file, fixable)| Finding {
                message,
                file,
                range: None,
                project: None,
                fixable,
                severity: None,
            })
            .into_iter()
            .collect())
    }

    fn fix(&self, ctx: &RuleContext, _diagnostics: &[Diagnostic]) -> Result<FixResult> {
        let branches = branch_patterns(ctx.options(), ID)?;
        if !matches!(problem(ctx)?, Some((_, _, Fixable::Deterministic))) {
            return Ok(FixResult::default());
        }
        let facts = detect_facts(ctx.root())?;
        ctx.create_tracked(GUIDE, &guide(&facts, &branches))?;
        Ok(FixResult {
            changed_files: vec![GUIDE.to_owned()],
            notes: vec![format!(
                "Review {GUIDE}: its commands come from `fscale facts`. Add what your team agreed on that firescale cannot know."
            )],
        })
    }
}
