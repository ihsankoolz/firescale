//! Config files read as flat entries (M25, ADR 0045): every scalar value
//! with the path of keys that leads to it and its line, whatever the format.
//! The config and CI detectors of the tamper guard query these entries, so
//! each of them reads JSON, JSONC, YAML, TOML, INI and JavaScript object
//! literals the same way.
//!
//! The readers are small and tolerant, written for the config files the
//! detectors read, not full parsers:
//!
//! - JSON with comments and trailing commas (`tsconfig.json`, `.eslintrc`).
//! - YAML in block style, with flow lists and maps on one line, quoted
//!   strings, block scalars (`|`, `>`) and comments. Anchors and aliases
//!   are kept as text.
//! - TOML: tables, arrays of tables, dotted and quoted keys, strings of
//!   every kind, numbers, booleans, arrays over several lines and inline
//!   tables.
//! - INI: sections, `key = value` or `key: value`, indented continuation
//!   lines, `#` and `;` comments.
//! - JavaScript and TypeScript: every object or array literal that is not
//!   inside another one, read with tree-sitter; values that are not
//!   literals are kept as their source text.
//!
//! A list item's path segment is its index in brackets (`[0]`).

use tree_sitter::Node;

use crate::imports::{parse, text, JsDialect};

/// A scalar value.
#[derive(Debug, Clone, PartialEq)]
pub enum Scalar {
    Str(String),
    Num(f64),
    Bool(bool),
    Null,
    /// Anything else, as its source text: a JS expression, a YAML alias, a
    /// TOML date.
    Other(String),
}

impl Scalar {
    /// The value as text: a string as it is, a number without a needless
    /// `.0`, anything else as written.
    #[must_use]
    pub fn as_text(&self) -> String {
        match self {
            Self::Str(text) | Self::Other(text) => text.clone(),
            Self::Num(number) => number_text(*number),
            Self::Bool(value) => value.to_string(),
            Self::Null => "null".to_owned(),
        }
    }

    /// The value as a number: a number, or a string that is one.
    #[must_use]
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Self::Num(number) => Some(*number),
            Self::Str(text) | Self::Other(text) => text.trim().parse().ok(),
            _ => None,
        }
    }

    /// The value as a boolean: a boolean, or a string `true`/`false` (INI
    /// and YAML write them so).
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            Self::Str(text) | Self::Other(text) => {
                match text.trim().to_ascii_lowercase().as_str() {
                    "true" | "yes" | "on" | "1" => Some(true),
                    "false" | "no" | "off" | "0" => Some(false),
                    _ => None,
                }
            }
            Self::Num(number) => Some(*number != 0.0),
            Self::Null => None,
        }
    }
}

/// A number as text, `80` rather than `80.0`.
#[must_use]
pub fn number_text(number: f64) -> String {
    if number.fract() == 0.0 && number.abs() < 1e15 {
        format!("{}", number as i64)
    } else {
        format!("{number}")
    }
}

/// One scalar value and the keys that lead to it.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Keys from the outside in; a list item's segment is `[<index>]`.
    pub path: Vec<String>,
    pub value: Scalar,
    /// 1-based line of the value (of its key for an INI value over several
    /// lines; of its first content line for a YAML block scalar).
    pub line: usize,
}

impl Entry {
    /// The path without list indexes, joined with `.`: `rules.no-console`.
    #[must_use]
    pub fn keys(&self) -> String {
        self.path
            .iter()
            .filter(|segment| !is_index(segment))
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(".")
    }

    /// True when the path, without list indexes, ends with `suffix`.
    #[must_use]
    pub fn ends_with(&self, suffix: &[&str]) -> bool {
        let keys: Vec<&str> = self
            .path
            .iter()
            .filter(|segment| !is_index(segment))
            .map(String::as_str)
            .collect();
        keys.len() >= suffix.len() && keys[keys.len() - suffix.len()..] == *suffix
    }
}

/// True for a list item's path segment, `[3]`.
#[must_use]
pub fn is_index(segment: &str) -> bool {
    segment.len() > 2
        && segment.starts_with('[')
        && segment.ends_with(']')
        && segment[1..segment.len() - 1]
            .bytes()
            .all(|b| b.is_ascii_digit())
}

/// The formats the readers know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// JSON, also with comments and trailing commas.
    Json,
    Yaml,
    Toml,
    Ini,
    Js(JsDialect),
}

/// The entries of a file, or the 1-based line where it cannot be read.
///
/// # Errors
///
/// The line of the first thing the reader cannot make sense of.
pub fn read(format: Format, source: &str) -> Result<Vec<Entry>, usize> {
    let source = source.trim_start_matches('\u{feff}');
    match format {
        Format::Json => json::read(source),
        Format::Yaml => yaml::read(source),
        Format::Toml => toml::read(source),
        Format::Ini => Ok(ini::read(source)),
        Format::Js(dialect) => js::read_roots(source, dialect).map(|roots| roots.concat()),
    }
}

/// The entries of a file in groups, one for each outermost JavaScript
/// object or array literal (so two configs passed to one call, such as
/// `tseslint.config({ … }, { … })`, stay apart although their paths look
/// alike), or one group for any other format.
///
/// # Errors
///
/// The line of the first thing the reader cannot make sense of.
pub fn read_roots(format: Format, source: &str) -> Result<Vec<Vec<Entry>>, usize> {
    match format {
        Format::Js(dialect) => js::read_roots(source.trim_start_matches('\u{feff}'), dialect),
        _ => read(format, source).map(|entries| vec![entries]),
    }
}

// ------------------------------------------------------------------- JSON

mod json {
    use super::{Entry, Scalar};

