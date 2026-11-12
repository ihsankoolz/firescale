//! Adding to a JSON file a person wrote without changing any line of it
//! (ADR 0027).
//!
//! Re-serialising a settings file would re-indent it, reorder nothing but
//! still rewrite lines, and drop the person's layout. Instead, new keys and
//! new list items are inserted as **new lines** right after the line that
//! opens their object or list. No existing line is edited: the inserted
//! entries carry their own trailing commas.
//!
//! This works when the opening `{` or `[` ends its line, which is how
//! editors and `JSON.stringify(x, null, 2)` write files. When it does not
//! (`"deny": []` on one line, a minified file), `insert_merge` returns
//! `None` and the caller reports what to add by hand. Every result is parsed
//! again and compared with the wanted value before it is returned.

use serde_json::{Map, Value};

/// Where a container opens: the byte just after the line end that follows
/// its bracket, and the indentation of the bracket's line.
#[derive(Debug, Clone)]
struct Opening {
    /// Byte offset where new lines go, or `None` when the bracket does not
    /// end its line.
    insert_at: Option<usize>,
    indent: String,
}

/// A step in a path through JSON: an object key or a list index.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    Key(String),
    Index(usize),
}

/// Finds where every object and list opens, by path.
fn openings(text: &str) -> Vec<(Vec<Step>, Opening)> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    // For each open container: its path, whether it is an object, the last
    // key read in it, and the index of the next list item.
    let mut stack: Vec<(Vec<Step>, bool, Option<String>, usize)> = Vec::new();
    let mut index = 0;
    let line_start = |at: usize| text[..at].rfind('\n').map_or(0, |n| n + 1);
    while index < bytes.len() {
        let c = bytes[index];
        match c {
            b'"' => {
                // Read the string, with escapes.
                let start = index + 1;
                let mut end = start;
                while end < bytes.len() && bytes[end] != b'"' {
                    if bytes[end] == b'\\' {
                        end += 1;
                    }
                    end += 1;
                }
                let raw = &text[start..end.min(bytes.len())];
                // A string followed by ':' is a key.
                let mut after = end + 1;
                while after < bytes.len() && bytes[after].is_ascii_whitespace() {
                    after += 1;
                }
                if after < bytes.len() && bytes[after] == b':' {
                    if let Some(top) = stack.last_mut() {
                        let key: String = serde_json::from_str(&format!("\"{raw}\""))
                            .unwrap_or_else(|_| raw.to_owned());
                        top.2 = Some(key);
                    }
                }
                index = end + 1;
                continue;
            }
            b'{' | b'[' => {
                let mut path = stack.last().map(|top| top.0.clone()).unwrap_or_default();
                if let Some(top) = stack.last_mut() {
                    if top.1 {
                        path.push(Step::Key(top.2.clone().unwrap_or_default()));
                    } else {
                        path.push(Step::Index(top.3));
                    }
                }
                // Does the bracket end its line?
                let rest = &text[index + 1..];
                let line_end = rest.find('\n').map_or(text.len(), |n| index + 1 + n);
                let ends_line =
                    text[index + 1..line_end].trim().is_empty() && line_end < text.len();
                let start = line_start(index);
                let indent: String = text[start..]
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .collect();
                found.push((
                    path.clone(),
                    Opening {
                        insert_at: ends_line.then_some(line_end + 1),
                        indent,
                    },
                ));
                stack.push((path, c == b'{', None, 0));
            }
            b'}' | b']' => {
                stack.pop();
            }
            b',' => {
                // In a list, each comma starts the next item.
                if let Some(top) = stack.last_mut() {
                    if !top.1 {
                        top.3 += 1;
                    }
                }
            }
            _ => {}
        }
        index += 1;
    }
    found
}

/// What to insert into one container.
struct Insertion {
    path: Vec<Step>,
    /// Rendered entries: `"key": value` or `value`.
    entries: Vec<Value>,
    keys: Vec<Option<String>>,
    /// True when the container already has something in it.
    non_empty: bool,
}

