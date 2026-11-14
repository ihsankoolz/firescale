//! Changes that need a person's approval (M26, ADR 0046): snapshots and
//! expected values that a change rewrote, and the approvals that count.
//!
//! - **Snapshots**: a snapshot whose name is on both sides and whose body
//!   differs (Jest and Vitest `__snapshots__/*.snap`, insta `*.snap`,
//!   syrupy `*.ambr`). A snapshot added (a new test) or removed (an
//!   obsolete one) is not one: deletions are the tests' business.
//! - **Expected values**: an assertion of the base that is gone, paired
//!   with one of the working tree that is new, whose shapes are the same
//!   once every literal is masked: `expect(total(cart)).toBe(30)` →
//!   `toBe(31)`, `assert total(cart) == 30` → `== 31`, an inline snapshot's
//!   text. Pairs are found per file.
//! - **Approvals** come only from GitHub: an `APPROVED` review of the pull
//!   request's head commit, by someone other than its author, with write
//!   access (`OWNER`, `MEMBER`, `COLLABORATOR`), who is not a bot, and whose
//!   latest review it is. Nothing in the repo, its history or its working
//!   tree is ever read as an approval.

use std::collections::BTreeMap;

/// True for a file of stored snapshots.
#[must_use]
pub fn is_snapshot_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let in_folder = |folder: &str| path.split('/').rev().skip(1).any(|part| part == folder);
    (name.ends_with(".snap") && (in_folder("__snapshots__") || in_folder("snapshots")))
        || (name.ends_with(".ambr") && in_folder("__snapshots__"))
}

/// One stored snapshot: its name, the line it starts on, and its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub name: String,
    pub line: usize,
    pub body: String,
}

/// The snapshots a snapshot file holds.
#[must_use]
pub fn snapshots(path: &str, text: &str) -> Vec<Snapshot> {
    let text = text.replace("\r\n", "\n");
    if path.ends_with(".ambr") {
        // syrupy: `# name: test_x` then the body, up to the next `# name:`.
        return split_at(&text, |line| {
            line.strip_prefix("# name: ")
                .map(|name| (name.to_owned(), String::new()))
        });
    }
    if text.contains("\nexports[`") || text.starts_with("exports[`") {
        // Jest and Vitest: exports[`name`] = `body`;
        return split_at(&text, |line| {
            let rest = line.strip_prefix("exports[`")?;
            let end = rest.find("`]")?;
            // The body starts on the header line: exports[`a 1`] = `body`;
            Some((rest[..end].to_owned(), rest[end + 2..].to_owned()))
        });
    }
    // insta: one snapshot per file, a `---` header, then the body.
    let body = match text.strip_prefix("---\n") {
        Some(rest) => rest.split_once("\n---\n").map_or(rest, |(_, body)| body),
        None => &text,
    };
    vec![Snapshot {
        name: path
            .rsplit('/')
            .next()
            .unwrap_or(path)
            .trim_end_matches(".snap")
            .to_owned(),
        line: 1,
        body: body.trim_end().to_owned(),
    }]
}

fn split_at(text: &str, name_of: impl Fn(&str) -> Option<(String, String)>) -> Vec<Snapshot> {
    let mut out: Vec<Snapshot> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if let Some((name, rest)) = name_of(line) {
            let body = if rest.trim().is_empty() {
                String::new()
            } else {
                format!("{}\n", rest.trim_end())
            };
            out.push(Snapshot {
                name,
                line: index + 1,
                body,
            });
            continue;
        }
        if let Some(last) = out.last_mut() {
            last.body.push_str(line.trim_end());
            last.body.push('\n');
        }
    }
    for snapshot in &mut out {
        snapshot.body = snapshot.body.trim_end().to_owned();
    }
    out
}

/// The snapshots of one file that both sides have, with a different body:
/// (name, line now).
#[must_use]
pub fn rewritten(path: &str, base: &str, head: &str) -> Vec<(String, usize)> {
    let before: BTreeMap<String, String> = snapshots(path, base)
        .into_iter()
        .map(|s| (s.name, s.body))
        .collect();
    snapshots(path, head)
        .into_iter()
        .filter(|s| before.get(&s.name).is_some_and(|body| *body != s.body))
        .map(|s| (s.name, s.line))
        .collect()
}