    struct Reader<'a> {
        bytes: &'a [u8],
        text: &'a str,
        at: usize,
        line: usize,
    }

    pub(super) fn read(source: &str) -> Result<Vec<Entry>, usize> {
        let mut reader = Reader {
            bytes: source.as_bytes(),
            text: source,
            at: 0,
            line: 1,
        };
        let mut entries = Vec::new();
        reader.blank()?;
        if reader.at >= reader.bytes.len() {
            return Ok(entries);
        }
        reader.value(&mut Vec::new(), &mut entries, 0)?;
        reader.blank()?;
        if reader.at < reader.bytes.len() {
            return Err(reader.line);
        }
        Ok(entries)
    }

    impl Reader<'_> {
        fn peek(&self) -> Option<u8> {
            self.bytes.get(self.at).copied()
        }

        fn bump(&mut self) -> Option<u8> {
            let byte = self.peek()?;
            self.at += 1;
            if byte == b'\n' {
                self.line += 1;
            }
            Some(byte)
        }

        /// Skips white space and comments.
        fn blank(&mut self) -> Result<(), usize> {
            loop {
                match self.peek() {
                    Some(b' ' | b'\t' | b'\r' | b'\n') => {
                        self.bump();
                    }
                    Some(b'/') if self.bytes.get(self.at + 1) == Some(&b'/') => {
                        while let Some(byte) = self.peek() {
                            if byte == b'\n' {
                                break;
                            }
                            self.bump();
                        }
                    }
                    Some(b'/') if self.bytes.get(self.at + 1) == Some(&b'*') => {
                        let start = self.line;
                        self.at += 2;
                        loop {
                            match self.bump() {
                                None => return Err(start),
                                Some(b'*') if self.peek() == Some(b'/') => {
                                    self.at += 1;
                                    break;
                                }
                                Some(_) => {}
                            }
                        }
                    }
                    _ => return Ok(()),
                }
            }
        }

        fn value(
            &mut self,
            path: &mut Vec<String>,
            entries: &mut Vec<Entry>,
            depth: usize,
        ) -> Result<(), usize> {
            if depth > 200 {
                return Err(self.line);
            }
            self.blank()?;
            let line = self.line;
            match self.peek() {
                Some(b'{') => {
                    self.bump();
                    loop {
                        self.blank()?;
                        match self.peek() {
                            Some(b'}') => {
                                self.bump();
                                return Ok(());
                            }
                            Some(b'"' | b'\'') => {}
                            _ => return Err(self.line),
                        }
                        let key = self.string()?;
                        self.blank()?;
                        if self.bump() != Some(b':') {
                            return Err(self.line);
                        }
                        path.push(key);
                        self.value(path, entries, depth + 1)?;
                        path.pop();
                        self.blank()?;
                        match self.bump() {
                            Some(b',') => {}
                            Some(b'}') => return Ok(()),
                            _ => return Err(self.line),
                        }
                    }
                }
                Some(b'[') => {
                    self.bump();
                    let mut index = 0;
                    loop {
                        self.blank()?;
                        if self.peek() == Some(b']') {
                            self.bump();
                            return Ok(());
                        }
                        path.push(format!("[{index}]"));
                        self.value(path, entries, depth + 1)?;
                        path.pop();
                        index += 1;
                        self.blank()?;
                        match self.bump() {
                            Some(b',') => {}
                            Some(b']') => return Ok(()),
                            _ => return Err(self.line),
                        }
                    }
                }
                Some(b'"' | b'\'') => {
                    let value = self.string()?;
                    entries.push(Entry {
                        path: path.clone(),
                        value: Scalar::Str(value),
                        line,
                    });
                    Ok(())
                }
                Some(_) => {
                    let start = self.at;
                    while let Some(byte) = self.peek() {
                        if matches!(
                            byte,
                            b',' | b'}' | b']' | b' ' | b'\t' | b'\r' | b'\n' | b'/'
                        ) {
                            break;
                        }
                        self.bump();
                    }
                    let word = &self.text[start..self.at];
                    let value = match word {
                        "true" => Scalar::Bool(true),
                        "false" => Scalar::Bool(false),
                        "null" => Scalar::Null,
                        _ => Scalar::Num(word.parse().map_err(|_| line)?),
                    };
                    entries.push(Entry {
                        path: path.clone(),
                        value,
                        line,
                    });
                    Ok(())
                }
                None => Err(self.line),
            }
        }

        fn string(&mut self) -> Result<String, usize> {
            let quote = self.bump().ok_or(self.line)?;
            let mut out = String::new();
            let start_line = self.line;
            loop {
                let rest = &self.text[self.at..];
                let Some(c) = rest.chars().next() else {
                    return Err(start_line);
                };
                self.at += c.len_utf8();
                match c {
                    '\n' => return Err(start_line),
                    '\\' => {
                        let Some(escaped) = self.text[self.at..].chars().next() else {
                            return Err(start_line);
                        };
                        self.at += escaped.len_utf8();
                        match escaped {
                            'n' => out.push('\n'),
                            't' => out.push('\t'),
                            'r' => out.push('\r'),
                            'b' => out.push('\u{8}'),
                            'f' => out.push('\u{c}'),
                            'u' => {
                                let hex = self.text.get(self.at..self.at + 4).ok_or(start_line)?;
                                self.at += 4;
                                let code = u32::from_str_radix(hex, 16).map_err(|_| start_line)?;
                                out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                            }
                            other => out.push(other),
                        }
                    }
                    c if c as u32 == u32::from(quote) => return Ok(out),
                    c => out.push(c),
                }
            }
        }
    }
}

// ------------------------------------------------------------------- YAML

mod yaml {
    use super::{Entry, Scalar};