/// Works out what `wanted` adds to `have`: keys missing from objects, and
/// list items missing from lists. Returns `None` when `wanted` changes or
/// removes something that is there.
fn additions(
    have: &Value,
    wanted: &Value,
    path: &mut Vec<Step>,
    out: &mut Vec<Insertion>,
) -> Option<()> {
    match (have, wanted) {
        (Value::Object(have), Value::Object(wanted)) => {
            let mut keys = Vec::new();
            let mut entries = Vec::new();
            for (key, value) in wanted {
                match have.get(key) {
                    None => {
                        keys.push(Some(key.clone()));
                        entries.push(value.clone());
                    }
                    Some(existing) => {
                        path.push(Step::Key(key.clone()));
                        additions(existing, value, path, out)?;
                        path.pop();
                    }
                }
            }
            if have.keys().any(|key| !wanted.contains_key(key)) {
                return None;
            }
            if !entries.is_empty() {
                out.push(Insertion {
                    path: path.clone(),
                    entries,
                    keys,
                    non_empty: !have.is_empty(),
                });
            }
            Some(())
        }
        (Value::Array(have), Value::Array(wanted)) => {
            // `wanted` is `have` plus new items.
            if !have.iter().all(|item| wanted.contains(item)) {
                return None;
            }
            let entries: Vec<Value> = wanted
                .iter()
                .filter(|item| !have.contains(item))
                .cloned()
                .collect();
            if !entries.is_empty() {
                let keys = vec![None; entries.len()];
                out.push(Insertion {
                    path: path.clone(),
                    entries,
                    keys,
                    non_empty: !have.is_empty(),
                });
            }
            Some(())
        }
        (have, wanted) if have == wanted => Some(()),
        _ => None,
    }
}

/// The indentation step a file uses: the smallest indentation of any line.
fn indent_unit(text: &str) -> String {
    text.lines()
        .map(|line| {
            line.chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect::<String>()
        })
        .filter(|indent| !indent.is_empty())
        .min_by_key(String::len)
        .unwrap_or_else(|| "  ".to_owned())
}

/// `value` as pretty JSON, every line after the first indented by `indent`.
fn render(value: &Value, indent: &str, unit: &str, newline: &str) -> String {
    let pretty = serde_json::to_string_pretty(value).unwrap_or_default();
    pretty
        .lines()
        .enumerate()
        .map(|(n, line)| {
            // serde_json indents with two spaces; use the file's own step.
            let depth = line.len() - line.trim_start().len();
            let inner = unit.repeat(depth / 2);
            if n == 0 {
                line.trim_start().to_owned()
            } else {
                format!("{indent}{inner}{}", line.trim_start())
            }
        })
        .collect::<Vec<_>>()
        .join(newline)
}

/// Adds what `wanted` has and `text` lacks, as new lines only. `wanted`
/// must be the parsed `text` with keys and list items added, nothing
/// changed or removed. Returns the new text, `Some(text)` unchanged when
/// nothing is missing, or `None` when the additions cannot be made without
/// changing a line.
#[must_use]
pub fn insert_merge(text: &str, wanted: &Value) -> Option<String> {
    let have: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    let mut inserts = Vec::new();
    additions(&have, wanted, &mut Vec::new(), &mut inserts)?;
    if inserts.is_empty() {
        return Some(text.to_owned());
    }
    let opens = openings(text);
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let unit = indent_unit(text);
    let mut edits: Vec<(usize, String)> = Vec::new();
    for insert in &inserts {
        let opening = opens
            .iter()
            .find(|(path, _)| *path == insert.path)?
            .1
            .clone();
        let at = opening.insert_at?;
        let inner = format!("{}{unit}", opening.indent);
        let count = insert.entries.len();
        let mut block = String::new();
        for (n, (value, key)) in insert.entries.iter().zip(&insert.keys).enumerate() {
            let rendered = render(value, &inner, &unit, newline);
            let comma = if n + 1 < count || insert.non_empty {
                ","
            } else {
                ""
            };
            match key {
                Some(key) => {
                    let key = serde_json::to_string(key).ok()?;
                    block.push_str(&format!("{inner}{key}: {rendered}{comma}{newline}"));
                }
                None => block.push_str(&format!("{inner}{rendered}{comma}{newline}")),
            }
        }
        edits.push((at, block));
    }
    edits.sort_by(|a, b| b.0.cmp(&a.0));
    let mut out = text.to_owned();
    for (at, block) in edits {
        out.insert_str(at, &block);
    }
    let check: Value = serde_json::from_str(out.trim_start_matches('\u{feff}')).ok()?;
    same(&check, wanted).then_some(out)
}

