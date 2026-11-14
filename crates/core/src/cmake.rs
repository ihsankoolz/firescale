//! A small reader of CMake's command syntax (ADR 0055), for the cmake
//! pack's rules and for finding a CMake project's `lint` target.
//!
//! A CMake file is a list of command invocations, `name(arguments)`, with
//! comments (`# …` and bracket comments `#[[ … ]]`), quoted arguments
//! (`"…"`, with `\` escapes) and bracket arguments (`[[ … ]]`, `[=[ … ]=]`)
//! between them. The pack's checks read commands, not text, so a
//! commented-out `cmake_minimum_required` or an `add_test` inside a message
//! string does not count (the lesson of the maven pack's review). The
//! reader returns `None` for a file it cannot follow (an unclosed
//! parenthesis, quote, bracket or bracket comment), so nothing is decided or
//! edited on a guess. It does not evaluate anything: variables, `if()`,
//! functions and `include()` are read as written.

/// One command invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// Its name, in lower case (CMake's command names ignore case).
    pub name: String,
    /// Its arguments as written, without quotes or brackets. A nested
    /// parenthesis is an argument of its own (`(` or `)`).
    pub args: Vec<String>,
    /// How deep it is in blocks: 0 at the top of the file, 1 inside an
    /// `if()`, `foreach()`, `while()`, `function()`, `macro()` or `block()`,
    /// and so on. The command that opens or closes a block is outside it.
    pub depth: usize,
    /// Where its name starts.
    pub start: usize,
    /// After its closing parenthesis.
    pub end: usize,
}

/// Commands that open a block, and the ones that close each.
const BLOCKS: [(&str, &str); 6] = [
    ("if", "endif"),
    ("foreach", "endforeach"),
    ("while", "endwhile"),
    ("function", "endfunction"),
    ("macro", "endmacro"),
    ("block", "endblock"),
];

impl Command {
    /// True when it runs every time the file is read: at the top, not in a
    /// block.
    #[must_use]
    pub fn top_level(&self) -> bool {
        self.depth == 0
    }

    /// True when any argument is `word`, ignoring case.
    #[must_use]
    pub fn has_arg(&self, word: &str) -> bool {
        self.args.iter().any(|arg| arg.eq_ignore_ascii_case(word))
    }
}

/// The length of a bracket opening `[`, `=`…, `[` at `at`, and its number
/// of `=`, or `None` when there is none.
fn bracket_open(bytes: &[u8], at: usize) -> Option<(usize, usize)> {
    if bytes.get(at) != Some(&b'[') {
        return None;
    }
    let mut level = 0;
    while bytes.get(at + 1 + level) == Some(&b'=') {
        level += 1;
    }
    (bytes.get(at + 1 + level) == Some(&b'[')).then_some((level + 2, level))
}

/// After the bracket close `]`, `=`×level, `]` that ends the bracket whose
/// content starts at `from`, with the content's end.
fn bracket_close(text: &str, from: usize, level: usize) -> Option<(usize, usize)> {
    let close = format!("]{}]", "=".repeat(level));
    text.get(from..)?
        .find(&close)
        .map(|offset| (from + offset, from + offset + close.len()))
}

/// After the comment that starts with `#` at `at`.
fn skip_comment(text: &str, at: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if let Some((open, level)) = bracket_open(bytes, at + 1) {
        return bracket_close(text, at + 1 + open, level).map(|(_, end)| end);
    }
    Some(
        text[at..]
            .find('\n')
            .map_or(text.len(), |offset| at + offset),
    )
}

/// The commands of a CMake file, in order, or `None` when it cannot be
/// read.
#[must_use]
pub fn commands(text: &str) -> Option<Vec<Command>> {
    let bytes = text.as_bytes();
    let mut found: Vec<Command> = Vec::new();
    let mut depth = 0usize;
    // CMake needs a line ending after a command before the next one.
    let mut line_ended = true;
    let mut at = 0;
    // A byte-order mark is not part of the first command's name.
    if text.starts_with('\u{feff}') {
        at = '\u{feff}'.len_utf8();
    }
    while at < bytes.len() {
        let byte = bytes[at];
        if byte.is_ascii_whitespace() {
            line_ended |= byte == b'\n';
            at += 1;
        } else if byte == b'#' {
            at = skip_comment(text, at)?;
        } else if byte.is_ascii_alphabetic() || byte == b'_' {
            if !line_ended {
                return None;
            }
            let start = at;
            while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_') {
                at += 1;
            }
            let name = text[start..at].to_ascii_lowercase();
            while at < bytes.len() && matches!(bytes[at], b' ' | b'\t') {
                at += 1;
            }
            if bytes.get(at) != Some(&b'(') {
                return None;
            }
            let (args, end) = arguments(text, at + 1)?;
            let closes = BLOCKS.iter().any(|(_, close)| *close == name);
            let opens = BLOCKS.iter().any(|(open, _)| *open == name);
            let continues = matches!(name.as_str(), "else" | "elseif");
            if closes || continues {
                depth = depth.checked_sub(1)?;
            }
            found.push(Command {
                name,
                args,
                depth,
                start,
                end,
            });
            if opens || continues {
                depth += 1;
            }
            line_ended = false;
            at = end;
        } else {
            return None;
        }
    }
    Some(found)
}

