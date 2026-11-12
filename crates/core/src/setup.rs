//! Telling "the environment is not ready" apart from a real failure
//! (ADR 0022).
//!
//! A verify step that fails because a dependency is not installed, or a
//! program is not on the PATH, is not something a code change fixes. Verify
//! reports it as "setup needed", with the command that sets the environment
//! up, so an agent does not edit code to make it pass.
//!
//! The detection is a short list of exact messages, documented in
//! docs/verify.md. Anything else, and anything that could be a mistake in the
//! code (a module that the project itself holds), stays a normal failure.

use std::path::Path;

/// Why a failed step looks like an environment that is not set up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupProblem {
    /// What is missing, in a few words: "the Python module 'jwt' is not
    /// installed".
    pub reason: String,
    /// A program that is not on the PATH at all, such as `python` or `pnpm`.
    /// Installing the project's dependencies does not bring it back.
    pub missing_program: Option<String>,
}

/// Programs that a project's own install command does not provide.
const BASE_PROGRAMS: [&str; 16] = [
    "bun", "cargo", "docker", "go", "node", "npm", "npx", "pip", "pipenv", "pnpm", "poetry", "py",
    "python", "python3", "uv", "yarn",
];

/// The text between the first pair of `quote` characters after `start`, or
/// the word after it when it is not quoted.
fn name_after(text: &str, start: usize) -> Option<&str> {
    let rest = text[start..].trim_start();
    let first = rest.chars().next()?;
    let name = if matches!(first, '\'' | '"' | '`') {
        let inner = &rest[1..];
        &inner[..inner.find(first)?]
    } else {
        let end = rest
            .find(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | ')'))
            .unwrap_or(rest.len());
        &rest[..end]
    };
    let name = name.trim();
    (!name.is_empty()).then_some(name)
}

/// True when `module`'s top-level name is a file or folder of the project or
/// of a folder above it, up to the repo root. A missing module of that kind
/// may be a mistake in the code, so it is not called a setup problem.
fn is_local_module(module: &str, root: &Path, dir: &Path) -> bool {
    let top = module.split('.').next().unwrap_or(module);
    if top.is_empty() {
        return true;
    }
    let mut current = Some(dir);
    while let Some(folder) = current {
        if folder.join(top).is_dir() || folder.join(format!("{top}.py")).is_file() {
            return true;
        }
        if folder == root {
            break;
        }
        current = folder.parent().filter(|parent| parent.starts_with(root));
    }
    false
}

/// A program named by a "not found" message, without a path or `.exe`.
fn program_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let lower = base.to_lowercase();
    lower
        .strip_suffix(".exe")
        .or_else(|| lower.strip_suffix(".cmd"))
        .unwrap_or(&lower)
        .to_owned()
}

fn missing(program: &str) -> SetupProblem {
    let name = program_name(program);
    if BASE_PROGRAMS.contains(&name.as_str()) {
        SetupProblem {
            reason: format!("the program `{name}` was not found on the PATH"),
            missing_program: Some(name),
        }
    } else {
        SetupProblem {
            reason: format!(
                "the command `{name}` was not found; the dependencies are probably not installed"
            ),
            missing_program: None,
        }
    }
}

/// The name in a POSIX shell's "not found" line: `sh: 1: vitest: not found`,
/// `sh: vitest: command not found`, `zsh: command not found: vitest`.
fn shell_not_found(line: &str) -> Option<&str> {
    if let Some(index) = line.find("command not found: ") {
        return name_after(line, index + "command not found: ".len());
    }
    let before = line
        .strip_suffix(": command not found")
        .or_else(|| line.strip_suffix(": not found"))?;
    let name = before.rsplit(": ").next()?.trim();
    (!name.is_empty() && !name.contains(' ')).then_some(name)
}