/// An assertion's shape with every string and number literal replaced by
/// `_`: two assertions with the same skeleton differ only in their values.
#[must_use]
pub fn skeleton(shape: &str) -> String {
    let chars: Vec<char> = shape.chars().collect();
    let mut out = String::with_capacity(shape.len());
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        // A string literal, with an optional Python or Rust prefix.
        let prefix_len = ["rb", "br", "b", "r", "f", "u", "fr", "rf"]
            .iter()
            .find(|p| {
                let p: Vec<char> = p.chars().collect();
                chars.get(index..index + p.len()).is_some_and(|s| {
                    s.iter()
                        .map(|c| c.to_ascii_lowercase())
                        .eq(p.iter().copied())
                }) && matches!(chars.get(index + p.len()), Some('"' | '\'' | '#'))
                    && (index == 0
                        || !(chars[index - 1].is_alphanumeric() || chars[index - 1] == '_'))
            })
            .map_or(0, |p| p.len());
        let at = index + prefix_len;
        if matches!(chars.get(at), Some('"' | '\'' | '`'))
            || (prefix_len > 0 && chars.get(at) == Some(&'#'))
        {
            // Rust raw strings: r#"…"#.
            let mut hashes = 0;
            let mut q = at;
            while chars.get(q) == Some(&'#') {
                hashes += 1;
                q += 1;
            }
            if let Some(&quote) = chars.get(q) {
                if matches!(quote, '"' | '\'' | '`') {
                    let mut end = q + 1;
                    let mut closed = false;
                    while end < chars.len() {
                        if chars[end] == '\\' && hashes == 0 {
                            end += 2;
                            continue;
                        }
                        if chars[end] == quote
                            && chars[end + 1..]
                                .iter()
                                .take(hashes)
                                .filter(|c| **c == '#')
                                .count()
                                == hashes
                        {
                            end += 1 + hashes;
                            closed = true;
                            break;
                        }
                        end += 1;
                    }
                    if closed {
                        out.push('_');
                        index = end;
                        continue;
                    }
                }
            }
        }
        // A number not inside a name.
        let starts_number = c.is_ascii_digit()
            && (index == 0
                || !(chars[index - 1].is_alphanumeric()
                    || chars[index - 1] == '_'
                    || chars[index - 1] == '.'));
        if starts_number {
            let mut end = index;
            while end < chars.len()
                && (chars[end].is_ascii_alphanumeric() || chars[end] == '_' || chars[end] == '.')
            {
                end += 1;
            }
            // A leading minus belongs to the number.
            if out.ends_with('-') {
                let before = out.chars().rev().nth(1);
                if before.is_none_or(|b| matches!(b, '(' | ',' | '=' | '[' | ' ' | '<' | '>' | '!'))
                {
                    out.pop();
                }
            }
            out.push('_');
            index = end;
            continue;
        }
        out.push(c);
        index += 1;
    }
    out
}

/// True when an assertion has a literal to change.
#[must_use]
pub fn has_literal(shape: &str) -> bool {
    skeleton(shape) != shape
}

/// Pairs a file's assertions that are gone with the ones that are new and
/// differ only in their literals: (index in `gone`, index in `new`).
#[must_use]
pub fn pairs(gone: &[&str], new: &[&str]) -> Vec<(usize, usize)> {
    let mut free: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, shape) in new.iter().enumerate() {
        if has_literal(shape) {
            free.entry(skeleton(shape)).or_default().push(index);
        }
    }
    let mut out = Vec::new();
    for (index, shape) in gone.iter().enumerate() {
        if !has_literal(shape) {
            continue;
        }
        if let Some(list) = free.get_mut(&skeleton(shape)) {
            if !list.is_empty() {
                out.push((index, list.remove(0)));
            }
        }
    }
    out
}

/// Who approved the head commit, from GitHub's list of the pull request's
/// reviews (`GET /repos/{owner}/{repo}/pulls/{number}/reviews`, one or more
/// pages, as `gh api --paginate --slurp` prints them or page after page).
///
/// # Errors
///
/// When the text is not that JSON.
pub fn approvers(reviews: &str, author: &str, head: &str) -> Result<Vec<String>, String> {
    let mut all: Vec<serde_json::Value> = Vec::new();
    let stream = serde_json::Deserializer::from_str(reviews).into_iter::<serde_json::Value>();
    for value in stream {
        let value = value.map_err(|error| format!("the reviews are not JSON: {error}"))?;
        flatten(value, &mut all);
    }
    // Each person's latest review decides.
    let mut latest: BTreeMap<String, &serde_json::Value> = BTreeMap::new();
    for review in &all {
        let state = review
            .get("state")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        // A comment does not change an approval.
        if state == "COMMENTED" || state == "PENDING" {
            continue;
        }
        let Some(login) = review
            .pointer("/user/login")
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        latest.insert(login.to_ascii_lowercase(), review);
    }
    Ok(latest
        .into_iter()
        .filter(|(login, review)| {
            let field = |path: &str| {
                review
                    .pointer(path)
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
            };
            field("/state") == "APPROVED"
                && field("/commit_id") == head
                && *login != author.to_ascii_lowercase()
                && field("/user/type") != "Bot"
                && matches!(
                    field("/author_association"),
                    "OWNER" | "MEMBER" | "COLLABORATOR"
                )
        })
        .map(|(_, review)| {
            review
                .pointer("/user/login")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_owned()
        })
        .collect())
}

