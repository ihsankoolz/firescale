//! What the `agent-ready` fixes share (ADR 0027): the managed block in
//! AGENTS.md and CLAUDE.md, the command that starts fscale in this repo, and
//! the files each agent reads.

use firescale_core::facts::{Project, RepoFacts, UNNAMED_ROOT};
use serde_json::Value;

use crate::group_project::contributing::{places, portable};

/// The line that opens firescale's block. Only a line that starts with this
/// counts, so text that merely looks similar is left alone.
pub(crate) const BEGIN: &str = "<!-- BEGIN firescale";
/// The line that closes it.
pub(crate) const END: &str = "<!-- END firescale -->";

/// What `fscale facts` reads to work out the commands, as cache inputs of
/// the rules whose output lists them.
pub(crate) const FACTS_INPUTS: [&str; 28] = [
    "package.json",
    "**/package.json",
    "pnpm-workspace.yaml",
    "pnpm-lock.yaml",
    "package-lock.json",
    "yarn.lock",
    "bun.lock",
    "bun.lockb",
    "nx.json",
    "turbo.json",
    "**/pyproject.toml",
    "**/requirements*.txt",
    "**/requirements/*.txt",
    "**/setup.py",
    "**/setup.cfg",
    "**/Pipfile",
    "**/pytest.ini",
    "**/tox.ini",
    "**/uv.lock",
    "**/poetry.lock",
    "**/Cargo.toml",
    "**/go.mod",
    "**/tsconfig.json",
    "**/vitest.config.*",
    "**/jest.config.*",
    "**/eslint.config.*",
    "**/ruff.toml",
    "**/mypy.ini",
];

/// Where a managed block is in a text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Block {
    /// No BEGIN line.
    Absent,
    /// The byte range of the whole block, BEGIN line to END line with its
    /// line end, and the text inside it.
    At {
        start: usize,
        end: usize,
        inside: String,
    },
    /// Two BEGIN lines, or a BEGIN line with no END line after it: a person
    /// has to look.
    Broken(String),
}

/// Finds the managed block.
pub(crate) fn find_block(text: &str) -> Block {
    let mut begins = Vec::new();
    let mut ends = Vec::new();
    let mut offset = 0;
    let mut fenced = false;
    for line in text.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);
        // Markers inside a code fence are an example, not the block.
        let opener = bare.trim_start();
        if opener.starts_with("```") || opener.starts_with("~~~") {
            fenced = !fenced;
        } else if fenced {
            // Skipped.
        } else if bare.starts_with(BEGIN) {
            begins.push((offset, offset + line.len()));
        } else if bare.trim_end() == END {
            ends.push((offset, offset + line.len()));
        }
        offset += line.len();
    }
    match (begins.as_slice(), ends.as_slice()) {
        ([], []) => Block::Absent,
        ([(start, inner)], [(close, end)]) if close >= inner => Block::At {
            start: *start,
            end: *end,
            inside: text[*inner..*close].to_owned(),
        },
        ([], _) => Block::Broken(format!(
            "it has a `{END}` line with no `{BEGIN} ...` line before it"
        )),
        ([_], []) => Block::Broken(format!(
            "it has a `{BEGIN} ...` line with no `{END}` line after it"
        )),
        _ => Block::Broken(
            "it has more than one firescale block, or the END line comes first".to_owned(),
        ),
    }
}