    /// One line that holds something: its 1-based number, indentation and
    /// text without the comment.
    #[derive(Clone, Copy)]
    struct Line<'a> {
        number: usize,
        indent: usize,
        text: &'a str,
    }

    pub(super) fn read(source: &str) -> Result<Vec<Entry>, usize> {
        let raw: Vec<&str> = source.lines().collect();
        let mut lines: Vec<Line<'_>> = Vec::new();
        for (index, line) in raw.iter().enumerate() {
            let number = index + 1;
            let trimmed = line.trim_end();
            let content = trimmed.trim_start_matches(' ');
            if content.starts_with('\t') {
                return Err(number);
            }
            let content = strip_comment(content).trim_end();
            if content.is_empty() || content == "---" || content == "..." {
                continue;
            }
            if content.starts_with('%') {
                continue;
            }
            lines.push(Line {
                number,
                indent: trimmed.len() - trimmed.trim_start_matches(' ').len(),
                text: content,
            });
        }
        if lines.is_empty() {
            return Ok(Vec::new());
        }
        let indent = lines[0].indent;
        let mut reader = Reader {
            raw: &raw,
            lines,
            at: 0,
            entries: Vec::new(),
        };
        reader.node(indent, &mut Vec::new(), 0)?;
        if reader.at < reader.lines.len() {
            return Err(reader.lines[reader.at].number);
        }
        Ok(reader.entries)
    }

    /// The text before a `#` that starts a comment (at the start, or after
    /// a space, and outside quotes).
    pub(super) fn strip_comment(text: &str) -> &str {
        let mut quote: Option<char> = None;
        let mut previous = ' ';
        for (index, c) in text.char_indices() {
            match quote {
                Some(q) if c == q => quote = None,
                Some(_) => {}
                None if (c == '"' || c == '\'')
                    && (previous == ' '
                        || previous == ':'
                        || previous == '['
                        || previous == '{'
                        || previous == ','
                        || previous == '-'
                        || index == 0) =>
                {
                    quote = Some(c);
                }
                None if c == '#' && (previous == ' ' || previous == '\t' || index == 0) => {
                    return &text[..index];
                }
                None => {}
            }
            previous = c;
        }
        text
    }

    struct Reader<'a> {
        raw: &'a [&'a str],
        lines: Vec<Line<'a>>,
        at: usize,
        entries: Vec<Entry>,
    }

    /// Where `key: rest` splits, for a line that is a mapping entry.
    fn split_key(text: &str) -> Option<(String, &str)> {
        let (key, rest) = if text.starts_with('"') || text.starts_with('\'') {
            let quote = text.chars().next()?;
            let end = text[1..].find(quote)? + 1;
            let after = &text[end + 1..];
            let after = after.trim_start();
            let rest = after.strip_prefix(':')?;
            if !(rest.is_empty() || rest.starts_with(' ')) {
                return None;
            }
            (text[1..end].to_owned(), rest)
        } else {
            let mut found = None;
            let bytes = text.as_bytes();
            for (index, byte) in bytes.iter().enumerate() {
                if *byte == b':' && (index + 1 == bytes.len() || bytes[index + 1] == b' ') {
                    found = Some(index);
                    break;
                }
                if matches!(*byte, b'[' | b'{' | b'"' | b'\'') && index == 0 {
                    return None;
                }
            }
            let index = found?;
            (text[..index].trim().to_owned(), &text[index + 1..])
        };
        Some((key, rest.trim_start()))
    }

    impl Reader<'_> {
        /// Reads the block at `indent`: a mapping or a sequence.
        fn node(
            &mut self,
            indent: usize,
            path: &mut Vec<String>,
            depth: usize,
        ) -> Result<(), usize> {
            if depth > 100 {
                return Err(self.lines.get(self.at).map_or(1, |l| l.number));
            }
            let Some(first) = self.lines.get(self.at) else {
                return Ok(());
            };
            if first.text == "-" || first.text.starts_with("- ") {
                self.sequence(indent, path, depth)
            } else {
                self.mapping(indent, path, depth)
            }
        }

        fn sequence(
            &mut self,
            indent: usize,
            path: &mut Vec<String>,
            depth: usize,
        ) -> Result<(), usize> {
            let mut index = 0;
            while let Some(&line) = self.lines.get(self.at) {
                if line.indent < indent {
                    break;
                }
                if line.indent > indent {
                    return Err(line.number);
                }
                if !(line.text == "-" || line.text.starts_with("- ")) {
                    break;
                }
                let rest = line.text[1..].trim_start();
                path.push(format!("[{index}]"));
                index += 1;
                if rest.is_empty() {
                    self.at += 1;
                    if let Some(next) = self.lines.get(self.at) {
                        if next.indent > indent {
                            let inner = next.indent;
                            self.node(inner, path, depth + 1)?;
                        }
                    }
                } else {
                    // `- key: value` opens a mapping whose keys sit where
                    // `key` does; `- - x` a nested sequence; else a scalar.
                    let inner = indent + (line.text.len() - rest.len());
                    let is_entry = split_key(rest).is_some()
                        && !rest.starts_with('[')
                        && !rest.starts_with('{');
                    if is_entry || rest == "-" || rest.starts_with("- ") {
                        // Read the item as if `rest` stood on its own line.
                        self.lines[self.at].indent = inner;
                        self.lines[self.at].text = rest;
                        self.node(inner, path, depth + 1)?;
                    } else {
                        self.at += 1;
                        self.scalar_value(rest, line.number, indent, path)?;
                    }
                }
                path.pop();
            }
            Ok(())
        }

        fn mapping(
            &mut self,
            indent: usize,
            path: &mut Vec<String>,
            depth: usize,
        ) -> Result<(), usize> {
            while let Some(&line) = self.lines.get(self.at) {
                if line.indent < indent {
                    break;
                }
                if line.indent > indent {
                    return Err(line.number);
                }
                if line.text == "-" || line.text.starts_with("- ") {
                    break;
                }
                let Some((key, rest)) = split_key(line.text) else {
                    return Err(line.number);
                };
                let number = line.number;
                self.at += 1;
                path.push(key);
                if rest.is_empty() {
                    if let Some(next) = self.lines.get(self.at) {
                        // A sequence may sit at the key's own indentation.
                        let is_item = next.text == "-" || next.text.starts_with("- ");
                        if next.indent > indent || (next.indent == indent && is_item) {
                            let inner = next.indent;
                            self.node(inner, path, depth + 1)?;
                        } else {
                            self.entries.push(Entry {
                                path: path.clone(),
                                value: Scalar::Null,
                                line: number,
                            });
                        }
                    } else {
                        self.entries.push(Entry {
                            path: path.clone(),
                            value: Scalar::Null,
                            line: number,
                        });
                    }
                } else {
                    self.scalar_value(rest, number, indent, path)?;
                }
                path.pop();
            }
            Ok(())
        }

        /// A value written after `key:` or `- `: a block scalar, a flow
        /// collection, a quoted or plain scalar (plain ones may continue on
        /// more indented lines).
        fn scalar_value(
            &mut self,
            rest: &str,
            number: usize,
            indent: usize,
            path: &mut Vec<String>,
        ) -> Result<(), usize> {
            let rest = rest.trim();
            // Tags and anchors before the value are left aside.
            let rest = rest
                .split_once(' ')
                .filter(|(head, _)| head.starts_with('!') || head.starts_with('&'))
                .map_or(rest, |(_, tail)| tail.trim_start());
            if rest.starts_with('|') || rest.starts_with('>') {
                // Every raw line after this one that is more indented than
                // the key, or blank, until one that is not.
                let mut body: Vec<&str> = Vec::new();
                let mut first_line = None;
                let mut block_indent = None;
                let mut line_index = number; // raw index of the next line
                while line_index < self.raw.len() {
                    let raw = self.raw[line_index];
                    let trimmed = raw.trim_start_matches(' ');
                    let this_indent = raw.len() - trimmed.len();
                    if trimmed.trim().is_empty() {
                        body.push("");
                        line_index += 1;
                        continue;
                    }
                    if this_indent <= indent {
                        break;
                    }
                    let block = *block_indent.get_or_insert(this_indent);
                    if this_indent < block {
                        break;
                    }
                    first_line.get_or_insert(line_index + 1);
                    body.push(&raw[block.min(raw.len())..]);
                    line_index += 1;
                }
                while body.last() == Some(&"") {
                    body.pop();
                }
                // Skip the parsed lines that belong to the block.
                while self
                    .lines
                    .get(self.at)
                    .is_some_and(|l| l.number <= line_index)
                {
                    self.at += 1;
                }
                // Folded (`>`) or literal (`|`): the lines are kept as they are,
                // which is all the detectors need.
                let text = body.join("\n");
                self.entries.push(Entry {
                    path: path.clone(),
                    value: Scalar::Str(text),
                    line: first_line.unwrap_or(number),
                });
                return Ok(());
            }
            if rest.starts_with('[') || rest.starts_with('{') {
                // A flow collection, maybe over several lines.
                let mut text = rest.to_owned();
                while !balanced(&text) {
                    let Some(next) = self.lines.get(self.at) else {
                        return Err(number);
                    };
                    text.push(' ');
                    text.push_str(next.text);
                    self.at += 1;
                }
                return flow(&text, number, path, &mut self.entries);
            }
            if rest.starts_with('"') || rest.starts_with('\'') {
                let quote = rest.chars().next().unwrap_or('"');
                let mut text = rest.to_owned();
                while !closed_quote(&text, quote) {
                    let Some(next) = self.lines.get(self.at) else {
                        return Err(number);
                    };
                    text.push(' ');
                    text.push_str(next.text.trim());
                    self.at += 1;
                }
                self.entries.push(Entry {
                    path: path.clone(),
                    value: Scalar::Str(unquote(&text)),
                    line: number,
                });
                return Ok(());
            }
            // A plain scalar, continued on more indented lines that are not
            // entries of their own.
            let mut text = rest.to_owned();
            while let Some(next) = self.lines.get(self.at) {
                if next.indent <= indent {
                    break;
                }
                text.push(' ');
                text.push_str(next.text.trim());
                self.at += 1;
            }
            self.entries.push(Entry {
                path: path.clone(),
                value: plain(&text),
                line: number,
            });
            Ok(())
        }
    }

    fn closed_quote(text: &str, quote: char) -> bool {
        let mut chars = text.chars().skip(1).peekable();
        while let Some(c) = chars.next() {
            if quote == '"' && c == '\\' {
                chars.next();
                continue;
            }
            if c == quote {
                if quote == '\'' && chars.peek() == Some(&'\'') {
                    chars.next();
                    continue;
                }
                return true;
            }
        }
        false
    }

    fn balanced(text: &str) -> bool {
        let mut depth = 0i32;
        let mut quote: Option<char> = None;
        for c in text.chars() {
            match quote {
                Some(q) if c == q => quote = None,
                Some(_) => {}
                None => match c {
                    '"' | '\'' => quote = Some(c),
                    '[' | '{' => depth += 1,
                    ']' | '}' => depth -= 1,
                    _ => {}
                },
            }
        }
        depth <= 0
    }

    pub(super) fn unquote(text: &str) -> String {
        let text = text.trim();
        if text.len() >= 2 && text.starts_with('"') && text.ends_with('"') {
            let inner = &text[1..text.len() - 1];
            let mut out = String::new();
            let mut chars = inner.chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    match chars.next() {
                        Some('n') => out.push('\n'),
                        Some('t') => out.push('\t'),
                        Some(other) => out.push(other),
                        None => {}
                    }
                } else {
                    out.push(c);
                }
            }
            out
        } else if text.len() >= 2 && text.starts_with('\'') && text.ends_with('\'') {
            text[1..text.len() - 1].replace("''", "'")
        } else {
            text.to_owned()
        }
    }

    /// A plain scalar: YAML 1.2's core schema for booleans, null and
    /// numbers, else a string.
    fn plain(text: &str) -> Scalar {
        let text = text.trim();
        match text {
            "true" | "True" | "TRUE" => Scalar::Bool(true),
            "false" | "False" | "FALSE" => Scalar::Bool(false),
            "null" | "Null" | "NULL" | "~" | "" => Scalar::Null,
            _ if text.starts_with('*') => Scalar::Other(text.to_owned()),
            _ => text
                .parse::<f64>()
                .ok()
                .filter(|_| {
                    // Rust also reads `inf`, `NaN` and `infinity`, which are
                    // strings in YAML (it writes `.inf` and `.nan`).
                    text.trim_start_matches('-')
                        .starts_with(|c: char| c.is_ascii_digit() || c == '.')
                        && !text.ends_with('.')
                })
                .map_or_else(|| Scalar::Str(text.to_owned()), Scalar::Num),
        }
    }

    /// Splits a flow collection's inside at top-level commas.
    fn split_items(inner: &str) -> Vec<&str> {
        let mut items = Vec::new();
        let mut depth = 0i32;
        let mut quote: Option<char> = None;
        let mut start = 0;
        for (index, c) in inner.char_indices() {
            match quote {
                Some(q) if c == q => quote = None,
                Some(_) => {}
                None => match c {
                    '"' | '\'' => quote = Some(c),
                    '[' | '{' => depth += 1,
                    ']' | '}' => depth -= 1,
                    ',' if depth == 0 => {
                        items.push(&inner[start..index]);
                        start = index + 1;
                    }
                    _ => {}
                },
            }
        }
        items.push(&inner[start..]);
        items
            .into_iter()
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .collect()
    }

    fn flow(
        text: &str,
        line: usize,
        path: &mut Vec<String>,
        entries: &mut Vec<Entry>,
    ) -> Result<(), usize> {
        let text = text.trim();
        if let Some(inner) = text.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
            for (index, item) in split_items(inner).into_iter().enumerate() {
                path.push(format!("[{index}]"));
                flow(item, line, path, entries)?;
                path.pop();
            }
            return Ok(());
        }
        if let Some(inner) = text.strip_prefix('{').and_then(|t| t.strip_suffix('}')) {
            for item in split_items(inner) {
                let (key, value) = match split_key(item) {
                    Some((key, value)) => (key, value),
                    None => (unquote(item.trim_end_matches(':')), ""),
                };
                path.push(key);
                if value.is_empty() {
                    entries.push(Entry {
                        path: path.clone(),
                        value: Scalar::Null,
                        line,
                    });
                } else {
                    flow(value, line, path, entries)?;
                }
                path.pop();
            }
            return Ok(());
        }
        if text.starts_with('[') || text.starts_with('{') {
            return Err(line);
        }
        let value = if text.starts_with('"') || text.starts_with('\'') {
            Scalar::Str(unquote(text))
        } else {
            plain(text)
        };
        entries.push(Entry {
            path: path.clone(),
            value,
            line,
        });
        Ok(())
    }
}

