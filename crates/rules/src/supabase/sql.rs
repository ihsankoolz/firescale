//! Just enough of PostgreSQL's syntax to follow tables and their row level
//! security through a project's migrations: statements split on `;`
//! outside comments, strings, quoted names and dollar-quoted bodies, and
//! the few statement shapes that create, rename, drop a table or switch its
//! row level security. Not a SQL parser; what it does not read it ignores.

use std::collections::BTreeMap;

/// The statements of a SQL text, with comments removed and string bodies
/// blanked, each on one line with single spaces.
#[must_use]
pub fn statements(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut current = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            '-' if next == Some('-') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                current.push(' ');
                continue;
            }
            '/' if next == Some('*') => {
                // Nested block comments, as PostgreSQL allows.
                let mut depth = 0usize;
                while i < chars.len() {
                    if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                        depth += 1;
                        i += 2;
                    } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
                current.push(' ');
                continue;
            }
            '\'' => {
                // A string: kept as '' so statements still read in shape.
                i += 1;
                while i < chars.len() {
                    if chars[i] == '\'' {
                        if chars.get(i + 1) == Some(&'\'') {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    if chars[i] == '\\' {
                        i += 1;
                    }
                    i += 1;
                }
                i += 1;
                current.push_str("''");
                continue;
            }
            '"' => {
                // A quoted name: kept, quotes and all.
                current.push('"');
                i += 1;
                while i < chars.len() {
                    if chars[i] == '"' {
                        if chars.get(i + 1) == Some(&'"') {
                            current.push('"');
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    current.push(chars[i]);
                    i += 1;
                }
                current.push('"');
                i += 1;
                continue;
            }
            '$' => {
                // $tag$ ... $tag$: a function body, skipped.
                let mut j = i + 1;
                while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
                    j += 1;
                }
                if chars.get(j) == Some(&'$')
                    && !chars[i + 1..j].first().is_some_and(char::is_ascii_digit)
                {
                    let tag: String = chars[i..=j].iter().collect();
                    let rest: String = chars[j + 1..].iter().collect();
                    match rest.find(&tag) {
                        Some(at) => {
                            i = j + 1 + rest[..at].chars().count() + tag.chars().count();
                        }
                        None => i = chars.len(),
                    }
                    current.push_str("$$");
                    continue;
                }
                current.push(c);
            }
            ';' => {
                push(&mut out, &current);
                current.clear();
            }
            _ => current.push(c),
        }
        i += 1;
    }
    push(&mut out, &current);
    out
}

fn push(out: &mut Vec<String>, statement: &str) {
    let line = statement.split_whitespace().collect::<Vec<_>>().join(" ");
    if !line.is_empty() {
        out.push(line);
    }
}

/// A table's name as `schema.table`, lower case unless quoted; an
/// unqualified name is in `public` (the default search path).
fn qualified(name: &str) -> Option<String> {
    let mut parts = Vec::new();
    let mut part = String::new();
    let mut quoted = false;
    let mut was_quoted = false;
    for c in name.trim().chars() {
        match c {
            '"' => {
                quoted = !quoted;
                was_quoted = true;
            }
            '.' if !quoted => {
                parts.push(if was_quoted {
                    part.clone()
                } else {
                    part.to_lowercase()
                });
                part.clear();
                was_quoted = false;
            }
            _ => part.push(c),
        }
    }
    parts.push(if was_quoted {
        part.clone()
    } else {
        part.to_lowercase()
    });
    if parts.iter().any(String::is_empty) {
        return None;
    }
    match parts.as_slice() {
        [table] => Some(format!("public.{table}")),
        [schema, table] => Some(format!("{schema}.{table}")),
        _ => None,
    }
}

/// The table name at the start of `text` (after keywords were consumed):
/// a possibly quoted, possibly schema-qualified identifier.
fn name_at(text: &str) -> Option<(String, &str)> {
    let mut end = 0;
    let mut in_quote = false;
    for c in text.chars() {
        if c == '"' {
            in_quote = !in_quote;
        } else if !in_quote && !(c.is_alphanumeric() || c == '_' || c == '.' || c == '$') {
            break;
        }
        end += c.len_utf8();
    }
    Some((qualified(&text[..end])?, &text[end..]))
}

fn strip_words<'a>(text: &'a str, words: &[&str]) -> Option<&'a str> {
    let mut rest = text;
    for word in words {
        let trimmed = rest.trim_start();
        let head = trimmed.get(..word.len())?;
        if !head.eq_ignore_ascii_case(word) {
            return None;
        }
        let after = &trimmed[word.len()..];
        if after
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
        {
            return None;
        }
        rest = after;
    }
    Some(rest.trim_start())
}

fn strip_optional<'a>(text: &'a str, words: &[&str]) -> &'a str {
    strip_words(text, words).unwrap_or(text)
}

/// What the migrations leave: each table they create, and whether row level
/// security is on for it at the end, with the migration that created it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub name: String,
    pub rls: bool,
    pub created_in: String,
}

/// Follows tables through migrations, in order.
#[must_use]
pub fn tables(migrations: &[(String, String)]) -> Vec<Table> {
    let mut state: BTreeMap<String, Table> = BTreeMap::new();
    for (file, text) in migrations {
        for statement in statements(text) {
            apply(&mut state, file, &statement);
        }
    }
    state.into_values().collect()
}

