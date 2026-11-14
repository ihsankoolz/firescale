//! Git hooks skipped (M25, ADR 0045): `--no-verify` and its forms in
//! scripts, hooks and CI, and firescale's own agent stop hook removed.
//!
//! Counted, each `-1`, over the whole change:
//!
//! - `--no-verify` on a git command (`git commit`, `git push`, `git merge`,
//!   …) or a release tool that passes it to git; not `cargo publish
//!   --no-verify`, which skips a build, not a hook;
//! - `git commit -n`, also in a group of short flags (`-anm`);
//! - `core.hooksPath` pointed at `/dev/null` or set empty.
//!
//! And `+1` for each agent hook that runs `fscale` or `firescale` in
//! `.claude/settings.json`, `.claude/settings.local.json`,
//! `.codex/hooks.json` or `.cursor/hooks.json`, so removing the stop hook
//! that sends an agent back on a failing verify is a finding.

use super::ci::shell_words;
use super::config::Measure;
use super::formats::{self, Format};
use super::Kind;

const AGENT_HOOK_FILES: [&str; 4] = [
    ".claude/settings.json",
    ".claude/settings.local.json",
    ".codex/hooks.json",
    ".cursor/hooks.json",
];

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// True for scripts, hook files, task runners and CI files.
pub(crate) fn reads(path: &str) -> bool {
    let name = file_name(path);
    let lower = name.to_ascii_lowercase();
    super::ci::reads(path)
        || AGENT_HOOK_FILES.contains(&path)
        || name == "package.json"
        || path.starts_with(".husky/")
        || path.contains("/.husky/")
        || path.starts_with(".githooks/")
        || matches!(
            lower.as_str(),
            "makefile"
                | "gnumakefile"
                | "justfile"
                | ".justfile"
                | "taskfile.yml"
                | "taskfile.yaml"
                | "lefthook.yml"
                | "lefthook.yaml"
                | ".lefthook.yml"
                | ".lefthook.yaml"
                | "lefthook-local.yml"
                | ".gitlab-ci.yml"
                | ".pre-commit-config.yaml"
        )
        || [".sh", ".bash", ".zsh", ".ps1", ".bat", ".cmd", ".mk"]
            .iter()
            .any(|extension| lower.ends_with(extension))
}

pub(crate) fn measures(path: &str, text: &str) -> Result<Vec<Measure>, usize> {
    let mut measures = Vec::new();
    if AGENT_HOOK_FILES.contains(&path) {
        let entries = formats::read(Format::Json, text)?;
        // Claude Code's switch that turns every hook off.
        for entry in entries.iter().filter(|e| e.path == ["disableAllHooks"]) {
            if entry.value.as_bool() == Some(true) {
                measures.push(Measure::count(
                    Kind::NoVerify,
                    "git hooks skipped",
                    -1,
                    entry.line,
                    "\"disableAllHooks\": true (every agent hook is off)",
                ));
            }
        }
        for entry in &entries {
            let in_hooks = entry.path.first().is_some_and(|k| k == "hooks");
            let is_command = entry
                .path
                .last()
                .is_some_and(|k| k == "command" || k == "bash" || k == "powershell");
            let value = entry.value.as_text();
            // It must run fscale's stop hook or verify, not just name it:
            // `echo fscale` or `fscale --version` does nothing.
            let words = shell_words(&value);
            let runs_fscale = words.iter().enumerate().any(|(at, w)| {
                let w = w.rsplit(['/', '\\']).next().unwrap_or(w);
                let is_fscale = matches!(
                    w,
                    "fscale" | "firescale" | "fscale.exe" | "fscale.cmd" | "firescale.cmd"
                );
                let next: Vec<&str> = words[at + 1..]
                    .iter()
                    .map(String::as_str)
                    .filter(|n| !n.starts_with('-'))
                    .take(2)
                    .collect();
                let leading_echo = words.first().is_some_and(|first| {
                    matches!(first.as_str(), "echo" | "printf" | "true" | ":" | "exit")
                });
                is_fscale
                    && !leading_echo
                    && (next == ["hooks", "run"] || next.first() == Some(&"verify"))
            });
            if in_hooks && is_command && runs_fscale {
                let event = entry.path.get(1).cloned().unwrap_or_default();
                measures.push(Measure::count(
                    Kind::NoVerify,
                    "agent hooks that run fscale",
                    1,
                    entry.line,
                    format!("{event} hook: {value}"),
                ));
            }
        }
    }
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('#')
            || trimmed.starts_with("//")
            || trimmed.to_ascii_lowercase().starts_with("rem ")
            || trimmed.starts_with("::")
        {
            continue;
        }
        for skip in hook_skips(trimmed) {
            measures.push(Measure::count(
                Kind::NoVerify,
                "git hooks skipped",
                -1,
                index + 1,
                skip,
            ));
        }
    }
    Ok(measures)
}