// ------------------------------------------------------------------- TOML

mod toml {
    use super::{Entry, Scalar};
    use std::collections::BTreeMap;

    pub(super) fn read(source: &str) -> Result<Vec<Entry>, usize> {
        let mut entries = Vec::new();
        let mut table: Vec<String> = Vec::new();
        let mut array_counts: BTreeMap<Vec<String>, usize> = BTreeMap::new();
        let lines: Vec<&str> = source.lines().collect();
        let mut index = 0;
        while index < lines.len() {
            let number = index + 1;
            let line = lines[index].trim();
            index += 1;
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(header) = line.strip_prefix("[[") {
                let header = header.split("]]").next().ok_or(number)?;
                let keys = dotted_keys(header).ok_or(number)?;
                let count = array_counts.entry(keys.clone()).or_insert(0);
                table = keys;
                table.push(format!("[{count}]"));
                *count += 1;
                continue;
            }
            if let Some(header) = line.strip_prefix('[') {
                let end = header.find(']').ok_or(number)?;
                table = dotted_keys(&header[..end]).ok_or(number)?;
                continue;
            }
            // key = value, the value maybe over several lines.
            let eq = find_eq(line).ok_or(number)?;
            let keys = dotted_keys(&line[..eq]).ok_or(number)?;
            let mut value = line[eq + 1..].trim().to_owned();
            // A multi-line string or array goes on until it closes.
            loop {
                let open = (value.starts_with("\"\"\"") && !closes(&value, "\"\"\""))
                    || (value.starts_with("'''") && !closes(&value, "'''"))
                    || ((value.starts_with('[') || value.starts_with('{')) && !balanced(&value));
                if !open {
                    break;
                }
                let Some(next) = lines.get(index) else {
                    return Err(number);
                };
                value.push('\n');
                value.push_str(next);
                index += 1;
            }
            let mut path = table.clone();
            path.extend(keys);
            value_entries(&value, number, &mut path, &mut entries)?;
        }
        Ok(entries)
    }