/// Looks at the output of a failed step for a sign that the environment is
/// not set up. `dir` is the folder the step ran in, `root` the repo root.
/// Returns `None` when unsure: then the step is a normal failure.
#[must_use]
pub fn setup_problem(
    output: &str,
    exit_code: Option<i32>,
    root: &Path,
    dir: &Path,
) -> Option<SetupProblem> {
    for line in output.lines() {
        let line = line.trim();
        // Python: `ModuleNotFoundError: No module named 'jwt'`, and
        // `python: No module named pytest` from `python -m pytest`.
        if let Some(index) = line.find("No module named ") {
            let module = name_after(line, index + "No module named ".len())?;
            if is_local_module(module, root, dir) {
                return None;
            }
            return Some(SetupProblem {
                reason: format!("the Python module '{module}' is not installed"),
                missing_program: None,
            });
        }
        // Windows: the Microsoft Store stand-in for a Python that is not installed.
        if line.starts_with("Python was not found") {
            return Some(missing("python"));
        }
        // cmd: 'vitest' is not recognized as an internal or external command
        if let Some(index) = line.find(" is not recognized as an internal or external command") {
            if let Some(name) = name_after(line, 0).filter(|_| index > 0) {
                return Some(missing(name));
            }
        }
        // npx --no with the tool not installed.
        if line.contains("canceled due to missing packages") {
            return Some(SetupProblem {
                reason:
                    "npx found the tool not installed; the dependencies are probably not installed"
                        .to_owned(),
                missing_program: None,
            });
        }
        // pnpm exec: Command "oxlint" not found
        if let Some(index) = line.find("Command \"") {
            if line.ends_with("not found") || line.ends_with("not found.") {
                if let Some(name) = name_after(line, index + "Command ".len()) {
                    return Some(missing(name));
                }
            }
        }
        // poetry run: Command not found: pytest
        if let Some(index) = line.find("Command not found: ") {
            if let Some(name) = name_after(line, index + "Command not found: ".len()) {
                return Some(missing(name));
            }
        }
        // uv run: error: Failed to spawn: `pytest`
        if let Some(index) = line.find("Failed to spawn: ") {
            if let Some(name) = name_after(line, index + "Failed to spawn: ".len()) {
                return Some(missing(name));
            }
        }
        // Node ESM: Cannot find package 'vitest' imported from ...
        if line.contains("ERR_MODULE_NOT_FOUND") || line.starts_with("Error: Cannot find package") {
            if let Some(index) = line.find("Cannot find package ") {
                if let Some(name) = name_after(line, index + "Cannot find package ".len()) {
                    let relative = name.starts_with('.')
                        || name.starts_with('/')
                        || name.starts_with('\\')
                        || name.chars().nth(1) == Some(':');
                    if relative {
                        return None;
                    }
                    return Some(SetupProblem {
                        reason: format!("the package '{name}' is not installed"),
                        missing_program: None,
                    });
                }
            }
        }
        // sh: exit code 127 means the shell could not find the command.
        if exit_code == Some(127) {
            if let Some(name) = shell_not_found(line) {
                return Some(missing(name));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Fixture, Track};

    fn problem(output: &str, code: i32) -> Option<SetupProblem> {
        let fixture = Fixture::new(
            &[
                ("svc/main.py", ""),
                ("svc/app/__init__.py", ""),
                ("shared/__init__.py", ""),
            ],
            Track::NoGit,
        )
        .unwrap();
        setup_problem(
            output,
            Some(code),
            fixture.root(),
            &fixture.root().join("svc"),
        )
    }

    fn reason(output: &str, code: i32) -> String {
        problem(output, code).map(|p| p.reason).unwrap_or_default()
    }

    #[test]
    fn a_missing_python_package_is_a_setup_problem() {
        let out = "ImportError while importing test module '/x/test_auth.py'.\nE   ModuleNotFoundError: No module named 'jwt'\n";
        assert_eq!(reason(out, 2), "the Python module 'jwt' is not installed");
        // `python -m pytest` with pytest missing.
        assert_eq!(
            reason("/usr/bin/python: No module named pytest\n", 1),
            "the Python module 'pytest' is not installed"
        );
        assert_eq!(
            reason(
                "E   ModuleNotFoundError: No module named 'google.protobuf'",
                2
            ),
            "the Python module 'google.protobuf' is not installed"
        );
    }

    #[test]
    fn a_missing_module_of_the_project_itself_is_a_normal_failure() {
        // Folders and files of the project or above it: maybe a code mistake.
        assert_eq!(
            problem("ModuleNotFoundError: No module named 'app.services'", 2),
            None
        );
        assert_eq!(
            problem("ModuleNotFoundError: No module named 'main'", 2),
            None
        );
        assert_eq!(
            problem("ModuleNotFoundError: No module named 'shared'", 2),
            None
        );
    }

    #[test]
    fn a_command_that_is_not_found_is_a_setup_problem() {
        let p = problem("sh: 1: vitest: not found\n", 127).unwrap();
        assert_eq!(p.missing_program, None);
        assert!(p.reason.contains("`vitest`"), "{}", p.reason);
        let p = problem("sh: line 1: pnpm: command not found\n", 127).unwrap();
        assert_eq!(p.missing_program.as_deref(), Some("pnpm"));
        let p = problem(
            "'python' is not recognized as an internal or external command,\noperable program or batch file.\n",
            9009,
        )
        .unwrap();
        assert_eq!(p.missing_program.as_deref(), Some("python"));
        let p = problem(
            "Python was not found; run without arguments to install from the Microsoft Store\n",
            9009,
        )
        .unwrap();
        assert_eq!(p.missing_program.as_deref(), Some("python"));
        let p = problem("npm error npx canceled due to missing packages and no YES option: [\"oxlint@1.0.0\"]\n", 1).unwrap();
        assert_eq!(p.missing_program, None);
        let p = problem(
            " ERR_PNPM_RECURSIVE_EXEC_FIRST_FAIL  Command \"oxlint\" not found\n",
            1,
        )
        .unwrap();
        assert!(p.reason.contains("`oxlint`"), "{}", p.reason);
        let p = problem("\nCommand not found: pytest\n", 1).unwrap();
        assert!(p.reason.contains("`pytest`"), "{}", p.reason);
        let p = problem("error: Failed to spawn: `pytest`\n  Caused by: No such file or directory (os error 2)\n", 2).unwrap();
        assert!(p.reason.contains("`pytest`"), "{}", p.reason);
        let p = problem("Error [ERR_MODULE_NOT_FOUND]: Cannot find package 'vitest' imported from /x/vitest.config.ts\n", 1).unwrap();
        assert_eq!(p.reason, "the package 'vitest' is not installed");
    }

    #[test]
    fn real_failures_are_not_setup_problems() {
        // A failing test, a lint error, a type error.
        assert_eq!(
            problem(
                "FAILED test_auth.py::test_login - assert 401 == 200\n1 failed, 3 passed\n",
                1
            ),
            None
        );
        assert_eq!(problem("src/a.ts:1:1 error no-unused-vars\n", 1), None);
        assert_eq!(
            problem(
                "error TS2322: Type 'string' is not assignable to type 'number'.\n",
                2
            ),
            None
        );
        // "not found" text with a code other than 127: a test about a 404, say.
        assert_eq!(
            problem("expected: not found\nsh: 1: x: not found\n", 1),
            None
        );
        // A relative import that is missing is a code problem.
        assert_eq!(
            problem(
                "Error [ERR_MODULE_NOT_FOUND]: Cannot find package '/x/y' imported from /x/z.js\n",
                1
            ),
            None
        );
        assert_eq!(problem("", 1), None);
    }
}
