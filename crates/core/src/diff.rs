//! A small unified diff, for showing what a fix would change.

/// One line of a diff. A line keeps its trailing `\n`, so a last line
/// without one is a different line from the same text with one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op<'a> {
    Keep(&'a str),
    Remove(&'a str),
    Add(&'a str),
}

/// Lines of context shown around a change.
const CONTEXT: usize = 3;

/// Above this many table cells, the changed middle of a file is shown as
/// "remove everything, add everything" instead of a minimal diff. It keeps
/// memory bounded for huge rewrites; the output is still a correct diff.
const MAX_CELLS: usize = 4_000_000;

/// Line-by-line diff. The common start and end are skipped first, so the
/// longest-common-subsequence table only covers the part that changed.
fn line_ops<'a>(before: &[&'a str], after: &[&'a str]) -> Vec<Op<'a>> {
    let prefix = before.iter().zip(after).take_while(|(a, b)| a == b).count();
    let suffix = before[prefix..]
        .iter()
        .rev()
        .zip(after[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let old = &before[prefix..before.len() - suffix];
    let new = &after[prefix..after.len() - suffix];

    let mut ops: Vec<Op<'a>> = before[..prefix].iter().map(|line| Op::Keep(line)).collect();
    let (rows, columns) = (old.len(), new.len());
    if rows.saturating_mul(columns) > MAX_CELLS {
        ops.extend(old.iter().map(|line| Op::Remove(line)));
        ops.extend(new.iter().map(|line| Op::Add(line)));
    } else {
        // common[i][j] = length of the LCS of old[i..] and new[j..].
        let mut common = vec![vec![0usize; columns + 1]; rows + 1];
        for i in (0..rows).rev() {
            for j in (0..columns).rev() {
                common[i][j] = if old[i] == new[j] {
                    common[i + 1][j + 1] + 1
                } else {
                    common[i + 1][j].max(common[i][j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < rows && j < columns {
            if old[i] == new[j] {
                ops.push(Op::Keep(old[i]));
                i += 1;
                j += 1;
            } else if common[i + 1][j] >= common[i][j + 1] {
                ops.push(Op::Remove(old[i]));
                i += 1;
            } else {
                ops.push(Op::Add(new[j]));
                j += 1;
            }
        }
        ops.extend(old[i..].iter().map(|line| Op::Remove(line)));
        ops.extend(new[j..].iter().map(|line| Op::Add(line)));
    }
    ops.extend(
        before[before.len() - suffix..]
            .iter()
            .map(|line| Op::Keep(line)),
    );
    ops
}

/// Renders a unified diff of one file with forward-slash paths. `before` is
/// `None` for a new file. Returns an empty string when nothing changed.
///
/// CRLF line endings are shown as LF, so a change that only converts line
/// endings produces no diff. A last line without a newline is marked the way
/// git marks it. All changes are shown in one hunk.
#[must_use]
pub fn unified_diff(file: &str, before: Option<&str>, after: &str) -> String {
    let old_text = before.map(|text| text.replace("\r\n", "\n"));
    let new_text = after.replace("\r\n", "\n");
    let before_lines: Vec<&str> = old_text
        .as_deref()
        .map(|text| text.split_inclusive('\n').collect())
        .unwrap_or_default();
    let after_lines: Vec<&str> = new_text.split_inclusive('\n').collect();
    let ops = line_ops(&before_lines, &after_lines);
    let changed = |op: &Op<'_>| !matches!(op, Op::Keep(_));
    let (Some(first), Some(last)) = (ops.iter().position(changed), ops.iter().rposition(changed))
    else {
        return String::new();
    };
    let start = first.saturating_sub(CONTEXT);
    let end = (last + CONTEXT + 1).min(ops.len());

    // Line numbers where the hunk starts, counted from 1.
    let old_start = ops[..start]
        .iter()
        .filter(|op| !matches!(op, Op::Add(_)))
        .count()
        + 1;
    let new_start = ops[..start]
        .iter()
        .filter(|op| !matches!(op, Op::Remove(_)))
        .count()
        + 1;
    let old_count = ops[start..end]
        .iter()
        .filter(|op| !matches!(op, Op::Add(_)))
        .count();
    let new_count = ops[start..end]
        .iter()
        .filter(|op| !matches!(op, Op::Remove(_)))
        .count();
    // By convention an empty side starts at line 0.
    let old_start = if old_count == 0 { 0 } else { old_start };
    let new_start = if new_count == 0 { 0 } else { new_start };

    let mut out = String::new();
    out.push_str(&match before {
        Some(_) => format!("--- a/{file}\n"),
        None => "--- /dev/null\n".to_owned(),
    });
    out.push_str(&format!("+++ b/{file}\n"));
    out.push_str(&format!(
        "@@ -{old_start},{old_count} +{new_start},{new_count} @@\n"
    ));
    for op in &ops[start..end] {
        let (sign, line) = match op {
            Op::Keep(line) => (' ', line),
            Op::Remove(line) => ('-', line),
            Op::Add(line) => ('+', line),
        };
        out.push(sign);
        out.push_str(line);
        if !line.ends_with('\n') {
            out.push_str("\n\\ No newline at end of file\n");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::unified_diff;

    #[test]
    fn a_new_file_is_all_additions() {
        assert_eq!(
            unified_diff(".gitignore", None, ".env\n.env.*\n"),
            "--- /dev/null\n+++ b/.gitignore\n@@ -0,0 +1,2 @@\n+.env\n+.env.*\n"
        );
    }

    #[test]
    fn appended_lines_come_with_three_lines_of_context() {
        let before = "1\n2\n3\n4\n5\n";
        let after = "1\n2\n3\n4\n5\n6\n7\n";
        assert_eq!(
            unified_diff("a/b.txt", Some(before), after),
            "--- a/a/b.txt\n+++ b/a/b.txt\n@@ -3,3 +3,5 @@\n 3\n 4\n 5\n+6\n+7\n"
        );
    }

    #[test]
    fn a_changed_line_is_a_removal_and_an_addition() {
        assert_eq!(
            unified_diff("f", Some("a\nb\nc\n"), "a\nB\nc\n"),
            "--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n"
        );
    }

    #[test]
    fn a_change_in_the_middle_of_a_long_file_has_the_right_line_numbers() {
        let before: String = (1..=40).map(|n| format!("line {n}\n")).collect();
        let after = before.replace("line 20\n", "line twenty\n");
        assert_eq!(
            unified_diff("f", Some(&before), &after),
            "--- a/f\n+++ b/f\n@@ -17,7 +17,7 @@\n line 17\n line 18\n line 19\n-line 20\n+line twenty\n line 21\n line 22\n line 23\n"
        );
    }

    #[test]
    fn crlf_input_is_shown_with_lf() {
        let diff = unified_diff("f", Some("a\r\n"), "a\r\nb\r\n");
        assert_eq!(diff, "--- a/f\n+++ b/f\n@@ -1,1 +1,2 @@\n a\n+b\n");
    }

    #[test]
    fn a_missing_final_newline_is_marked() {
        // The old last line had no newline; the fix adds one and appends a line.
        assert_eq!(
            unified_diff("f", Some("node_modules"), "node_modules\n.env\n"),
            "--- a/f\n+++ b/f\n@@ -1,1 +1,2 @@\n-node_modules\n\\ No newline at end of file\n+node_modules\n+.env\n"
        );
        assert_eq!(
            unified_diff("f", Some("a\n"), "a\nb"),
            "--- a/f\n+++ b/f\n@@ -1,1 +1,2 @@\n a\n+b\n\\ No newline at end of file\n"
        );
    }

    #[test]
    fn no_change_is_an_empty_diff() {
        assert_eq!(unified_diff("f", Some("a\n"), "a\n"), "");
        assert_eq!(unified_diff("f", Some(""), ""), "");
    }

    #[test]
    fn a_small_change_to_a_very_large_file_stays_cheap() {
        let before: String = (1..=200_000).map(|n| format!("{n}\n")).collect();
        let after = format!("{before}last\n");
        let diff = unified_diff("big", Some(&before), &after);
        assert!(
            diff.ends_with(" 199998\n 199999\n 200000\n+last\n"),
            "{diff}"
        );
        assert!(diff.contains("@@ -199998,3 +199998,4 @@"), "{diff}");
    }

    #[test]
    fn a_huge_rewrite_falls_back_to_remove_all_add_all() {
        let before: String = (0..3000).map(|n| format!("old {n}\n")).collect();
        let after: String = (0..3000).map(|n| format!("new {n}\n")).collect();
        let diff = unified_diff("f", Some(&before), &after);
        assert!(diff.contains("@@ -1,3000 +1,3000 @@"));
        assert_eq!(
            diff.lines().filter(|line| line.starts_with('-')).count(),
            3001
        );
    }
}