    fn closes(value: &str, quote: &str) -> bool {
        value.len() >= 6 && value[3..].contains(quote)
    }

    /// The index of the `=` that ends the key, outside quotes.
    fn find_eq(line: &str) -> Option<usize> {
        let mut quote: Option<char> = None;
        for (index, c) in line.char_indices() {
            match quote {
                Some(q) if c == q => quote = None,
                Some(_) => {}
                None => match c {
                    '"' | '\'' => quote = Some(c),
                    '=' => return Some(index),
                    _ => {}
                },
            }
        }
        None
    }

    fn dotted_keys(text: &str) -> Option<Vec<String>> {
        let mut keys = Vec::new();
        let mut current = String::new();
        let mut quote: Option<char> = None;
        for c in text.trim().chars() {
            match quote {
                Some(q) if c == q => quote = None,
                Some(_) => current.push(c),
                None => match c {
                    '"' | '\'' => quote = Some(c),
                    '.' => keys.push(std::mem::take(&mut current).trim().to_owned()),
                    c => current.push(c),
                },
            }
        }
        keys.push(current.trim().to_owned());
        (!keys.iter().any(String::is_empty)).then_some(keys)
    }

    fn balanced(text: &str) -> bool {
        let mut depth = 0i32;
        let mut quote: Option<char> = None;
        let mut comment = false;
        for c in text.chars() {
            if comment {
                if c == '\n' {
                    comment = false;
                }
                continue;
            }
            match quote {
                Some(q) if c == q => quote = None,
                Some(_) => {}
                None => match c {
                    '"' | '\'' => quote = Some(c),
                    '#' => comment = true,
                    '[' | '{' => depth += 1,
                    ']' | '}' => depth -= 1,
                    _ => {}
                },
            }
        }
        depth <= 0
    }

    /// Splits the inside of an array or inline table at top-level commas,
    /// with the line offset of each item.
    fn split_items(inner: &str) -> Vec<(usize, String)> {
        let mut items = Vec::new();
        let mut depth = 0i32;
        let mut quote: Option<char> = None;
        let mut comment = false;
        let mut current = String::new();
        let mut offset = 0;
        let mut item_offset = None;
        for c in inner.chars() {
            if c == '\n' {
                offset += 1;
                comment = false;
                current.push(c);
                continue;
            }
            if comment {
                continue;
            }
            match quote {
                Some(q) if c == q => {
                    quote = None;
                    current.push(c);
                }
                Some(_) => current.push(c),
                None => match c {
                    '#' => comment = true,
                    ',' if depth == 0 => {
                        items.push((item_offset.unwrap_or(offset), std::mem::take(&mut current)));
                        item_offset = None;
                    }
                    c => {
                        if !c.is_whitespace() {
                            item_offset.get_or_insert(offset);
                        }
                        match c {
                            '"' | '\'' => quote = Some(c),
                            '[' | '{' => depth += 1,
                            ']' | '}' => depth -= 1,
                            _ => {}
                        }
                        current.push(c);
                    }
                },
            }
        }
        items.push((item_offset.unwrap_or(offset), current));
        items
            .into_iter()
            .map(|(offset, item)| (offset, item.trim().to_owned()))
            .filter(|(_, item)| !item.is_empty())
            .collect()
    }