fn apply(state: &mut BTreeMap<String, Table>, file: &str, statement: &str) {
    // create [or replace]? [global|local]? [temp|temporary|unlogged]? table [if not exists] name
    if let Some(rest) = strip_words(statement, &["create"]) {
        let rest = strip_optional(rest, &["global"]);
        let rest = strip_optional(rest, &["local"]);
        if strip_words(rest, &["temp"]).is_some() || strip_words(rest, &["temporary"]).is_some() {
            return;
        }
        let rest = strip_optional(rest, &["unlogged"]);
        if let Some(rest) = strip_words(rest, &["table"]) {
            let rest = strip_optional(rest, &["if", "not", "exists"]);
            if let Some((name, _)) = name_at(rest) {
                state.entry(name.clone()).or_insert(Table {
                    name,
                    rls: false,
                    created_in: file.to_owned(),
                });
            }
        }
        return;
    }
    if let Some(rest) = strip_words(statement, &["drop", "table"]) {
        let rest = strip_optional(rest, &["if", "exists"]);
        for name in rest.split(',') {
            let name = name.trim();
            let name = name
                .strip_suffix(" cascade")
                .or_else(|| name.strip_suffix(" CASCADE"))
                .or_else(|| name.strip_suffix(" restrict"))
                .or_else(|| name.strip_suffix(" RESTRICT"))
                .unwrap_or(name);
            if let Some((name, _)) = name_at(name.trim()) {
                state.remove(&name);
            }
        }
        return;
    }
    if let Some(rest) = strip_words(statement, &["alter", "table"]) {
        let rest = strip_optional(rest, &["if", "exists"]);
        let rest = strip_optional(rest, &["only"]);
        let Some((name, rest)) = name_at(rest) else {
            return;
        };
        let rest = rest.trim_start().trim_start_matches('*').trim_start();
        let lower = rest.to_lowercase();
        if let Some(new) = strip_words(rest, &["rename", "to"]) {
            if let Some((new_name, _)) = name_at(new) {
                // A rename keeps the schema.
                let schema = name.split('.').next().unwrap_or("public");
                let table = new_name.rsplit('.').next().unwrap_or(&new_name);
                let renamed = format!("{schema}.{table}");
                if let Some(mut table) = state.remove(&name) {
                    table.name.clone_from(&renamed);
                    state.insert(renamed, table);
                }
            }
            return;
        }
        if let Some(new) = strip_words(rest, &["set", "schema"]) {
            let schema = new.trim().trim_matches('"').to_lowercase();
            let table = name.rsplit('.').next().unwrap_or(&name).to_owned();
            if let Some(mut entry) = state.remove(&name) {
                entry.name = format!("{schema}.{table}");
                state.insert(entry.name.clone(), entry);
            }
            return;
        }
        // Several actions can be listed; the last switch of RLS wins.
        let on = lower.rfind("enable row level security");
        let off = lower.rfind("disable row level security");
        let forced = lower.rfind("force row level security");
        let on = on.max(forced);
        if let Some(table) = state.get_mut(&name) {
            match (on, off) {
                (Some(a), Some(b)) => table.rls = a > b,
                (Some(_), None) => table.rls = true,
                (None, Some(_)) => table.rls = false,
                (None, None) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(files: &[(&str, &str)]) -> Vec<Table> {
        let owned: Vec<(String, String)> = files
            .iter()
            .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
            .collect();
        tables(&owned)
    }

    #[test]
    fn splits_outside_comments_strings_and_bodies() {
        let sql = "create table a (x text default 'a;b'); -- c; d\n/* e; /* f; */ g; */\ncreate function f() returns void as $body$ begin; end; $body$ language plpgsql;\nselect 1";
        let got = statements(sql);
        assert_eq!(got.len(), 3, "{got:?}");
        assert!(got[0].starts_with("create table a"));
        assert!(got[1].starts_with("create function f()"));
    }

    #[test]
    fn follows_create_enable_disable_rename_and_drop() {
        let tables = run(&[
            (
                "1_a.sql",
                "create table public.notes (id int);\nCREATE TABLE IF NOT EXISTS \"Todos\" (id int);\ncreate table private.secrets (id int);\ncreate temp table scratch (id int);\ncreate table gone (id int);",
            ),
            (
                "2_b.sql",
                "alter table notes enable row level security;\nalter table public.\"Todos\" enable row level security;\nalter table \"Todos\" disable row level security;\ndrop table if exists gone cascade;\nalter table notes rename to memos;",
            ),
        ]);
        let names: Vec<(&str, bool)> = tables.iter().map(|t| (t.name.as_str(), t.rls)).collect();
        assert_eq!(
            names,
            vec![
                ("private.secrets", false),
                ("public.Todos", false),
                ("public.memos", true),
            ]
        );
    }

    #[test]
    fn several_actions_in_one_alter() {
        let tables = run(&[(
            "1.sql",
            "create table t (id int); alter table only t add column y int, enable row level security;",
        )]);
        assert!(tables[0].rls);
    }
}