/// The arguments of a command whose `(` ends just before `at`, and where
/// the command ends (after its `)`).
fn arguments(text: &str, mut at: usize) -> Option<(Vec<String>, usize)> {
    let bytes = text.as_bytes();
    let mut args = Vec::new();
    let mut depth = 0usize;
    loop {
        let byte = *bytes.get(at)?;
        match byte {
            b'#' => at = skip_comment(text, at)?,
            b'(' => {
                depth += 1;
                args.push("(".to_owned());
                at += 1;
            }
            b')' => {
                at += 1;
                if depth == 0 {
                    return Some((args, at));
                }
                depth -= 1;
                args.push(")".to_owned());
            }
            b'"' => {
                let start = at + 1;
                at = start;
                loop {
                    match *bytes.get(at)? {
                        b'\\' => at += 2,
                        b'"' => break,
                        _ => at += 1,
                    }
                }
                args.push(text.get(start..at)?.to_owned());
                at += 1;
            }
            b'[' if bracket_open(bytes, at).is_some() => {
                let (open, level) = bracket_open(bytes, at)?;
                let (content_end, end) = bracket_close(text, at + open, level)?;
                args.push(text.get(at + open..content_end)?.to_owned());
                at = end;
            }
            _ if byte.is_ascii_whitespace() => at += 1,
            _ => {
                // An unquoted argument: up to whitespace, a parenthesis, a
                // comment or a quote; `\` escapes the next character.
                let start = at;
                while let Some(&b) = bytes.get(at) {
                    if b.is_ascii_whitespace() || matches!(b, b'(' | b')' | b'#' | b'"') {
                        break;
                    }
                    at += 1;
                    if b == b'\\' {
                        at += text.get(at..)?.chars().next().map_or(0, char::len_utf8);
                    }
                }
                args.push(text.get(start..at)?.to_owned());
            }
        }
    }
}

/// The version a `cmake_minimum_required(VERSION …)` asks for, as written,
/// and the one CMake takes its policies from, as (major, minor): the
/// minimum, or with a range (`3.5...3.10`) its maximum. The first call at
/// the top of the file; with none there, the newest of those in blocks
/// (`if(CMAKE_VERSION VERSION_LESS 3.12)` picks one by the CMake running:
/// found in review).
#[must_use]
pub fn required_version(commands: &[Command]) -> Option<(String, (u32, u32))> {
    let calls: Vec<(String, (u32, u32))> = commands
        .iter()
        .filter(|command| command.name == "cmake_minimum_required" && command.top_level())
        .take(1)
        .filter_map(version_of)
        .collect();
    if let Some(first) = calls.into_iter().next() {
        return Some(first);
    }
    commands
        .iter()
        .filter(|command| command.name == "cmake_minimum_required")
        .filter_map(version_of)
        .max_by_key(|(_, version)| *version)
}

fn version_of(call: &Command) -> Option<(String, (u32, u32))> {
    let at = call
        .args
        .iter()
        .position(|arg| arg.eq_ignore_ascii_case("VERSION"))?;
    let written = call.args.get(at + 1)?.clone();
    let policy = written.rsplit("...").next().unwrap_or(&written);
    let mut parts = policy.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().map_or(Some(0), |minor| minor.parse().ok())?;
    Some((written, (major, minor)))
}