    /// The byte index of the bracket that closes the one `value` opens.
    fn closing(value: &str) -> Option<usize> {
        let mut depth = 0i32;
        let mut quote: Option<char> = None;
        let mut comment = false;
        for (index, c) in value.char_indices() {
            if comment {
                if c == '\n' {
                    comment = false;
                }
                continue;
            }
            match quote {
                Some(q) if c == q => quote = None,
                Some(_) => {}
                None => match c {
                    '"' | '\'' => quote = Some(c),
                    '#' => comment = true,
                    '[' | '{' => depth += 1,
                    ']' | '}' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(index);
                        }
                    }
                    _ => {}
                },
            }
        }
        None
    }

    fn strip_comment(value: &str) -> &str {
        let mut quote: Option<char> = None;
        for (index, c) in value.char_indices() {
            match quote {
                Some(q) if c == q => quote = None,
                Some(_) => {}
                None => match c {
                    '"' | '\'' => quote = Some(c),
                    '#' => return &value[..index],
                    _ => {}
                },
            }
        }
        value
    }

    fn value_entries(
        value: &str,
        line: usize,
        path: &mut Vec<String>,
        entries: &mut Vec<Entry>,
    ) -> Result<(), usize> {
        let value = value.trim();
        if let Some(inner) = value.strip_prefix("\"\"\"") {
            let inner = inner.rsplit_once("\"\"\"").map_or(inner, |(body, _)| body);
            entries.push(Entry {
                path: path.clone(),
                value: Scalar::Str(inner.trim_start_matches('\n').to_owned()),
                line,
            });
            return Ok(());
        }
        if let Some(inner) = value.strip_prefix("'''") {
            let inner = inner.rsplit_once("'''").map_or(inner, |(body, _)| body);
            entries.push(Entry {
                path: path.clone(),
                value: Scalar::Str(inner.trim_start_matches('\n').to_owned()),
                line,
            });
            return Ok(());
        }
        // A comment after the value; an array or inline table ends at its
        // closing bracket (comments inside it are skipped item by item).
        let value = if value.starts_with('[') || value.starts_with('{') {
            &value[..closing(value).map_or(value.len(), |end| end + 1)]
        } else {
            strip_comment(value).trim()
        };
        if let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
            for (index, (offset, item)) in split_items(inner).into_iter().enumerate() {
                path.push(format!("[{index}]"));
                value_entries(&item, line + offset, path, entries)?;
                path.pop();
            }
            return Ok(());
        }
        if let Some(inner) = value.strip_prefix('{').and_then(|v| v.strip_suffix('}')) {
            for (offset, item) in split_items(inner) {
                let eq = find_eq(&item).ok_or(line)?;
                let keys = dotted_keys(&item[..eq]).ok_or(line)?;
                let depth = path.len();
                path.extend(keys);
                value_entries(&item[eq + 1..], line + offset, path, entries)?;
                path.truncate(depth);
            }
            return Ok(());
        }
        let scalar = if let Some(inner) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"'))
        {
            Scalar::Str(inner.replace("\\\"", "\"").replace("\\\\", "\\"))
        } else if let Some(inner) = value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
            Scalar::Str(inner.to_owned())
        } else if value == "true" {
            Scalar::Bool(true)
        } else if value == "false" {
            Scalar::Bool(false)
        } else if let Ok(number) = value.replace('_', "").parse::<f64>() {
            Scalar::Num(number)
        } else if value.is_empty() || value.starts_with('"') || value.starts_with('\'') {
            return Err(line);
        } else {
            Scalar::Other(value.to_owned())
        };
        entries.push(Entry {
            path: path.clone(),
            value: scalar,
            line,
        });
        Ok(())
    }
}

// -------------------------------------------------------------------- INI

mod ini {
    use super::{Entry, Scalar};

    pub(super) fn read(source: &str) -> Vec<Entry> {
        let mut entries: Vec<Entry> = Vec::new();
        let mut section = String::new();
        let mut open: Option<usize> = None; // the entry continuation lines extend
        for (index, raw) in source.lines().enumerate() {
            let number = index + 1;
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                open = None;
                continue;
            }
            if trimmed.starts_with('#') || trimmed.starts_with(';') {
                continue;
            }
            let indented = raw.starts_with(' ') || raw.starts_with('\t');
            if indented {
                if let Some(at) = open {
                    if let Scalar::Str(value) = &mut entries[at].value {
                        if !value.is_empty() {
                            value.push('\n');
                        }
                        value.push_str(strip_inline_comment(trimmed));
                    }
                    continue;
                }
            }
            if let Some(name) = trimmed.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
                section = name.trim().to_owned();
                open = None;
                continue;
            }
            let split = trimmed
                .find(['=', ':'])
                .map(|at| (trimmed[..at].trim(), trimmed[at + 1..].trim()));
            let (key, value) = split.unwrap_or((trimmed, ""));
            entries.push(Entry {
                path: vec![section.clone(), key.to_owned()],
                value: Scalar::Str(strip_inline_comment(value).to_owned()),
                line: number,
            });
            open = Some(entries.len() - 1);
        }
        entries
    }

    /// `value  # note` and `value ; note` with white space before the mark.
    fn strip_inline_comment(value: &str) -> &str {
        for mark in [" #", "\t#", " ;", "\t;"] {
            if let Some(at) = value.find(mark) {
                return value[..at].trim_end();
            }
        }
        value
    }
}

// --------------------------------------------------------------------- JS

mod js {
    use super::{parse, text, Entry, JsDialect, Node, Scalar};
    use crate::imports::walk;