/// The ways a line skips git hooks, each as the command that does it.
fn hook_skips(line: &str) -> Vec<String> {
    let mut found = Vec::new();
    // One command at a time, so `cargo publish --no-verify && git push`
    // is read right.
    for command in line.split(['&', '|', ';']) {
        let words = shell_words(command);
        // A quoted script (`"save": "git commit …"`, `sh -c "…"`) is read
        // as a line of its own.
        // Not the value of a commit message (`-m "fix --no-verify docs"`).
        let nested: Vec<String> = words
            .iter()
            .enumerate()
            .filter(|(at, w)| {
                let message = at
                    .checked_sub(1)
                    .and_then(|before| words.get(before))
                    .is_some_and(|before| {
                        before == "--message"
                            || (before.starts_with('-')
                                && !before.starts_with("--")
                                && before.ends_with(['m', 'F']))
                    });
                w.contains(' ') && !message
            })
            .flat_map(|(_, w)| hook_skips(w))
            .collect();
        if !nested.is_empty() {
            found.extend(nested);
            continue;
        }
        let lower: Vec<String> = words
            .iter()
            .map(|w| w.trim_matches(['"', '\'', ',', '`']).to_owned())
            .filter(|w| !w.is_empty())
            .collect();
        // Shown from the git command on (or without a JSON or YAML key in
        // front), so the same command moved from `package.json` to a shell
        // script reads the same.
        let from = lower
            .iter()
            .position(|w| w == "git")
            .unwrap_or_else(|| usize::from(lower.first().is_some_and(|w| w.ends_with(':'))));
        let shown = |_: &str| {
            lower[from.min(lower.len())..]
                .join(" ")
                .chars()
                .take(160)
                .collect::<String>()
        };
        let no_verify = lower
            .iter()
            // git takes any unambiguous start of a long option:
            // `--no-veri` and longer mean `--no-verify`.
            .any(|w| {
                w.len() >= "--no-veri".len()
                    && "--no-verify".starts_with(w.split('=').next().unwrap_or(w))
            });
        let cargo_package = lower.windows(2).any(|pair| {
            pair[0].rsplit(['/', '\\']).next() == Some("cargo")
                && matches!(pair[1].as_str(), "publish" | "package")
        });
        if no_verify && !cargo_package {
            found.push(shown(command));
            continue;
        }
        // `git commit -n` and short-flag groups holding `n`.
        if let Some(at) = lower.iter().position(|w| w == "git") {
            let mut rest = lower[at + 1..].iter();
            let mut sub = None;
            while let Some(word) = rest.next() {
                if word == "-c" || word == "-C" {
                    rest.next();
                    continue;
                }
                if !word.starts_with('-') {
                    sub = Some(word.clone());
                    break;
                }
            }
            if sub.as_deref() == Some("commit") {
                let mut after = rest.peekable();
                while let Some(word) = after.next() {
                    if word == "--" {
                        break;
                    }
                    let is_group = word.len() > 1
                        && word.starts_with('-')
                        && !word.starts_with("--")
                        && word[1..].chars().all(|c| c.is_ascii_alphabetic());
                    if is_group {
                        // `-m` and `-F` take the next word as their value.
                        let letters = &word[1..];
                        if let Some(value_at) = letters.find(['m', 'F', 'c', 'C', 't']) {
                            if letters[..value_at].contains('n') {
                                found.push(shown(command));
                                break;
                            }
                            if value_at + 1 == letters.len() {
                                after.next();
                            }
                            continue;
                        }
                        if letters.contains('n') {
                            found.push(shown(command));
                            break;
                        }
                    }
                }
            }
        }
        // core.hooksPath pointed nowhere.
        let joined = lower.join(" ");
        if let Some(at) = joined
            .find("core.hookspath")
            .or_else(|| joined.find("core.hooksPath"))
        {
            let tail = joined[at + "core.hookspath".len()..].trim_start_matches(['=', ' ']);
            let target = tail.split_whitespace().next().unwrap_or("");
            // `git -c core.hooksPath=… <command>` points one command's hooks
            // elsewhere; `git config core.hooksPath` to nowhere turns them off.
            let inline = joined[..at].trim_end().ends_with("-c");
            if inline
                || target.is_empty()
                || target == "/dev/null"
                || target == "nul"
                || target == "NUL"
            {
                found.push(shown(command));
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::super::config::{compare, Located};
    use super::*;

    fn located(file: &str, text: &str) -> Vec<Located> {
        measures(file, text)
            .expect("readable")
            .into_iter()
            .map(|measure| Located {
                file: file.to_owned(),
                measure,
            })
            .collect()
    }

    #[test]
    fn finds_the_ways_to_skip_hooks() {
        for line in [
            "git commit --no-verify -m wip",
            "git push --no-verify origin HEAD",
            "git commit -anm \"wip\"",
            "git commit -n -m x",
            "git -c core.hooksPath=/dev/null commit -m x",
            "git -c core.hooksPath=.none commit -m x",
            "git commit -m wip -n",
            "git commit --no-verif -m x",
            "\"release\": \"standard-version && git push --no-verify --follow-tags\",",
        ] {
            assert_eq!(hook_skips(line).len(), 1, "{line}");
        }
        for line in [
            "git commit -m \"no -n here\"",
            "git commit -am message",
            "cargo publish --no-verify",
            "git push -n",
            "npm run verify",
            "git config core.hooksPath .githooks",
            "git commit -m \"fix --no-verify docs\"",
            "git commit --no-verbose -m x",
        ] {
            assert!(hook_skips(line).is_empty(), "{line}");
        }
    }

    #[test]
    fn a_new_skip_is_a_finding_and_a_moved_one_is_not() {
        let base = located(
            "package.json",
            "{\n  \"scripts\": {\n    \"save\": \"git commit -m wip\"\n  }\n}\n",
        );
        let head = located(
            "package.json",
            "{\n  \"scripts\": {\n    \"save\": \"git commit --no-verify -m wip\"\n  }\n}\n",
        );
        let found = compare(&base, &head);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].line, 3);
        assert_eq!(found[0].what, "git commit --no-verify -m wip");
        // The same command moved into a shell script is not new.
        let moved = located("scripts/save.sh", "git commit --no-verify -m wip\n");
        assert!(compare(&head, &moved).is_empty());
    }

    #[test]
    fn removing_the_agent_stop_hook_is_a_finding() {
        let settings = "{\n  \"hooks\": {\n    \"Stop\": [\n      { \"hooks\": [{ \"type\": \"command\", \"command\": \"pnpm --silent fscale hooks run --agent claude\" }] }\n    ]\n  }\n}\n";
        let base = located(".claude/settings.json", settings);
        assert_eq!(base.len(), 1);
        let head = located(".claude/settings.json", "{\n  \"hooks\": {}\n}\n");
        let found = compare(&base, &head);
        assert_eq!(found.len(), 1);
        assert!(found[0].on_base);
        assert!(found[0]
            .what
            .starts_with("Stop hook: pnpm --silent fscale hooks run"));
        assert!(compare(&base, &base).is_empty());
    }

    #[test]
    fn a_stop_hook_that_no_longer_runs_fscale_or_all_hooks_off_is_a_finding() {
        let settings = |command: &str| {
            format!("{{\n  \"hooks\": {{\n    \"Stop\": [{{ \"hooks\": [{{ \"type\": \"command\", \"command\": \"{command}\" }}] }}]\n  }}\n}}\n")
        };
        let base = located(
            ".claude/settings.json",
            &settings("pnpm --silent fscale hooks run --agent claude"),
        );
        for command in ["echo fscale", "pnpm fscale --version", "true"] {
            assert_eq!(
                compare(&base, &located(".claude/settings.json", &settings(command))).len(),
                1,
                "{command}"
            );
        }
        assert!(compare(
            &base,
            &located(".claude/settings.json", &settings("npx firescale verify"))
        )
        .is_empty());
        let off = located(
            ".claude/settings.local.json",
            "{ \"disableAllHooks\": true }\n",
        );
        let found = compare(&[], &off);
        assert_eq!(found.len(), 1);
        assert!(found[0].what.contains("disableAllHooks"));
    }
}