/// The languages a `project()` call enables, as written: those after
/// `LANGUAGES`, or in the older form those after the name; C and CXX when
/// it names none (CMake's default), nothing for `NONE`.
#[must_use]
pub fn project_languages(project: &Command) -> Vec<String> {
    const KEYWORDS: [&str; 4] = ["VERSION", "DESCRIPTION", "HOMEPAGE_URL", "LANGUAGES"];
    let args = project.args.get(1..).unwrap_or_default();
    let listed: Vec<&String> = match args.iter().position(|arg| arg == "LANGUAGES") {
        Some(at) => args[at + 1..]
            .iter()
            .take_while(|arg| !KEYWORDS.contains(&arg.as_str()))
            .collect(),
        None if args
            .first()
            .is_some_and(|arg| KEYWORDS.contains(&arg.as_str())) =>
        {
            Vec::new()
        }
        None => args
            .iter()
            .take_while(|arg| !KEYWORDS.contains(&arg.as_str()))
            .collect(),
    };
    if listed.is_empty() {
        return vec!["C".to_owned(), "CXX".to_owned()];
    }
    // Language names are case-sensitive: `cxx` is no language.
    listed
        .into_iter()
        .filter(|language| language.as_str() != "NONE")
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(text: &str) -> Vec<String> {
        commands(text)
            .unwrap()
            .into_iter()
            .map(|command| command.name)
            .collect()
    }

    #[test]
    fn comments_strings_and_brackets_are_not_commands() {
        let text = "# add_test(a)\n#[[ enable_testing()\n]]\nCMake_Minimum_Required(VERSION 3.20)\nmessage(\"add_test(x) )\" [=[ ) ]=])\n#[==[ project(x) ]==]\nproject (app # a comment ) here\n  LANGUAGES CXX)\nif((A) AND B)\nendif()\n";
        assert_eq!(
            names(text),
            [
                "cmake_minimum_required",
                "message",
                "project",
                "if",
                "endif"
            ]
        );
        let all = commands(text).unwrap();
        assert_eq!(all[1].args, ["add_test(x) )", " ) "]);
        assert_eq!(all[2].args, ["app", "LANGUAGES", "CXX"]);
        assert_eq!(all[3].args, ["(", "A", ")", "AND", "B"]);
        assert_eq!(
            &text[all[2].start..all[2].end],
            "project (app # a comment ) here\n  LANGUAGES CXX)"
        );
    }

    #[test]
    fn blocks_have_depth_and_a_line_holds_one_command() {
        let text = "if(A)\n  project(x)\nelseif(B)\n  function(f)\n    add_test(NAME t COMMAND t)\n  endfunction()\nelse()\nendif()\nproject(y)\n";
        let depths: Vec<(String, usize)> = commands(text)
            .unwrap()
            .into_iter()
            .map(|command| (command.name, command.depth))
            .collect();
        let expected = [
            ("if", 0),
            ("project", 1),
            ("elseif", 0),
            ("function", 1),
            ("add_test", 2),
            ("endfunction", 1),
            ("else", 0),
            ("endif", 0),
            ("project", 0),
        ];
        assert_eq!(
            depths,
            expected.map(|(name, depth)| (name.to_owned(), depth))
        );
        // CMake: "Expected a newline".
        assert_eq!(commands("project(x) set(A 1)\n"), None);
        assert_eq!(
            names("project(x) #[[ c ]] # d\nset(A 1)\n"),
            ["project", "set"]
        );
        assert_eq!(commands("endif()\n"), None);
    }

    #[test]
    fn escapes_and_crlf_and_a_bom() {
        let text = "\u{feff}set(A \"a\\\"b\" c\\ d)\r\nproject(x)\r\n";
        let all = commands(text).unwrap();
        assert_eq!(all[0].args, ["A", "a\\\"b", "c\\ d"]);
        assert_eq!(all[1].name, "project");
    }

    #[test]
    fn a_file_it_cannot_follow_is_none() {
        for text in [
            "project(x",
            "set(A \"x)",
            "set(A [[x)",
            "#[[ open",
            "project x",
            "(x)",
            "if((A)",
        ] {
            assert_eq!(commands(text), None, "{text}");
        }
        assert_eq!(commands("").unwrap(), Vec::new());
        assert_eq!(commands("caf\u{e9}(x)"), None);
        assert_eq!(names("message(caf\u{e9})"), ["message"]);
    }

    #[test]
    fn a_projects_languages_default_to_c_and_cxx() {
        let languages = |text: &str| {
            let all = commands(text).unwrap();
            project_languages(&all[0]).join(",")
        };
        assert_eq!(languages("project(x)"), "C,CXX");
        assert_eq!(languages("project(x VERSION 1.0)"), "C,CXX");
        assert_eq!(languages("project(x LANGUAGES C)"), "C");
        assert_eq!(
            languages("project(x VERSION 1 LANGUAGES C CXX DESCRIPTION d)"),
            "C,CXX"
        );
        assert_eq!(languages("project(x C)"), "C");
        assert_eq!(languages("project(x cxx)"), "cxx");
        assert_eq!(languages("project(x NONE)"), "");
    }

    #[test]
    fn the_policy_version_is_a_ranges_maximum() {
        let version = |text: &str| required_version(&commands(text).unwrap()).map(|v| v.1);
        assert_eq!(
            version("cmake_minimum_required(VERSION 3.20)"),
            Some((3, 20))
        );
        assert_eq!(
            version("cmake_minimum_required(VERSION 2.8.12)"),
            Some((2, 8))
        );
        assert_eq!(version("cmake_minimum_required(VERSION 3)"), Some((3, 0)));
        assert_eq!(
            version("cmake_minimum_required(VERSION 2.8...3.10 FATAL_ERROR)"),
            Some((3, 10))
        );
        assert_eq!(
            version("cmake_minimum_required(version \"3.5\")"),
            Some((3, 5))
        );
        assert_eq!(version("cmake_minimum_required(VERSION ${V})"), None);
        assert_eq!(version("project(x)"), None);
        assert_eq!(
            version("if(CMAKE_VERSION VERSION_LESS 3.12)\n  cmake_minimum_required(VERSION 3.1)\nelse()\n  cmake_minimum_required(VERSION 3.12)\nendif()\n"),
            Some((3, 12))
        );
        assert_eq!(
            version("cmake_minimum_required(VERSION 3.20)\nif(X)\n  cmake_minimum_required(VERSION 2.8)\nendif()\n"),
            Some((3, 20))
        );
    }
}