    pub(super) fn read_roots(source: &str, dialect: JsDialect) -> Result<Vec<Vec<Entry>>, usize> {
        let language = match dialect {
            JsDialect::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            JsDialect::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        };
        let tree = parse(source, &language).ok_or(1usize)?;
        let root = tree.root_node();
        if root.has_error() {
            let mut line = 1;
            walk(root, |node| {
                if line == 1 && (node.is_error() || node.is_missing()) {
                    line = node.start_position().row + 1;
                }
                node.has_error()
            });
            return Err(line);
        }
        let mut roots = Vec::new();
        walk(root, |node| {
            if matches!(node.kind(), "object" | "array") {
                let mut entries = Vec::new();
                literal(node, source, &mut Vec::new(), &mut entries, 0);
                roots.push(entries);
                return false;
            }
            true
        });
        Ok(roots)
    }

    fn key_text(node: Node<'_>, source: &str) -> String {
        match node.kind() {
            "string" => string_value(node, source)
                .unwrap_or_else(|| text(node, source.as_bytes()).to_owned()),
            _ => text(node, source.as_bytes()).to_owned(),
        }
    }

    fn string_value(node: Node<'_>, source: &str) -> Option<String> {
        match node.kind() {
            "string" => {
                let raw = text(node, source.as_bytes());
                Some(raw.get(1..raw.len().saturating_sub(1))?.to_owned())
            }
            "template_string" => {
                let mut cursor = node.walk();
                if node
                    .children(&mut cursor)
                    .any(|c| c.kind() == "template_substitution")
                {
                    return None;
                }
                let raw = text(node, source.as_bytes());
                Some(raw.get(1..raw.len().saturating_sub(1))?.to_owned())
            }
            _ => None,
        }
    }