/// Equal, except that list items may be in another order: new items go in
/// at the start of a list, where they need no change to an existing line.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, value)| b.get(key).is_some_and(|other| same(value, other)))
        }
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len()
                && a.iter().all(|item| b.iter().any(|other| same(item, other)))
                && b.iter().all(|item| a.iter().any(|other| same(item, other)))
        }
        _ => a == b,
    }
}

/// `have` with `add` merged in: keys added to objects (recursively), items
/// added to lists when not already there. Values that differ are kept as
/// they are in `have`.
#[must_use]
pub fn merged(have: &Value, add: &Value) -> Value {
    match (have, add) {
        (Value::Object(have), Value::Object(add)) => {
            let mut out: Map<String, Value> = have.clone();
            for (key, value) in add {
                let next = match have.get(key) {
                    Some(existing) => merged(existing, value),
                    None => value.clone(),
                };
                out.insert(key.clone(), next);
            }
            Value::Object(out)
        }
        (Value::Array(have), Value::Array(add)) => {
            let mut out = have.clone();
            for item in add {
                if !out.contains(item) {
                    out.push(item.clone());
                }
            }
            Value::Array(out)
        }
        (have, _) => have.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn every_line_kept(before: &str, after: &str) -> bool {
        let mut lines = after.lines();
        before
            .lines()
            .all(|line| lines.any(|candidate| candidate == line))
    }

    #[test]
    fn keys_and_items_go_in_as_new_lines() {
        let text = "{\n    \"model\": \"opus\",\n    \"permissions\": {\n        \"allow\": [\n            \"Bash(make test)\"\n        ]\n    }\n}\n";
        let have: Value = serde_json::from_str(text).unwrap();
        let wanted = merged(
            &have,
            &json!({ "permissions": { "allow": ["Bash(npx firescale verify *)"], "deny": ["Read(**/.env)"] }, "hooks": { "Stop": [] } }),
        );
        let out = insert_merge(text, &wanted).expect("insertable");
        assert!(every_line_kept(text, &out), "{out}");
        assert!(same(&serde_json::from_str::<Value>(&out).unwrap(), &wanted));
        // The file's own four-space step is used.
        assert!(
            out.contains("\n        \"deny\": [\n            \"Read(**/.env)\"\n        ],\n"),
            "{out}"
        );
    }

    #[test]
    fn nothing_missing_gives_the_text_back() {
        let text = "{ \"a\": [1] }";
        let wanted: Value = serde_json::from_str(text).unwrap();
        assert_eq!(insert_merge(text, &wanted).as_deref(), Some(text));
    }

    #[test]
    fn a_bracket_that_does_not_end_its_line_is_refused() {
        let text = "{\n  \"permissions\": { \"deny\": [] }\n}\n";
        let have: Value = serde_json::from_str(text).unwrap();
        let wanted = merged(
            &have,
            &json!({ "permissions": { "deny": ["Read(**/.env)"] } }),
        );
        assert_eq!(insert_merge(text, &wanted), None);
        assert_eq!(insert_merge("{\"a\":1}", &json!({"a":1,"b":2})), None);
    }

    #[test]
    fn crlf_files_get_crlf_lines() {
        let text = "{\r\n  \"a\": 1\r\n}\r\n";
        let out = insert_merge(text, &json!({"a": 1, "b": {"c": [true]}})).expect("insertable");
        assert!(!out.replace("\r\n", "").contains('\n'), "{out:?}");
        assert!(every_line_kept(text, &out));
    }

    #[test]
    fn same_key_names_at_two_levels_are_told_apart() {
        // Claude's settings have "hooks" twice: the event map and each group's list.
        let text = "{\n  \"hooks\": {\n    \"Stop\": [\n      {\n        \"hooks\": [\n          { \"type\": \"command\", \"command\": \"x\" }\n        ]\n      }\n    ]\n  }\n}\n";
        let have: Value = serde_json::from_str(text).unwrap();
        let add = json!({ "hooks": { "Stop": [ { "hooks": [ { "type": "command", "command": "npx firescale hooks run --agent claude" } ] } ] } });
        let wanted = merged(&have, &add);
        let out = insert_merge(text, &wanted).expect("insertable");
        assert!(every_line_kept(text, &out));
        assert!(same(&serde_json::from_str::<Value>(&out).unwrap(), &wanted));
    }

    #[test]
    fn a_change_to_an_existing_value_is_not_an_insertion() {
        let text = "{\n  \"a\": 1\n}\n";
        assert_eq!(insert_merge(text, &json!({"a": 2})), None);
    }
}