/// The line ending a text uses: CRLF when it has any, else LF.
pub(crate) fn newline_of(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// `block` (written with LF) in the text's own line endings.
pub(crate) fn in_line_endings(block: &str, newline: &str) -> String {
    if newline == "\n" {
        block.to_owned()
    } else {
        block.replace('\n', newline)
    }
}

/// The text with `block` in place: the old block replaced, or the block
/// appended after a blank line. Text outside the block is kept byte for byte.
pub(crate) fn with_block(text: &str, block: &str) -> Option<String> {
    let newline = newline_of(text);
    let block = in_line_endings(block, newline);
    match find_block(text) {
        Block::At { start, end, .. } => {
            let mut out = String::with_capacity(text.len() + block.len());
            out.push_str(&text[..start]);
            out.push_str(&block);
            out.push_str(&text[end..]);
            Some(out)
        }
        Block::Absent => {
            let mut out = text.to_owned();
            if out.trim().is_empty() {
                return Some(block);
            }
            if !out.ends_with('\n') {
                out.push_str(newline);
            }
            out.push_str(newline);
            out.push_str(&block);
            Some(out)
        }
        Block::Broken(_) => None,
    }
}

/// True when the text's block is exactly `block` (in its line endings).
pub(crate) fn block_is_current(text: &str, block: &str) -> bool {
    let block = in_line_endings(block, newline_of(text));
    match find_block(text) {
        Block::At { start, end, .. } => text[start..end] == block,
        _ => false,
    }
}

/// How this repo starts fscale: through its own `fscale` package script
/// when it has one, else `npx firescale` (the same as the starter CI).
pub(crate) fn fscale_command(facts: &RepoFacts, package_json: Option<&Value>) -> String {
    let has_script = package_json
        .and_then(|json| json.get("scripts"))
        .and_then(|scripts| scripts.get("fscale"))
        .is_some_and(Value::is_string);
    if !has_script {
        return "npx firescale".to_owned();
    }
    match facts.package_manager.as_deref() {
        Some("pnpm") => "pnpm --silent fscale".to_owned(),
        Some("yarn") => "yarn --silent fscale".to_owned(),
        Some("bun") => "bun run --silent fscale".to_owned(),
        _ => "npm run --silent fscale --".to_owned(),
    }
}

/// A project's command as every machine writes it (review of M13): a
/// Python project managed by uv, Poetry or Pipenv runs through its manager
/// whether or not this machine has a `.venv`; any other Python command is
/// `python -m ...`, never this machine's interpreter path.
pub(crate) fn stable_command(command: &str, project: Option<&Project>, install: bool) -> String {
    let Some(project) = project.filter(|project| project.kind == "python") else {
        return command.to_owned();
    };
    let (run, sync) = match project.package_manager.as_deref() {
        Some("uv") => ("uv run", "uv sync"),
        Some("poetry") => ("poetry run", "poetry install"),
        Some("pipenv") => ("pipenv run", "pipenv install --dev"),
        _ => return portable(command, Some(project)),
    };
    if install {
        return sync.to_owned();
    }
    // What runs after the interpreter or the manager: `pytest`, `ruff check .`.
    let tool = project
        .interpreter
        .as_ref()
        .and_then(|interpreter| command.strip_prefix(interpreter.command.as_str()))
        .or_else(|| command.strip_prefix(run))
        .map_or(command, str::trim_start);
    format!("{run} {tool}")
}

/// True for a Python project whose manager keeps its own environment.
fn managed_python(project: &Project) -> bool {
    project.kind == "python"
        && matches!(
            project.package_manager.as_deref(),
            Some("uv" | "poetry" | "pipenv")
        )
}

fn folder_heading(folder: &str) -> String {
    if folder == "." || folder == UNNAMED_ROOT {
        "### The repo root\n".to_owned()
    } else {
        format!("### `{folder}`\n")
    }
}

/// The managed block of AGENTS.md: the commands from `fscale facts`, and
/// the definition of done. LF line endings; the same on every machine
/// ([`stable_command`]).
pub(crate) fn agents_block(facts: &RepoFacts, bin: &str) -> String {
    let mut out = format!(
        "{BEGIN}: written by `fscale sync` from `fscale facts`. Edits inside this block are replaced; write your own notes outside it. -->\n\n## Commands\n\n"
    );
    let mut sections = Vec::new();
    // The root's own scripts first, exactly as `fscale facts` names them,
    // with the root Node project's install. Other kinds at the root (a
    // Python project) are listed as places below.
    let root = &facts.commands;
    let mut root_lines = Vec::new();
    let root_node = facts
        .projects
        .iter()
        .find(|p| p.root == "." && p.kind == "node");
    if let Some(install) = root_node.and_then(|p| p.commands.install.as_ref()) {
        root_lines.push(format!("- Install: `{install}`\n"));
    }
    for (label, command) in [
        ("Build", &root.build),
        ("Lint", &root.lint),
        ("Typecheck", &root.typecheck),
        ("Test", &root.test),
    ] {
        if let Some(command) = command {
            root_lines.push(format!("- {label}: `{command}`\n"));
        }
    }
    if !root_lines.is_empty() {
        sections.push(format!("{}\n{}", folder_heading("."), root_lines.concat()));
    }
    for place in places(facts) {
        let node_root =
            place.folder == "." && place.project.is_none_or(|project| project.kind == "node");
        if node_root && !root_lines.is_empty() {
            continue;
        }
        let commands = &place.commands;
        let mut lines = Vec::new();
        for (label, command) in [
            ("Install", &commands.install),
            ("Build", &commands.build),
            ("Lint", &commands.lint),
            ("Typecheck", &commands.typecheck),
            ("Test", &commands.test),
        ] {
            if let Some(command) = command {
                let shown = stable_command(command, place.project, label == "Install");
                lines.push(format!("- {label}: `{shown}`\n"));
            }
        }
        if !lines.is_empty() {
            let heading = match place.project {
                Some(project) if place.folder == "." && project.kind != "node" => {
                    format!("### The repo root ({} project)\n", project.kind)
                }
                _ => folder_heading(&place.folder),
            };
            sections.push(format!("{heading}\n{}", lines.concat()));
        }
    }
    if sections.is_empty() {
        out.push_str("No build, lint, typecheck or test commands were found yet. Add them (for example as package scripts), then run `fscale sync` to list them here.\n");
    } else {
        out.push_str("Run each command in the folder it names.\n\n");
        out.push_str(&sections.join("\n"));
    }
    if facts
        .projects
        .iter()
        .any(|project| project.kind == "python" && !managed_python(project))
    {
        out.push_str("\nFor a Python project, make a virtual environment in its folder first (`python -m venv .venv`) and activate it.\n");
    }
    out.push_str(&format!(
        "\n## Definition of done\n\nBefore you say a task is done, run `{bin} verify` from the repo root and fix what it reports. It runs the firescale rules, then lint, typecheck and tests.\n\n- Never skip, delete or weaken a test, a lint rule or a check to make it pass.\n- `SETUP NEEDED` means the machine is not ready: a dependency, a program or a service is missing. Run the setup command it names, or tell the user. Do not change code for it. `{bin} doctor` lists what is missing.\n- To learn how to fix a rule's finding: `{bin} explain <rule>`.\n\n{END}\n"
    ));
    out
}

/// The managed block of CLAUDE.md: it makes Claude Code read AGENTS.md,
/// which it skips when a CLAUDE.md exists.
pub(crate) fn claude_block() -> String {
    format!("{BEGIN}: lets Claude Code read AGENTS.md. Written by `fscale sync`. -->\n\n@AGENTS.md\n\n{END}\n")
}

/// True when CLAUDE.md already imports AGENTS.md outside a code fence.
pub(crate) fn imports_agents_md(text: &str) -> bool {
    let mut fenced = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if !fenced
            && trimmed
                .split_whitespace()
                .any(|word| word == "@AGENTS.md" || word == "@./AGENTS.md")
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK: &str = "<!-- BEGIN firescale: x -->\nnew\n<!-- END firescale -->\n";

    #[test]
    fn a_block_is_appended_once_and_then_replaced_in_place() {
        let text = "# Agents\r\n\r\nOurs.\r\n<!-- firescale:begin -->look-alike<!-- firescale:end -->\r\nlast";
        let first = with_block(text, BLOCK).unwrap();
        assert!(first.starts_with(text));
        assert!(first.ends_with(
            "last\r\n\r\n<!-- BEGIN firescale: x -->\r\nnew\r\n<!-- END firescale -->\r\n"
        ));
        assert!(block_is_current(&first, BLOCK));
        let edited = first.replace("\r\nnew\r\n", "\r\nhand edit\r\n");
        assert!(!block_is_current(&edited, BLOCK));
        assert_eq!(with_block(&edited, BLOCK).unwrap(), first);
        assert_eq!(with_block(&first, BLOCK).unwrap(), first);
    }

    #[test]
    fn a_broken_block_is_left_for_a_person() {
        assert!(matches!(
            find_block("<!-- BEGIN firescale -->\nno end\n"),
            Block::Broken(_)
        ));
        assert!(matches!(
            find_block("<!-- BEGIN firescale -->\n<!-- END firescale -->\n<!-- BEGIN firescale -->\n<!-- END firescale -->\n"),
            Block::Broken(_)
        ));
        assert_eq!(with_block("<!-- END firescale -->\n", BLOCK), None);
        assert_eq!(
            find_block("text <!-- BEGIN firescale --> inline\n"),
            Block::Absent
        );
    }

    #[test]
    fn imports_are_found_outside_code_fences_only() {
        assert!(imports_agents_md("# x\n@AGENTS.md\n"));
        assert!(imports_agents_md("See @AGENTS.md for commands.\n"));
        assert!(!imports_agents_md("```\n@AGENTS.md\n```\n"));
        assert!(!imports_agents_md("email me@AGENTS.md.example\n"));
    }
}
