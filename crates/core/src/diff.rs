//! A small unified diff, for showing what a fix would change.

/// One line of a diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op<'a> {
    Keep(&'a str),
    Remove(&'a str),
    Add(&'a str),
}

/// Lines of context shown around a change.
const CONTEXT: usize = 3;

/// Line-by-line diff using a longest-common-subsequence table. Fine for the
/// small config files a fix edits.
fn line_ops<'a>(before: &[&'a str], after: &[&'a str]) -> Vec<Op<'a>> {
    let (rows, columns) = (before.len(), after.len());
    // common[i][j] = length of the LCS of before[i..] and after[j..].
    let mut common = vec![vec![0usize; columns + 1]; rows + 1];
    for i in (0..rows).rev() {
        for j in (0..columns).rev() {
            common[i][j] = if before[i] == after[j] {
                common[i + 1][j + 1] + 1
            } else {
                common[i + 1][j].max(common[i][j + 1])
            };
        }
    }
    let mut ops = Vec::with_capacity(rows + columns);
    let (mut i, mut j) = (0, 0);
    while i < rows && j < columns {
        if before[i] == after[j] {
            ops.push(Op::Keep(before[i]));
            i += 1;
            j += 1;
        } else if common[i + 1][j] >= common[i][j + 1] {
            ops.push(Op::Remove(before[i]));
            i += 1;
        } else {
            ops.push(Op::Add(after[j]));
            j += 1;
        }
    }
    ops.extend(before[i..].iter().map(|line| Op::Remove(line)));
    ops.extend(after[j..].iter().map(|line| Op::Add(line)));
    ops
}

/// Renders a unified diff of one file with forward-slash paths and LF line
/// endings. `before` is `None` for a new file. Returns an empty string when
/// nothing changed. All changes are shown in one hunk.
#[must_use]
pub fn unified_diff(file: &str, before: Option<&str>, after: &str) -> String {
    let before_lines: Vec<&str> = before
        .map(|text| text.lines().collect())
        .unwrap_or_default();
    let after_lines: Vec<&str> = after.lines().collect();
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
        out.push('\n');
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
    fn crlf_input_is_shown_with_lf() {
        let diff = unified_diff("f", Some("a\r\n"), "a\r\nb\r\n");
        assert_eq!(diff, "--- a/f\n+++ b/f\n@@ -1,1 +1,2 @@\n a\n+b\n");
    }

    #[test]
    fn no_change_is_an_empty_diff() {
        assert_eq!(unified_diff("f", Some("a\n"), "a\n"), "");
    }
}