    fn literal(
        node: Node<'_>,
        source: &str,
        path: &mut Vec<String>,
        entries: &mut Vec<Entry>,
        depth: usize,
    ) {
        let line = node.start_position().row + 1;
        if depth > 100 {
            return;
        }
        match node.kind() {
            "object" => {
                let mut cursor = node.walk();
                for child in node.named_children(&mut cursor) {
                    match child.kind() {
                        "pair" => {
                            let (Some(key), Some(value)) = (
                                child.child_by_field_name("key"),
                                child.child_by_field_name("value"),
                            ) else {
                                continue;
                            };
                            path.push(key_text(key, source));
                            literal(value, source, path, entries, depth + 1);
                            path.pop();
                        }
                        "shorthand_property_identifier" => {
                            path.push(text(child, source.as_bytes()).to_owned());
                            entries.push(Entry {
                                path: path.clone(),
                                value: Scalar::Other(text(child, source.as_bytes()).to_owned()),
                                line: child.start_position().row + 1,
                            });
                            path.pop();
                        }
                        _ => {}
                    }
                }
            }
            "array" => {
                let mut cursor = node.walk();
                let mut index = 0;
                for child in node.named_children(&mut cursor) {
                    if child.kind() == "comment" {
                        continue;
                    }
                    path.push(format!("[{index}]"));
                    index += 1;
                    literal(child, source, path, entries, depth + 1);
                    path.pop();
                }
            }
            "parenthesized_expression" | "as_expression" | "satisfies_expression" => {
                if let Some(inner) = node.named_child(0) {
                    literal(inner, source, path, entries, depth + 1);
                }
            }
            "string" | "template_string" => {
                let value = string_value(node, source).map_or_else(
                    || Scalar::Other(text(node, source.as_bytes()).to_owned()),
                    Scalar::Str,
                );
                entries.push(Entry {
                    path: path.clone(),
                    value,
                    line,
                });
            }
            "number" => {
                let raw = text(node, source.as_bytes()).replace('_', "");
                let value = raw
                    .parse::<f64>()
                    .map_or_else(|_| Scalar::Other(raw.clone()), Scalar::Num);
                entries.push(Entry {
                    path: path.clone(),
                    value,
                    line,
                });
            }
            "unary_expression" => {
                let raw = text(node, source.as_bytes()).replace([' ', '_'], "");
                let value = raw.parse::<f64>().map_or_else(
                    |_| Scalar::Other(text(node, source.as_bytes()).to_owned()),
                    Scalar::Num,
                );
                entries.push(Entry {
                    path: path.clone(),
                    value,
                    line,
                });
            }
            "true" => entries.push(Entry {
                path: path.clone(),
                value: Scalar::Bool(true),
                line,
            }),
            "false" => entries.push(Entry {
                path: path.clone(),
                value: Scalar::Bool(false),
                line,
            }),
            "null" | "undefined" => entries.push(Entry {
                path: path.clone(),
                value: Scalar::Null,
                line,
            }),
            _ => entries.push(Entry {
                path: path.clone(),
                value: Scalar::Other(text(node, source.as_bytes()).to_owned()),
                line,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(format: Format, source: &str) -> Vec<(String, String, usize)> {
        read(format, source)
            .expect("readable")
            .into_iter()
            .map(|entry| (entry.path.join("."), entry.value.as_text(), entry.line))
            .collect()
    }

    fn has(entries: &[(String, String, usize)], path: &str, value: &str, line: usize) -> bool {
        entries
            .iter()
            .any(|(p, v, l)| p == path && v == value && *l == line)
    }

    #[test]
    fn reads_json_with_comments_and_trailing_commas() {
        let entries = flat(
            Format::Json,
            "{\n  // a comment\n  \"compilerOptions\": {\n    \"strict\": true, /* x */\n    \"lib\": [\"es2022\", \"dom\",],\n  },\n  \"n\": 80.5\n}\n",
        );
        assert!(has(&entries, "compilerOptions.strict", "true", 4));
        assert!(has(&entries, "compilerOptions.lib.[1]", "dom", 5));
        assert!(has(&entries, "n", "80.5", 7));
        assert_eq!(read(Format::Json, "{\n \"a\": \n}"), Err(3));
    }

    #[test]
    fn reads_a_github_workflow() {
        let source = "name: CI\non:\n  pull_request:\n  push:\n    branches: [main, \"release\"]\njobs:\n  test:\n    runs-on: ubuntu-latest\n    continue-on-error: false\n    steps:\n      - uses: actions/checkout@v4 # pinned\n      - name: Test\n        run: |\n          pnpm install\n          pnpm test || true\n\n          echo done\n        continue-on-error: true\n      - run: cargo test\n";
        let entries = flat(Format::Yaml, source);
        assert!(has(&entries, "on.pull_request", "null", 3));
        assert!(has(&entries, "on.push.branches.[1]", "release", 5));
        assert!(has(
            &entries,
            "jobs.test.steps.[0].uses",
            "actions/checkout@v4",
            11
        ));
        assert!(has(&entries, "jobs.test.steps.[1].name", "Test", 12));
        assert!(has(
            &entries,
            "jobs.test.steps.[1].run",
            "pnpm install\npnpm test || true\n\necho done",
            14
        ));
        assert!(has(
            &entries,
            "jobs.test.steps.[1].continue-on-error",
            "true",
            18
        ));
        assert!(has(&entries, "jobs.test.steps.[2].run", "cargo test", 19));
        assert!(has(&entries, "jobs.test.continue-on-error", "false", 9));
    }

    #[test]
    fn reads_yaml_sequences_at_the_key_indent_and_nested_items() {
        let source = "rules:\n- a\n- b: 1\n  c: [x, {y: 2}]\nextends:\n  - - deep\n";
        let entries = flat(Format::Yaml, source);
        assert!(has(&entries, "rules.[0]", "a", 2));
        assert!(has(&entries, "rules.[1].b", "1", 3));
        assert!(has(&entries, "rules.[1].c.[1].y", "2", 4));
        assert!(has(&entries, "extends.[0].[0]", "deep", 6));
    }

    #[test]
    fn yaml_words_rust_reads_as_numbers_stay_strings() {
        let entries = flat(
            Format::Yaml,
            "a: inf\nb: NaN\nc: -Infinity\nd: -1.5\ne: .5\n",
        );
        assert_eq!(
            read(Format::Yaml, "a: inf\n").expect("readable")[0].value,
            Scalar::Str("inf".into())
        );
        assert!(has(&entries, "b", "NaN", 2));
        assert!(has(&entries, "c", "-Infinity", 3));
        assert_eq!(
            read(Format::Yaml, "d: -1.5\ne: .5\n").expect("readable")[1].value,
            Scalar::Num(0.5)
        );
        assert!(has(&entries, "d", "-1.5", 4));
    }

    #[test]
    fn reads_toml() {
        let source = "[tool.coverage.report]\nfail_under = 90 # floor\n\n[tool.ruff.lint]\nignore = [\n  \"E501\",\n  # a comment\n  \"F401\",\n]\nper-file-ignores = { \"tests/*\" = [\"S101\"] }\n\n[[bin]]\nname = 'a'\n[[bin]]\nname = 'b'\n[lints.clippy]\npedantic = { level = \"warn\", priority = -1 }\n";
        let entries = flat(Format::Toml, source);
        assert!(has(&entries, "tool.coverage.report.fail_under", "90", 2));
        assert!(has(&entries, "tool.ruff.lint.ignore.[0]", "E501", 6));
        assert!(has(&entries, "tool.ruff.lint.ignore.[1]", "F401", 8));
        assert!(has(
            &entries,
            "tool.ruff.lint.per-file-ignores.tests/*.[0]",
            "S101",
            10
        ));
        assert!(has(&entries, "bin.[1].name", "b", 15));
        assert!(has(&entries, "lints.clippy.pedantic.level", "warn", 17));
        assert!(has(&entries, "lints.clippy.pedantic.priority", "-1", 17));
    }

    #[test]
    fn reads_ini() {
        let source = "[flake8]\nmax-line-length = 100\nper-file-ignores =\n    tests/*: S101\n    a.py: E501 ; why\n\n[coverage:report]\nfail_under: 85\n";
        let entries = flat(Format::Ini, source);
        assert!(has(&entries, "flake8.max-line-length", "100", 2));
        assert!(has(
            &entries,
            "flake8.per-file-ignores",
            "tests/*: S101\na.py: E501",
            3
        ));
        assert!(has(&entries, "coverage:report.fail_under", "85", 8));
    }

    #[test]
    fn reads_js_object_literals() {
        let source = "import js from \"@eslint/js\";\nexport default [\n  js.configs.recommended,\n  {\n    rules: {\n      \"no-console\": \"off\",\n      eqeqeq: [\"error\", \"always\"],\n    },\n  },\n];\nmodule.exports = { coverageThreshold: { global: { lines: 80, branches: -1 } } };\n";
        let entries = flat(Format::Js(JsDialect::Tsx), source);
        assert!(has(&entries, "[1].rules.no-console", "off", 6));
        assert!(has(&entries, "[1].rules.eqeqeq.[0]", "error", 7));
        assert!(has(&entries, "[0]", "js.configs.recommended", 3));
        assert!(has(&entries, "coverageThreshold.global.lines", "80", 11));
        assert!(has(&entries, "coverageThreshold.global.branches", "-1", 11));
        assert!(read(Format::Js(JsDialect::Tsx), "export default { a: ").is_err());
    }

    #[test]
    fn reads_each_outermost_js_literal_as_a_root_of_its_own() {
        let source = "export default tseslint.config(\n  { files: [\"a.js\"], rules: { eqeqeq: \"off\" } },\n  { rules: { eqeqeq: \"error\" } },\n);\n";
        let roots = read_roots(Format::Js(JsDialect::Tsx), source).expect("readable");
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0][0].path, ["files", "[0]"]);
        assert_eq!(roots[1][0].keys(), "rules.eqeqeq");
        assert_eq!(roots[1][0].line, 3);
        assert_eq!(
            read_roots(Format::Json, "{\"a\": 1}")
                .expect("readable")
                .len(),
            1
        );
    }

    #[test]
    fn entry_paths_ignore_list_indexes() {
        let entry = Entry {
            path: vec!["[1]".into(), "rules".into(), "no-console".into()],
            value: Scalar::Str("off".into()),
            line: 1,
        };
        assert_eq!(entry.keys(), "rules.no-console");
        assert!(entry.ends_with(&["rules", "no-console"]));
        assert!(!entry.ends_with(&["x", "rules", "no-console"]));
    }
}