fn flatten(value: serde_json::Value, out: &mut Vec<serde_json::Value>) {
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                flatten(item, out);
            }
        }
        other => out.push(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_jest_syrupy_and_insta_snapshots() {
        let jest = "// Jest Snapshot v1\n\nexports[`cart totals 1`] = `\n\"30\"\n`;\n\nexports[`cart renders 1`] = `<div>hi</div>`;\n";
        let found = snapshots("src/__snapshots__/cart.test.ts.snap", jest);
        assert_eq!(
            found
                .iter()
                .map(|s| (s.name.as_str(), s.line))
                .collect::<Vec<_>>(),
            [("cart totals 1", 3), ("cart renders 1", 7)]
        );
        let ambr = "# serializer version: 1\n# name: test_total\n  30\n# ---\n# name: test_tax\n  3\n# ---\n";
        assert_eq!(
            snapshots("tests/__snapshots__/test_cart.ambr", ambr).len(),
            2
        );
        let insta = "---\nsource: src/lib.rs\nexpression: total()\n---\n30\n";
        let found = snapshots("src/snapshots/app__total.snap", insta);
        assert_eq!(found[0].body, "30");
        assert!(is_snapshot_path("src/__snapshots__/a.test.ts.snap"));
        assert!(is_snapshot_path("tests/snapshots/x__y.snap"));
        assert!(is_snapshot_path("tests/__snapshots__/test_x.ambr"));
        assert!(!is_snapshot_path("src/a.snap"));
    }

    #[test]
    fn only_a_snapshot_on_both_sides_with_another_body_is_rewritten() {
        let path = "src/__snapshots__/cart.test.ts.snap";
        let base = "exports[`a 1`] = `1`;\n\nexports[`b 1`] = `2`;\n";
        assert!(rewritten(path, base, base).is_empty());
        // Added and removed snapshots are not rewritten ones.
        assert!(rewritten(
            path,
            base,
            "exports[`a 1`] = `1`;\n\nexports[`c 1`] = `3`;\n"
        )
        .is_empty());
        assert_eq!(
            rewritten(
                path,
                base,
                "exports[`a 1`] = `1`;\n\nexports[`b 1`] = `9`;\n"
            ),
            [("b 1".to_owned(), 3)]
        );
        // Line endings alone are not a change.
        assert!(rewritten(path, base, &base.replace('\n', "\r\n")).is_empty());
    }

    #[test]
    fn pairs_assertions_that_differ_only_in_their_values() {
        assert_eq!(
            skeleton("expect(total(cart)).toBe(30)"),
            "expect(total(cart)).toBe(_)"
        );
        assert_eq!(skeleton("assert_eq!(f(x1),-2.5)"), "assert_eq!(f(x1),_)");
        assert_eq!(skeleton("assertresult==r#\"a\"b\"#"), "assertresult==_");
        assert_eq!(
            skeleton("expect(x).toMatchInlineSnapshot(`\"30\"`)"),
            "expect(x).toMatchInlineSnapshot(_)"
        );
        let gone = ["expect(total(cart)).toBe(30)", "expect(name).toBe(\"a\")"];
        let new = ["expect(name).toBe(\"b\")", "expect(total(cart)).toBe(31)"];
        assert_eq!(pairs(&gone, &new), [(0, 1), (1, 0)]);
        // A different matcher, or a different subject, is not a value change.
        assert!(pairs(&["expect(x).toBe(3)"], &["expect(x).toBeDefined()"]).is_empty());
        assert!(pairs(&["expect(f(1)).toBe(3)"], &["expect(g(1)).toBe(3)"]).is_empty());
        // Nothing to change: no literal.
        assert!(pairs(&["expect(a).toEqual(b)"], &["expect(a).toEqual(b)"]).is_empty());
    }

    #[test]
    fn counts_only_approvals_of_the_head_by_someone_else_with_write_access() {
        let review = |login: &str, state: &str, commit: &str, association: &str, kind: &str| {
            format!(
                r#"{{"user":{{"login":"{login}","type":"{kind}"}},"state":"{state}","commit_id":"{commit}","author_association":"{association}"}}"#
            )
        };
        let head = "a".repeat(40);
        let old = "b".repeat(40);
        let reviews = format!(
            "[{},{},{},{},{}][{},{}]",
            review("author", "APPROVED", &head, "OWNER", "User"),
            review("stale", "APPROVED", &old, "MEMBER", "User"),
            review("drive-by", "APPROVED", &head, "CONTRIBUTOR", "User"),
            review("bot", "APPROVED", &head, "MEMBER", "Bot"),
            review("changed-mind", "APPROVED", &head, "MEMBER", "User"),
            review("changed-mind", "CHANGES_REQUESTED", &head, "MEMBER", "User"),
            review("reviewer", "APPROVED", &head, "COLLABORATOR", "User"),
        );
        assert_eq!(approvers(&reviews, "Author", &head).unwrap(), ["reviewer"]);
        // A later comment does not undo an approval.
        let commented = format!(
            "[{},{}]",
            review("reviewer", "APPROVED", &head, "MEMBER", "User"),
            review("reviewer", "COMMENTED", &head, "MEMBER", "User")
        );
        assert_eq!(
            approvers(&commented, "author", &head).unwrap(),
            ["reviewer"]
        );
        assert!(approvers("not json", "author", &head).is_err());
        assert!(approvers("[]", "author", &head).unwrap().is_empty());
    }
}
